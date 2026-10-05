# FerroCAD — MVP path

Status: research/planning (2026-10-02). Companion to
[`rewrite-strategy.md`](rewrite-strategy.md) (direction) and
[`milestones.md`](milestones.md) (record). This note defines the minimum viable
product, the smallest model/workflow/persistence it needs, and the slices that get
there.

---

## 1. MVP definition

> **FerroCAD MVP = a headless, geometry-free parametric document engine, exposed as a
> drop-in `FreeCAD` package.** An unmodified Python script that uses only the public
> `FreeCAD.App` / `FreeCAD.Base` API can: create a document; add typed objects and
> properties (including dynamic, enum, link, quantity, placement); wire dependencies
> with expressions; recompute deterministically; group/contain objects; observe
> changes; undo/redo; and persist the whole document to a file and load it back.

**Why this is the right MVP.** It is the largest coherent slice that needs **no geometry
kernel and no GUI** — the two things that dominate the remaining work — yet it is what
every workbench is *built on*. Data, parameters, dependencies, containment, undo and
save/load are the contract between FreeCAD and its Python workbenches; geometry is
something workbenches generate *on top of* that contract.

**Non-goals (explicitly out of MVP):**

- the geometry kernel (`Part::*`, `PartDesign::*` shapes) — no OCCT;
- any GUI: Coin3D, Qt, `bite-gpui` (that is the separate M5 UI track, and the
  MVP app shell planned in [`app-shell-vision.md`](app-shell-vision.md));
- upstream's zip `.FCStd` container (MVP persists FerroCAD's own JSON under a `.FCStd`
  name; adopting the zip is a later packaging decision);
- the C++ `App::FeatureTest` fixture classes in `Document.py` (§7);
- network/Addon-manager, macros UI, `FreeCADGui` beyond the console stub.

---

## 2. The MVP workflow (definition of done)

One Python file, run with `./run.sh`, must succeed end-to-end:

```python
import FreeCAD as App

doc = App.newDocument("MVP")

# --- model: typed + dynamic + enum properties -----------------------------
params = doc.addObject("App::FeaturePython", "Params")
params.addProperty("App::PropertyLength", "Length", "Base", "Source length")
params.addProperty("App::PropertyEnumeration", "Mode")
params.Mode = ["Fast", "Accurate"]        # enumeration values
params.Mode = "Accurate"
params.Length = "10 mm"

derived = doc.addObject("App::FeaturePython", "Derived")
derived.addProperty("App::PropertyLength", "Result", "Base", "Computed")
derived.setExpression("Result", "Params.Length * 2")   # dependency edge
doc.recompute()
assert str(derived.Result) == "20 mm"
assert derived.State == ["Up-to-date"]

# --- workflow: containment, links, undo -----------------------------------
group = doc.addObject("App::DocumentObjectGroup", "Group")
group.addObject(derived)
assert derived in group.Group

doc.openTransaction("bump length")
params.Length = "15 mm"
doc.commitTransaction()
doc.recompute()
assert str(derived.Result) == "30 mm"
doc.undo()
doc.recompute()
assert str(derived.Result) == "20 mm"

# --- persistence: save + reload -------------------------------------------
doc.saveAs(path)
App.closeDocument("MVP")
doc2 = App.open(path)
assert str(doc2.getObject("Derived").Result) == "20 mm"
assert doc2.getObject("Params").Mode == "Accurate"
```

This exercises every axis below and is the acceptance demo for the MVP
(`examples/mvp_workflow.py`).

---

## 3. The minimum: model, workflow, persistence

### 3.1 Model — the object/property core

