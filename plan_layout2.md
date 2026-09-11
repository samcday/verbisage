# Layout support, part 2: rebase + limitation elimination

Continues `plan_layout.md`. Scope: (0) rebase `drift_type` onto `origin/main`,
(1) XKB geometry `include` + `row.vertical` + `section.angle`, (2) touch
spatial scoring (HeliBoard-inspired), (3) unify the spatial abstraction and make
it runtime-selectable. One atomic commit per numbered step.

## Context

- `drift-type`'s layout commit `a151f52` sits on `prawn/layout_info`, parent
  `e962ad3` (= `origin/shark2`). `origin/main` (`f63256a`) is `e962ad3` + 3
  commits: docs cleanup and the `Normaliser` -> `Normalizer` rename.
- Upstream `origin/main` references `crate::math::normalizer::Normalizer` in
  `solver/drift_type.rs` and `path/waypoints.rs`. The `keyboard_layout` shim
  must therefore expose `math::normalizer::Normalizer` before the rebased tree
  will compile.
- Network `git fetch` currently fails (SSH host key); the local `origin/main`
  ref is used. If the remote is newer, redo the rebase after a fresh fetch.
- "Frozen" is retired terminology: `PrefixCompleter` may be changed.

## Part 0 — Rebase `prawn/layout_info` onto `origin/main`

### 0.1 `keyboard_layout`: adopt upstream rename + doc wording
- Rename `src/math/normaliser.rs` -> `src/math/normalizer.rs`; rename
  `Normaliser`/`Normaliser2D` -> `Normalizer`/`Normalizer2D` (keep the added
  `scale()` accessor).
- Update `src/math/mod.rs`, `src/layout/rect_key_layout.rs` references.
- Apply upstream's comment/doc wording fixes in `math/*` and `layout/*`.
- Acceptance: `cargo test` (default and `--features xkb`).
- Commit.

### 0.2 `drift_type`: rebase onto `origin/main`
- Create a safety tag/branch for the pre-rebase tip.
- `git rebase origin/main` on `prawn/layout_info`.
- Resolve conflicts:
  - deleted-by-us / edited-by-them on `src/math/*` and `src/layout/*`: keep
    deleted.
  - `src/math/mod.rs`, `src/layout/mod.rs`: keep the re-export shims
    (`pub mod line_segment;`, `pub use keyboard_layout::…::*;`).
  - everything else (`line_segment.rs`, `solver/*`, `path/*`, `lib.rs`): take
    upstream.
  - `Cargo.toml`: keep the `keyboard-layout` dependency.
- Acceptance: `cargo test`, `cargo test --doc`, `drift-type-android` builds.
- The rebase itself is the commit.

## Part 1 — XKB geometry: includes + `row.vertical` + `section.angle`

All in `keyboard_layout/src/readers/xkb/geometry.rs` (+ `source.rs` wiring).

- Capture `include` / `augment` / `override "path(name)"` statements in order
  while parsing a geometry body.
- Resolve `<root>/geometry/<path>` across `extra_paths`, `$XKB_CONFIG_ROOT`,
  and `/usr/share/X11/xkb`; paths may contain `/` (e.g.
  `digital_vndr/pc(common)`).
- Find `partial hidden xkb_geometry "<name>"` in the target file; recurse with
  a depth/cycle guard.
- Merge with augment semantics: included rects fill first, the including
  geometry's own definitions override.
