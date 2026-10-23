# Task — split document persistence into two serializers

Status: task (2026-10-23). Records the owner decision of 2026-10-23: native JSON and
upstream `.FCStd` become **two explicit, independently testable serializers**. Companion
to [`../architecture/part-draft-layering.md`](../architecture/part-draft-layering.md), the
[conformance runbook](../workbenches/conformance.md) and
[`../direction/mvp-path.md`](../direction/mvp-path.md) (§8 decision 3, which this closes).

**Legend.** **(verified)** = read from the tree on 2026-10-23; **(recommended)** = not implemented.

## Decision

- The **native JSON format** (extension **`.ferrocad`**, recommended) is FerroCAD's save
  format and may evolve freely.
- **Upstream `.FCStd`** is an import/export format, conformance-tracked: upstream tests
  write and reopen it; **(verified)** `ColorPerFaceTest.py`, `Document.py`.
- Both paths are selected **explicitly**, never by sniffing content. **No `.FCStd` writer
  exists today.**

## 1. Extension and format-detection policy

### 1.1 Why `.ferrocad`

`.fe203` was the alternative considered. `.ferrocad` names the product, matches the
`ferrocad` crate namespace, and does not read as a version number; `.fe203` is memorable
but unexplained and easy to mistake for a revision counter. Matching is case-insensitive;
canonical spelling is lowercase `.ferrocad`.

### 1.2 Behaviour by suffix

| Suffix | Read | Write | Notes |
| --- | --- | --- | --- |
| `.ferrocad` | native JSON only | native JSON | default save format |
| `.FCStd` (any case) | ZIP + `Document.xml`; transitional JSON fallback (§1.3) | placeholder JSON now; real ZIP **after** the writer lands | conformance format |
| none / unknown | `ValueError` naming `.ferrocad` and `.FCStd` | `ValueError` | no silent default; the GUI dialog supplies the suffix |

### 1.3 The transitional rule

Today `saveAs` writes JSON whatever the path says and `openDocument` compensates with a
content check; **(verified)** `lib.rs:4853-4902`, `has_zip_magic`. Because it applies to
**every** path, a ZIP named `.ferrocad` is read as FCStd — the silent change this decision
forbids.

Until `write_fcstd` lands: (1) the `.FCStd` write arm is a named
`FcstdWriter::JsonPlaceholder` — the suffix chose the backend, which currently emits
native bytes; (2) the `.FCStd` read arm is ZIP first, then native only for non-ZIP bytes
carrying the native marker (§1.4), so a malformed ZIP is reported and never retried;
(3) `.ferrocad` never falls back to FCStd, and an unknown suffix is never guessed.

**The one dishonest case** is `saveAs("x.FCStd")` producing a non-FreeCAD file. It is
removed by deleting the `JsonPlaceholder` arm at the writer switch (checklist item 8); CI
then asserts the ZIP magic. The marker-gated read fallback stays read-only and can go
after one release.

### 1.4 Native envelope

`SavedDocument` is currently `{name, objects}` with no format or version; **(verified)**
`crates/ferrocad_core/src/document/mod.rs:983-1006`. Add, with serde
defaults so old files still load **(recommended)**:

```json
{"format": "ferrocad", "version": 1, "name": "…", "objects": [...]}
```

Missing envelope = version 1 (the transitional files); a *higher* version errors with
"written by a newer FerroCAD", and unknown top-level keys are ignored, not denied.

## 2. Module and API shape

### 2.1 Core: one boundary, two backends

`SavedDocument` stays the only currency between the document model and persistence
**(verified)**; `Document::to_saved`/`from_saved` do not change. Proposed layout
**(recommended)**:

| Path | Holds |
| --- | --- |
| `document/format.rs` | `DocumentFormat`, suffix parsing, `read`/`write` dispatch |
| `document/native.rs` | serde encode/decode + version envelope (the JSON halves of `save_to_file`/`load_from_file`) |
| `document/fcstd.rs` (+ `fcstd/zip.rs`) | ZIP + XML; `read_fcstd` today, `write_fcstd` later, archive writer new |

