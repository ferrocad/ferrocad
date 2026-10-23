# Links: `App::Link`, link arrays, and their interlocks

Status: task (2026-10-23). L0–L3 landed 2026-10-23; L4+ remain. Records the owner
decision of 2026-10-23 that the remaining
`Part` gaps are **TNP / Links / FCStd**, and that they are one problem seen from three
sides. Companion to [`../architecture/lineage-and-naming.md`](../architecture/lineage-and-naming.md)
(the lineage layer this doc depends on) and [`format-serializers.md`](format-serializers.md)
(the two encoders). Backlog: the `Part` kernel entry in
[`../TODO-23-10-2026.md`](../TODO-23-10-2026.md) §P1; conformance method in
[`../workbenches/conformance.md`](../workbenches/conformance.md).

**Legend.** **(verified)** = read from this tree on 2026-10-23, upstream `33adf61`.
**(proposed)** = not implemented. **(unverified)** = upstream source is outside the
sparse checkout, so the contract is taken from a test, a companion note or usage.

A link is three things at once: a **reference** (an object plus optional sub-element
names), a **transform** (placement, scale, whether to copy), and a **generated element
set** (an array can expand into child objects). FerroCAD has the reference, half of the
element set, and none of the transform.

---

## 1. What "Links" means here

| Surface | Upstream contract | FerroCAD today (**verified**) |
| --- | --- | --- |
| `App::Link` | A document object that references one object, with `Placement`/`LinkPlacement`, `Scale`/`ScaleVector`, `LinkTransform`, `LinkMode`, `LinkCopyOnChange`, and an element list ([`occt-history-spike.md`](../architecture/occt-history-spike.md) §6) | **L0–L2.** Full property base plus a Part-side `ObjectType` whose `execute` transforms the target's `Shape`. Element expansion, `LinkMode` and copy-on-change are stored but inert. |
| `LinkedObject` | `App::PropertyLink` (or a link-sub) to one object | **Implemented** as `Property::Link`; attribute get/set resolves the target (`lib.rs:2651-2663`, `2745`). |
| `setLink(obj, subname=None)` | Sets the link and narrows it to sub-elements | **Implemented.** Writes `LinkedObject` and, with `subname`, a `Property::LinkSub`. The shared link table accepts `App::Link*` and `Part::LinkArray*`. |
| `getLinkedObject(resolve=…)` | Returns the true target, following link chains | **Implemented** (L0): `resolve=False` returns the immediate target, `resolve=True` follows chains with a cycle guard. |
| `isLink()` | True for `App::Link` and every subclass | **Implemented** (L0), using the same table that also treats `Part::LinkArray*` as links. |
| `getSubObject(subname, …)` | Walks `Obj.Sub.Sub` paths, links and element lists; `retType` selects object/matrix/placement | **Implemented** (L0): dotted paths, `ElementList` children, `Group` members and `LinkedObject` hops. `retType` 0/1 → object, 2 → `(obj, Matrix, None)`, 3 → Placement, 4 → Matrix. |
| `getElementIndex` | Splits `Array.Element3` and reports the index | **Implemented** (L0): `Element3`, `Array.Element3`, `Array_Element3` and `3.` → 3; else -1. |
| `Placement`, `LinkPlacement`, `Scale`, `ScaleVector`, `LinkTransform`, `LinkMode` | Transform semantics; a link's placement composes with the target's | **L2.** The link's `Placement` (or `LinkPlacement` when `LinkTransform` is true) and a uniform scale are applied. Non-uniform `ScaleVector` is deferred. |
| `LinkCopyOnChange` | An enum selecting copy-on-change, with a shadow `LinkCopyOnChangeGroup` (values **unverified**) | **Missing**. No clone/copy machinery exists to make it meaningful. |
| `App::LinkElement` | The generated child of an expanded link | **Implemented** (L3): `App::LinkElement` carries a placement and a composed `Shape`, participates in `ElementList`, and `Part::LinkArrayElement` stays registered as an alias for old files. |
| `App::LinkGroup`, `App::LinkList` | Link containers (a link to many objects, with element expansion) | **Missing** as object types. The *property* types `PropertyLinkList` exist. |
| `Part::LinkArray{Point,Linear,Path,Circular}` | `App::Link` subclasses whose `execute` fills `PlacementList`/`ElementList` (`LinkArray.h`: `public App::Link`) | **Implemented as peers, not subclasses** (`link_array.rs:1-12`). Placements, element suppression and save/restore mostly work; the shared element sync now creates real `App::LinkElement` children. |
| `Part::LinkArrayPolar`, `Part::Part2DObject` | Supporting types the circular tests construct | **Implemented minimally** as test fixtures (`link_array.rs:1425-1475`, `1568-1584`). |
| `ShowElement`, `ElementCount`, `PlacementList`, `ElementList` | Link output properties | **L3.** `ShowElement=true` creates one `App::LinkElement` per placement (with a shape and placement) and lists it in `ElementList`; collapsing removes the children and writes `PlacementList` plus the whole shape. |
| `ScaleList`, `_LinkTouched`, the immutable/output statuses | `LinkArray::enforceLinkArrayPropertyStatus` | **Missing.** `ElementList`/`PlacementList` carry `HIDDEN\|OUTPUT\|NORECOMPUTE\|READONLY` (`link_array.rs:42-43, 183-213`); there is no immutable flag and no `ScaleList`. |
| `OutList` | Every object referenced by any link property | **Implemented, but wrong for links**: it returns `Group` members only (`lib.rs:2515-2519` → `group_members`). `InList` *is* link-aware (`lib.rs:2521-2558`). |

