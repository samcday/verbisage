# verbisage — Plan

## Objective

A multi-purpose word-list / word-frequency library with three trait interfaces,
a daemon (stdin/stdout JSON protocol + D-Bus transport), CLI one-shot modes,
multi-language path resolution, and typed client libraries.

## Coding conventions (mandatory)

- Rust edition 2024
- **No beginner comments** (`// TODO` or `// This function does X`) — use clean doc comments (`///`) for contracts only
- `cargo fmt` before every commit
- Incremental commits with `git add` of only the changed files
- All traits require `Send + Sync`
- `rusqlite::Connection` wrapped in `Mutex` (not a pool)
- `~` expansion on all user-facing paths
- Feature flags: `sqlite`, `hunspell` (both default on), `dbus` (`zbus` v5 + `tokio`, optional with `p2p` feature for tests)

## Architecture

### Core traits

| Area | Trait | Methods |
|------|-------|---------|
| Dictionary | `DictionaryBackend` | `query_prefixes`, `get_frequency`, `contains` |
| Spell-check | `SpellChecker` | `is_correct`, `suggest` |
| Prediction | `Predictor` | `predict_next` |

### DaemonHandler (lazy per-language caching)

`DaemonHandler` stores:

- `config: DaemonConfig` — CLI-provided configuration (data dirs, backend kind, sqlite params, etc.)
- `cache: Mutex<HashMap<String, Arc<CachedBackend>>>` — per-language backend cache
- `default_lang: String` — fallback when request has no `lang` override

`CachedBackend` holds the triple `(dictionary, spellchecker, predictor)` for one language.

**Two constructors:**

1. `DaemonHandler::new(dict, sc, predictor, default_lang)` — pre-populates cache with one backend. Used by tests and one-shot CLI modes.
2. `DaemonHandler::with_config(config)` — starts with empty cache. Backends are lazily created per-language when first requested. Used by daemon modes.

**Lazy backend resolution** (`get_or_load_backend(lang)`):

1. Check cache → return cached `Arc<CachedBackend>` if found
2. Build backend for `lang`:
   - Clone `config.language_paths`, set language to requested tag
   - Match `config.backend`:
     - **File**: `resolve_dict_files()` → `FileDictionaryBackend::from_multiple_files()`
     - **Sqlite**: `resolve_sqlite_files()` → `SqliteDictionaryBackend::from_sqlite()`
     - **Hunspell**: `from_tag(lang)` or `from_files(affix, dict)`
   - On any failure → create `FileDictionaryBackend::new()` with no spellchecker
3. Insert into cache, return `Arc`

**`--path` in daemon mode**: if `config.eager_path` is set, `with_config()` pre-populates the cache with `default_lang` loaded from that path eagerly.

**`lang` parameter on every request**:
- `DaemonRequest.lang: Option<String>` (optional JSON field, `#[serde(default)]`)
- `resolve_lang(Some(custom))` → uses `custom` to select/cache the right backend
- `resolve_lang(None)` → uses `default_lang`
- DBus interface: `lang: &str` is a required parameter on all 5 methods

### DaemonConfig (`src/daemon/config.rs`)

```rust
pub struct DaemonConfig {
    pub backend: BackendKind,              // file | sqlite | hunspell
    pub language_paths: LanguagePaths,     // data dirs + overrides from CLI
    pub default_lang: String,
    pub sqlite_table: String,              // "words"
    pub sqlite_word_col: String,           // "word"
    pub sqlite_freq_col: String,           // "frequency"
    pub hunspell_affix: Option<PathBuf>,   // --affix override
    pub hunspell_dict: Option<PathBuf>,    // --dict override
    pub eager_path: Option<PathBuf>,       // --path → eager load for default_lang
    pub eager_system_dict: Option<String>,
    pub eager_user_dict: Option<String>,
}
```

`BackendKind` is a `ValueEnum` used by clap for `--backend`. Publicly exported from `verbisage::daemon::BackendKind`.

`DaemonConfig::from_cli(...)` constructs from parsed CLI args. `DaemonConfig::default_for(lang)` creates a minimal config for tests.

### LanguagePaths (`src/dictionary/paths.rs`)

Resolves system + user dictionary files for a given language using filename patterns
with `{lang}` templating. Falls back from `en_US` → `en` per-pattern.

```
LanguagePaths::new("de_DE")
    .with_system_dir(...)
    .with_user_dir(...)
    .resolve_dict_files()     → ["de_DE.dic", "de_DE.freq", ...]
    .resolve_sqlite_files()   → ["database_de_DE.db", "lm_de_DE.db"]
```

### Daemon transports

1. **Stdio** (`src/daemon/stdio.rs`): Line-delimited JSON on stdin/stdout. Event loop reads requests, calls `handler.handle(req)`, writes responses.
2. **D-Bus** (`src/daemon/dbus.rs`): `#[interface(name = "org.verbisage.Dictionary1")]` with 5 async methods. Registers on session bus at `/org/verbisage/Dictionary`.
3. **`--dbus` flag**: In daemon mode, serves D-Bus. In one-shot modes, connects to a running D-Bus daemon.

### Clients

1. **`StdioClient`** (`src/clients/stdio.rs`): Spawns `verbisaged` as subprocess, JSON protocol over pipes.
2. **`DbusClient`** (`src/clients/dbus.rs`): Session bus or P2P connection (tests).

## CLI entrypoints (`src/bin/verbisaged.rs`)

| Mode | Behavior |
|------|----------|
| `Daemon` | Creates `DaemonConfig::from_cli()` + `DaemonHandler::with_config()`. Never exits on missing files. |
| `Check` | Eager `open_backend(cli, lang)`. Exits 1 on failure. |
| `Correct` | Same as Check. |
| `Query` | Same as Check. |
| `Predict` | Not yet wired. |

`open_backend()`, `open_file_backend()`, `open_sqlite_backend()` are one-shot helpers
that return `(Box<dyn DictionaryBackend>, Option<Box<dyn SpellChecker>>)` eagerly.

## Test strategy

- Unit tests for backends, paths, subsequence, prediction
- `StdioClient` test spawns real `verbisaged` binary as subprocess
- `DbusClient` P2P test uses `UnixStream::pair()` + `zbus::blocking::connection::Builder` with `p2p` + `server` — no session bus needed
- Tests construct `DaemonHandler::new(...)` with pre-built backends

## Implementation status

| Step | What | Commit |
|------|------|--------|
| 1 | Scaffold + traits + `FileDictionaryBackend` + `CompactDictionary` | initial |
| 2 | `SqliteDictionaryBackend` | initial |
| 3 | `SpellChecker` trait + 3 impls | initial |
| 4 | `Predictor` trait + 2 impls | initial |
| 5 | Daemon protocol + stdio loop + `DaemonHandler` | initial |
| 6 | CLI binary with `--backend`/`--mode` | initial |
| 7 | `LanguagePaths` with language fallback | initial |
| 8 | D-Bus transport + client | initial |
| 9 | StdioClient + DbusClient | initial |
| 10 | P2P integration test | initial |
| 11 | `lang` param on all requests | `42e7bb9` |
| 12 | `--language` optional, no fail on missing files | `61cee13` |
| 13 | Per-language cached backends (`DaemonConfig` + `get_or_load_backend`) | `8289d9d` |

## Next steps

1. Wire `Predictor` into daemon handler construction and CLI predict mode
2. Add `learn`/`add_word` protocol method for runtime learning
3. Config file support
4. FFI module
5. Signal handling / graceful shutdown
