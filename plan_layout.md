# Layout support: actionable implementation plan

Status: ready to execute. This plan covers extracting a shared keyboard-layout
library, wiring it into `drift_type` and `verbisage`, and adding layout-aware
completion/correction. XKB support is deliberately the **last** step, after all
other layout formats and after the prediction/correction algorithms are
modified.

Related docs: `plan_android.md` (engine), `plan_caps.md` (§7 layout milestone),
`plan_of_plans.md` (phase ordering), `CONTEXTUAL-API.md` (transport surface).

---

## 0. Goal

1. A standalone `keyboard_layout` crate that owns keyboard geometry and
   format readers, used by both `drift_type` and `verbisage`.
2. Physical and touch typing both express their layout as one intermediary
   (`RowLayout`), with readers for HeliBoard simple, HeliBoard/FlorisBoard
   JSON, Unicode Keyboard3 (UTS #35 part 7), and XKB.
3. `verbisage` can register a layout (upload token, session-only cache) and use
   it to improve completion and spelling correction.
4. Final tree/behaviour for existing completion/correction is unchanged when no
   layout is supplied.

## 1. Locked decisions

- **Packaging:** standalone git repo at `../keyboard_layout` (sibling of
  `verbisage`). Single crate `keyboard_layout`, package `keyboard-layout`,
  Apache-2.0, edition 2024. Feature-gated readers, **not** a workspace.
- **Extraction set:** layout traits/types + minimal math + `Circle`. Leave
  `line_segment` (and everything solver/path) in `drift_type`.
- **Compatibility:** `drift_type` keeps its public surface via re-export shims;
  `drift-type-android` must keep compiling unchanged.
- **Float geometry** lands in three separate steps: extract as-is, add
  `RectKey::from_rect`, then fix `RectKey::normalise` length handling.
- **Physical intermediary:** `RowLayout` → `RectKeyLayout::from_row_layout`,
  porting the behaviour of k3lp's `K3ComputedLayout` (row centring, stretch,
  gap keys).
- **Readers return `RowLayout`.** Readers with explicit geometry (XKB geometry)
  fill `KeySpec::rect`; others leave it `None` and let `from_row_layout`
  compute it.
- **No vendored geometry.** XKB geometry is read from the system tree
  (`$XKB_CONFIG_ROOT`) and user dirs, never assumed; ANSI/ISO synthesis from
  keycode families is the fallback.
- **XKB** uses the `kbvm` crate for symbols/includes/merge/rules and keysym
  decoding; the geometry parser/resolver is ours.
- **Custom layout inputs (v1):** rows/rects, Unicode Keyboard3, HeliBoard
  simple/JSON. (Custom XKB user dirs are reachable through `kbvm`.)
- **`verbisage` holds layouts concretely** as `Arc<RectKeyLayout>`; the layout
  traits are not made object-safe.
- **Layout-only spatial first**: no per-touch coordinates in this pass; the
  design leaves a slot for touch points later.
- **XKB is last.** It may not start until Phases 1–6 are complete.

## 2. Repository layout

```
keyboard_layout/                    # new repo, git init
  Cargo.toml
  LICENSE                           # Apache-2.0
  .gitignore                        # /target
  src/
    lib.rs
    physical.rs                     # RowLayout, Row, KeySpec, KeyRect,
                                    #   RectKeyLayout::from_row_layout
    math/
      mod.rs  float.rs  lerp.rs  point.rs  normaliser.rs
      collider/{mod.rs, circle.rs}
    layout/
      mod.rs  rect_key_layout.rs  mock_layout.rs
    readers/
      mod.rs
      heli_simple.rs                # feature heli-simple
      heli_json.rs                  # feature heli-json  (serde, serde_json)
      unicode.rs                    # feature unicode-keyboard3 (roxmltree)
      xkb/                          # feature xkb (kbvm)  -- LAST
        mod.rs
        source.rs
        geometry.rs
  tests/                            # reader fixtures + geometry fixtures
```

Feature flags:

```toml
[features]
default = []
heli-simple = []
heli-json = ["dep:serde", "dep:serde_json"]
unicode-keyboard3 = ["dep:roxmltree"]
xkb = ["dep:kbvm"]
```

## 3. Working conventions

- **Atomic commits.** Every numbered step below is exactly one commit. Do not
  bundle steps. Do not commit a step until its acceptance check passes.
