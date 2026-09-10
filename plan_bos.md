# BOS (beginning-of-sentence) marker

Status: design locked, not implemented. Replaces the earlier
engine-synthesis proposal (rejected: the engine cannot infer sentence
starts, so it must not manufacture them) and the wire/internal layering
(rejected as overcomplicated).

## The marker, two forms

- **Wire/API form: `"<s>"`** (SRILM-conventional). Human-readable,
  keyboard-authored as a context element, debuggable in logs and protocol
  dumps. Any caller may include it; dumb clients that don't track
  boundaries send nothing and are unaffected.
- **Internal canonical form: U+FFFF.** A Unicode noncharacter:
  representable in Rust `char`/`str` and Python `str`, valid UTF-8
  (`EF BF BF`), permanently reserved — never assigned to a real character.
  Flows losslessly through every backend (sqlite `TEXT` + exact `=`,
  marisa byte keys, patricia `i32` code points, Rust collections) and all
  transports (D-Bus strings, JSON `\uffff`, CLI args). Rejected: `0x110000`
  (unrepresentable in any real string type — it only works as the
  patricia codec's *integer* sentinel), `U+E000` (private use, collidable
  by definition).

Translation `"<s>"` → U+FFFF happens once at API ingress (single choke
point, alongside input validation). The sentinel is filtered from all
user-visible outputs at the gather layer.

## Keyboard contract

Keyboards that track sentence starts (autocaps-capable ones do) truncate
pre-boundary words and prepend `"<s>"`; nothing precedes in-sentence →
context is exactly `["<s>"]`. Empty/absent context keeps its existing
meaning (unknown position → unigram path, unchanged behavior) — it is
explicitly *not* treated as sentence start.

## Backend contract

Backends receive the U+FFFF sentinel as an ordinary context element and:

- **If BOS-aware**: convert it to whatever is store-appropriate.
  - SQLite: literal sentinel rows (requires sentence-split training
    inserting the marker; no schema change, exact-`=` matching handles it).
  - Patricia: translate to the native `0x110000` sentinel id via the
    prepared-context path (a naive string lookup can never hit it — the
    materialization layer skips that codepoint).
- **Otherwise**: truncate context to the words after BOS
  (`["<s>"]` → unigram path, `["<s>", w, …]` → n-gram lookup on the tail).
  Uniform fallback, no per-backend inventiveness.
- **No BOS in context**: status quo — nothing to drop, no special
  handling; backends do exactly what they do today.

Either way, a miss degrades into standard backoff — byte-identical
behavior on stores without BOS data. No gate flags.

## Verification (when implemented)

- Fixtures with sentinel rows pin BOS preference over flat unigrams
  (`<s> hello` frequent → `hello` outranks globally-frequent rivals on
  `["<s>"]` context).
- Fixtures without sentinel rows pin byte-identical behavior to today.
- Guard test: the sentinel never appears in any user-visible candidate
  list; round-trip test `"<s>"` ↔ U+FFFF at ingress.
