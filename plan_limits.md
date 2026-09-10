# Result limits (`Complete` 1k / bounded queries 200k)

Status: implemented in `2b13b40` (on top of picks `8584916`, `21e3ec2`).
This doc records the as-built design, the rationale, and what's left.
(Note: an earlier draft of this content lived in `plan_caps.md`, which is
about capitalization handling — "caps" as in limits lives here now.)

## The two knobs

| Method | Knob (CLI / config / struct) | Default |
|---|---|---|
| `Complete` (current-word) | `--max-complete-results` / `[daemon] max_complete_results` / `DaemonConfig::max_complete_results` | **1_000** |
| `QueryLimited` (bounded queries) | `--max-query-results` / `[daemon] max_query_results` / `DaemonConfig::max_query_results` | **200_000** |

Split rationale: `Complete` serves interactive keystroke traffic (small,
latency-sensitive); bounded queries serve bulk/filtered dictionary reads
(import/export style). One shared cap would either starve queries or leave
completion unbounded. No per-language caps — global only.

## Semantics

- `max == 0` → empty result, unchanged (short-circuits downstream).
- `0 < max <= cap` → honored exactly by the transport; engines apply only
  their own documented truncation (the transplanted prefix engine still
  carries its internal `min(100)` truncations — interim state, removed with
  the `android.rs` switch, which honors `max` exactly).
- `max > cap` → `Err("requested max {max} exceeds {Complete|bounded-query} cap {cap}")`.
  Rejected, never silently clamped: the caller asked for N and must know it
  didn't get N. Daemon errors surface over D-Bus via the existing error path.

## Enforcement point

`DaemonHandler::complete` / `DaemonHandler::query_limited`, as the first
check — before language resolution and backend loading (fail fast, no
backend instantiation for rejected requests). Single choke point shared by
all transports; D-Bus `Complete`/`QueryLimited` pass `max` straight through
(`u32 as usize`, no `min()`), keeping only their input-shape validation
(size/control-char/whitespace rules).

## Precedence

```
CLI flag > [daemon] config-file key > built-in default
```

- Built-in defaults live in `CompletionConfig::default()` (library):
  `max_complete_results: 1_000`, `max_query_results: 200_000`
  (plus `response_deadline` / `search_budget`, 5s each — shape only for now).
- `DaemonConfig::default_for` and `DaemonConfig::from_cli` seed from those
  defaults; `from_cli` applies CLI flags when present.
- `verbisaged` overlays `[daemon]` config-file values only when the
  corresponding CLI flag is absent (same file as named-backend passthrough).
- `DaemonHandler::new` (tests, pre-built backends) gets defaults via
  `default_for`; `with_config` carries whatever was resolved.

## Library relationship

Engines ignore caps: `PrefixCompleter` passes `max` through (transplant
frozen); the cap lives at the transport edge, not in ranking logic.
`CompletionConfig` is the single source of default values shared by library
and daemon; `DaemonConfig` is the effective per-process copy.

## Verification (done)

- Handler tests: engine wiring + at-cap pass-through, over-cap rejection
  for both methods, configurability via `with_config` with tightened caps
  (rejection fires before any backend load).
- Full suite green on `sqlite,dbus,patricia` and default features (76 + 2).
- CLI smoke: `--help` lists both flags; a config file with
  `[daemon] max_complete_results = 500` parses and round-trips via
  `config-dump`.

## Future / non-goals

- **stdio parity**: required — stdio JSON protocol + `StdioClient` expose
  `complete` / `query_limited` matching D-Bus (decided). They hit the same
  handler choke points, so no new enforcement work.
- **One-shot CLI**: caps are a daemon-transport concern; `verbisage`
  one-shot modes bypass the handler and are uncapped.
- **Per-language caps**: not planned; reopen only on evidence of need.
- **Rejection observability**: over-cap requests return errors to the caller;
  no separate metric/log line. Revisit if abuse debugging needs it.
