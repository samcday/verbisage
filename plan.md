# verbisage — Plan

## Objective

A multi-purpose word-list / word-frequency library that wraps disparate dictionary
formats (plain word-lists, frequency files, Hunspell dictionaries, SQLite n-gram
tables) behind three clean orthogonal trait interfaces:

| Area | Trait | Methods |
|------|-------|---------|
| **Swipe‑typing** | `DictionaryBackend` | `query_prefixes`, `get_frequency`, `contains` |
| **Spell‑check** | `SpellChecker` | `is_correct`, `suggest` |
| **Next‑word prediction** | `Predictor` | `predict_next` |

These traits are then exposed through three inter-changeable surface areas:

1. **Native Rust crate** (`src/lib.rs`) — the core; re‑exports everything.
2. **Daemon** (`src/bin/verbisaged.rs`) — line‑delimited JSON over stdin/stdout
   (and optionally D‑Bus behind a Cargo feature).
3. **FFI** (`src/ffi.rs`) — C ABI (deferred to a later iteration).

---

## Directory layout

```
verbisage/
  Cargo.toml
  plan.md
  src/
    lib.rs                  # crate root; module declarations + re‑exports
    dictionary/
      mod.rs                # DictionaryBackend trait, DictionaryQuery,
                            # DictionaryResult, SharedQueryCache
      file.rs               # FileDictionaryBackend     (in‑memory HashMap)
      sqlite.rs             # SqliteDictionaryBackend   (rusqlite, opt‑in)
      compact.rs            # CompactDictionary         (index‑based reference impl)
      subsequence.rs        # subsequence‑variant generator (shared helper)
    spellcheck/
      mod.rs                # SpellChecker trait
      dictionary.rs         # SpellChecker impl for any DictionaryBackend
      sqlite.rs             # SpellChecker impl for SqliteDictionaryBackend (opt‑in)
      hunspell.rs           # SpellChecker impl via zspell (opt‑in)
    prediction/
      mod.rs                # Predictor trait, Prediction struct
      sqlite.rs             # Predictor impl for n‑gram SQLite tables (opt‑in)
      frequency.rs          # Predictor impl: top‑N by frequency, context‑free
    daemon/
      mod.rs                # DaemonConfig, error types, dispatch entry‑point
      protocol.rs           # Request / Response / Error serde types
      handlers.rs           # Handler trait + dispatch logic
      stdio.rs              # stdin/stdout event loop
    bin/
      verbisaged.rs         # daemon binary entry‑point
    ffi.rs                  # C ABI (future)
```

---

## Core traits

### DictionaryBackend  (src/dictionary/mod.rs)

```rust
pub trait DictionaryBackend: Send + Sync {
    fn query_prefixes(&self, queries: &[DictionaryQuery]) -> Vec<DictionaryResult>;
    fn get_frequency(&self, word: &str) -> f64;
    fn contains(&self, word: &str) -> bool;
}
```

Supporting types copied from `libswipetyping`:

```rust
pub struct DictionaryQuery {
    pub prefix: Option<String>,       // words must start with this
    pub suffix: Option<String>,       // words must end with this
    pub min_length: Option<usize>,
    pub max_length: Option<usize>,
}

pub struct DictionaryResult {
    pub word: String,
    pub confidence: f64,              // 0.0 – 1.0, or -1.0 if unknown
}
```

**Backend implementations:**

| Backend | File | Strategy |
|---------|------|----------|
| `FileDictionaryBackend` | `dictionary/file.rs` | In‑memory `HashMap` + sorted `Vec` + prefix/length indices. Full‑scan queries (adequate for dictionaries < 1 M entries). |
| `SqliteDictionaryBackend` | `dictionary/sqlite.rs` | Wraps a `rusqlite::Connection`. Builds batch SQL queries with `LIKE` / `LENGTH()`. Created index on word column automatically. |
| `CompactDictionary` | `dictionary/compact.rs` | Single `Vec<String>` with `WordIndex` look‑aside caches (first‑letter, length). Reference implementation for memory‑conscious environments. |

### SpellChecker  (src/spellcheck/mod.rs)

```rust
pub trait SpellChecker: Send + Sync {
    fn is_correct(&self, word: &str) -> bool;
    fn suggest(&self, word: &str) -> Vec<String>;
}
```

