# Task — split document persistence into two serializers

Status: task (2026-10-23; the parallel/independent framing made explicit 2026-10-24). Records
the owner decision of 2026-10-23: native JSON and upstream `.FCStd` become **two explicit,
independently testable serializers**. F1–F8 landed 2026-10-23: `.FCStd` is now the real ZIP
container on both read and write. Companion
to [`../architecture/part-draft-layering.md`](../architecture/part-draft-layering.md), the
[conformance runbook](../workbenches/conformance.md) and
[`../direction/mvp-path.md`](../direction/mvp-path.md) (§8 decision 3, which this closes).

**Legend.** **(verified)** = read from the tree on 2026-10-23; **(recommended)** = not implemented.

## 0. Parallel, independent serializers (2026-10-24)

The two serializers are **parallel and independent**. The native format is ours and may evolve;
`.FCStd` is the conformance-tracked path. Concretely:

- Neither encoder is built by transforming the other's output, and neither reader falls back to
  the other format. The suffix selects the backend; content sniffing is gone.
- A change to the native envelope is a native migration, not a conformance event. `.FCStd` output
  is judged only by the upstream probes (§4).
- Native persistence must therefore **not** be defined in terms of `.FCStd`; otherwise our own
  format changes would move the conformance target.

Lineage persistence follows from that split ([`../architecture/lineage-and-naming.md`](../architecture/lineage-and-naming.md) §0, §6):

- In `.FCStd`, lineage must remain **upstream-compatible**. `PartShapeN.brp` stays raw BREP
  (`ShapeProperty::fcstd_members` → `brep()`); the native `FCSHPROP` envelope is never placed
  inside it. A real FreeCAD must still open the shape.
- Lineage metadata is **additive**: a sidecar member, or an upstream-compatible `ElementMap`
  element, rides alongside the raw BREP, and a reader that does not know it still loads the shape.
  S6b implements the sidecar (`PartShapeN.lineage`, referenced by a `FerroCADSidecar` attribute);
  an upstream-compatible `ElementMap` element remains a follow-up.
- In the native format the shape and its lineage share one versioned envelope, so there is no
  "shape without metadata" state to migrate around.

## Decision

- The **native JSON format** (extension **`.ferrocad`**, recommended) is FerroCAD's save
  format and may evolve freely. `.fe2o3` (the iron-oxide spelling the owner also
  considered) and `.fe203` are accepted aliases for both read and write; `.ferrocad`
  remains canonical.
- **Upstream `.FCStd`** is an import/export format, conformance-tracked: upstream tests
  write and reopen it; **(verified)** `ColorPerFaceTest.py`, `Document.py`.
- Both paths are selected **explicitly**, never by sniffing content. The `.FCStd` writer emits
  the real ZIP container (F8); it is not a JSON placeholder.

## 1. Extension and format-detection policy

### 1.1 Why `.ferrocad`

`.fe203` was the alternative considered (also spelled `.fe2o3`). `.ferrocad` names the
product, matches the `ferrocad` crate namespace, and does not read as a version number;
`.fe2o3` is the iron-oxide pun and is now accepted as an alias. Matching is
case-insensitive; canonical spelling is lowercase `.ferrocad`.

### 1.2 Behaviour by suffix

| Suffix | Read | Write | Notes |
| --- | --- | --- | --- |
| `.ferrocad` | native JSON only | native JSON | default save format |
| `.fe2o3` / `.fe203` (any case) | native JSON only | native JSON | accepted aliases; `.ferrocad` is canonical |
| `.FCStd` (any case) | ZIP + `Document.xml`; transitional JSON fallback (§1.3) | real ZIP + `Document.xml` + `GuiDocument.xml` + `PartShapeN.brp` | conformance format; upstream-compatible |
| none / unknown | `ValueError` naming `.ferrocad`/`.fe2o3` and `.FCStd` | `ValueError` | no silent default; the GUI dialog supplies the suffix |

### 1.3 The transitional rule (retired at F8)

During F1–F7 `saveAs` wrote JSON whatever the path said and `openDocument` compensated
with a content check; that phase is over. Since F8 the `.FCStd` write arm emits the real
ZIP container and `has_zip_magic` is gone. The `.FCStd` **read** arm still accepts a
marker-gated native file for documents saved during the transition (read-only); it can be
deleted after one release. `.ferrocad`/`.fe2o3` never fall back to FCStd, and an unknown
suffix is never guessed.

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

`fcstd.rs` parses the ZIP central directory itself (stored/deflate, no ZIP64, no
encryption) and reads `Document.xml` into `SavedDocument`. Since 2026-10-23 (F4) it also
reads `PropertyLength`/`PropertyAngle`, `PropertyEnumeration`, `PropertyLinkSub`,
`PropertyLinkList`, `PropertyLinkSubList` (flattened to `LinkList`), `PropertyPythonObject`
(into `python_state`) and the expression engine, and translates upstream's property
`status` bitmask. Since F8a it also restores module payloads: `<Part file>` and
`<ShapeList>` members go back through `property_types::restore_extension`, and
`GuiDocument.xml` restores `Visibility`, `Transparency`, colours and `DisplayMode`. Other
module-owned property types, appearance/material and `ElementMap` are still skipped.