| Capability | Today | Needed for MVP |
| --- | --- | --- |
| `Document`/`DocumentObject`, naming, `PropertiesList`, dynamic props | ✅ | — |
| Property types: `Float`/`Integer`/`Bool`/`String`/`Length`/`Quantity`/`Placement`/`Rotation`/`Vector`/`Color`/`Link`/`LinkSub`/`LinkList`/`PythonObject`/`FileIncluded`/`Map`/`IntPairList` | ✅ most | round-trip completeness |
| `PropertyEnumeration` | ❌ | **new** |
| Property **status flags** (`Prop_None`/`ReadOnly`/`Transient`/`Hidden`/`NoRecompute`) | partial | `getPropertyStatus`/`setPropertyStatus` |
| `DocumentObject.State` (`Up-to-date`/`Touched`/`Invalid`/`Recompute`) + propagation | minimal (`Touched`/`Up-to-date` only) | **extend** |
| Type checking on assignment (typed errors) | partial | `testWrongTypes` |
| Dependency graph + recompute + observers | ✅ | recompute polish |
| Expressions (`setExpression`, `Test.Property` paths) | partial | **nested paths**, recompute counts |

### 3.2 Workflow — the user operations

| Capability | Today | Needed for MVP |
| --- | --- | --- |
| Create/remove objects, unique `Name` | ✅ | — |
| **`Label`** semantics: unique allocation, sanitization, `Label`≠`Name`, duplicate modes | ❌ | **new** |
| Groups / `App::Part` containers, `addObject`, `getParentGeoFeatureGroup`, `OutList`/`InList` | ✅ core | container chains, link rewriting |
| Extensions (`addExtension`, dynamic extension round-trip) | partial | persistence/state |
| Transactions (`open`/`commit`/`abort`) + `undo`/`redo` | ✅ nominal | **real undo stack** |
| `UndoMode`, `clearUndos`, `getAvailableUndos` | ❌ | **new** |
| Recompute ordering, `MustExecute`, `touch` | ✅ | invalid propagation |

### 3.3 Persistence — the durable state

| Capability | Today | Needed for MVP |
| --- | --- | --- |
| `saveAs`/`save`/`open` (JSON), document registry | ✅ | — |
| Round-trip of **all** property types (incl. `Enum`, `Link`, `Map`) | partial | **complete** |
| Python object / `Proxy` (`dumps`/`loads`) | ✅ | — |
| `Document.dumpContent`/`restoreContent` + `DocumentObject.Content` (XML) | partial | **Content**, `validateXml` |
| `FileIncluded` apply | partial | `testApplyFiles` |
| Recovery snapshot | ✅ | — |
| Undo/redo **across** save | ❌ | transaction persistence |

---

## 4. Acceptance criteria (measurable)

1. **`Document.py` real gaps → 0.** Upstream's own `src/Mod/Test/Document.py` passes
   **except** the C++ `App::FeatureTest*` classes (~32 tests, §7). That takes
   `Document.py` from **87/137** to **~105/137**, and total conformance from
   **160 → ~176**.
2. **No regressions.** `BaseTests`, `UnitTests`, `StringHasher`, `UnicodeTests`,
   `TestIntPairList`, and all currently-passing `Document*` cases stay green.
3. **The MVP workflow script passes** (`examples/mvp_workflow.py`), wired into CI.
4. ~~**The `ctypes` fallback runs the same MVP script** (backend-agnostic parity).~~
   Dropped: the fallback is a second, narrower model, not a transport for the
   same core, and is slated for removal (see [`architecture.md`](architecture.md) §4-5).
5. *(Stretch)* `TestIntPairList` fully green (`Touched` state) and the `FreeCADInitTests`
   package-init contract passes.

---

## 5. Baseline (what the gap actually is)

`tools/conformance.py --root ../freecad-upstream`:

```
Document.py       137 tests  (pass 87, fail 47, error  3)
BaseTests.py       49 tests  (pass 48, fail  1, error  0)   # only the C++ fixture
UnitTests.py       12/12     StringHasher 4/4     UnicodeTests 1/2
TestIntPairList     8/8
FreeCADInitTests    3 tests  (pass  0, fail  0, error  3)
--------------------------------------------------------------
total: 160 passed, 48 failed, 7 errored
```

