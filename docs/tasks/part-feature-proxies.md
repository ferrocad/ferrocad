# Task — Python feature proxies (`Part::FeaturePython` and friends)

Status: task (2026-10-24). P2 in [`../record/TODO-24-10-2026.md`](../record/TODO-24-10-2026.md).
**P2a–P2e landed 2026-10-24; both Draft M2 modules run green.**
Companion to [`../architecture/part-draft-layering.md`](../architecture/part-draft-layering.md)
§9 (M2), [`../architecture/python-bindings.md`](../architecture/python-bindings.md) and
[`../architecture/property-value-extension.md`](../architecture/property-value-extension.md).

## Why this is the gate

`Part` now works as a library: every upstream Part test module loads, and 529 of 541
runnable cases pass. Draft and SimpleCAD do not consume `Part` that way. They create
document objects whose **behaviour is Python**: `Draft.make_clone`, the array objects,
`Part::Part2DObjectPython`, and every `Draft` tool that defines an `execute` in a script.
Until those proxies exist, Draft stays at M1 (curves are real, objects are not) and
SimpleCAD cannot move past S0.

`ferrocad_core` already has the two halves this needs: `object_registry::ObjectType`
(supplies defaults and `execute`) and `Property::PythonObject` (a persisted opaque
value). What is missing is the bridge that lets a Python object be the behaviour.

## Upstream contract (read these first)

- `../freecad-upstream/src/App/FeaturePython.cpp` / `FeaturePythonPyImp.cpp` —
  `PythonFeature::execute`, `onChanged`, `onDocumentRestored`, and the proxy pickling.
- `../freecad-upstream/src/Mod/Part/App/PartFeaturePython.cpp` —
  `Part::FeaturePython` and `Part::Part2DObjectPython`.
- `../freecad-upstream/src/Mod/Draft/draftmake/make_clone.py` — the first real consumer.
- The failing Draft M2 modules: `drafttests/test_creation.py`, `test_array.py`.

The observable contract that matters:

| Behaviour | Upstream |
| --- | --- |
| Creation | `doc.addObject("Part::FeaturePython", name, objProxy=proxy, viewProxy=view)` |
| Execute | `proxy.execute(obj)` on recompute, followed by `proxy.onChanged(obj, prop)` |
| Shape | the proxy writes `obj.Shape`; the object is a `Part::Feature` underneath |
| Attachment | `Part::Part2DObjectPython` adds `AttachmentSupport`, `MapMode`, `MapPathParameter` |
| Persistence | the proxy is pickled into `Document.xml`; our native format uses `python_state` |
| Restore | `onDocumentRestored(obj)` after a load, so the proxy can rebuild transient state |

## Slices

| Slice | Work | Acceptance |
| --- | --- | --- |
| **P2a ✅** | Register `Part::FeaturePython` as an `ObjectType` whose `execute` calls the stored Python proxy. Store the proxy in `Property::PythonObject`; expose `Proxy` on the object. | A script object recomputes and its `Shape` is visible; `cargo test -p ferrocad_part_py` green. |
| **P2b ✅** | `doc.addObject(..., objProxy=..., viewProxy=...)` and `getObject(...).Proxy`; `onChanged` after a property write. | A proxy that counts `execute`/`onChanged` calls observes the right counts. |
| **P2c ✅** | Persist and restore the proxy in both formats; call `onDocumentRestored`. Native JSON uses the existing `python_state` envelope; `.FCStd` follows upstream's `<Python value>` only if the payload can be made compatible (otherwise report it in `WriteReport::dropped`). | Save → open → recompute with the same proxy; a document without a proxy still loads. |
| **P2d ✅** | `Part::Part2DObjectPython` with `AttachmentSupport`/`MapMode` and the plane/line/point attach modes Draft sets. | A sketch-like 2D object attaches to a face and survives recompute. |
| **P2e ✅** | Draft M2 acceptance: run the two `drafttests` modules against the proxies. | Record the pass/load counts; do not gate on a number. |

**As built (2026-10-24).** The proxies execute outside the document mutex, drain their
pending requests, purge the touch flags they set and recompute dependents (a 64-round
non-convergence guard). `onChanged` fires after attribute, `setPropertyByName`, `Label`
and `Group` writes; `attach=True` calls `proxy.attach(obj)`; `isDerivedFrom` carries the
Part/App hierarchy Draft branches on. Native persistence reuses `python_state`; `.FCStd`
reports the dropped proxy and still round-trips the shape. `Part::Part2DObjectPython`
exists and its `Part::AttachExtensionPython` supplies `AttachmentSupport`/`MapMode`/
`MapPathParameter` with the plane/line/point attach modes, `changeAttacherType` and
`positionBySupport`.

**P2d/P2e (2026-10-24).** The attachment extension lives in
`crates/ferrocad_part_py/src/attach.rs`; the Draft M2 measurement is
`drafttests/test_creation.py` **26 passed** and `test_array.py` **1 passed**, with zero
failures, errors or load failures. The residual errors closed with `PropertyLinkSubList`
accepting `(object, subname)` entries, the link extension, `Placement.translate`/`.Matrix`,
`getSubObject` returning the named sub-shape (`Part::FeaturePy::getSubObject`),
`Shape.getSubShape`/`findPlane`, the curve `TypeId`s and `Part.getShape`.
`test_hatch` needed one more piece: a `python/TechDraw` shim for `makeGeomHatch` (see
[`../workbenches/conformance.md`](../workbenches/conformance.md)).

## Constraints

- `ferrocad_core` stays geometry-free. The proxy storage and the `execute` dispatch
  belong to core; the `Shape` output belongs to `ferrocad_part_py`.
- A proxy is process state, not document state: a missing proxy is an **honest**
  invalid object after load, not a silent no-op. Report it the way the module reports
  dropped values.
- Do not make the Python callback part of the native schema's required fields; an
  object with no proxy is still a valid `Part::FeaturePython` (upstream allows it).
- Keep the existing `App::FeaturePython` behaviour intact; this is a Part subclass,
  not a replacement.

## Acceptance

- `drafttests/test_creation.py` and `test_array.py` load and run (counts recorded).
- `Part` conformance stays at **529 passed, 0 errored** and `pytest tests` at 444.
- A native **and** `.FCStd` round trip of a scripted object keeps its `Shape` and its
  proxy, and a load with the proxy unavailable leaves the object invalid but readable.