| Impl | File | Strategy |
|------|------|----------|
| `DictionarySpellChecker<B>` | `spellcheck/dictionary.rs` | Generic over `DictionaryBackend`. `is_correct` → `backend.contains`. `suggest` → subsequence matching + prefix‑based edit distance. Clients provide the `max_edit_distance`. |
| `SqliteSpellChecker` | `spellcheck/sqlite.rs` | `is_correct` → `SELECT 1 … LIMIT 1`. `suggest` → `LIKE` with wildcard patterns or a registered Levenshtein function. |
| `HunspellSpellChecker` | `spellcheck/hunspell.rs` | Wraps `zspell::Dictionary`. `is_correct` → `zspell::Dictionary::check`. `suggest` → `zspell::Dictionary::suggest`. Thread‑safe, caches the `.aff`/`.dic` behind `Arc`. |

### Predictor  (src/prediction/mod.rs)

```rust
pub struct Prediction {
    pub word: String,
    pub confidence: f64,  // 0.0 – 1.0
}

pub trait Predictor: Send + Sync {
    fn predict_next(&self, context: &[&str], max_suggestions: usize) -> Vec<Prediction>;
}
```

| Impl | File | Strategy |
|------|------|----------|
| `SqlitePredictor` | `prediction/sqlite.rs` | Queries an n‑gram table with configurable column names. Builds `WHERE col IN (…) ORDER BY freq DESC`. |
| `FrequencyPredictor<B>` | `prediction/frequency.rs` | Generic over `DictionaryBackend`. Ignores context, returns top‑N words by `get_frequency`. Useful fallback when no n‑gram data is available. |

---

## Daemon protocol

### Transport

Line‑delimited JSON over stdin / stdout.  One request or response per line,
no trailing commas, no top‑level array — pure line‑delimited JSON.

### Request format

```json
{"id":1, "method":"is_correct", "params":{"word":"helo"}}
{"id":2, "method":"suggest",    "params":{"word":"helo", "max":5}}
{"id":3, "method":"query",      "params":{"prefix":"hel", "suffix":"o", "min_len":4, "max_len":6}}
{"id":4, "method":"predict",    "params":{"context":["how","are"], "max":3}}
{"id":5, "method":"frequency",  "params":{"word":"hello"}}
```

### Response format

```json
{"id":1, "result":false}
{"id":2, "result":["hello","halo","help"]}
{"id":3, "result":[{"word":"hello","confidence":100.0},{"word":"helo","confidence":50.0}]}
{"id":4, "result":[{"word":"you","confidence":0.8}]}
{"id":5, "result":100.0}
{"id":null, "error":"parse error: invalid JSON"}
```

### Startup

The binary reads `--backend` (or `VERBISAGE_BACKEND` env) to select and
configure the dictionary source.  Supported backends:

```
verbisaged --backend file --path /usr/share/dict/words
verbisaged --backend sqlite --path ngrams.db --table unigrams --word-col term --freq-col count
verbisaged --backend hunspell --affix /usr/share/hunspell/en_US.aff --dict /usr/share/hunspell/en_US.dic
```

---

## Implementation order

| Step | What | Commit |
|------|------|--------|
| 1 | `plan.md` + directory structure + updated `Cargo.toml` | `e11cc3f` — scaffold |
| 2 | `dictionary/` module (traits, `FileDictionaryBackend`, `CompactDictionary`, subsequence helper) | 2nd commit |
| 3 | `dictionary/sqlite.rs` (feature‑gated) | 3rd commit |
| 4 | `spellcheck/` module (trait + all three impls) | 4th commit |
| 5 | `prediction/` module (trait + both impls) | 5th commit |
| 6 | `daemon/` module + `bin/verbisaged.rs` | 6th commit |
| 7 | Wire `lib.rs`, strip `main.rs`, verify with `cargo check` | 7th commit |

---

## Design decisions & attention points

- **Thread safety** — All three traits require `Send + Sync`.  `SqliteDictionaryBackend` wraps
  a single `Connection` behind `&self` (no `Mutex` — rusqlite `Connection` is `Send` but
  not `Sync`).  Wrap in `Mutex` or pool accordingly.  `.clone()` opens a new `:memory:` DB
  for the copy — document this.
- **Unicode** — No implicit normalisation.  Callers must NFC/NFD‑normalise before passing
  strings to any API.
- **`zspell` cost** — `suggest` is expensive.  Cache the `zspell::Dictionary` in an `Arc`.
- **Feature flags** — `sqlite` / `hunspell` / `dbus` are opt‑in.  Default features include
  `sqlite` and `hunspell`.
- **Dependencies** — Keep the dependency tree minimal.
  - `serde` + `serde_json` (always)
  - `rusqlite` (bundled feature, optional)
  - `zspell` (optional)
  - `zbus` (optional, deferred)
- **No beginner comments** — No `// TODO: implement` or `// This function does X`.
  Use clean doc comments (`///`) to describe contracts; rely on clear naming for
  the rest.
