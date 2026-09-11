# Contextual completion, prediction and swipe integration

The accepted objective is shared contextual ranking, preservation of the
working swipe integration, and Stevia predictions refreshed after each
accepted word. This branch starts at Prawn's `completion` branch `96d1e7c`.
The paired Stevia branch starts at the tested `95db18fa` swipe UI.

## Requirements and acceptance

- [x] Keep upstream `completion/`; do not restore obsolete `completion.rs`.
- [x] Selectively restore `b325ab6` (Patricia pin) and `825d0cb` (swipe).
  Resolution retains the newer backend frequency contract.
- [ ] Verify restored swipe baseline, then retain whole-path recognition,
  actual layout geometry, finite ranked results, bounded work, stale-response
  protection, editable top guess, alternatives, consecutive words, Shift,
  fading trail, stationary long press and one-step completion undo.
- [ ] Explicit result limits: honor caller maximum, reject requests above
  configured transport caps; remove silent clamps from restored swipe.
- [ ] `plan_caps`: explicit per-request NFC/case preparation, registries and
  empty language map, exact backend matching, normalized frequencies, stored
  casing, consistent finite-score/lexical ordering, frontend case preference.
- [ ] Share the edit generator and contextual scoring between completion,
  prediction and spelling. Preserve the distinction between native counts and
  Patricia's quantized probabilities; do not fabricate corpus counts.
- [ ] `plan_android`: contextual input, prefix+edit candidate gathering before
  truncation, shared language score, edit costs, case preference, exact-match
  promotions, deterministic ordering and documented missing-data fallbacks.
- [ ] Use the shared engine for next-word prediction with empty current input;
  gather contextual candidates as well as unigram candidates.
- [ ] Beginning-of-sentence context is authored by the keyboard. Translate at
  entry, preserve supported SQLite/Patricia markers, safely back off otherwise,
  and never expose markers as suggestions.
- [ ] D-Bus and stdio have matching completion/prediction parameters and limits;
  CLI exposes preparation selection. Keep existing clients usable.
- [ ] Search budgets and transport response deadlines are enforced and tested.
- [ ] Stevia passes bounded context before the cursor, refreshes after accepted
  typed/swiped words and suggestion selection, supports chaining predictions,
  and invalidates stale results on edits/cursor/focus/language/mode changes.
- [ ] Test typed context (including `see you l` fixtures), prediction chains,
  both count and probability dictionaries, beginning-of-sentence, Unicode,
  request limits, unavailable services, cancellation, undo and swipe regressions.
- [ ] Run isolated real daemon/Wayland integration with real dictionary data;
  validate the actual interaction behavior, not just mocked response lists.

`plan_of_plans.md` is an ordering aid, not a reason to repeat completed work.
The sequence is limits/foundations, shared engine and transports, frontend,
then end-to-end acceptance. Deferred layout uploads/multi-touch, learned user
dictionaries, dictionary conversion and broad multilingual pipelines remain
separate projects. Existing English v202 supports one preceding context word;
tests with richer fixture data must not be presented as shipped-corpus coverage.

## Current evidence

Upstream refs were refreshed on 2026-09-11; `completion` is still `96d1e7c`.
The first offline baseline attempt found missing bundled-SQLite build dependencies
in the reused cache. Resolve dependencies in the isolated builder, then run the
baseline; this is not a test success. No device or installed deployment changed.

Foundation checkpoint: restored baseline passed (82 unit tests plus 2 doctests).
After exact matching, normalized counts, explicit text preparation and shared
scoring/edit extraction, all-feature tests passed: 97 unit tests, 4 backend
contract regressions and 2 doctests. This does not establish the new completion
engine, contextual transports, Stevia prediction chaining or runtime acceptance.
Full folding uses caseless 0.2.2 (Unicode 16 tables); NFC remains explicit.
The planning example that maps dotted capital I to plain i under default full
folding is corrected: non-Turkic folding yields i plus combining dot.
Logs: ../restored-tests.log and ../all-foundations-tests.log.
