# Plan 5: Presage Table Unification, Frequency Updates, and Writability

## Problems

### 1. Presage table mismatch

The `presage` format preset uses a separate `words` table for dictionary queries and `ngrams` table for predictions. The actual Presage format has **no `words` table** — the `_1_gram` table serves as both the dictionary and the unigram frequency source. Currently:

- `SqliteDictionaryBackend` queries `words(word, frequency)` — a separate table
- `SqliteNgramBackend` queries `ngrams(context_1, next_word, frequency)` — unigrams are rows where `context_1 IS NULL`

This means for Presage data, we're querying two tables for what's logically the same data, and any frequency updates to one won't be reflected in the other.

### 2. No frequency update mechanism

There's no way to update n-gram frequencies when the user selects a word. `SqliteNgramBackend` has zero write methods. `SqliteDictionaryBackend` has `add_word()` but it's not called from anywhere, and it conflates "add new word" with "update frequency".

### 3. `add_word()` is ambiguous

Current `add_word(word, frequency)` does `INSERT OR REPLACE` — it both adds new words AND overwrites existing frequencies. These are different operations with different semantics and should be separate methods.

### 4. No writability guard

System files (e.g. `/usr/share/...`) could theoretically be written to if the code has permissions. There's no distinction between writable user files and read-only system files.

---

## Design

### Core principle: frequency lives on NgramBackend, membership lives on DictionaryBackend

- **`DictionaryBackend`** — read-only queries + `add_word()` for membership on backends that can't store frequency (MARISA, Hunspell). The `add_word` method takes an optional frequency; backends that can't store it ignore it.
- **`NgramBackend`** — owns all frequency writes. `increase_ngram_frequency(ngram, delta, save_unknown)` handles unigrams through trigrams. For Presage SQLite, this writes to the same table the dict backend reads from, so updates are shared.

---

### Part A: Presage table unification

#### A1: `SqliteFormat::Presage` preset

The existing `presage` preset defines `table=words` for dict and `table_ngrams=ngrams` for ngrams. This is wrong for actual Presage data.

New preset behavior for `presage`:
- `table_ngrams = "ngrams"`, `context_cols = ["context_1", "context_2"]`, `next_col = "next_word"`, `freq_col = "frequency"`
- **No separate `table`** — dictionary is derived from ngram table's unigram rows
- Both `Capability::Dictionary` and `Capability::Ngrams` are enabled

#### A2: `SqliteDictionaryBackend` — ngram unigram mode

Add a mode where the dictionary backend queries the ngram table's unigram rows instead of a dedicated words table.

New fields on `SqliteDictionaryBackend`:
```rust
pub struct SqliteDictionaryBackend {
    // ... existing fields ...
    /// When true, queries the ngram table's unigram rows (context IS NULL)
    /// instead of a dedicated (word, frequency) table.
    ngram_unigram_mode: bool,
    /// Column names for the ngram table (only used when ngram_unigram_mode is true).
    ngram_context_columns: Vec<String>,
    ngram_next_word_column: String,
    /// Whether write operations are allowed on this backend.
    writable: bool,
}
```

New constructor:
```rust
pub fn from_ngram_unigrams(
    conn: SharedSqliteConnection,
    table_name: &str,
    context_columns: &[String],
    next_word_column: &str,
    frequency_column: &str,
    writable: bool,
) -> Self
```

Query changes when `ngram_unigram_mode` is true:

| Operation | Simple mode SQL | Ngram unigram mode SQL |
|---|---|---|
| `contains(word)` | `WHERE word = ?` | `WHERE context_1 IS NULL AND next_word = ?` |
| `frequency(word)` | `SELECT freq FROM words WHERE word = ?` | `SELECT freq FROM ngrams WHERE context_1 IS NULL AND next_word = ?` |
| `query_prefixes` | `WHERE word LIKE 'prefix%'` | `WHERE context_1 IS NULL AND next_word LIKE 'prefix%'` |