Of the **50** `Document.py` failures, **32 are the C++ `App::FeatureTest` fixture**;
the remaining **~18 are real product gaps**, clustered in §6. Everything else the harness
runs except `FreeCADInitTests` already passes.

---

## 6. Slice plan

Ordered by dependency and value. Sizes are rough (test yield in parentheses).

### Track A — model

- **A1 · Object state & property status. ✅ DONE.** Full `DocumentObject.State`
  (`Up-to-date`/`Touched`/`Invalid`), property status flags (`addProperty` attr +
  `getPropertyStatus`/`setPropertyStatus`/`getTypeOfProperty`), touch-on-assign
  (`Prop_Output`/`Prop_NoRecompute` suppress it), and `Prop_NoPersist` dropped on save.
  *(Landed 3: `TestIntPairList.test_changes_touch_the_object`,
  `Document.testProp_NonePropertyLink`, `Document.testAttributeOfDynamicProperty`.)*
  `testWrongTypes` (type-registry validation) moved to A2.
- **A2 · `PropertyEnumeration` + assignment type-checking. ✅ DONE.** `App::PropertyEnumeration`
  (set-from-list, select by index/value, `enum_vals`, validation) and type-registry validation
  (`addObject` rejects extension types, `addProperty` requires a property type, `findObjects(Type=)`
  validates). *(Landed: `Document.testEnum`, `Document.testWrongTypes`.)*
- **A3 · Link completeness.** `PropertyLink`/`LinkList`/`LinkSub` with `Prop_None`,
  backlinks (`InList`/`OutList` growth), rename/removal rewriting. *(parts of GroupCases;
  ~1–2.)*

### Track B — workflow

- **B1 · Label semantics. ✅ DONE.** Unique name/label allocation, sanitization
  (`My Label` → `My_Label`, control chars → `_`), and the `DuplicateLabels` preference
  (kept verbatim when enabled, unique otherwise), for both `addObject` and `copyObject`.
  *(Landed all 4: `DocumentDuplicateLabelCases` is now 6/6.)*
- **B2 · Undo/redo engine. ✅ DONE.** A general, reversible change set (property
  changes, object add/remove, expression set/remove) with named transactions carrying
  process-unique ids; `UndoNames`/`RedoNames`/`UndoCount`/`RedoCount`, `clearUndos`,
  `UndoMode`, `getBookedTransactionID`/`getAvailableUndos`/`getAvailableRedos`,
  `ActiveObject` following `addObject`, transactional expressions, and a link-type-aware
  `InList` (any `Link`/`LinkList`/`LinkSub` plus expression backlinks).
  *(Landed: `UndoRedoCases` ×4, `MultiDocumentUndo`, `TestIntPairList`, and `testExpression`'s
  undo parts.)*
- **B3 · Containers & links.** `GeoFeatureGroupExtension` / `App::Part` chains,
  `getParentGeoFeatureGroup` across nesting, link rewriting on add/remove.
  *(DocumentGroupCases ×4; ~4.)*
- **B4 · Expressions v2.** Nested-path evaluation (`Object.Placement.Rotation.Angle`),
  `recompute()` object counting, transactional expressions + proxy `onChanged`.
  *(testExpression, testRecompute; ~2.)*
- **B5 · Extensions in the document lifecycle.** Dynamic extension kinds round-trip and
  fire the right events. *(testExtensions; ~1.)*

### Track C — persistence

- **C1 · Full property round-trip.** Save/restore every property type (Enum, Link,
  LinkSub, Map, Quantity, Placement, PythonObject), incl. `SaveRestoreSpecialGroup`
  (group + view-provider proxy). *(DocumentSaveRestoreCases; ~1–2.)*
