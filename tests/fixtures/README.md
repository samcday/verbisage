# Test fixtures

## `layout-us-normal.json`

A byte-for-byte frozen copy of a real Stevia US normal-layer export: the actual
widget-exporter key rectangles serialized by the real `layout_upload_json`
completer serializer. It is not a handcrafted 26-key approximation.

- SHA256: `32dbc4e6f0df7b1f42324f9cecfbd623402fd62734feac2a76738d4808ed0f1f`
- Keys: 29 (three letter rows plus `,`, space, `.`)
- Coordinates: real widget rectangles; each key 36x50 on a 36-unit pitch
- Period key: main label `.` with an apostrophe among its long-press
  alternates, exactly as the exporter emitted it
- Frozen source copy:
  `../claude/exported-us-fixture-20260914/layout-us-normal.json`
- Source provenance manifest:
  `../claude/exported-us-fixture-20260914/provenance.json`
- Generation evidence:
  `../claude/zcode-round-05/20260914T014723.612515Z-coordinator-export-us-fixture`
- Generator source commit: pending (Stevia round 06); the generating tree was
  dirty and is captured in the generation evidence manifest. The final client
  source reference is pending, so the fixture hash and this provenance are the
  stable identity recorded here.

The tests parse this file into `verbisage::layout::LayoutUpload`, register it
through the real service, and score real dictionary words against its
rectangles. Labels are mapped exactly as exported and dictionary spellings are
preserved exactly. Nothing on the system is read or written.