#### A3: `build_sqlite` wiring

When `has_dict && has_ngrams` and the format is `presage` (no separate `table` configured):

```rust
// Both dict and ngram share the same ngrams table
let table_ngrams = def.table_ngrams.as_deref().unwrap_or("ngrams");
let context_cols = /* ... */;
let next_col = def.next_col.as_deref().unwrap_or("next_word");
let freq_col = def.freq_col.as_deref().unwrap_or("frequency");
let writable = /* determined from path source */;

let dict = SqliteDictionaryBackend::from_ngram_unigrams(
    shared.clone(),
    table_ngrams,
    &context_cols,
    next_col,
    freq_col,
    writable,
);
let ngram_backend = SqliteNgramBackend::new(
    shared,
    table_ngrams,
    &context_cols,
    next_col,
    freq_col,
    context_cols.len(),
    writable,
);
// Both point to the same table → frequency updates are shared
```

When `has_dict` only and `has_ngrams` only: existing behavior, each gets its own table.

---

### Part B: Writability

#### B1: Constructor-controlled writability

`writable: bool` is a constructor parameter, not auto-detected. The caller decides:

| Backend | Constructor | `writable` value |
|---|---|---|
| `SqliteDictionaryBackend::from_sqlite` | takes `writable: bool` | as passed |
| `SqliteDictionaryBackend::from_sqlite_readonly` | fixed | always `false` |
| `SqliteDictionaryBackend::from_ngram_unigrams` | takes `writable: bool` | as passed |
| `SqliteNgramBackend::new` | takes `writable: bool` | as passed |
| `FileDictionaryBackend` | N/A | always `true` (in-memory) |
| `MarisaDictionaryBackend` | N/A | always `false` (static trie) |
| `HunspellDictionaryBackend` | N/A | always `false` (static format) |
| `MarisaNgramBackend` | N/A | always `false` (immutable trie) |

All write methods check `self.writable` first and return an error if false.

#### B2: `build_sqlite` writability decision

`build_sqlite` determines writability from how the path was resolved:
- User-provided explicit path from config → `true`
- Path resolved from `LanguagePaths` system patterns → `false`

---

### Part C: DictionaryBackend trait — write methods

```rust
pub trait DictionaryBackend: Send + Sync {
    // ... existing methods ...

    /// Whether this backend supports write operations.
    fn is_writable(&self) -> bool { false }

    /// Add a word to the dictionary.
    ///
    /// For backends that store frequency, `frequency` is used.
    /// For backends that only track membership (MARISA, Hunspell),
    /// `frequency` is ignored.
    ///
    /// `allow_existing`: if true, overwrite when the word exists;
    /// if false, return Err when the word already exists.
    fn add_word(&self, word: &str, frequency: f64, allow_existing: bool)
        -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Err("not supported".into())
    }
}
```

Per-backend implementation:

| Backend | `is_writable` | `add_word` |
|---|---|---|
| `SqliteDictionaryBackend` | `self.writable` | `INSERT` / `INSERT OR REPLACE` per `allow_existing` |
| `FileDictionaryBackend` | `true` | in-memory, respects `allow_existing` |
| `MarisaDictionaryBackend` | `false` | error (immutable) |
| `HunspellDictionaryBackend` | `false` | error (immutable) |
| `MergedDictionary` | any inner is writable | delegates to first writable backend |

---

### Part D: NgramBackend trait — write methods

```rust
pub trait NgramBackend: Send + Sync {
    // ... existing methods ...

    /// Whether this backend supports write operations.
    fn is_writable(&self) -> bool { false }

    /// Increase the frequency of an n-gram by `delta`.
    ///
    /// `ngram` is the full sequence including context and next word
    /// (e.g., `["hello", "world"]` for bigram "hello world").
    /// For unigrams, `ngram` is `["world"]`.
    ///
    /// `save_unknown`: if true, create the n-gram with frequency = `delta`
    /// when it doesn't exist; if false, return Err for unknown n-grams.
    fn increase_ngram_frequency(
        &self,
        ngram: &[&str],
        delta: f64,
        save_unknown: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Err("not supported".into())
    }
}
```

