# PocketFed integration

The baseline below describes the stable `codex/pocketfed` branch. This
`codex/swipe-prototype` branch adds the changes and updated reader pin described
in [SWIPE.md](SWIPE.md).

This repository imports the public history of
[InsanePrawn/verbisage](https://gitlab.com/InsanePrawn/verbisage). GitHub does not
identify the cross-host import as a native GitHub fork. The integration branch
is `codex/pocketfed`, based on upstream commit
`97aabc197c4e08555bb716f910f614c7c6e601aa`.

The application Rust sources, Cargo manifest and lockfile reproduce the tested
PocketFed `verbisage-0.1.0-1.2.pocketfed.fc46` source package. The source package
SHA-256 is `875c57efd35eb6f316fa1ba3355705f72d474899524a31221ce4cd5a6ebad96c`.
The history separates the original Patricia integration from the subsequent
current-word ranking and transport fixes. This import introduces no additional
runtime changes. Public build examples, manual pages and the private-bus test
live in `contrib/`; no device or desktop-session logs are included in our commits.

The `patricia_dict` submodule pins
[`samcday/android-patricia-dict`](https://github.com/samcday/android-patricia-dict)
at `ba460455e692a1ece86ebf990027c3cfb1e413e4`, based on upstream
`cdab42d9b93a0b33070804b37098568c3a3227f8`. Its Rust code, manifest, lockfile and
license/provenance files match the curated reader source used by that package.
The original public upstream models, RPM and planning/session files remain in
history and the inherited tree; the commands below do not use them.

## Build and test

Use a Rust toolchain supporting edition 2024, a C compiler, pkg-config and the
SQLite development library. These commands were checked with Fedora Rawhide
Rust/Cargo 1.98 and system SQLite. D-Bus uses zbus; a libdbus development package
is not required.

```sh
git clone --recurse-submodules https://github.com/samcday/verbisage.git
cd verbisage
cargo build --locked --no-default-features --features sqlite,dbus,patricia --bins
cargo test --locked --no-default-features --features sqlite,dbus,patricia
```

For an existing clone, run `git submodule update --init --recursive` first.
The explicit feature selection enables Patricia, SQLite and D-Bus. It disables
Hunspell, which remains an optional upstream backend. This configuration builds
Rust library code and the three binaries; it does not provide a C ABI/shared
library. The inherited Meson integration has not been updated for this variant;
use Cargo as shown above.

The enabled Rust suite passes 73 tests and two doctests, including completion
ranking, invalid priors and input bounds, literal SQLite patterns (`'`, `%`, `_`
and backslash), and stdio transport selection under a D-Bus configuration.
Run the real daemon fixture on a private session bus with Python 3, PyGObject
(`gi.repository.Gio`) and `dbus-run-session` installed:

```sh
dbus-run-session -- python3 contrib/test-dbus.py \
  --daemon target/debug/verbisaged
```

It checks `helo` → `hello`, limits, ordering, invalid languages and missing data
using a generated text dictionary. To exercise an installed English Patricia
dictionary, add:

```sh
dbus-run-session -- python3 contrib/test-dbus.py \
  --daemon target/debug/verbisaged \
  --patricia-dict /usr/share/android-patricia-dictionaries/en_US.dict
```

The separately packaged English dictionary comes from
[Helium314/aosp-dictionaries](https://codeberg.org/Helium314/aosp-dictionaries).
The corpus used for the trial has SHA-256
`bd950ef4b57655120eee65cee62a5d216a63f721d9a8bb759ce2022437840443`.
Its source rebuild and license records belong to the data package. The runtime
expects a separately installed data file and never downloads it. The Patricia
submodule retains upstream example dictionaries, including this corpus; the
build commands above do not install those fixtures.

## Runtime and API

`contrib/verbisage.toml` selects the read-only Patricia backend and the separate
`/usr/share/android-patricia-dictionaries/en_US.dict` data file. Run a daemon with
`target/debug/verbisaged --mode dbus --config contrib/verbisage.toml`. It owns the
session-bus name `org.verbisage.Dictionary`, object
`/org/verbisage/Dictionary`, interface `org.verbisage.Dictionary1`.

The packaged configuration lives at `/etc/verbisage/config.toml`. An existing
`$XDG_CONFIG_HOME/verbisage/config.toml` or `~/.config/verbisage/config.toml`
takes precedence; `--config` explicitly selects a file. The activation example
`contrib/org.verbisage.Dictionary.service` assumes binaries installed in
`/usr/bin`. The manual pages describe the client and daemon. Generate current
introspection XML with
`target/debug/verbisage-introspect org.verbisage.Dictionary.xml`.

- `Complete(s word, u max, s lang) -> a(sd)` returns one ranked current-word
  candidate list. It omits case-equivalent exact input, leaving the frontend
  responsible for keeping the raw literal selectable. Maximum output is 100;
  input is limited to 128 Unicode characters and 512 UTF-8 bytes. Empty or
  whitespace input and zero max return no candidates. Control characters and
  oversized input return InvalidArgs.
- `QueryLimited(as prefixes, as suffixes, u min_len, u max_len, s lang, u max)
  -> a(sd)` caps results at 100. Zero length bounds are unconstrained and zero
  max returns no results. Patricia streams matching entries while retaining
  only the requested best results. Short prefixes can still traverse a large
  dictionary range; the output limit is not a constant-time guarantee.
- Existing `Query`, `Suggest`, `IsCorrect`, `Predict`, `Frequency`, `AddWord`
  and `BumpNgram` remain available. Writes and learning are unsupported for
  read-only Patricia data. Invalid languages or unavailable dictionaries return
  errors. There is no backend-identity D-Bus property.

## Current-word ranking

`Complete` combines the highest `max + 1` prefix matches (at most 101) with all
usable single-edit candidates, deduplicates them and truncates the ranked list.
Weights are 1.0 for intact prefixes, 0.9 for adjacent transpositions and
missing/extra doubled letters, 0.65 for other insertions/deletions, and 0.5 for
substitutions. Multiply the weight by `ln(1 + frequency)` divided by the largest
candidate prior. Nonfinite or negative frequencies are unavailable; if all
priors are unavailable, match weights decide order. Equal scores sort lexically.
These are heuristic scores, not calibrated probabilities. Patricia stored ranks
and SQLite counts have different scales; the shared rule is not a fitted model.

Known words and fragments shorter than three characters receive prefix
expansions only. Initial-capital and all-capital variants also count as known.
Prefix lookup still follows the backend's case behavior. Insertions and
substitutions use the existing English a-z alphabet. There are no vocabulary
exceptions. The English regression sample includes `helo`, `teh`, `recieve`,
`thier`, `wrold`, `writting`, `comming`, `becuase`, `tomorow` and `adress`.

Patricia completions exclude blacklisted and not-a-word entries. Its ngram API
supports the existing prediction interface, but the current Stevia integration
uses only `Complete`: one asynchronous call, the literal first, then server
order. Gesture decoding, keyboard layout/geometry, automatic correction,
next-word UI and learning are outside this change. English single-edit
correction is a limited model of typing errors, not Android's full correction
engine. SQLite's bounded query escapes literal patterns and uses an SQL limit.

## Licensing

Verbisage's upstream manifest declares `MIT OR Apache-2.0`. The downstream source
changes use the same license choice; `LICENSE-APACHE` supplies the Apache-2.0
text selected by the package. The optional linked Patricia dependency is
GPL-3.0-only. Its author confirmed that license, relayed by the user on
2026-09-07; see `patricia_dict/LICENSE-PROVENANCE.md` for the precise downstream
metadata delta. The pinned upstream reader did not already contain that license
file. Distributing a binary with this feature must account for GPL-3.0-only and
all linked dependency notices. Cargo's locked registry dependencies are fetched
normally here; the Fedora source package separately vendors them for offline
rebuilds and ships their notices.