**Upstream not verifiable here.** `src/App/Link.h`/`Link.cpp` are not in the sparse
checkout. The list comes from the spike §6 and from use in the checkout
(`Draft/.../draftlink.py:126-136`, `BIM/.../TestArchAxisGui.py:36`,
`Part/.../TestPartMirror.py:119`); `LinkMode`'s values and metadata are **(unverified)**.

---

## 2. The current model (**verified**)

Links are **names plus graph edges**, not objects with behaviour.

- **Storage.** `Property::Link(String)`, `Property::LinkList(Vec<String>)`,
  `Property::LinkSub(String, Vec<String>)` (`ferrocad_core/src/property.rs:29-34`). The
  comments call this a "POC simplification"; the names are raw document-object names.
- **Ordering.** `Document::dependency_edges` turns every `Link`/`LinkList`/`LinkSub`
  value into an edge (`document/mod.rs:361-400`), skipping the `Group` property and
  adding expression references; `recompute_order` errors on a cycle (`:441-455`). So
  `LinkedObject` already orders correctly, and `ElementList` makes the array depend on
  its elements.
- **Python.** `__getattr__` resolves a link name to an object, or `None` when the name
  is missing (`lib.rs:2651-2690`); `__setattr__` coerces by the existing property's
  variant (`lib.rs:2744-2750`) through `py_to_link`/`py_to_link_list`/`py_to_link_sub`
  (`lib.rs:956-1044`). `LinkSub` round-trips as `(object_or_None, [names])`.
- **View.** `VIEW_PROPERTIES` (`Visibility`, `Transparency`, `ShapeColor`,
  `ShapeAppearance`, `ShapeMaterial`, `DiffuseColor`, `DisplayMode`) are forwarded to a
  per-object `ViewObject` for *every* object (`lib.rs:2692-2704`, `2871-2880`,
  `4123-4131`). A link has colour and visibility but nothing to draw;
  `part_view.rs:130-152` asserts exactly that.
- **Output shape.** Only `Part::LinkArray*` produce one, and only when
  `ShowElement=false`: `finish_array` transforms the `LinkedObject`'s shape by every
  element placement and compounds the results (`link_array.rs:315-345`). `App::Link`
  has no shape at all. The arrays' own `Placement` is **not** applied to that compound;
  `Part::Box` bakes its `Placement` into `Shape` (`ferrocad_part/src/lib.rs:362-366`),
  so the display convention is "shape is world", and moving an array does nothing.
