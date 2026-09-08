# Whole-word swipe prototype

`codex/swipe-prototype` adds optional Drift Type recognition to the stable
`codex/pocketfed` integration. It is a downstream experiment; it is not a change
to the stable package or an upstream Drift Type submission.

The application starts from Verbisage `040dd993ce21adeb97d93bcb16c25eda8767e865`.
The Patricia submodule advances to `c76e17ae04aa563fc683d03e61bd673b20ab863e`,
which merges upstream `552c652c75c4d32640e90d702862b93f7e275f2b` and retains the
GPL-3.0-only licensing metadata. Drift Type is a Cargo Git dependency pinned to
`f63256a5bb973b10a4e11e190cfbb105fc89da0a`. Its upstream license is Apache-2.0;
its existing manifest incorrectly names that identifier as a license filename.
The source is used without changes. Drift Type prohibits AI-generated upstream
contributions; no code, report, comment or request has been submitted there.

## Build and exercise

```sh
git clone --branch codex/swipe-prototype --recurse-submodules \
  https://github.com/samcday/verbisage.git
cd verbisage
cargo build --locked --no-default-features --features sqlite,dbus,patricia,swipe --bins
cargo test --locked --no-default-features --features sqlite,dbus,patricia,swipe

dbus-run-session -- python3 contrib/test-dbus.py \
  --daemon target/debug/verbisaged \
  --patricia-dict /usr/share/android-patricia-dictionaries/en_US.dict

dbus-run-session -- python3 contrib/test-swipe.py \
  --daemon target/debug/verbisaged \
  --patricia-dict /usr/share/android-patricia-dictionaries/en_US.dict \
  --output swipe-replay.json
```

Use the same Rust/Cargo, C compiler and SQLite development dependencies as the
stable branch. Private-bus tests additionally require dbus-run-session and
Python PyGObject/Gio. The dictionary is supplied separately. An optimized
binary uses `cargo build --release` with the same feature selection.

The `swipe` feature is opt-in and implies Patricia and D-Bus. The existing
`Complete` API and its tests remain available. SQLite completion is retained;
the swipe prototype requires a Patricia dictionary backend. Config and session
service paths are unchanged. This branch does not install or activate a service
by itself, and it does not change keyboard settings.

## Request contract

On the existing `org.verbisage.Dictionary1` interface at
`/org/verbisage/Dictionary`, the new method is:

```
RecognizeSwipe(a(ddu) trace, a(sdddd) keys, u max, s lang) -> a(sd)
```

Each trace point is `(x, y, elapsed_ms)`. Each key is
`(lowercase_label, left, top, width, height)`. All positions and rectangles use
the keyboard widget's logical coordinate system. The first timestamp is zero;
timestamps are nondecreasing. This version handles one finger and one whole
word, with no prefix, suffix or preceding-word context. The frontend submits
once on release, discards canceled or stale responses, and presents candidates
for explicit selection. It must not treat a failed request as text to commit.

Results are deduplicated and ordered best first, with a finite higher-is-better
heuristic score. Scores do not express a calibrated probability and cannot be
compared between gestures. `max` is capped at 100; zero produces an empty list
for a valid request and available dictionary.

Requests accept 2–512 points, 2–64 keys, and at most 10,000 milliseconds. Key
labels must be distinct single lowercase ASCII letters, so the useful maximum
is 26 keys. Coordinates must be finite and bounded by 16,384 logical units in
magnitude; key bounds must be nonnegative and fit inside that extent, with
width and height at least one unit. Stationary and malformed traces are
rejected, including motion that collapses after conversion to Drift's f32
coordinates. Invalid shapes return InvalidArgs; missing dictionaries,
unsupported backends, busy workers and exhausted budgets return Failed.

## Recognition and work limits

Drift extracts candidate endpoint labels from the actual trace and layout.
Verbisage passes those constraints to Patricia's node visitor. The visitor
prunes incompatible starting letters, unavailable characters and overlong
words, excludes blacklisted, not-a-word and beginning-of-sentence markers,
and checks both elapsed time and node count at every visited node. Flagged
terminal words do not prevent searching valid descendants.

A request visits at most 131,072 trie nodes, with a 500 ms candidate-search
budget, retains at most 2,048 candidates, and accepts dictionary words of
2–48 ASCII letters present on the supplied keyboard. A request-local owned
snapshot keeps strings alive for Drift's borrowed dictionary trait. Patricia's
stored probability divided by 255 supplies the unigram prior, matching Drift's
existing AOSP-entry helper; it is not treated as a corpus count. Drift then
scores the complete path using its default solver settings and 40 resampling
points. No letter-by-letter nearest-key decoder substitutes for Drift.

The daemon permits one swipe worker at a time and creates no pending swipe
queue. Dictionary/gesture work runs on the daemon's blocking-worker runtime,
while ordinary D-Bus completion stays responsive. A response deadline is one
second. A timed-out worker keeps its permit until it finishes; timing out does
not forcibly interrupt Drift's current scoring loop. Node, candidate, word and
trace limits bound that remaining work. The UI must still ignore stale replies.

Keys and trace are translated together to zero origin before constructing
Drift's layout. This works around its current rectangle-size normalization
behavior for nonzero origins. Rectangle bounds are then rounded to logical
pixels, as required by Drift's public `RectKey` constructor. Motion points
remain floating point. Tests compare both ranks and scores after translation.

## Validation and limitations

The Rust suite adds real Drift/Patricia gesture regressions, input and geometry
validation, excluded dictionary flags, node/time exhaustion on nonmatching
tries, and a blocked-worker test showing that completion remains responsive
and abandoned work does not accumulate. The private-bus replay script checks
synthetic paths for ten words, deterministic perturbations and transformed
layouts, then exercises malformed requests and `Complete` before and after.
It writes exact inputs, rankings and elapsed times for inspection.

These synthetic traces test integration and deterministic behavior. They are
not a human gesture corpus or an estimate of real typing accuracy. English
ASCII, whole-word, single-finger operation is the initial scope; accented
letters, non-Latin layouts, multi-touch, personalization and learning are not
implemented. The frequency-limited candidate pool can exclude rare words. In the current
30-case synthetic replay, 29 targets appear in the first two candidates. The
perturbed `swipe` trace is a recorded quality limitation: it is absent from the
first six candidates. The script checks deterministic behavior for that case
and records its rank without asserting recognition. Increasing the pool from
2,048 to 4,096 did not recover it and increased measured debug latency, so the
smaller limit is retained. These observations are not human-corpus accuracy or
native-device performance claims.
Drift's API and scoring are experimental. Recognition quality and native
latency need testing with real keyboard gestures before relying on this as a
finished input method.