- **Gates.** After each crate-level step run `cargo test` for that crate. After
  `drift_type` changes run its full suite and doctests. After `verbisage`
  changes run `cargo test` plus the full feature matrix
  (`--features sqlite,hunspell,patricia,dbus`, and the default set).
- **No behaviour change without a test.** Any step that changes existing output
  must add or update a test in the same commit.
- **Preserve licenses/attribution** when copying grammar or fixture data.

---

## Phase 1 — `keyboard_layout` core

### 1.1 Extract the layout module as-is

- `git init` the new repo; add `LICENSE` (Apache-2.0), `.gitignore`
  (`/target`), and `Cargo.toml` (crate `keyboard_layout`, package
  `keyboard-layout`, edition 2024, deps `unicode-segmentation`, `log`).
- Move from `../drift-type/src`, preserving module paths so intra-module
  `crate::math::…` / `crate::…` references still resolve:
  - `math/{float,lerp,point,normaliser}.rs`
  - `math/collider/{mod,circle}.rs`
  - `layout/{mod,rect_key_layout,mock_layout}.rs`
- Curate `lib.rs` to `pub use` the same root names `drift_type` exposes:
  `Point`, `Collider`, `Key`, `KeyboardLayout`, `DEFAULT_IGNORED_LABELS`,
  `RectKey`, `RectKeyLayout` (and `Float` for internal/public signatures).
- Do **not** move `math/line_segment.rs`.
- Retarget doc-tests in the moved files from `drift_type::*` to
  `keyboard_layout::*`.
- Keep the module paths `math::` and `layout::` so a later import-shim in
  `drift_type` is trivial.
- Acceptance: `cargo test` in the new crate passes (moved tests included).
- Atomic commit.

### 1.2 Add `RectKey::from_rect`

- Keep `RectKey::new(..., left: u16, top: u16, width: u16, height: u16)`
  delegating to a new float constructor:
  `RectKey::from_rect(main_label, secondary_labels, left: Float, top: Float,
  width: Float, height: Float)`.
- No behaviour change.
- Add a test asserting `new` and `from_rect` agree for integer-valued inputs.
- Acceptance: `cargo test`; downstream builds unaffected.
- Atomic commit.

### 1.3 Fix `RectKey::normalise` length scaling

- Add `Normaliser2D::scale()` (returns the normalisation ratio) or an
  equivalent length-scaling accessor.
- Change `RectKey::normalise` so width/height are multiplied by the scale
  (lengths), not passed through `norm_x`/`norm_y` (which subtract the layout
  origin). This fixes nonzero-origin layouts; zero-origin layouts are
  unchanged.
- Verify against `drift_type` (see Phase 2) before trusting it; if upstream
  doctests regress, keep the fix but adjust the affected tests in the same
  commit and record why.
- Acceptance: new-crate tests pass; geometry tests for a nonzero-origin layout.
- Atomic commit.

### 1.4 Add `RowLayout` and `from_row_layout`

Define in `src/physical.rs`:

```rust
pub struct RowLayout { pub rows: Vec<Row>, pub ignored_labels: Vec<String> }
pub struct Row { pub keys: Vec<KeySpec> }
pub struct KeySpec {
    pub main: Option<String>,       // None = gap/spacer
    pub secondary: Vec<String>,
    pub width: Float,               // relative units, default 1.0
    pub stretch: bool,              // fill remaining row width
    pub rect: Option<KeyRect>,      // explicit geometry if the source has it
}
pub struct KeyRect { pub x: Float, pub y: Float, pub width: Float, pub height: Float }
```

- Port k3lp `K3ComputedLayout` semantics into
  `RectKeyLayout::from_row_layout(&RowLayout, &RowMetrics)`:
  - equal row heights (`RowMetrics::total_height / rows`), with optional
    remainder distribution;
  - key widths from relative `width`, normalised so a full row is the layout
    width; rows whose sum is below full width are centred; `stretch` keys take
    the remaining space; `main = None` keys are spacers (`gap`);
  - when `KeySpec::rect` is present, use it verbatim (XKB geometry path);
  - build `RectKey`s (main + secondary labels) and reuse the existing
    `RectKeyLayout::new` normaliser/position-map/median logic.
- `RowMetrics { total_width: Float, total_height: Float }` with a sensible
  default (e.g. 1.0 × rows).
- Tests: centred rows, stretch fill, gap keys, explicit rects, secondary
  labels resolvable via `location_of`.
- Atomic commit.

---

## Phase 2 — `drift_type` integration

