# Capitalization (and text normalization) handling

"cApaBiLiTy" handling: how verbisage compares text across case, accent,
and composition differences. Covers the two per-call knobs (normalization
× case folding), the registries, the lang map, case-signal scoring, and
the evidence grounding each choice.

Terminology (conversation): **input** = the incomplete word handed to
`complete()`; **context** = n-gram context words. Code keeps `prefix`
(`prefix.rs`, `PrefixCompleter`, trait params) — if "prefix" appears in
discussion, ask which is meant.

## 1. The two knobs (per call, never tied to a backend)

- **Normalization** `{none, nfc}`.
- **Case folding** `{none, ascii_lowercase, unicode_lowercase, full,
  lang_specific}`.
- Both travel per call (bundled as one `TextPrep { normalization, fold }`
  struct, instantiated separately for input and context where both exist).
  Supersedes the earlier construction-time-folder and `folder()`-on-trait
  designs — deleted, not deferred.

## 2. Registry semantics

- `none` (both knobs): byte-exact. The default everywhere, library and
  daemon included.
- `nfc`: Unicode NFC compose. First stage whenever any normalization is
  active (fixed order — case/translit ops are specified over composed
  text; observable: `É` U+00C9 → `é` U+00E9 vs decomposed `e`+mark).
  Whether the framework auto-prepends it to non-identity chains or configs
  must list it explicitly: **open**.
- `ascii_lowercase`: ASCII-only fast path. Documented footgun (non-ASCII
  passes through → likely miss); caller's responsibility.
- `unicode_lowercase`: `str::to_lowercase` (`ß→ß`, `İ→i̇`).
- `full`: Unicode full case folding (`ß→ss`, `İ→i`, `Σ/ς→σ`). The reason
  both lower variants exist.
- `lang_specific`: resolved per call via the lang map; falls back to
  `unicode_lowercase` when the language specifies nothing. Future
  language-specific implementations (e.g. `de-translit` ä→ae) plug into
  the registry + map with no API change.
- **Idempotency contract**: every entry satisfies `f(f(x)) == f(x)`,
  tested per entry — what makes pre-normalized upstream input harmless.

## 3. Defaults and resolution

- Library + daemon APIs: `none, none` (exact). CLI: `nfc, lang_specific`
  (CLI folds inputs before dispatch; daemon dispatches exact as received).
- `lang_specific` resolution per call: explicit selection > lang database >
  `unicode_lowercase`. The database is an **empty hardcode for now — not
  "no entries ever"**: language-specific entries land later, together with
  the specific algorithms (e.g. `de-translit`) they point at. Designed to
  become a loadable system file (JSON or binary); no hardcoded language
  tables in Rust beyond the map itself, ever. No TOML config surface.
- Override lives on **every API surface**: CLI flags, Rust per-call params,
  stdio protocol params, FFI params, D-Bus method params. (Helper
  *functions* stay Rust + FFI only — what crosses the other transports is
  the *selection*, not the implementation.)
- Hunspell `.aff` conversion tables (`ICONV`/`OCONV`, `TRY`) noted as a
  future native per-language source; not built now.

## 4. NFC as internal default (evidence-backed)

- All text crossing API boundaries is NFC; caller input assumed NFC;
  text-built backends convert at ingest and assume NFC on hot query paths
  (no per-query re-normalization masking caller bugs).
- Evidence: patricia codec is form-sensitive with zero normalization of
  its own (verified in code — no NFC/NFD calls, dead `fast_to_lowercase_cp`
  with no call sites); corpus dump shows composed keys (`Caf\xc3\xa9`,
  no `NFC=False` lines in the systematic stdin check).
- Patricia findings (for the record): keys exact up to the 48-code-point
  cap; scores are quantized `u8` probabilities (0–255), never counts —
  count→probability conversion happens at dictionary build time upstream;
  our backend's dual raw+lowered querying is our layer, removed under
  identity-default.

## 5. Matching architecture

