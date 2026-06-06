# Plan 6: SQLite Presage Backend Rewrite

## Goal

Standardize all SQLite backends on the Presage schema and introduce a `marisa_sqlite` composite backend type.

## Presage Schema (the only SQLite format)

| Table | Columns | Role |
|---|---|---|
| `_1_gram` | `word TEXT, count INTEGER, UNIQUE(word)` | Dictionary + Unigrams |
| `_2_gram` | `word_1 TEXT, word TEXT, count INTEGER, UNIQUE(word_1, word)` | Bigrams |
| `_3_gram` | `word_2 TEXT, word_1 TEXT, word TEXT, count INTEGER, UNIQUE(word_2, word_1, word)` | Trigrams |

One DB file, one set of tables. `_1_gram` serves as dictionary AND unigram source. No separate dict table.

## Requirements

1. Presage schema is the **only** SQLite format — drop old single-table format entirely
2. Merge dict + ngram backends into a single shared backend
3. Lazy DB creation: auto-create the file + schema on first write (`add_word` or `increase_ngram_frequency`), but **only** if the file didn't exist. If the file already exists, do NOT modify the schema
4. New `marisa_sqlite` backend type: read-only system (marisa trie) + writable user (sqlite presage)

---

## Phase 1: New `PresageSqliteBackend` struct

Merge `SqliteDictionaryBackend` and `SqliteNgramBackend` into a single struct that holds the shared connection and implements both `DictionaryBackend` and `NgramBackend` (wrapped in `SmoothedPredictor`).

### 1.1. Create `src/dictionary/presage_sqlite.rs` (or rename `sqlite.rs`)

New struct:

```rust
struct PresageSqliteBackend {
    conn: SharedSqliteConnection,
    writable: bool,
    created_new_file: bool,  // track whether we created the file (for schema init)
}
```

The backend implements `DictionaryBackend` directly. For `NgramBackend`, wrap it in a newtype or implement separately and wrap in `SmoothedPredictor`.

### 1.2. Dictionary operations (from `_1_gram`)

- `contains(word)` — `SELECT 1 FROM _1_gram WHERE word = ?`
- `get_frequency(word)` — `SELECT count FROM _1_gram WHERE word = ?` → normalize to 0.0–1.0
- `query_prefixes(queries)` — `SELECT word, count FROM _1_gram WHERE word LIKE ?` (prefix/suffix/length filters)
- `add_word(word, freq, allow_existing)` — INSERT or UPDATE on `_1_gram`

### 1.3. Ngram operations (per-order table routing)

- `max_order()` → 3
- `unigram_total()` → `SELECT SUM(count) FROM _1_gram`
- `ngram_count(ngram)` → dispatch to `_1_gram`, `_2_gram`, or `_3_gram` based on `ngram.len()`
- `candidates(context, max_candidates)` → query appropriate table, group by `word`, ORDER BY count DESC, LIMIT
- `increase_ngram_frequency(ngram, delta, save_unknown)` → dispatch to appropriate table

### 1.4. Lazy schema creation

On `open(path)`:
- If file exists → open read/write, `created_new_file = false`
- If file doesn't exist → create file, `created_new_file = true`

On first write (`add_word` or `increase_ngram_frequency`):
- If `created_new_file` is true → run `CREATE TABLE` statements, then proceed
- If `created_new_file` is false → skip schema creation, proceed with INSERT/UPDATE

The `CREATE TABLE` statements:
```sql
CREATE TABLE IF NOT EXISTS _1_gram (word TEXT PRIMARY KEY, count INTEGER DEFAULT 1)
CREATE TABLE IF NOT EXISTS _2_gram (word_1 TEXT, word TEXT, count INTEGER DEFAULT 1, UNIQUE(word_1, word))
CREATE TABLE IF NOT EXISTS _3_gram (word_2 TEXT, word_1 TEXT, word TEXT, count INTEGER DEFAULT 1, UNIQUE(word_2, word_1, word))
```

### 1.5. Remove old code

- Remove old single-table format support from `SqliteNgramBackend`
- Remove `ngram_unigram_mode` path from `SqliteDictionaryBackend` (shared-table mode)
- Clean up `SqlitePreset` — only Presage schema fields remain

---

## Phase 2: Update `build_sqlite`

In `src/backends/build.rs`:

### 2.1. Simplify the function

With Presage as the only format, `build_sqlite` becomes straightforward:
1. Resolve the DB path
2. Open `PresageSqliteBackend` (lazy creation)
3. Return `(Box<dyn DictionaryBackend>, Option<Box<dyn SpellChecker>>, Option<Box<dyn Predictor>>)` where:
   - Dictionary = `PresageSqliteBackend`
   - Spell checker = `SqliteSpellChecker` wrapping the dict
   - Predictor = `SmoothedPredictor` wrapping the ngram side of `PresageSqliteBackend`

### 2.2. Connection sharing

Dict and predictor share the same `SharedSqliteConnection` inside `PresageSqliteBackend`.

---

## Phase 3: `marisa_sqlite` backend type

New `BackendType::MarisaSqlite` with config:

```toml
[backends.main]
type = "marisa_sqlite"
system_backend = "default_marisa"   # read-only trie for dict/ngrams
user_backend = "user_presage"       # writable sqlite for mutations

[backends.user_presage]
type = "sqlite"
user_dir = "~/.local/share/verbisage"
user_patterns = ["user_{lang}.db"]
```

### 3.1. Config struct

Add to `BackendDef`:
```rust
pub struct MarisaSqliteBackendDef {
    pub system_backend: String,  // backend reference name
    pub user_backend: String,    // backend reference name
}
```

### 3.2. `build_marisa_sqlite` function

1. Resolve `system_backend` → build marisa backend (dict + predictor from trie)
2. Resolve `user_backend` → build sqlite backend (`PresageSqliteBackend`)
3. Merge outputs:
   - Dictionary: `MergedDictionary([marisa_dict, sqlite_dict])`
   - Spell checker: from sqlite dict (user words matter for corrections)
   - Predictor: `MergedPredictor([marisa_predictor, sqlite_predictor])`
4. Mutations route: word-add → sqlite (first writable), ngram-bump → sqlite (first success)

---

## Phase 4: Cleanup & Tests

### 4.1. Remove dead code
- Old `SqliteNgramBackend` single-table implementation
- Old `SqliteDictionaryBackend` simple/ngram_unigram dual mode
- `SqlitePreset` enum (no longer needed — Presage is the only format)
- `per_order_tables`, `table_pattern`, `context_cols`, `next_col` config fields

### 4.2. Update `ResolvedBackendDef`
- Remove sqlite-specific fields that are no longer configurable (`table_ngrams`, `context_cols`, `next_col`, etc.)
- Keep only: `format` (can be dropped entirely if Presage is the only format), `user_dir`, `user_patterns`

### 4.3. Update `BackendDef` enum
- Add `MarisaSqlite` variant
- Simplify `Sqlite` variant (remove format preset selection)

### 4.4. Tests
- Test `PresageSqliteBackend` dict operations (contains, frequency, prefix queries, add_word)
- Test ngram operations (count, candidates, increase_frequency)
- Test lazy DB creation (file doesn't exist → create on write)
- Test existing file → no schema modification
- Test `marisa_sqlite` composite backend (merged dict, merged predictor, write routing)
- Test `SqliteSpellChecker` with Presage schema