- **C2 · `Content` + XML validate.** `DocumentObject.Content` XML dump,
  `restoreContent`, `validateXml`, and a small pure-Python `Show` shim
  (`Show.Containers.ContainerChain`) that `Document.py` imports. *(testContent,
  testValidateXml, testContainerChainGroupInPart; ~3.)*
- **C3 · File-included apply.** `testApplyFiles` (undo/redo of file assignments + name
  persistence). *~1.*

### Track D — MVP surface

- **D1 · MVP demo + CI.** `examples/mvp_workflow.py` (the §2 script) and a committed
  test; run it on both backends in CI so it cannot regress.
- **D2 · Remaining `DocumentBasicCases`.** `testAddRemove`, `testObjects`,
  `testNoRecomputeParent`, `testNotification_Issue2902Part2`, `testIssue24571`,
  `testRawAxis`. *(~6; a couple may hinge on property sub-object reference semantics —
  see §8.)*
- **D3 · Stretch: `FreeCADInitTests` package-init shim.** `Logger`, `ReturnType`,
  `PropertyType` (IntEnum), `__cmake__`/`__ModDirs__`/`__ModCache__`/`__MacroDirs__`,
  `__main__` bootstrap leaks, and the unit/quantity constant tables.

**Suggested sequence:** A1 → B1 → A2 → B2 → B3 → C1 → B4 → C2 → D1 → D2 → D3.
A1 and B1 are independent and can run in parallel. A2/B2 unlock the most tests.

---

## 7. Deliberately out of scope: the C++ `App::FeatureTest` fixture

`FeatureTestColumn` (21), `FeatureTestRow` (6), `FeatureTestAbsAddress` (4),
`FeatureTestAttribute` (1) = **32 tests** exercise a C++ *test* object that emulates
spreadsheet cell addressing (`getCellAddress`, `AA13`, …). It is fixture semantics, not
product behaviour, so the MVP excludes it. If full `Document.py` parity is wanted later,
implementing this one object in `ferrocad_core` + `ferrocad_py` is mechanical and would
take `Document.py` to 137/137.

---

## 8. Risks & open decisions

1. **Property sub-object reference semantics.** `obj.Placement.Rotation.Axis = …`
   appears to mutate a copy in upstream (the C++ getters return by value) — we matched
   that. But `Document.testRawAxis` relies on it. Either the test is failing upstream,
   or some property sub-objects are returned **live**. Resolve with a live-reference
   probe before D2; it may change how `PropertyPlacement`/`PropertyRotation` expose
   sub-objects. *(Tracked since slice 15/16.)*
2. **`UndoMode`/undo scope.** Full command-object undo is the MVP's biggest item; scope
   it to the operations the tests exercise (add/remove/property/group/dynamic) rather
   than a generic command framework.
3. **JSON vs zip `.FCStd`.** MVP keeps JSON. If real `.FCStd` interop is a goal, decide
   before C1, because it changes the persistence layer.
4. **`Show` shim.** `Document.py` imports `Show.Containers`; a minimal pure-Python shim
   is cheap but is upstream utility code — keep it clearly marked and minimal.
5. **Expression evaluation depth.** `testExpression` needs nested-path *evaluation*
   (not just parsing). Confirm the reachable object paths (`Placement.Rotation.Angle`).
6. **`FreeCADInitTests` unit tables** are large and mechanical; keep as stretch unless
   the boot contract is a near-term deliverable.

---

## 9. Relation to other docs

- [`rewrite-strategy.md`](rewrite-strategy.md) — overall direction and risk register.
- [`milestones.md`](milestones.md) — the M0→M4 record and per-slice detail.
- [`app-shell-vision.md`](app-shell-vision.md) — the next chunk after this headless MVP: the
  interactive dev-tool window (inspector, property editor, Python console, undo/redo, open/save)
  built on this engine.
- [`python-ui-research.md`](python-ui-research.md) — the M5 UI/Python-host direction
  (separate from this MVP).
