# `android.rs` completion engine

HeliBoard-style completer (`src/completion/android.rs`) implementing
`CompletionEngine`, consuming the registries, scoring, and contracts
locked in `plan_caps.md` / `plan_bos.md` / backend contract docs
(commit `db5a35a`). On completion it becomes the handler default;
`prefix.rs` stays as the alternative impl (transplant frozen, untouched).

## 0. Dependencies (build order)

1. Backend conversion passes: normalized frequencies + exact matching +
   NFC ingest (engine **assumes** the contract, never compensates).
2. `TextPrep` registries (`Normalization {none, nfc}`,
   `CaseFold {none, ascii_lower, unicode_lower, full, lang_specific}`),
   `LangDb` empty hardcode, caller helpers (Rust).
3. Shared edit generator + canonical `interpolate_score` merged into
   `prediction` (`android.rs` is first consumer; `prefix.rs` keeps its
   frozen copy).
4. `android.rs` → flip handler default → BOS backend support follows
   (`plan_bos.md`; sqlite rows, patricia sentinel translation).

## 1. Input model

```rust
struct TextPrep { normalization: Normalization, fold: CaseFold }
struct CompletionInput<'a> {
    input: &'a str,              // incomplete word ("input", not "prefix")
    input_prep: TextPrep,
    context: &'a [&'a str],      // n-gram context, keyboard-truncated at sentence bounds
    context_prep: TextPrep,
    case_preference: CasePreference, // Insensitive | PreferMatched (default)
}
```

Trait gains `complete_with(&self, input: &CompletionInput, max: usize)`;
existing `complete(prefix, max)` delegates with identity preps,
empty context, `PreferMatched`. `lang_specific` in either prep resolves
via `LangDb` (+ optional lang tag) inside the engine.

## 2. Candidate generation

- **Recall**: bounded prefix fetch via `query_limited` with the input
  folded by `input_prep` (backend folded-index/dual-variant behavior per
  `plan_caps.md` §5); corrections via the **shared** edit generator
  (alphabet/neighbour-source seam defaulting to `a-z`; uploaded layouts
  plug in at the layout milestone — no third copy). Known-word and
  short-fragment suppression carry over as configurable policy.
- **BOS**: translate `"<s>"` → U+FFFF once at entry; look context up
  verbatim (aware backends convert, others truncate-to-after-BOS,
  absent-BOS is status quo per `plan_bos.md`); sentinel filtered from
  all outputs at the gather layer.
- **Out of scope** (server-side tap completion): touch/proximity
  traversal, multi-word space terminals, shortcuts, digraph expansion,
  best-first `DicNodesCache` search (not expressible via `query_*`),
  per-language transliteration pipelines beyond registry + map.

## 3. Scoring (per candidate)

`score = 1.0 + normalized_compound × 0.1`, with:

- **Language term**: shared `interpolate_score` over context folded with
  `context_prep` (count=1 smoothing, uniform deltas, depth bounded by
  backend `max_order()`); `language_distance = 1.0 - prob`, weighted
  ×1.1214.
- **Spatial term**: constant-zero slot ×0.1524, retained for the touch
  milestone (no coordinates server-side). Correction candidates map the
  transplant weights into it as a spatial-cost analog
  (`spatial = 1.0 - weight`: intact 0.0, edits 0.1/0.35/0.5) — preserves
  transplant ranking semantics inside the HeliBoard formula and gives the
  shared generator's edit classes a principled cost slot.
- **Case term** (input-vs-candidate only): active iff folding is on *and*
  preference is `PreferMatched` *and* raw input carries casing;
  ~0.01/word agreement bonus, tiebreak-level. Context-side term dead
  (committed context is autocaps-polluted; pre-folded stores have no raw
  keys). Autocapping keyboards send `Insensitive`, zeroing the term.
- **Promotions**: folded-equal (exact) ×1.1, byte-equal (perfect) ×1.1 —
  spec values, adapted: "exact" is modulo active folding.
- **`is_exact`** known exactly (completion path vs correction path) —
  the transplant's `starts_with` reconstruction goes away.
- Sort score-desc, lexical tiebreak (`total_cmp` canonical); truncate to
  `max` exactly (no internal `min(100)`); transport caps (1k/200k)
  already enforced upstream.

## 4. Degradation matrix

No-input → top unigrams (`candidates(&[])` → frequency → alphabetical →
empty). No n-gram backend → frequency ranking; no unigram data →
alphabetical; nothing → empty. `None` input ≡ empty input.

## 5. Wiring the default flip

Handler `complete` switches one line (`PrefixCompleter::new` →
`AndroidCompleter::new`) plus n-gram sourcing: pass
`predictor.as_ref().and_then(|p| p.ngram_backend())` (`None` when the
chain has no predictor — engine degrades per §4). Return type unchanged
(`Vec<DictionaryResult>`; `is_exact` internal until a consumer needs it).

## 6. Tests

- Transplant-parity: same fixtures as `prefix.rs` rank equivalently
  where semantics overlap (guards the weight→spatial mapping).
- Mixed-case n-gram fixtures pinning bonus on/off (`PreferMatched` vs
  `Insensitive` ordering); `<s>`-row fixtures pinning BOS preference +
  no-row fixtures pinning byte-identical fallback; sentinel round-trip
  + never-in-output guard.
- Degradation matrix; `max`-fidelity (request N → exactly top N);
  stored-casing preservation; `search_budget` deadline honored on gather;
  registry matrix (ß/İ/Σ/NFC-NFD/idempotency) if not already covered.

## 7. Open (unchanged from prior threads)

Bonus scale + `PreferMatched` default are locked (~0.01, default
`PreferMatched`); remaining: shared-generator weight-policy details
beyond the spatial mapping above, and whether `is_exact` needs a
companion case field or score-alone suffices (lean: score alone).
