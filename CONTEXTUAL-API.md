# Contextual completion and prediction

The default engine ranks prefix matches and single-edit corrections together,
using the same language model for current-word and next-word requests. The
stored spelling is returned. Native Patricia probabilities are interpolated
directly; count models normalize their counts. Scores are relative ranking
heuristics, not calibrated probabilities.

Rust uses `completion::CompletionInput` and `AndroidCompleter::complete_with`.
The input and committed context have independent `TextPrep` selections. Library
and daemon defaults are exact (`none`, `none`); CLI defaults are NFC and language
folding. The language registry initially has no language-specific entries and
falls back to Unicode lowercase. Full non-Turkic folding maps dotted capital I
to i plus combining dot. Folding never silently adds NFC.

## CLI

```
verbisage --backend patricia --system-dict /path/en_US.dict --user-dict '' \
  complete --word l --context 'see you' --fold full --context-fold full
verbisage --backend sqlite --system-dict /path/en_US.db --user-dict '' \
  predict --context 'see you'
```

Both commands accept `--normalize`, `--fold`, `--context-normalize`,
`--context-fold`, `--case-preference`, and `--max`. Case preference is
`prefer_matched` or `insensitive`. Normalization is `none` or `nfc`; folding is
`none`, `ascii_lowercase`, `unicode_lowercase`, `full`, or `lang_specific`.
The CLI uses the same engine locally or through `--mode dbus`.

## D-Bus

Name `org.verbisage.Dictionary`, path `/org/verbisage/Dictionary`, interface
`org.verbisage.Dictionary1`:

* `CompleteWith(s word, as context, u max, s lang, (ss) input_prep,
  (ss) context_prep, s case_preference, s layout, a(dd) points) -> a(sd)`
* `PredictWith(as context, u max, s lang, (ss) context_prep) -> a(sd)`
* `RegisterLayout(s layoutJson) -> s token`
* `ForgetLayout(s token) -> b`
* `RecognizeSwipe(a(ddu) trace, s layout, u max, s lang) -> a(sd)` (swipe
  builds; see Gesture recognition)

Each prep tuple is `(normalization, fold)`. Empty current input requests
next-word candidates. Existing methods remain available; legacy `Complete`
retains its empty/whitespace-input empty-result convention. `Suggest` takes
trailing `s layout` and `a(dd) points` arguments.

`DbusClient::complete_with` and `predict_with` expose typed Rust wrappers.
`DbusClient::{register_layout, forget_layout, complete_with_layout,
suggest_layout}` expose the layout surface. The service uses two completion
workers. A worker that outlives a response keeps its slot until it actually
finishes, preventing an abandoned-work queue.

## Keyboard layouts

A client may register a keyboard layout and receive a content-hash token, then
reference that token on completion (`CompleteWith`) and correction (`Suggest`)
requests. Layouts are held in a bounded session-only in-memory cache; nothing
is written to disk. An unknown token is an explicit error.

The spatial model is selected per request from the available data: no layout
uses the geometry-free alphabet; a layout token alone uses key-to-key
proximity; a layout token plus `points` uses touch. `points` are `[x, y]` pairs,
one per input character; a non-empty list whose length differs from the input
length is an error. Touch requests use HeliBoard's additive spatial/language
model with an edit-accuracy gate, while layout-only requests use key proximity;
without either, the previous geometry-free output is unchanged.

A client registers the layer it is actually showing, in its own spelling: an
active Shift layer uploads capital labels. Layout labels are prepared with the
request's own `input_prep` before the edit alphabet is built, so a folded input
still reaches them; with the default no-op preparation labels are used as
authored. A label that does not prepare to exactly one character is skipped.

Empty input asks for next-word candidates and is ranked by the geometry-free
model even when a layout token is supplied: there is no typed position to
compare against, so the spatial branch charged every candidate the same
first-completion cost and collapsed realistic low probabilities into a lexical
tie. `PredictWith` has no layout parameter and is unaffected.

A candidate's spatial cost is the cost of the edit that produced it, plus the
first-completion cost for a prefix match. That edit is aligned by construction
and its substitutions are already priced by the layout — anchored on the touch
point when there is one — so proximity decides between same-length candidates.
Comparing input and candidate character by character instead charged every
following character for a single missing letter, which sank ordinary omission
corrections below unrelated neighbouring keys.

Two consequences worth knowing. A dropped repeated letter and a slip onto a
neighbouring key are both cheap and comparable, so the language model decides
between them; a slip onto a distant key is not, and loses even to a likelier
word. And alternate labels are positions only: they let a correction start from
a typed long-press accent, but the edit alphabet comes from single-character
main labels, so an alternate does not generate an accent correction.