For `SqliteNgramBackend`:
- Bigram `["hello", "world"]` → `UPDATE ngrams SET frequency = frequency + delta WHERE context_1 = 'hello' AND next_word = 'world'`
- If row doesn't exist + `save_unknown=true` → `INSERT INTO ngrams (context_1, next_word, frequency) VALUES ('hello', 'world', delta)`
- Unigram `["world"]` → `UPDATE ngrams SET frequency = frequency + delta WHERE context_1 IS NULL AND next_word = 'world'`

For `MarisaNgramBackend`: always returns error (MARISA trie is immutable once built).

---

### Part E: Frequency update on word selection

When the user picks a predicted word, the caller should:

```rust
// Given context ["the", "cat"] and chosen word "sat":
// Bump the n-gram for the full sequence (trigram)
ngram_backend.increase_ngram_frequency(&["the", "cat", "sat"], 1.0, true)?;
// Bump the bigram
ngram_backend.increase_ngram_frequency(&["cat", "sat"], 1.0, true)?;
// Bump the unigram (also updates dictionary frequency since they share the table)
ngram_backend.increase_ngram_frequency(&["sat"], 1.0, true)?;
```

For backends where dict and ngram share the same table (Presage), the unigram bump covers both dictionary frequency and ngram frequency in a single write.

For backends where dict and ngram are separate, the caller may also need to call `dict_backend.add_word("sat", 1.0, true)` to ensure dictionary membership.

This is the caller's responsibility. The plan just provides the primitives.

---

### Part F: External interfaces

The following external interfaces currently don't expose any write methods. They need to be updated to expose `add_word` and `increase_ngram_frequency`:

1. **CLI** (`src/cli.rs`) — add subcommands for `word-add` and `ngram-bump`
2. **Stdio daemon** (`src/daemon/handlers.rs`, `src/daemon/protocol.rs`) — add `word_add` and `ngram_bump` JSON-RPC methods
3. **DBus** (`src/clients/dbus.rs`) — add corresponding D-Bus methods

Parameters for the external methods:
- `word_add`: `{ word, frequency, allow_existing }`
- `ngram_bump`: `{ ngram: [str], delta, save_unknown }`

---

## Implementation Order

- [x] Add `ngram_unigram_mode` + `writable` to `SqliteDictionaryBackend` + new constructor + adapted queries
- [x] Add `writable` to `SqliteNgramBackend` constructor
- [x] Update `SqliteFormat::Presage` preset — no separate `table`, both capabilities from ngrams
- [x] Add `is_writable` + `add_word(word, freq, allow_existing)` to `DictionaryBackend` trait with default impls
- [x] Implement `add_word` on `SqliteDictionaryBackend`, `FileDictionaryBackend`
- [x] Add `is_writable` + `increase_ngram_frequency(ngram, delta, save_unknown)` to `NgramBackend` trait with default impls
- [x] Implement `increase_ngram_frequency` on `SqliteNgramBackend`
- [x] Wire `build_sqlite` — shared table for presage, writability from path source
- [x] Update `MergedDictionary` to delegate `add_word`
- [ ] Add external interface methods (CLI, stdio, dbus)
- [ ] Tests for new functionality

## Done
All core backend changes implemented, all 62 tests pass.

## Remaining
- External interfaces (CLI, stdio, dbus) need to expose the new write methods
- Tests for `increase_ngram_frequency` and `add_word` through the traits


## Constraints
- `cargo fmt` before every commit
- Small incremental commits, code files only
- System files must never be written to — enforce via `writable` flag checked in every write method
- `add_word` and `increase_ngram_frequency` are strictly separate: one for membership/initial freq, one for bumping
- Frequency values must remain non-negative
