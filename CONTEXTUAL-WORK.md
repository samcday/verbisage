# Contextual completion, prediction and swipe integration

This implements the agreed source milestone: shared contextual ranking,
preservation of the working swipe integration, and Stevia predictions refreshed
after every accepted word. Verbisage starts at Prawn's `completion` branch
`96d1e7cedb5984582929a19d7b934f2b58bd3e78`; its remote was rechecked on
2026-09-11 and still points there. Stevia starts at the tested `95db18fa` UI.

## Implementation

- Retain upstream `completion/` and selectively restore the Patricia pin and
  whole-path Drift Type recognition. Do not restore obsolete `completion.rs`.
- Honor requested result counts; reject requests above configurable transport
  caps, without silent `min(100)` truncation. Candidate budgets are separate
  from display limits and exhaustion is an error.
- Explicit per-request NFC/case preparation, an extensible empty language map,
  exact backend matching, normalized frequencies, stored result spelling and
  canonical finite-score/lexical ordering.
- Shared edit generation and count/probability scoring. Patricia probabilities
  remain native probabilities, never invented counts. Its spelling candidates
  are now context-ranked before the display limit, using one prepared context.
  Merged predictors preserve each model's stored-word/prepared-key policy.
- `AndroidCompleter` ranks prefix and edit candidates before truncation. Empty
  input uses this same engine for next-word prediction. Case preference is a
  per-call input-side signal; stored candidate spelling is preserved.
- Explicit keyboard-authored sentence boundaries: wire `<s>`, internal U+FFFF,
  SQLite marker rows and Patricia native 0x110000 prepared-context addressing.
  Missing marker data backs off; markers never become suggestions.
- D-Bus `CompleteWith`/`PredictWith`, stdio equivalents and CLI preparation
  options. Legacy methods remain available. D-Bus has two completion workers;
  stdio has one. Timed-out work retains its slot until it exits; late replies
  are discarded. Built-in scans cooperate with search budgets.
- Stevia sends at most three complete context words from a bounded suffix of
  the text before the cursor. It excludes the current fragment and crosses no
  visible sentence boundary. Predictions wait for application text updates
  after acceptance; ordinary completion still works without surrounding text.
- Focus, selection, mode, purpose, language and context changes invalidate
  requests. Exact acknowledgement of a swipe-selection undo preserves restored
  alternatives. Next-word selection also has whole-word Backspace undo.

Two deliberate adaptations of `plan_android`: quality increases as distance
falls; edit likelihood is a joint factor rather than an additive-only term,
which regressed `helo -> hello` in the acceptance fixture. Native models
separate an absent candidate at an available conditional order (zero floor)
from unavailable context (backoff). Native candidates use stored spelling;
count models use prepared keys. Full non-Turkic Unicode folding maps dotted
capital I to i plus combining dot; NFC remains explicit.

`plan_of_plans.md` remains upstream's ordering aid, not a reason to redo completed
work. The effective order was limits/foundations, shared engine/transports,
frontend integration, then real interaction acceptance.

## Patricia delta

`patricia_dict` points to the paired `codex/contextual-prepared` branch. Its new
commits are `7b76b8d` (prepared v2 follower cache and available context order)
and `32121e2` (native sentence-start addressing and removal of duplicate target
lookups in v2/v402/v403 scoring). These build on the earlier integration pin
`c76e17a`. No Drift Type source delta was needed for this milestone.

## Verification on 2026-09-11

- Verbisage, all features: 101 unit tests, 4 backend regressions, 8 contextual
  regressions, 1 real D-Bus/stdio parity test and 2 doctests passed.
- Verbisage, default features: 84 unit tests, 4 backend regressions, 5 contextual
  regressions and 1 doctest passed.
- Patricia: 4 unit and 27 n-gram tests passed, including prepared/unprepared
  parity across formats. A Verbisage fixture additionally verifies the actual
  native integer sentence-start marker and missing-marker backoff.
- Stevia: all 73 configured Meson checks passed on private headless Phoc,
  including 33 Verbisage completer cases and 14 gesture/widget cases. The
  container requires the documented Glycin rendering override for GTK images.
- Eleven native real-dictionary cases passed: fading trail, swipe acceptance,
  tap-after-swipe, consecutive swipes, swipe alternative undo/reselection,
  editing, focus cancellation, Shift, typed undo/reselection, undo invalidation
  on focus change and ordinary literal input.
- Three native SQLite cases passed: `see -> you -> later`, contextual
  `see you l -> later`, and prediction undo/reselection. A native v403 case
  passed `swipe hello -> accept -> you -> later`. These use deliberately
  constructed counts/probabilities, not claims about production corpus content.
- Real English v202 data returns `hello` first for `helo`, and `later` first
  for `l` after `see you`. Unrestricted next-word output after `you` includes
  `have`, `can`, `know`; the corpus does not guarantee the illustrative chain.

Local logs live beside the checkout in `final-all-tests.log`,
`final-default-tests.log`, `final-stevia-tests.log`, `native-final-matrix.json`,
and the individual `native-context-*` output directories. Reusable native test
sources and fixture instructions are in the paired Stevia `tests/native/`;
Verbisage's `examples/context_fixture.rs` builds the native probability fixture.

The private tests exposed and fixed an ignored Patricia `--system-dict`
override. Earlier real-corpus runs happened to use the same bytes installed in
the builder: both paths were checked against SHA-256
`bd950ef4b57655120eee65cee62a5d216a63f721d9a8bb759ce2022437840443`.
The distinct v403 fixture then verified that explicit selection actually works.

## Limits of this checkpoint

The existing rebuilt English v202 dictionary contains bigrams: the format
supports one preceding context word. Unigram-only dictionary contents cannot
supply contextual associations simply because an engine supports n-grams.
Conversion of Presage data into richer Patricia dictionaries, generic ingest,
learning, multi-touch and broader language/layout policies remain follow-up
projects; no new production dictionary was generated for this milestone.

Removing redundant preparation and duplicate trie walks lowered broad
next-word queries on this desktop from approximately 440–470 ms to 240–300 ms.
Typed completion measured 7–12 ms in the same release smoke run. These are
observations, not device latency guarantees. Further caching/recall optimization
and an aarch64/device trial are appropriate before promoting the new source
checkpoint through COPR and the personal image. No phone, installed settings,
COPR packages, image or deployment changed during this milestone.
