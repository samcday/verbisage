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
- Published client revision:
  [`8f8b6bdc649b520cfa4210dc9c22e0af7b76039a`](https://github.com/samcday/stevia/commit/8f8b6bdc649b520cfa4210dc9c22e0af7b76039a)
- [Identical client fixture](https://github.com/samcday/stevia/blob/8f8b6bdc649b520cfa4210dc9c22e0af7b76039a/tests/fixtures/layout-us-normal.json)
- [Widget exporter](https://github.com/samcday/stevia/blob/8f8b6bdc649b520cfa4210dc9c22e0af7b76039a/tests/export-layout-fixture.c)
  and [private compositor runner](https://github.com/samcday/stevia/blob/8f8b6bdc649b520cfa4210dc9c22e0af7b76039a/tests/native/export-layout-fixture.py)
- [Generation instructions](https://github.com/samcday/stevia/blob/8f8b6bdc649b520cfa4210dc9c22e0af7b76039a/tests/fixtures/README.md)

Check out that client revision, build Stevia and follow its generation
instructions, then compare the generated file with this fixture using
`sha256sum` or `cmp`. Both committed copies have the SHA256 above. The initial
export was generated before the client commits and subsequently reproduced
byte-for-byte. The coordinator's original command logs and dirty-tree manifest
are local artifacts, unavailable from this checkout; the public source,
fixture and regeneration instructions above are the inspectable references.

The tests parse this file into `verbisage::layout::LayoutUpload`, register it
through the real service, and score real dictionary words against its
rectangles. Labels are mapped exactly as exported and dictionary spellings are
preserved exactly. Nothing on the system is read or written.