```rust
pub enum DocumentFormat { Native, Fcstd }
impl DocumentFormat {
    pub fn from_path(path: &str) -> Result<Self, FormatError>; // case-insensitive
    pub fn extension(self) -> &'static str;                   // "ferrocad" | "FCStd"
}
pub fn read(path: &str) -> Result<(SavedDocument, DocumentFormat), FormatError>;
pub fn write(doc: &SavedDocument, path: &str, format: DocumentFormat) -> Result<(), FormatError>;
```

- `read_fcstd` is reachable only from the `Fcstd` arm; there is no "sniff this path"
  entry point, and `has_zip_magic` is deleted (`FcstdError::NotZip` covers validation).
- `ferrocad_core` never depends on `ferrocad_occt` **(verified)**: shape bytes come from
  the module-owned `ExtensionData::save()`, and the writer only places them in members.

### 2.2 Python-facing behaviour

| API | Today **(verified)** | Proposed **(recommended)** |
| --- | --- | --- |
| `saveAs(path)` | JSON always; records `file_name` (`lib.rs:1542-1557`) | select by suffix, write, record `(file_name, format)` |
| `save()` | reuses `file_name`, JSON (`lib.rs:1559-1578`) | reuse the **recorded format**, so a later rename cannot flip meaning |
| `load(path)` | JSON only (`lib.rs:1580-1595`) | same selector as `openDocument` |
| `openDocument(path)` | ZIP magic → FCStd, else JSON (`lib.rs:4853-4887`) | suffix → backend; delete `has_zip_magic` |

`dump`/`restoreContent` stay native JSON in memory (not a file format). `FileName` is
read-only (no setter) **(verified)**, so `doc.FileName = p; doc.save()` is unavailable.

## 3. The `.FCStd` backend

### 3.1 What the reader does today **(verified)**

`fcstd.rs` is read-only: it parses the ZIP central directory itself (stored/deflate, no
ZIP64, no encryption), reads only `Document.xml`, and returns a `SavedDocument` with
`property_status`, `expressions`, `extensions`, `python_state` and `view` empty. It
deliberately skips `.brp` members and every type outside the eight App types listed in
`fcstd.rs:14-17`. Measured in the fixture, that drops `PropertyLength` (10),
`PropertyAngle` (9), `PropertyLinkSub` (9), `PropertyEnumeration` (8), the expression
engine (16) and all 13 `Part::PropertyPartShape` — enough for the tangent test, not for a
faithful import.

### 3.2 Writer breakdown **(recommended)**

ZIP: single-disk, unencrypted, deflated, never ZIP64; members are `Document.xml`,
`GuiDocument.xml`, `PartShapeN.brp` and `DiffuseColor*` (no thumbnail in v1).

**`Document.xml`.** Emit what the fixture shows: `<Document SchemaVersion="4"
FileVersion="1" ProgramVersion="FerroCAD <version>">`, document `<Properties>` (the model
has no UID/comment/dates, so write `Label`), `<Objects Count>` with `<ObjectDeps>` from
the existing link graph, and `<ObjectData Count>` with one `<Object name>` per
`SavedObject`. Static transient properties are `<_Property>` (no value), dynamic ones are
`<Property>` — matching `SavedObject::from_object` (`mod.rs:1008-1043`).

**Property elements.** `SavedObject` stores values, not type ids, so the writer derives
the element from `Property::type_name()`. Verified: scalars are `<String|Integer|Bool
value>` and `<Float value>` (also for `PropertyLength`/`PropertyAngle`), enumeration is
`<Integer value>`, geometry is `<PropertyVector x y z>` / `<PropertyPlacement Px..Q3 A
Ox..Oz>`, links are `<Link>`, `<LinkList count>` and `<LinkSub value count><Sub>`, and
module values are `<Part file>` or `<ShapeList>`. Unverified: `Rotation`/`Matrix`/list
element names, `ColorList`, and `PythonObject` (`<Python value="<base64 pickle>">`).