### 3.2 Writer breakdown **(recommended)**

**Landed 2026-10-23 (F5).** `ferrocad_core::document::fcstd_write` provides
`write_fcstd(saved, path)` (and an in-memory `encode_fcstd`): a minimal ZIP writer with a
local CRC-32, `Document.xml`, and `PartShapeN.brp` members. F8 wired `saveAs` to it, so the
JSON placeholder is gone. The mapping below is the target; what the writer emits today is
the subset described in §3.2.1.

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

**`.brp` payloads.** `ShapeProperty::fcstd_members()` returns raw ASCII BREP for the
`PartShapeN.brp` member; `save()` is the **native** envelope and is not what `.FCStd` writes
(§0). Write the raw bytes verbatim to `PartShape.brp`, `PartShape1.brp`, … with
`<Part file="PartShapeN.brp"/>`. For `Part::PropertyTopoShapeList`, upstream uses one member
per entry (`<TopoShape file="PartShapeN.M.brp"/>`); `ExtensionData::fcstd_members` already
returns the per-entry list, so core never parses the list framing.

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

### 3.2.1 What the writer emits today **(verified)**

`Document.xml` with scalars, quantities (Length/Angle by unit signature), vectors,
placements, links, link lists/subs, string lists, enumerations (with a `CustomEnumList`
when the values are known), expressions and `ObjectDeps`, property `status` bits (static
transients as valueless `_Property` placeholders), object extensions, and both
`Part::PropertyPartShape` and `Part::PropertyTopoShapeList` (one `PartShapeN.brp` per
entry via `ExtensionData::fcstd_members`). `GuiDocument.xml` carries one view provider per
object with `Visibility`, `Transparency`, `ShapeColor`, `DiffuseColor` (plus the packed
`DiffuseColorN` members) and `DisplayMode`.

It **errors** on `Property::FileIncluded` and `Property::PythonObject` (naming the
object, property and type). `python_state` is FerroCAD's own JSON envelope, not an
upstream pickle: it is recorded as dropped in the `WriteReport` rather than written as
`<Python value>`. Thumbnails and `ElementMap` remain follow-ups.

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

- [x] **1. Dispatch split.** `DocumentFormat`, suffix parsing, native/FCStd dispatch in
  `openDocument`, `load`, `saveAs`; `has_zip_magic` is deleted; the named JSON
  placeholder keeps `.FCStd` writes working. Tests cover every suffix and the
  refusals. **Landed 2026-10-23.**
- [x] **2. Native envelope.** `format`/`version` with serde defaults; bare files still
  load. **Landed 2026-10-23.**
- [x] **3. Migrate our artefacts** (§4), including regenerating the two user pages.
  **Landed 2026-10-23.**
- [x] **4. Reader gaps needed by the writer:** `PropertyLength`, `PropertyAngle`,
  `PropertyEnumeration`, `PropertyLinkSub`, `PropertyLinkList`/`LinkSubList`, the
  expression engine and `App::PropertyPythonObject`. Verified with hand-written XML
  units and the upstream fixtures. **Landed 2026-10-23.**
- [x] **5. FCStd ZIP writer behind a direct-call API:** `write_fcstd`/`encode_fcstd`
  with `Document.xml` + `.brp` for `Part::PropertyPartShape`; not wired to `saveAs`.
  **Landed 2026-10-23.**
- [x] **6. FCStd writer breadth:** shape lists, expressions, `ObjectDeps`, statuses,
  extensions, and a dropped-value report for `python_state`/`Proxy`. **Landed
  2026-10-23.**
- [x] **7. `GuiDocument.xml`:** `Visibility`, `Transparency`, `ShapeColor`,
  `DiffuseColor`, `DisplayMode`, with the packed `DiffuseColorN` members. **Landed
  2026-10-23.**
- [x] **8. Switch.** `saveAs(.FCStd)` emits ZIP; the placeholder is gone; the magic and
  the archive members are asserted. **Landed 2026-10-23.**
- [x] **9. Lineage sidecar (S6b).** `ExtensionData::fcstd_sidecar` writes
  `PartShapeN.lineage` beside the raw `.brp`, referenced by `FerroCADSidecar`; the reader
  prefers it and falls back to the raw member. **Landed 2026-10-24.** Still open: the
  thumbnail, an upstream-compatible `ElementMap` element, and an upstream-FreeCAD open
  check.

## What is not done (2026-10-23)

- **Module-payload read is partial.** `.brp` and shape-list members restore, but other
  module-owned types, shape appearance/material and `ElementMap` are skipped, and the
  reader ignores module-specific payloads it does not recognise. `ElementMap` is deliberately
  not written yet; when it lands it stays additive beside the raw `.brp` (§0).
- **A few writer values remain lossy.** `Property::FileIncluded` and
  `Property::PythonObject` error by name; `python_state` is reported as dropped, not
  written as an upstream pickle; `ElementMap` and thumbnails are follow-ups.
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
