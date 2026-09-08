# Verbisage

This `codex/swipe-prototype` branch adds experimental whole-word gesture
recognition. See [SWIPE.md](SWIPE.md) for its API, build and replay checks.
The stable integration remains on `codex/pocketfed`.

Verbisage provides dictionary queries, spelling suggestions and prediction through
Rust, a command-line client, and a session D-Bus daemon.

This is a public downstream import of
[InsanePrawn/verbisage on GitLab](https://gitlab.com/InsanePrawn/verbisage), preserving
its history. The `codex/pocketfed` branch adds an optional Android Patricia backend
and a bounded, unified current-word completion API used by Stevia.

See [DOWNSTREAM.md](DOWNSTREAM.md) for the upstream pins, build commands, API,
ranking rules, test coverage and limitations. Clone with `--recurse-submodules`
to obtain the pinned Patricia dependency. Registry dependencies are resolved from
`Cargo.lock`; they are not vendored into this repository.