- Backends compare exactly what they're given. Scan backends (file,
  compact, hunspell) support arbitrary per-call folding by folding both
  sides at match time — fully general, zero configuration. Indexed and
  prebuilt backends (sqlite, marisa, patricia) stay exact + NFC +
  dual-variant fallback, documented per backend.
- N-gram stores assume pre-folded (lowercased) keys, matching presage
  corpus convention; context folded per call with the context prep.
- **Results keep stored casing** — folding is canonicalization for
  comparison only; callers fold display text themselves via the same
  helpers. Helper *functions* live in Rust + planned FFI; every transport
  (CLI, stdio, D-Bus) instead accepts the *selection* (knob values +
  optional lang tag for `lang_specific` resolution) per request.

## 6. Case scoring (signal, not filter)

- `CasePreference { CaseInsensitive, PreferMatched }`, keyboard-authored
  per call from (autocaps state, user setting); absent → `PreferMatched`.
  `case_origin` collapsed away: with no context-side signal (committed
  context is autocaps-polluted; pre-folded stores have no raw keys),
  preference-alone loses nothing.
- Active term is **input-vs-candidate only**, iff folding is on *and*
  preference is `PreferMatched` *and* raw input carries casing;
  `PreferMatched` ≡ ~0.01/word agreement bonus, `Insensitive` ≡ no case
  term. Autocapping keyboards send `Insensitive`, zeroing the term.
- No per-mechanism folding flags: folding is a property of the per-call
  prep, scoring never sees unfolded data it shouldn't.

## 7. Relation to other tracks

- Frequency contract (`dictionary/mod.rs`, `NgramBackend` docs, commit
  `db5a35a`): normalized 0.0–1.0/`-1.0`, raw storage with boundary
  conversion, count-unit bump deltas. Case work composes (neighbour
  weight × normalized prior); frequency first, then `android.rs`.
- Completion engines: transplant frozen (`prefix.rs` folds internally
  already); `android.rs` consumes registries + `CasePreference` natively.
- Layout milestone: neighbour-weighted substitution plugs into the shared
  edit generator's alphabet source; layout handles are global tokens +
  documented client responsibility, session-only (no disk writes, ever),
  server-side in-memory cache with eviction.
- BOS marker (`plan_bos.md`): wire `"<s>"`, internal U+FFFF, backend-owned
  conversion, truncate-to-after-BOS fallback, absent-BOS is status quo.

## 8. Tests

Registry matrix per entry (ß/İ/Σ/NFC-vs-NFD/idempotency); per-backend
exact-by-default + folded-matching tests; lang resolution
(CLI > db > unicode default); stored-casing preservation; `PreferMatched`
vs `Insensitive` ordering pins on mixed-case fixtures; transplanted
prefix tests untouched.

## 9. Resolved decisions

(a) **NFC explicit**: configs list `nfc` in the chain; no framework
auto-prepend. A missing `nfc` on non-ASCII data fails silently, so
registry tooling should warn when a non-identity chain lacks it —
explicitness with a lint, not magic.
(b) **Case scoring**: `PreferMatched` ≡ ~0.01/word agreement bonus
(input-vs-candidate only); `Insensitive` ≡ no case term, nothing to
scale. Default when folding is active and the signal absent:
`PreferMatched`.
(c) **Dropped as moot**: with per-call preps resolved inside the engine
from (selection + lang), there are no backend-derived defaults left to
override — the "override API shape" question dissolves. Provenance
(store of unknown normalization) is handled by the caller picking
correctly, same doctrine as layout tokens.
(d) **No TOML config surface, ever.** Hardcoded `LangDb` in source —
empty *for now*, entries landing together with the specific algorithms
they point at. Override lives on every API surface (CLI flags; Rust,
stdio, FFI, D-Bus per-request selections). The loadable system file
replaces the hardcode later.
(e) **stdio parity required**: stdio JSON protocol + `StdioClient` expose
`complete` / `query_limited` matching D-Bus (pending implementation).
