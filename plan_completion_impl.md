# Completion implementation plan (`bad_completion` salvage + rewrite)

## 0. Goals and locked decisions

* Follow repo structure: new `src/completion/` module — shared types in `mod.rs`,
  transplanted algorithm in `prefix.rs`, HeliBoard engine in `android.rs` later.
* Transplant rule for `prefix.rs`: **move, don't modify** — the branch's
  `src/completion.rs` logic lands in `prefix.rs` with semantics intact; only
  structural fitting is allowed (module split, trait wrapper, type moves, test
  relocation). All behavior changes listed below apply to `android.rs` and the
  transport layer (§4), NOT to the transplant.
* Hook up n-grams (`NgramBackend`); handle `prefix: None` (no pretext → top unigrams);
  degrade gracefully when the backend has no n-grams or no unigram data
  (all deferred to `android.rs`; the transplant keeps the branch's
  empty-in → empty-out behavior).
* Honor the caller's `max` end-to-end. Caps live at the transport edge as
  **config knobs** (CLI > config file > default): `Complete` defaults to
  **1 000**, `query_*` to **200 000**; oversized requests get `InvalidArgs`,
  never a silent clamp. (Branch's `max.min(100)` in 4 places is rejected.)
* Timeouts are configurable and **default to 5 s** (branch's hardcoded 1 s / 500 ms
  are rejected).
* Salvage the Patricia trie integration; **exclude all markdown/docs/packaging**
  (`DOWNSTREAM.md`, `README.md`, `LICENSE-APACHE`, `contrib/*`).
* Completion **will** be exposed via D-Bus (`Complete`), against the new engine.
* `query_limited` keeps a default impl (truncate-after-scan fallback); native
  efficient overrides for **patricia + sqlite only** for now (file/marisa/merged
  later).
* Keep the `bad_completion` XDG/`/etc` config-path change and keep
  `pub mod completion;` in `lib.rs`.

## 1. Critical review of `HEAD..bad_completion -- src` (summary)

* `src/completion.rs` is a flat file with a free function — violates the requested
  `src/completion/{mod.rs,android.rs}` structure; duplicates `suggest_edits`
  single-edit logic with hardcoded a-z weights; has **no n-gram hookup**, returns
  empty on empty input (wrong: should be top unigrams), no degradation matrix.
* `max.min(100)` in `completion.rs:22,114`, `daemon/dbus.rs:87,205`, `swipe.rs`.
* Hardcoded `Duration::from_secs(1)` (D-Bus swipe deadline) and `SEARCH_BUDGET`
  500 ms (`swipe.rs`); convoluted nested `spawn_blocking` + `timeout`.
* `DictionaryBackend::query_limited` default impl is **not** performant
  (full `query_prefixes` + truncate); only sqlite (SQL `LIMIT` + LIKE escaping —
  genuinely good) and patricia (streaming bounded insert — good) have efficient
  overrides. `is_empty()` addition + `loaded = any(!is_empty)` fix in `build.rs`
  are good and kept.
* Unrelated baggage to exclude: `src/swipe.rs` + `drift_type` git dep + `swipe`
  feature, D-Bus swipe machinery, `stdio.rs` transport enforcement,
  `verbisaged.rs` `exit(1)`, `Cargo.toml` rusqlite-bundled/cdylib removals.

## 2. Cherry-pick sequence (one at a time, amend to drop unwanted parts)

Branch commits (oldest → newest):

1. `7d5ffd6` — Patricia backend + bounded D-Bus queries
2. `a27c8f6` — completion ranking + sqlite bounded query
3. `6d211b0`, `040dd99` — docs only → **skip**
4. `b325ab6` — submodule bump to swipe pin → **skip** (keep pick-1 pin)
5. `825d0cb` — swipe prototype → **skip entirely**

### Pick 1: `7d5ffd6`
* `git cherry-pick 7d5ffd6`
* Drop unrelated hunk (whole-file safe): `git checkout HEAD^ -- src/bin/verbisaged.rs`
  (drops `exit(1)`). **Keep** `src/config.rs` (XDG change stays per decision).
* `Cargo.toml` needs a targeted fixup inside the same amend (whole-file checkout
  would nuke the wanted `patricia` feature): keep `patricia_dict` path-dep +
  `patricia` feature + submodule, but **restore** `rusqlite features=["bundled"]`
  and `crate-type = ["rlib", "cdylib"]`.
* Keep: `src/dictionary/patricia.rs`, `BackendType::Patricia` wiring
  (`backends/mod.rs`, `backends/build.rs`), `is_empty()` + `query_limited`
  trait defaults + `Arc` forwards, file/hunspell `is_empty`, `loaded` fix,
  `QueryLimited` D-Bus/client/handler plumbing, lang-tag validation,
  `dictionary_queries` refactor. The inherited `min(100)` clamp in
  `query_limited` stays temporarily; reworked to the 200k knob in §4.
* `git commit --amend` (optionally `-m` note: XDG kept; exit-change dropped).
* Verify: `cargo build --locked --no-default-features --features sqlite,dbus,patricia`
  and `cargo test` with those features.

### Pick 2: `a27c8f6` (as built: full pick + module split, nothing dropped)
* `git cherry-pick a27c8f6`
* Kept verbatim: D-Bus `Complete` endpoint + client + handler (still wired to the
  transplanted free function via `pub use prefix::complete`; reworked against
  the engine + 200k knob in §4), `stdio.rs` transport enforcement (kept for
  feature parity), `sqlite.rs` +120, `lib.rs`, patricia test assert (resolves
  through the re-export, passes unchanged).
* Only structural change in the amend: `git rm src/completion.rs`, add
  `src/completion/mod.rs` + `src/completion/prefix.rs` per §3 (transplant,
  semantics frozen).
* Amend message: original message kept + paragraph noting the module split.
* Verify: build + tests as above (73 lib tests + 2 doctests); `git status` shows
  no `*.md`, `contrib/`, `swipe`, or `drift_type` residue.

## 3. New module layout (lands in the pick-2 amend as buildable code)

`src/lib.rs` keeps `pub mod completion;` unchanged.

### `src/completion/mod.rs` (shared, stable across the later android switch)
* `CompletionCandidate { word: String, score: f64, is_exact: bool }`
  (convert to/from `DictionaryResult` at the handler edge).
* `CompletionConfig { max_complete_results: usize (transport cap knob, default
  1_000), max_query_results: usize (transport cap knob, default 200_000),
  response_deadline: Duration (default 5 s), search_budget: Duration
  (default 5 s) }` + `Default`.
* `CompletionEngine` trait (`Send + Sync`):
  `fn complete(&self, prefix: Option<&str>, max: usize) -> Vec<CompletionCandidate>`
  — `None` = no pretext → top-unigram path; `max` honored exactly.
* Shared helpers: the transplanted ranking body stays inline in `prefix.rs`
  untouched; `mod.rs` holds only new scaffolding (trait, config, candidate
  type, n-gram interpolation helper for `android.rs`). Helper extraction happens
  with the `android.rs` commit if it needs them.
* Shared n-gram interpolation helper (linear interpolation over `NgramBackend`,
  same formula as `spellcheck/suggest.rs::interpolate_score`) — new scaffolding
  placed here so `android.rs` reuses it later; the `prefix.rs` transplant does
  NOT use it.
* `CompletionConfig` (cap knobs default 1_000 / 200_000, timeouts default 5 s) exists from
  the start as the stable shape, but the transplant only passes `max` through as
  before — knob enforcement and timeout plumbing land in §4 / `android.rs`.
* `pub mod prefix;` now; `pub mod android;` added later; re-export both engines.

### `src/completion/prefix.rs` (transplant of branch `src/completion.rs`, semantics frozen)
* `PrefixCompleter` wrapping the transplanted `complete()` logic, exposed via
  `CompletionEngine`. Structural fitting only: move the function body as-is,
  wrap it in the struct/trait, relocate its unit tests here unchanged.
* Logic kept byte-for-byte where possible: bounded prefix lookup via
  `query_limited`, single-edit candidate merge **before** truncation (weights
  1.0 intact / 0.9 transposition + doubled-letter edits / 0.65 other
  insert-delete / 0.5 substitution), known-word + `<3`-char correction
  suppression, `ln_1p` prior, sort, truncation — **including the existing
  `min(100)` truncations and the empty-input → empty-output rule**.
* Minimal signature adaptation only: the trait takes `prefix: Option<&str>`;
  the transplant maps `None` to the branch's empty-input path (returns empty),
  preserving behavior. No n-gram hookup, no top-unigram fallback, no cap/knob
  changes in the transplant — those land with `android.rs` (§4 and below).
* Branch unit tests move here verbatim (ranking-before-truncation, valid-word /
  short-fragment, NaN/Inf frequencies, determinism).

### Later: `src/completion/android.rs`
* HeliBoard completion-mode engine on the same trait: trie-prefix traversal,
  `CT_COMPLETION` costs (`COST_COMPLETION = 0.00624`,
  `COST_FIRST_COMPLETION = 0.4836`), compound
  `spatial * 0.1524 + language * 1.1214` distance, output score
  `1.0 + normalized * 0.1`, bigram/trigram scoring via the shared `mod.rs`
  helper. Default flips from `PrefixCompleter` to `AndroidCompleter` in one
  place (constructor/`Default`/handler wiring); `prefix.rs` stays as the
  alternative impl.
* Open question: should `prefix.rs` keep single-edit correction merging, or go
  pure-prefix (corrections left to spellchecker/`android.rs`)? Keeping preserves
  tested `helo → hello` behavior; stripping makes it a smaller stepping-stone.

## 4. D-Bus completion exposure (modify the kept impls, against the new engine)
* Rework the kept `DaemonHandler::complete` / `VerbisageDbus::complete` /
  `DbusClient::complete` (currently wired to the transplanted free function)
  onto `CompletionEngine`: first `PrefixCompleter`, switched to
  `AndroidCompleter` when it lands. Protocol params; stdio `complete` /
  `query_limited` protocol entries + `StdioClient` methods required
  (parity with D-Bus — decided).
* Cap knobs (`--max-complete-results` / `--max-query-results`, config-file
  `[daemon]` section, defaults 1_000 / 200_000) enforced in
  `DaemonHandler::complete` / `query_limited`: `max == 0` → `[]` (unchanged);
  `max` over cap → `Err` (surfaces as `InvalidArgs`/daemon error); the
  inherited `min(100)` clamps in `Complete`/`QueryLimited` are replaced by
  pass-through + handler enforcement.
* Input validation consistent with branch (size/control-char/whitespace rules)
  but returning errors, not silent truncation.

## 5. Verification
* After each pick and after §3/§4: `cargo fmt`, build + test with
  `--no-default-features --features sqlite,dbus,patricia`; confirm D-Bus
  `helo → hello`, limits, ordering, invalid-lang and missing-data errors.
* Confirm excluded residue absent: no `*.md` additions, no `contrib/`, no
  `src/swipe.rs`, no `drift_type`, bundled sqlite + cdylib intact.
* Degradation matrix covered by tests at the `android.rs` stage: full (ngram) →
  frequency-only → alphabetical → empty; `None` pretext → top unigrams;
  `max` fidelity (request N, get exactly the engine's top N, no clamp).
  The transplant stage only needs its relocated branch tests passing unchanged
  (plus the tree building with the new module layout).
