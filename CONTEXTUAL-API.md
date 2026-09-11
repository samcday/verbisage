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
  (ss) context_prep, s case_preference) -> a(sd)`
* `PredictWith(as context, u max, s lang, (ss) context_prep) -> a(sd)`

Each prep tuple is `(normalization, fold)`. Empty current input requests
next-word candidates. Existing methods remain available; legacy `Complete`
retains its empty/whitespace-input empty-result convention.

`DbusClient::complete_with` and `predict_with` expose typed Rust wrappers.
The service uses two completion workers. A worker that outlives a response
keeps its slot until it actually finishes, preventing an abandoned-work queue.

## Stdio

Requests are one JSON object per line. Results contain word/confidence objects:

```json
{"id":1,"method":"complete_with","lang":"en_US","params":{"word":"l","context":["see","you"],"max":6,"options":{"input_prep":{"normalization":"nfc","fold":"full"},"context_prep":{"normalization":"nfc","fold":"full"},"case_preference":"insensitive"}}}
```

`predict_with` takes `context`, `max`, and `options` (context preparation is
used). `complete` accepts the same parameters as `complete_with`; `predict`
also accepts optional preparation options. `query_limited` accepts ordinary
query constraints plus `max`. The Rust `StdioClient` has matching helpers.

## Limits and status

Completion/prediction default to an output cap of 1,000; bounded queries default
to 200,000. Requests above configured caps return errors. `max = 0` returns no
results. The legacy prefix engine and swipe no longer silently clamp at 100.

`CompletionConfig` separates response/search durations (five seconds by default)
from the intermediate search limit (200,000 candidates). Exhausting a search
budget returns an error. Built-in scans check cancellation during traversal;
SQLite also uses its VM progress callback. Third-party backends should override
`DictionaryBackend::search_words` to cooperate with cancellation.

A caller can explicitly pass `<s>` for sentence start; it is never inferred by
the service or emitted as a suggestion. SQLite supports marker rows. Native
Patricia sentinel support and Stevia prediction chaining are still pending.
Broad next-word searches also need further performance work before device
acceptance. See `CONTEXTUAL-WORK.md` for measured evidence and remaining work.
