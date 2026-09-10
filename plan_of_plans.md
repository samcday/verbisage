# Plan of plans: implementation order with sources

Each step names the plan file it comes from. Status: done items are
committed on `main`; everything else is queued.

## Done

- **Cherry-pick `7d5ffd6`** (Patricia backend, `QueryLimited`, `is_empty`,
  XDG config kept; `exit(1)` + system-sqlite + rlib-only dropped) —
  `plan_completion_impl.md` §2 Pick 1 → commit `8584916`.
- **Cherry-pick `a27c8f6` + module split** (`src/completion.rs` →
  `src/completion/{mod.rs, prefix.rs}` transplant, semantics frozen;
  D-Bus `Complete`, stdio enforcement, sqlite bounded query kept) —
  `plan_completion_impl.md` §2 Pick 2 + §3 → commit `21e3ec2`.
- **Split result caps** (`Complete` 1k / bounded queries 200k, reject-not-
  clamp, CLI > config > default, handler choke points) —
  `plan_limits.md` (full as-built record) → commit `2b13b40`.
- **Backend contract docs** (NFC, exact matching, normalized frequencies,
  count units) — frequency/caps threads → commit `db5a35a`.

## Phase A — foundations

1. **Registries + helpers + `LangDb` empty hardcode** — `plan_caps.md`
   §1–§3 (knobs, registry semantics, resolution), `plan_android.md` §0.1.
2. **Backend conversions, per backend** (normalized frequencies, exact
   matching, NFC ingest, canonical `total_cmp` sort, NaN-ordering test,
   `result_matches_query` flip) — frequency thread + `plan_caps.md` §5,
   `plan_android.md` §0.1. Open: sqlite `GLOB` vs prefix-range.
3. **`interpolate_score` merge + shared edit generator** (alphabet-source
   seam, transplant-derived weights) — extraction report (§1–§3 of the
   matching-analysis thread), `plan_android.md` §0.3.

## Phase B — engine + API surface

4. **`CompletionInput` + trait method + `android.rs`** (folded prefix
   fetch, shared-generator corrections, HeliBoard compound with
   spatial-mapped transplant weights, input-only case term, BOS
   `"<s>"`→U+FFFF at entry, degradation matrix) — `plan_android.md`
   §1–§4, scoring constants from `plan_completion.md` §5, BOS handling
   from `plan_bos.md`, case rules from `plan_caps.md` §6.
5. **Flip handler default + transport params together**
   (`PrefixCompleter` → `AndroidCompleter` + n-gram sourcing; per-request
   prep/case-preference/lang params on D-Bus *and* stdio at once; CLI
   `--normalize`/`--fold`) — `plan_android.md` §5, `plan_caps.md` §3,
   stdio parity requirement from `plan_caps.md` §9(e).

## Phase C — follow-through

6. **BOS backend support** (sqlite sentence-split rows; patricia
   `"<s>"`→`0x110000` translation) — `plan_bos.md` (backend contract).
7. **Timeout plumbing** (`search_budget` in gather, `response_deadline`
   transport-side) — `plan_limits.md` (shape exists in
   `CompletionConfig`), `plan_android.md` §6 tests.

## Explicitly later / never (all locked)

- Layouts upload, touch mode, per-language pipelines beyond registry+map,
  FFI helpers — layout/correction threads (deferred).
- `[languages.*]` config, per-language caps, construction-time folders,
  `folder()`-on-trait, engine synthesis for BOS, dual-form NFC fallback —
  rejected in `plan_caps.md` §9 / `plan_bos.md`.

## Source-file index

- `plan_completion.md` — HeliBoard algorithm spec (input).
- `plan_completion_impl.md` — cherry-pick/transplant record + module
  layout (as-built history). (mostly done?)
- `plan_caps.md` — capitalization & normalization design (normative).
- `plan_limits.md` — result-limit knobs (as-built record).
- `plan_bos.md` — BOS marker design (normative).
- `plan_android.md` — android engine build spec (normative).
- `plan_of_plans.md` — this file (ordering).