Lossiness to settle: `Property::String` cannot distinguish `PropertyString` from
`PropertyFile`; `PropertyUUID`/`PropertyMap` have no variant; `Quantity` always reports
`App::PropertyLength`, losing `PropertyAngle`; and `python_state` is not a property. Add a
type-id map to `SavedObject` (recommended) or accept a documented approximation.

**`.brp` payloads.** `ShapeProperty::save()` already returns ASCII BREP
(`shape_property.rs:77-82`, `ferrocad_occt::write_brep` → `BRepTools::Write`). Write it
verbatim to `PartShape.brp`, `PartShape1.brp`, … with `<Part file="PartShapeN.brp"/>`.
For `Part::PropertyTopoShapeList`, upstream uses one member per entry
(`<TopoShape file="PartShapeN.M.brp"/>`), but `ShapeListProperty::save()` is one
length-framed blob; expose per-entry blobs (an `fcstd_payloads()` hook on `ExtensionData`
defaulting to `None`) rather than teaching core to parse the framing.

**`GuiDocument.xml` is required, not optional.** `ColorPerFaceTest.py` saves, reopens,
sets `Visibility = True` and asserts six restored `DiffuseColor` entries **(verified)**.
Map `ViewState` (`view.rs:27-52`) to one `<ViewProvider name>` per object with a `view`:
`Visibility`→Bool, `Transparency`→Integer, `ShapeColor`→`<PropertyColor
value="0xRRGGBB00">`, `DiffuseColor`→`<ColorList file="DiffuseColorN"/>` (LE `u32` count +
one `u32` per colour, packed `0xRRGGBB00`). `DisplayMode` needs a name→index table per
object type; write index 0 until one exists.

**Versioning.** Version the native envelope, `SchemaVersion`/`FileVersion`, the property-
element table, member naming (`PartShapeN.brp`, `DiffuseColorN`, by object position) and
the ASCII BREP flavour; a writer constant records the schema that was emitted.

**Failure policy.** Do not silently drop an unrepresentable property: error naming the
object, property and type, with an explicit "export anyway" option returning the dropped
list. Quietly losing `PropertyLinkSub` is worse than refusing the save.

## 4. Migrating our workflows and docs

Generated user pages are built by running `tools/workflows/`; edit the scripts, then
regenerate, never the pages; **(verified)** `docs/README.md`. The proposed source changes:

| File | Change |
| --- | --- |
| `tools/workflows/00_first_model.py:99`, `03_undo_and_persistence.py:66` | `first.FCStd`, `undoable.FCStd` → `.ferrocad` |
| `docs/user/tutorials/first-model.md`, `docs/user/how-to/undo-and-persistence.md` | regenerate only |
| `examples/file_roundtrip.py`, `examples/mvp_workflow.py` | path + the "JSON under a `.FCStd` name" docstring |
| `tests/test_base_surface.py` (10 sites), `tests/test_part.py:63`, `crates/ferrocad_toy/tests/round_trip.rs:254` | `.ferrocad`; keep one explicit FCStd round-trip per feature |
| `crates/ferrocad_core/tests/fcstd.rs` | unchanged: it reads the **upstream** fixture |

**Conformance policy.** Upstream files are never edited, so every upstream `.FCStd` save
must keep working; the harness groups are the gate for the switch, and our fixtures move
to `.ferrocad`. FCStd probes: `ColorPerFaceTest`, `TopoShapeListTest`, `TestLinkArray*`,
`regression_tests`, `Document.py`.

## 5. Tests and CI

- **Native round-trip:** every `Property` variant (plus a registered test extension),
  `property_status` (static vs dynamic transient), expressions, `extensions`,
  `python_state`, and all `ViewState` fields. Extend
  `tests/test_base_surface.py::TestPersistence` and `TestPythonObjectPersistence`.
