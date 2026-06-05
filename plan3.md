# PLAN 3 — Config unification & client mode

> Created: 2026-06-05
> Assumes: All code from session-ses_1680.md is already applied (MarisaPredictor, ngram_path, etc.)

---

## Completed (prior sessions)

1. **MarisaDictionaryBackend** — `src/dictionary/marisa.rs`
   - `from_file(&Path)` loads `.marisa` trie
   - `contains()` via `Trie::lookup()`
   - `query_prefixes()` via `Agent::set_query_str()` + `predictive_search()` while loop
   - 2 tests pass

2. **MarisaPredictor** — `src/prediction/marisa.rs`
   - Loads `.trie` + `.counts` companion files
   - Counts: 4-byte magic header (`0x0098a15a`) + flat u32 LE array indexed by trie key ID
   - Trie keys: `"1 <word>"` (unigram), `"2 <w1> <w2>"` (bigram), `"3 <w1> <w2> <w3>"` (trigram)
   - `predict_next(context, max)` → unigrams for `[]`, bigrams for `[w1]`, trigrams for `[w1, w2]`
   - Aggregates counts for same next-word across different suffixes
   - Confidence = count / total (normalized)
   - 5 tests pass

3. **Build wiring** — `src/backends/build.rs`
   - `build_marisa()` returns `(dict, None, predictor)` where predictor = `MarisaPredictor` when `Capability::Ngrams`
   - `build_marisa_predictor()` resolves ngram files via: explicit `ngram_path` → dict dir companion → LanguagePaths patterns

4. **Config plumbing** — `src/backends/mod.rs`
   - `BackendDef` has `ngram_path: Option<String>` (serde default)
   - `ResolvedBackendDef` has `ngram_path: Option<String>`
   - `resolve_marisa_backend()` handles `enable_ngrams` → inserts `Capability::Ngrams`

5. **LanguagePaths** — `src/dictionary/paths.rs`
   - Added `system_marisa_ngram_trie_patterns`, `user_marisa_ngram_trie_patterns`
   - Added `system_marisa_ngram_counts_patterns`, `user_marisa_ngram_counts_patterns`
   - Added `resolve_marisa_ngram_trie_files()`, `resolve_marisa_ngram_counts_files()`
   - Default patterns: `database_{lang}/ngrams.trie`, `database_{lang}/ngrams.counts`

6. **Test fix** — `src/prediction/marisa.rs` test builder
   - Fixed: counts file must be indexed by trie's *internal key ID*, not insertion order
   - Use `trie.lookup()` to resolve each key's ID after building

---

## Current state (what you need to know)

### Config file
- Path: `~/.config/verbisage/daemon.conf` (function `default_config_path()` in `src/config.rs`)
- Format: TOML, parsed by `toml` crate
- `Config` struct (`src/config.rs:33-46`): fields `backend`, `language_default`, `paths`, `sqlite`, `backends`
- `load_config(path)` → `Option<Config>`

### CLI args
- `SharedArgs` struct (`src/cli.rs:21-82`) — flattened into both binaries
- `--dbus` is a `bool` flag (line 73)
- `apply_defaults(config)` merges config file → CLI args, applies hardcoded defaults

### Client binary (`src/bin/verbisage.rs`)
- `main()`: loads config, applies defaults, dispatches to mode handlers
- Each mode handler (`run_check`, `run_correct`, `run_predict`, `run_query`) branches on `shared.dbus`:
  - `true` → D-Bus client calls
  - `false` → local backend

### Daemon binary (`src/bin/verbisaged.rs`)
- `main()`: loads config, applies defaults, builds `DaemonConfig`
- Branches on `shared.dbus`: `true` → D-Bus server, `false` → stdio server

### Key files to modify
| File | What changes |
|------|-------------|
| `src/config.rs` | Rename config path `daemon.conf` → `config.toml`; add `[client]` section |
| `src/cli.rs` | Replace `dbus: bool` with `mode: Option<ClientMode>`; merge config default |
| `src/bin/verbisage.rs` | Replace `shared.dbus` branches with `shared.mode` match |
| `src/bin/verbisaged.rs` | No mode changes (daemon ignores `[client]` section) |

---

## Tasks

### Task 1: Rename config file ✅
- Changed `default_config_path()` from `daemon.conf` to `config.toml`

### Task 2: Add `[client]` section to Config ✅
- Added `ClientConfig` struct with `mode: Option<String>` field
- Added `client: Option<ClientConfig>` to `Config` struct
- Valid values: `"standalone"` (default), `"dbus"`

### Task 3: Replace `--dbus` with `--mode` ✅
- Added `ClientMode` enum (`Standalone`, `Dbus`, `Stdio`) with `ValueEnum` derive
- In `SharedArgs`: replaced `dbus: bool` with `mode: Option<ClientMode>`
- In `apply_defaults()`: resolve mode from config `[client].mode`
- Validates mode values (warns on invalid)

### Task 4: Update client binary dispatch ✅
- Each `run_*` function takes `client_mode: ClientMode` parameter
- Default: `Standalone` (local backend)
- `--mode dbus` CLI flag overrides config

### Task 5: Update daemon binary ✅
- Replaced `--dbus` with unified `--mode`
- Valid modes for daemon: `stdio` (default), `dbus`
- `standalone` is rejected with error message

### Task 6: Tests & build ✅
- `cargo check --all-targets` — clean
- `cargo test --features marisa` — 49 passed, 0 failed, 1 ignored

---

## Mode resolution

Each binary resolves its own mode independently. Resolution order:
1. **CLI `--mode`** (highest priority, overrides everything)
2. **Config section** — client reads `[client].mode`, daemon reads `[daemon].mode`
3. **Hardcoded default** — client defaults to `standalone`, daemon defaults to `stdio`

`SharedArgs::resolve_mode(cli_mode, config_mode, default_mode)` handles this in `src/cli.rs`.

## Example config

```toml
backend = "file"
language_default = "en_US"

[client]
mode = "standalone"  # or "dbus"

[daemon]
mode = "stdio"  # or "dbus"

[paths]
system_dir = "/usr/share/verbisage"
user_dir = "~/.local/share/verbisage"
```