- Add `keyboard_layout = { path = "../keyboard_layout" }` to
  `../drift-type/Cargo.toml`.
- Delete the moved files from `drift-type/src`.
- Replace `drift-type/src/math/mod.rs` with:
  `pub mod line_segment;` + `pub use keyboard_layout::math::*;`
- Replace `drift-type/src/layout/mod.rs` with:
  `pub use keyboard_layout::layout::*;`
- Because the shims re-export the submodules, all existing `crate::math::…` /
  `crate::layout::…` paths and root re-exports keep resolving; expect no edits
  in `path/*`, `solver/*`, `feature_extraction.rs`, `line_segment.rs`.
- Acceptance:
  - `cargo test` and `cargo test --doc` in `drift-type`.
  - `drift-type-android` builds and its tests pass against the path dep.
- Atomic commit.

---

## Phase 3 — Non-XKB readers

Each reader converts text into `RowLayout`. Shared helpers live in
`readers/mod.rs`. Add a `LayoutError` type with useful messages.

### 3.1 HeliBoard simple rows (`heli-simple`)

- Parse the simple text format: one key per line; whitespace-separated tokens
  are `label [popup…]` (first = main, rest = secondary); two consecutive
  newlines start a new row.
- Ignore/record special labels (`_space_`, `_shift_`, …) as gaps or non-letter
  keys; keep it minimal.
- Acceptance: fixture round-trips to expected rows/labels.
- Atomic commit.

### 3.2 HeliBoard / FlorisBoard JSON (`heli-json`)

- Serde model for the supported subset: normal `text_key`/`auto_text_key`,
  `label`, `popup`, `width`, and row structure; ignore selector classes and
  action keys (record a warning).
- `width` semantics: `0` = default, `-1` = stretch, else relative fraction.
- Acceptance: fixtures for a plain layout, a Dvorak-style layout, and one with
  `width`/popups.
- Atomic commit.

### 3.3 Unicode Keyboard3 (`unicode-keyboard3`)

Reimplement the relevant k3lp subset in Rust; do **not** bind the Kotlin lib.

- **Structures:** `Keyboard3` root, `Key` (`id`, `output`, `longPressKeyIds`,
  `multiTapKeyIds`, `width`, `gap`, `stretch`), `Layer`/`Row`, `Variables`
  (`string`, `set`), `Displays`.