- **FCStd read and write:** the existing `crates/ferrocad_core/tests/fcstd.rs` baseline
  stays; add scalars, links, placement, one shape and one shape list written and read back
  with `read_fcstd`, comparing members, then the Python `saveAs`/`openDocument` pair and
  the upstream probes.
- **Cross-format refusal:** a ZIP renamed `.ferrocad` errors (no FCStd fallback); a
  truncated ZIP named `.FCStd` says "not a ZIP archive", not a JSON parse error; unknown
  suffixes error naming both.
- **Explicit-selection CI:** a Rust unit test over `DocumentFormat::from_path`; a Python
  test that `.ferrocad` output starts with `{` + the marker and, after the switch, `.FCStd`
  starts with `PK\x03\x04`. `read_fcstd` is reachable only from the FCStd arm, so the
  compiler also enforces the split.

## 6. Checklist (risk order)

- [ ] **1. Dispatch split.** `DocumentFormat`, suffix parsing, native/FCStd dispatch in
  `openDocument`, `load`, `saveAs`; delete `has_zip_magic`; keep the JSON placeholder for
  `.FCStd` writes. Tests for every suffix and for refusal. *Lowest risk.*
- [ ] **2. Native envelope.** `format`/`version` with serde defaults; keep loading bare
  files; update tests and `examples/file_roundtrip.py`.
- [ ] **3. Migrate our artefacts** (§4), regenerate the two user pages.
- [ ] **4. Reader gaps needed by the writer:** `PropertyLength`, `PropertyAngle`,
  `PropertyEnumeration`, `PropertyLinkSub`, the expression engine,
  `App::PropertyPythonObject`. Verify with fixtures.
- [ ] **5. FCStd ZIP writer behind a direct-call API:** `Document.xml` + `.brp` for
  `Part::PropertyPartShape`; not yet wired to `saveAs`.
- [ ] **6. FCStd writer breadth:** shape lists, expressions, `python_state`/`Proxy`,
  `ObjectDeps`, no-silent-loss policy.
- [ ] **7. `GuiDocument.xml`:** `Visibility`, `Transparency`, `ShapeColor`,
  `DiffuseColor`, `DisplayMode`. Must be green before the switch.
- [ ] **8. Switch.** `saveAs(.FCStd)` emits ZIP; delete `JsonPlaceholder`; add the magic
  assertion; default, `--part` and `--draft` groups unchanged. *Highest risk.*
- [ ] **9. Follow-ups.** Thumbnail, `ElementMap`, an upstream-FreeCAD open check.

## What is not done (2026-10-23)

- **No `.FCStd` writer.** No `write_fcstd`, ZIP writer or `GuiDocument.xml` emitter. The
  only Document.xml generation is the recovery stub in
  `python/FreeCAD/__init__.py:662-707` — bare `<Object>` tags, not a document.
- **No module-payload read.** `.brp` members and shape properties are skipped by design;
  a reopened upstream document loses its shapes.
- **Document metadata is not persisted.** `SavedDocument` has no UID, comment, meta or
  dates; `PyDocument.meta`/`comment` are not saved **(verified)**.
- **Type ids are lossy** for `String`-backed types; no `SavedObject` type map. No
  `ElementMap`, so written sub-shape names will not match upstream's.
- **`FileName` has no setter**, `Document.load` is JSON-only until item 1, and
  `docs/README.md` has no index entry for this file (this task may not edit it).

## Risks

- **The switch is coupled to breadth.** Upstream `.FCStd` writers are not just Part:
  `Document.py` round-trips expressions, `LinkSub`, `PythonObject` and proxies. Switching
  before those are covered regresses the default group, not only `--part`.
- **BREP compatibility.** ASCII BREP should be readable by upstream, but `ElementMap` and
  member naming are unverified; open a FerroCAD-written file in real FreeCAD first.
- **Colour packing.** The `DiffuseColor` format is inferred from one 8-byte member; confirm
  against upstream `PropertyColorList::SaveDocFile` before landing.