- Add `row.vertical` (stack keys vertically, advancing `y`) and
  `section.angle` (rotate the section's resolved rects about its origin).
- `geometry::parse` gains a roots context; `source::resolve_geometry` passes
  roots.
- Out of scope: `overlay`, `doodad`, `indicator`, multi-outline shapes.
- Tests: inline include; system `hhk` (`win1` -> `basic`), `typematrix` (multi
  include), `digital_vndr/pc(common)`; a synthetic vertical/angled fixture.
- Acceptance: `cargo test --features xkb`.
- Commit(s).

## Part 2 — Touch spatial scoring

Module shape:
```
src/spatial/
  mod.rs       # TouchPoint{x,y}, SpatialInput{None,Layout,Touch}, constants,
               #   SpatialInput::edit_source() and word_distance()
  physical.rs  # layout-only model (relocated LayoutEdits) + key-to-key distance
  touch.rs     # TouchEdits, anchored on layout.normalise(point[i])
```
`spellcheck/edits.rs` keeps `EditSource`, `LatinAlphabet`, `visit_edits`;
`EditSource::substitutions(ch, index)` gains the index.

HeliBoard constants in `spatial/mod.rs`:
`DISTANCE_WEIGHT_LENGTH = 0.1524`, `DISTANCE_WEIGHT_LANGUAGE = 1.1214`,
`NORMALIZED_SPATIAL_DISTANCE_THRESHOLD_FOR_EDIT = 0.095`,
`TYPING_MAX_OUTPUT_SCORE_PER_INPUT = 0.1`.

### 2.1 `spatial/` module + `EditSource` index + `TouchEdits`
- Extend `EditSource` with the index; update `visit_edits` and all impls.
- Add `SpatialInput` (owns `Arc<RectKeyLayout>`), `TouchPoint`, `EditSources`.
- `TouchEdits` anchors neighbor search on `layout.normalise(point[index])`,
  falling back to the typed char's key center when no point is present.
- Acceptance: unit tests for touch-anchored substitutions and distances.
- Commit.

### 2.2 Completion scoring
- `android.rs`: when `SpatialInput != None`, use the HeliBoard additive model
  (`compound = d_spatial*0.1524 + d_language*1.1214`,
  `score = 1 - compound/maxDistance`) with the `0.095` edit gate and existing
  promotions. `SpatialInput::None` keeps the current output byte-for-byte.
- `prefix.rs`: override `PrefixCompleter::complete_with`; when spatial is
  present multiply the transplanted confidence by the joint factor
  `(1 - d_spatial).clamp(0.1, 0.9)`. `None` is unchanged.
- `CompletionInput.layout` becomes `CompletionInput.spatial: SpatialInput`.
- Acceptance: `helo -> hello` and no-spatial parity pinned; nearer-touch ranks
  higher; far edits gated.
- Commit.

### 2.3 Correction scoring
- `SuggestionInput.layout` becomes `spatial: SpatialInput`.
- `DictionarySpellChecker::suggest_with` uses the shared spatial edit source
  and ranks by touch/layout distance; no-spatial behavior unchanged.
- Acceptance: layout/touch correction tests; parity test.
- Commit.

### 2.4 Transport points
- `CompleteParams` / `SuggestParams`: add `points: Vec<[f32; 2]>` (x, y only);
  empty means none; a non-empty count must equal the input char count, else an
  error.
- D-Bus `CompleteWith` / `Suggest`: add an `a(dd)` argument; update
  introspection and P2P tests.
- stdio JSON and `clients/{stdio,dbus}.rs`: `*_layout(..., points)` helpers.
- Acceptance: transport round-trip test; mismatch rejected.
- Commit.

### 2.5 Unify + runtime-selectable + docs
- Ensure completion and spellcheck both consume only `SpatialInput`; daemon
  selection is data-driven (none -> `None`, token -> `Layout`, token + points
  -> `Touch`).
- Update `CONTEXTUAL-API.md` and the layout-milestone notes.
- Acceptance: full feature matrix; no-spatial parity across engines.
- Commit.

## Verification

- `keyboard_layout`: default + each reader + `xkb` all-features.
- `drift_type`: tests + doctests; `drift-type-android` build.
- `verbisage`: default and `--features sqlite,hunspell,patricia,dbus`.
- Parity tests prove `SpatialInput::None` output is unchanged for completion and
  correction.
- XKB geometry tests cover include chains and a vertical/angled model.