- **Elements.** `ElementList` is a `LinkList` of ordinary document objects named
  `<Array>_Element<N>` (`link_array.rs:257-280`). They carry `Placement`, `Suppressed`
  and an unused `LinkedObject`; they have no `Shape` and no execute.

So registration proves nothing: `App::Link`'s type entry exists but has no execute path,
while the arrays are real `ObjectType`s with their own execute.

---

## 3. The interlock with lineage

A link stores `("Box", ["Face3"])`; the document keeps `"Face3"` as an opaque string
and the kernel resolves it **positionally** today: `ferrocad_occt/src/backend.rs:1659-1672`
says so in a comment — `FaceN`/`EdgeN` is the N-th sub-shape, and stable *named*
resolution "needs the lineage mapper (a later slice)".

The link-array references go through exactly that call: `Path`, `Axis`, `BinormalAxis`,
`Direction` are `LinkSub`s (`link_array.rs:1023-1061`, `1425-1433`), read by `link_sub`
(`:120-128`) and resolved by `axis_from_reference` → `edge_axis` → `backend.resolve`
(`:1166-1225`). The same holds for `Part::Fillet`/`Chamfer` (`ferrocad_part/src/dress_up.rs:202`).
The upstream array tests never reorder a source, so the positional fallback is not
exercised.

After a rebuild, `Face3`/`Edge1` may denote a different element, and nothing detects it.
A link reference therefore has **persistence identity but no rebuild identity**. The
lineage layer supplies the second: `Lineage::resolve` maps the stored name through the
shape's element map, and a retired name must fail loudly rather than retarget
([`lineage-and-naming.md`](../architecture/lineage-and-naming.md) §3.3, §5).

The dependency split matters for sequencing:

- **Whole-object links** (`LinkedObject = Box`, `LinkList = [Box, Cyl]`) need only the
  document's name graph. They can be finished without lineage.
- **Sub-element links** (`LinkSub`, a link subname, `ElementList[i].Face3`) need the
  map; this is the part of the link API that cannot be finished independently. The
  reverse direction — selection pick → stored name (`mapSubElement`) — is also
  lineage's; links only consume what it produces.
- **Element identity is ordinal too.** Elements are named by index
  (`<Array>_Element<N>`, `link_array.rs:268`), so `Array.Element3` survives a resize only
  while index 3 is the same instance — the same problem one level up.

---

## 4. The interlock with formats

Native JSON and `.FCStd` encode the same reference vocabulary differently; neither is
complete today.

| Surface | Native JSON (**verified**) | `.FCStd` read (**verified**) |
| --- | --- | --- |
| `PropertyLink` | Serde's variant tag plus the name; persisted by `SavedObject::from_object` unless transient/no-persist (`document/mod.rs:1008-1043`) | Read as `<Link value="Name"/>` (`fcstd.rs:390-392`) |
| `PropertyLinkList` | Variant plus a name list | **Dropped** — not in `fcstd.rs:14-17` and no parse arm |
| `PropertyLinkSub` | `LinkSub(name, subs_list)` | **Dropped**; the fixture writes `<LinkSub value="Sketch001" count="1"><Sub value="N_Axis"/></LinkSub>` |
| `PropertyLinkSubList` | **No variant**: `property_types.rs:148` maps it to `Property::LinkList`, so per-item subnames are lost at `addProperty` time | **Dropped**; fixture form is `<LinkSubList count="N"><Link obj="Sketch" sub="Vertex1"/></LinkSubList>` |
| `PropertyPlacementList` / `ElementList` (arrays) | Persisted as a `PlacementList` and a `LinkList` of names; element objects are ordinary saved objects, so their placements survive | **Dropped** (`PropertyPlacementList` and `PropertyLinkList` are not parsed), so a reopened upstream array has neither elements nor placements |

Three link-specific losses follow:

1. **Flattened sub-lists.** `PropertyLinkSubList` cannot represent Sketcher `Support`,
   `ExternalGeometry`, or a binder with two faces.
2. **No version on a reference.** `LinkSub` records neither the map version it was
   written against nor whether it resolved; see lineage §5.
3. **Name-only restore.** `Document::from_saved` re-adds objects by saved name and
   overlays properties (`document/mod.rs:878-905`); a dangling name resolves to `None`
   instead of failing the load — silent loss for a replaced source.

The format task already isolates both encoders; links add two requirements to it: the
link list/sub reader is checklist item 4, and `ElementMap` (a reference's other half) is
item 9. The writer's property elements are in
[`format-serializers.md`](format-serializers.md) §3.2; our fixtures confirm `<Link>`,
`<LinkList count>`, `<LinkSub value count><Sub>` and `<LinkSubList><Link obj sub>`. How
upstream persists `App::Link`'s element list itself is **(unverified)** — no `App::Link`
fixture is in the checkout.

---

## 5. Plan in slices

Ordering principle: make identity and introspection real before adding behaviour, and do
not fake sub-element resolution before the map exists.

| Slice | Goal | Work | Breaking? | Depends on |
| --- | --- | --- | --- | --- |
| **L0** | Recognise links without guessing from a prefix | Add a base-type table (`App::DocumentObject` → `App::Link` → `Part::LinkArray*`) and use it in `setLink` instead of `starts_with`; add `isLink()`, `getLinkedObject(resolve)`, `getElementIndex()`; make `getSubObject` split on `.` and walk whole-object links | No (new methods; `setLink` starts working on arrays) | — |
| **L1** | One link property base | Move the shared property set (`Placement`, `LinkedObject`, `ShowElement`, `ElementList`, `ElementCount`, `PlacementList`, `Scale`/`ScaleVector`, `LinkTransform`, `LinkMode`, `LinkCopyOnChange`) into a base `App::Link` `ObjectType`; let `add_object_with` fold base defaults before derived ones (derived wins); delete the duplicate `array_properties` | Files gain properties; old files load | L0 |
| **L2** | `App::Link` has output | Register the execute where a kernel exists (`ferrocad_part_py::register_kernel` is the composition root, `ferrocad_part_py/src/lib.rs:55-58`): transform the target's `Shape` by `LinkPlacement`/`Placement`, apply scale, compound when applicable; apply the array's own `Placement` in `finish_array`; return a real `Shape`. Fix `OutList` to cover all link properties | Additive for `App::Link`; the array placement fix changes geometry | L1 |
| **L3** | Element semantics | `App::LinkElement` (or make `Part::LinkArrayElement` a registered alias), with a link back to the source, a shape (or a resolved `getSubObject`), and non-ordinal identity through lineage once available; make `ElementList[i]` addressable from `getSubObject("Array.Element3")`; keep `ShowElement` collapse reversible | Type id change for new elements; keep the old type registered so saved files load | L1, L2 |
| **L4** | `LinkCopyOnChange` and containers | `App::LinkGroup`/`App::LinkList`; `LinkMode`; copy-on-change as a shadow object (needs an object clone/copy facility plus its dependencies). This is where "copy the source, not the shape" lands | Yes (new object types and saved-property semantics) | L2 |
| **L5** | Sub-element resolution | Resolve `LinkSub`, link subnames and element sub-paths through `ferrocad_lineage` first, positional fallback only when no map exists; invalidate rather than retarget when the name is retired; record the map version on the reference | Behavioural for stale refs | **`ferrocad_lineage` S1–S5** |

L0–L2 are the non-breaking envelope: new methods, properties and an execute path without
changing stored references. **L0–L2 landed 2026-10-23**; **L3 landed the same day**: the
link and the arrays now generate real `App::LinkElement` children and collapse them
reversibly. L4 is the first slice that needs a copy model. Only L5 is blocked on the
lineage crate; sub-elements still resolve positionally until S5.

**Where `App::Link` lives is a decision, not a detail.** The defaults are in
`ferrocad_core`, which cannot construct a shape; the execute needs the kernel. Putting it
in `ferrocad_part_py` (where the arrays already register) means shape semantics only in a
Part-enabled build. Splitting a geometry-free `App::Link` from a Part-side
`LinkShapeExtension` is more faithful and more work.

---

## 6. Tests

- **Link arrays (present).** `TestLinkArray{Point,Linear,Path,Circular}.py` are in the
  `--part` group (`tools/conformance.py:66-74`): **19 of 27 reported cases pass**.
  The remaining eight are one property-change hook (five cases), the missing execute
  error text in `getStatusString`, recompute on restore, and a one-ulp quaternion
  difference; the TODO entry records the eight and their causes. Rust coverage is
  `crates/ferrocad_part_py/tests/part_link_array.rs` (3 tests).
- **`App::Link` shape/placement (new).** Mirror `TestPartMirror.py:119` without
  `PartDesign`: link a `Part::Box`, set `Placement` and `LinkPlacement`, assert the
  bound box moves; link a link; set `LinkTransform`/`Scale` and assert composition.
  Today `link.Placement` raises, which is the failing assertion to write first.
- **`.FCStd` and native round-trip (new).** `.FCStd` depends on format checklist items 4
  and 5: read `PropertyLinkList`/`LinkSub`/`LinkSubList`/`PlacementList`, then the array
  files' `saveAs`→`openDocument` cases become meaningful in the upstream format. Native
  already round-trips `Link`/`LinkSub` (`tests/test_base_surface.py::TestPersistence`,
  `:231-256`); add a dangling/renamed target and an array's
  `ElementList`/`PlacementList`.
- **Link + rebuilt source (new).** The regression that forces lineage: set
  `Fillet.Base = (Box, ["Edge1"])` or `LinkArray.Path = (Box, ["Edge1"])`, rebuild the
  source so the edge's ordinal moves, and assert the reference either follows the same
  logical edge or marks the feature invalid — never silently resolves to another edge.
  The inverse case is the same test with a retired name.

---

## 7. Open questions and non-goals

- **GUI display modes.** `DisplayMode`, `OverrideMaterial`, per-element colour and
  `PartGui::ViewProviderLinkArray` are out of scope; only `Shape` must exist.
- **External linking.** Links into another document (`LinkScope::Global`) are out of
  scope, including `Part::LinkArrayPoint`'s upstream global `PointObject` scope.
- **`LinkCopyOnChange` semantics.** Which copy-on-change behaviour comes first, whether
  the shadow group must be a real `App::LinkGroup`, and whether it copies properties or
  shape are open.
- **Type identity.** `isLink()`/`isDerivedFrom()` need a base table; whether to grow it
  into a `Base::Type` registry (needed by `Draft`/`BIM` detection) is a wider decision.
- **`LinkedObject` type.** `setLink(..., subname)` replaces the `Property::Link` value
  with a `Property::LinkSub`, so `getTypeIdOfProperty("LinkedObject")` flips. Upstream's
  storage is **(unverified)**.
- **Self/cycle / non-goals.** `setLink` permits a self-link and the dependency graph
  silently ignores it (`dependency_edges` skips `from == obj.id`); upstream rejects the
  assignment. No `Draft Clone`, linker UI, or link-aware selection translation — that
  is the reverse map, owned by lineage.

---

## What is not done (2026-10-23)

- **Copy semantics.** `LinkCopyOnChange`, `LinkMode` and `App::LinkGroup`/`LinkList` are
  inert; non-uniform `ScaleVector` is deferred.
- **No `LinkSubList` value.** It is flattened to `LinkList` at property creation; the
  `.FCStd` reader now reads link lists/subs but flattens `LinkSubList` the same way.
- **No lineage-backed resolution.** Every `FaceN`/`EdgeN` is still an ordinal.
- **Not indexed.** `docs/README.md` has no entry for this file; this task may not edit it.

Upstream revision: `33adf618dcebfb167795a219175e7dbb18f7a353`.