An upload carries either explicit key rectangles (`keys`: label, alt labels,
left/top/width/height) for touch layouts, or `rows` (the shared `RowLayout`
intermediary) for physical layouts. The CLI accepts `--layout <value>` on
`complete` and `correct`. A file path is auto-detected as HeliBoard simple rows,
HeliBoard/FlorisBoard JSON, or Unicode Keyboard3 XML; a non-path value is
treated as an XKB layout name (e.g. `us`, `us(intl)`, `de(nodeadkeys)`),
resolved from the system/user XKB data with geometry when available. When a
layout is present, correction candidates are limited to nearby keys and
weighted by key distance; without one the geometry-free a–z alphabet is used,
preserving prior output.

## Gesture recognition

`RecognizeSwipe(a(ddu) trace, s layout, u max, s lang) -> a(sd)` resolves a
complete single-finger gesture against a registered layout. `layout` is a
token from `RegisterLayout`: the same registry and the same immutable layout
object completion uses, so one registration serves `CompleteWith` and
`RecognizeSwipe`. The token is resolved before a recognition worker is
dispatched; forgetting or evicting it afterwards cannot change a request
already accepted, while an unknown token before that point is the explicit
`unknown layout token` error completion reports, and an empty token is
rejected. There is no geometry-free fallback; the prototype's raw key-rectangle
argument is gone. `lang` is the complete selected language, independent of the
layout, and resolves exactly as for completion: regional fallback applies,
a missing dictionary is an error, and nothing substitutes English.

Trace points `(x, y, elapsed ms)` are in the registered layout's own widget
coordinates. The layout normalizes them once, so translated, scaled and
fractional geometry recognize the same path. A trace needs 2..512 points with
nondecreasing timestamps of at most 10 s, finite coordinates and real motion
both before and after normalization; stationary or collapsed traces are
rejected, as is a layout with fewer than two gesturable labels. Main and
alternate labels both locate a word's graphemes, compared with dictionary
words in one canonical form on both sides: NFC, then Unicode lowercase. An
active Shift layer's capitals and a decomposed `e` + combining acute therefore
both meet a dictionary's `é`. Results carry the stored spelling, one per
distinct scoring form, preferring the likelier stored spelling. A word is
offered only when every grapheme is on the layout or among its ignored
labels, so an incomplete path is never scored; ignored labels are skipped in
paths but never count as missing, and words containing labels the layout
neither maps nor ignores are excluded for now. Candidate search keeps the
time, node and result budgets; the caller's `max` is honoured up to the
configured Complete cap, and `max = 0` returns nothing without any work.

## Stdio

Requests are one JSON object per line. Results contain word/confidence objects:

```json
{"id":1,"method":"complete_with","lang":"en_US","params":{"word":"l","context":["see","you"],"max":6,"options":{"input_prep":{"normalization":"nfc","fold":"full"},"context_prep":{"normalization":"nfc","fold":"full"},"case_preference":"insensitive"}}}
```

`predict_with` takes `context`, `max`, and `options` (context preparation is
used). `complete` accepts the same parameters as `complete_with`; `predict`
also accepts optional preparation options. `query_limited` accepts ordinary
query constraints plus `max`. The Rust `StdioClient` has matching helpers.
Completion and prediction use one worker with a response deadline; a timed-out
worker remains occupied until it exits, and its late reply is discarded.

## Limits and status

Completion/prediction default to an output cap of 1,000; bounded queries default
to 200,000. Requests above configured caps return errors. `max = 0` returns no
results. The legacy prefix engine and swipe no longer silently clamp at 100.

Gesture recognition has its own capacity, separate from completion so a busy
keyboard cannot starve typing. `--swipe-workers` (CLI), `[daemon]
swipe_workers` (config file) or the built-in default of two set how many
recognitions may hold a CPU worker at once, in that order of precedence. A
requested value is honoured as given, never clamped; zero and, in swipe-enabled
builds, counts above Tokio's semaphore limit are rejected with an error.
Each permit is held inside the blocking worker until that work really
exits, so a response that timed out, was cancelled or lost its client still
occupies its slot and abandoned work stays bounded. A request beyond the
configured count is refused as busy rather than queued.

`CompletionConfig` separates response/search durations (five seconds by default)
from the intermediate search limit (200,000 candidates). Exhausting a search
budget returns an error. Built-in scans check cancellation during traversal;
SQLite also uses its VM progress callback. Third-party backends should override
`DictionaryBackend::search_words` to cooperate with cancellation.

A caller can explicitly pass `<s>` for sentence start; it is never inferred by
the service or emitted as a suggestion. SQLite supports marker rows. Patricia
resolves the native sentence-start sentinel in prepared context and backs off
when no marker data exists. The paired Stevia branch refreshes predictions
after accepted words and supports chained selections and Backspace undo.
See `CONTEXTUAL-WORK.md` for measured evidence and device-trial limitations.