- **Reader:** XML via `roxmltree`; `ImportResolver` trait for local imports
  (CLDR-base imports optional and reported, not fatal); a `K3String` subset
  (`\uXXXX`, `\u{…}`, `\`-escapes, `${var}`, `$[set]`, NFD normalisation);
  markers/descriptors treated as non-character keys.
- **Mapping:** pick a layer (id or `modifiers`), map rows/keys to `RowLayout`
  (`gap` → `main = None`, `stretch`, `width`, long-press → `secondary`).
- **Deferred:** `<flicks>`, `<transforms>`/`<reorder>`, `<forms>`/`<scanCodes>`,
  `uset` properties, property-based conformance tests.
- **Fixtures:** vendor the CLDR sample XMLs used by k3lp (`fr-t-k0-test.xml`,
  `mt-t-k0-47key.xml`, `ja-Latn.xml`, `pcm.xml`) with Unicode/CLDR attribution.
- Acceptance: fixtures produce the expected rows/labels; at least one
  gap/stretch/long-press case.
- Atomic commit.

---

## Phase 4 — `verbisage` layout registry and transport

### 4.1 Registry + cache

- Add `keyboard_layout = { path = "../keyboard_layout", features = [...] }` to
  `verbisage/Cargo.toml` (non-optional; readers enabled as needed).
- New `src/layout.rs`:
  - wire DTO `LayoutUpload { keys: Option<Vec<KeyBox>>, rows: Option<RowSpec>,
    ignored_labels: Vec<String> }`, `KeyBox { label, alt_labels, left, top,
    width, height }` (floats), `RowSpec` for the physical rows form;
  - `build_layout(&LayoutUpload) -> Result<RectKeyLayout, String>` dispatching
    on `keys` vs `rows`, with validation (finite, positive sizes, unique
    single-grapheme labels);
  - `layout_token(...)` content hash (FNV-1a) over normalised geometry so the
    same layout collapses to one token;
  - `LayoutCache` bounded LRU (default cap 16), session-only, no disk.
- Acceptance: unit tests for register/dedup/evict/build-from-rows.
- Atomic commit.

### 4.2 Protocol, transports, CLI

- `daemon/protocol.rs`: `register_layout`, `forget_layout` params; add
  `layout: Option<String>` to `CompleteParams` and `SuggestParams` (and the
  `complete_with`/`complete`/`suggest` paths).
- `daemon/handlers.rs`: `DaemonHandler` owns the cache; `register_layout`,
  `forget_layout`, `get_layout`; resolve the token before building engines;
  unknown token → explicit error.
- `daemon/dbus.rs`: `RegisterLayout`, `ForgetLayout`, and a layout-token arg on
  `CompleteWith`/`Complete`/`Suggest`; update introspection and the private-bus
  tests.
- `daemon/stdio.rs` and `clients/{dbus,stdio}.rs`: matching methods/helpers.
- CLI (`bin/verbisage.rs`): `--layout <file>` on one-shot complete/suggest,
  auto-detecting HeliBoard simple/JSON and Unicode Keyboard3 (XKB comes later);
  optionally `[daemon] layout` to pre-register at startup.
- Acceptance: transport tests register a layout, complete with its token, and
  reject an unknown token; CLI smoke with a fixture file.
- Atomic commit.

---

## Phase 5 — Completion/correction algorithm changes

### 5.1 Generalise the edit generator (`EditSource`)

- In `spellcheck/edits.rs`, replace `Alphabet` with:

  ```rust
  pub trait EditSource {
      fn letters(&self) -> &[char];
      /// Replacement for `ch` with a multiplicative weight (higher = better).
      fn substitutions(&self, ch: char) -> Vec<(char, f64)>;
  }
  ```

- `LatinAlphabet` returns all 26 letters at weight `0.5`, preserving today's
  substitution weight exactly.
- `visit_edits` takes `&dyn EditSource`; update `spellcheck/suggest.rs` and
  `completion/android.rs` call sites and the `edits` tests.
- Acceptance: all existing completion/spellcheck tests pass unchanged.
- Atomic commit.

### 5.2 Layout-aware completions

- Add `LayoutEdits<'a>(&'a RectKeyLayout)` implementing `EditSource`:
  - `letters()` = collected single-grapheme main labels;
  - `substitutions(ch)` = keys within `median_key_diameter() * <radius>`,
    weight `(1 - d_norm).clamp(0.1, 0.9)` where
    `d_norm = distance(key_center(ch), key_center(candidate)) /
    median_key_diameter()`; base-character fallback for accents; geometry-free
    default stays `0.5`.
- `CompletionInput` gains `layout: Option<Arc<RectKeyLayout>>` (or a borrowed
  equivalent), defaulting to `None`; `AndroidCompleter` selects `LayoutEdits`
  when present, else `LatinAlphabet`.
- Replace the constant spatial term so the layout signal reaches scoring while
  preserving the existing joint-factor behaviour.
- Tests: nearby-key substitution outranks far-key; no-layout output is
  byte-identical to before; transplant parity; `helo→hello` still ranks.
- Atomic commit.

### 5.3 Layout-aware corrections

- `SpellChecker` gains
  `suggest_with(&self, input: &SuggestionInput<'_>, max: usize) -> Vec<String>`
  with a default that delegates to `suggest`.
- `SuggestionInput { word, context, layout: Option<Arc<RectKeyLayout>> }`.
- `suggest_edits` gains an `Option<&dyn EditSource>` parameter; existing call
  sites (`spellcheck/dictionary.rs`, `spellcheck/sqlite.rs`,
  `spellcheck/hunspell.rs`, `dictionary/patricia.rs`) pass `None`.
- Daemon `suggest` resolves the token and calls `suggest_with`.
- Acceptance: layout-aware spelling test; no-layout behaviour unchanged.
- Atomic commit.

---

## Phase 6 — Integration tests and docs (non-XKB)

- End-to-end test: build a fixture layout → register → token → complete and
  suggest, asserting the layout influences ranking.
- Update `CONTEXTUAL-API.md` with `RegisterLayout`/`ForgetLayout`, the layout
  token parameters, and CLI `--layout`.
- Update the layout-milestone status in `plan_of_plans.md` / `plan_caps.md`.
- Acceptance: full feature matrix passes.
- Atomic commit(s) (tests and docs may be split).

---

## Phase 7 — XKB support (LAST)

Must not begin before Phases 1–6 are done. `kbvm` provides text parsing and
keysyms; the geometry parser/resolver is ours. No vendored geometry.

### 7.1 `XkbSource` + text reader (`xkb` feature)

- `readers/xkb/source.rs`:

  ```rust
  pub struct XkbSource {
      pub root: PathBuf,                 // default $XKB_CONFIG_ROOT or /usr/share/X11/xkb
      pub rules: String,                 // default "evdev"
      pub layout: String,                // "us"
      pub variant: Option<String>,       // "intl"
      pub model: Option<String>,         // default pc104/pc105
      pub extra_roots: Vec<PathBuf>,     // user dirs, $XDG_CONFIG_HOME/xkb, $XKB_CONFIG_EXTRA_PATH
  }
  impl FromStr for XkbSource { /* "us", "us(intl)" */ }
  ```

- Use `kbvm`: `Context::keymap_from_names(RMLVO)` and `expand_names` to resolve
  symbols and the geometry component (model-driven). Walk
  `Keymap::keys()` → `Key::name()` (`AE01`, `AD01`, `AC01`, `AB01`, `LSGT`, …)
  → `groups()/levels()/symbols()`; decode with `kbvm`'s `Keysym`.
- Map keycode families to rows/columns; build `RowLayout`; detect ANSI/ISO from
  `<LSGT>` presence; use the geometry rects when available (7.2), else leave
  `rect = None` so `from_row_layout` synthesises.
- Dispatch to the geometry parser for the resolved model.
- Acceptance: `us`, `us(intl)`, `de` produce expected rows; user-dir custom
  layout resolves; missing data falls back to synthesis.
- Atomic commit.

### 7.2 Geometry parser/resolver

- `readers/xkb/geometry.rs`: parse `xkb_geometry "name" { … }` from the system
  tree and user dirs, spanning:
  - `include`/map selection (`include "hhk(basic)"`),
  - dotted defaults inherited through scopes (`key.shape`, `key.gap`,
    `row.left`, `row.top`, `row.vertical`, `section.left`, `section.angle`),
  - `shape` outlines (`{ [w,h] }`), `approx`, `cornerRadius`,
  - `section` `top`/`left`/`angle`, `row` accumulation (`key.gap`), per-key
    shape overrides `{ <KEY>, "SHAPE" }`,
  - floats and negative values.
- Produce explicit `KeyRect`s per key (fill `KeySpec::rect`).
- The grammar may be seeded from `xkb-parser`'s MIT/Apache `.pest` rules with
  attribution; the resolver is ours.
- Acceptance: parse `geometry/pc` (pc104, pc105) and at least one exotic model
  (e.g. `typematrix`, `kinesis`, `sun`) into the expected positions; malformed
  input is a clean error, never a panic.
- Atomic commit(s) (parser and resolver may be split).

### 7.3 Fallback synthesis + XKB integration

- Ensure the ANSI/ISO synthesis path is used whenever geometry is absent or
  fails to parse, and is covered by tests.
- Extend `LayoutUpload`/CLI to accept an XKB layout by name/model
  (in addition to files), and allow custom user XKB dirs.
- Update `CONTEXTUAL-API.md` for XKB layout selection.
- Acceptance: full matrix plus XKB feature on/off builds; end-to-end test with
  a system layout and a user-dir layout.
- Atomic commit(s).

---

## Phase 8 — Final verification

- `keyboard_layout`: `cargo test` with each reader feature individually and
  all together; `cargo build --features xkb`.
- `drift_type`: `cargo test`, `cargo test --doc`.
- `drift-type-android`: build + tests.
- `verbisage`: default features; `--features sqlite,hunspell,patricia,dbus`;
  with the `keyboard_layout` readers enabled.
- Confirm no-layout completion/correction output is unchanged (regression
  tests from Phase 5.1/5.2).
- Confirm custom layouts work through both the upload token path and the CLI.
- Confirm XKB works with system data, user dirs, and with geometry unavailable
  (synthesis fallback).

## 4. Commit boundaries summary

One atomic commit per numbered step:

- 1.1 extraction; 1.2 `from_rect`; 1.3 normalise fix; 1.4 `RowLayout`.
- 2 `drift_type` integration.
- 3.1 heli-simple; 3.2 heli-json; 3.3 unicode.
- 4.1 registry; 4.2 transports/CLI.
- 5.1 `EditSource`; 5.2 completions; 5.3 corrections.
- 6 integration + docs.
- 7.1 XKB source/text; 7.2 geometry parser/resolver; 7.3 fallback + integration.
- 8 final verification fixes (if any).

No commit message text is prescribed here; write a concise, repo-style message
per commit at the time.
