# Kernel state sharing

Status: current architecture (2026-10-26). The two supported compositions described
here are implemented, as is the `_kernel_id` identity diagnostic (K0) and the
mixed-image guard (K1). The consolidated `Kernel` facade and the modular-wheel
`PyCapsule` ABI are **proposals** and are labelled as such.
Work items: [`../tasks/kernel-state-sharing.md`](../tasks/kernel-state-sharing.md)
(K0–K4).

Companion to [`repackaging.md`](repackaging.md) (the edition model),
[`distribution.md`](distribution.md) (payload mechanics),
[`occt-bundling.md`](occt-bundling.md) (the kernel closure),
[`edition-publishing.md`](edition-publishing.md) (the Parametric release plan),
and [`occt-integration.md`](occt-integration.md) §4 (where the kernel lives).

`ferrocad_parametric` (the edition binary) and `ferrocad_combined_py` (the
host-Python extension) are two packaging compositions of one invariant: **there is
exactly one process-wide FerroCAD core per process.** This note explains why
separate Rust `cdylib` extension modules split that core, which state is
process-wide today, and what must be true before independently distributed wheels
can exist.

## 1. Why separate `cdylib`s split the kernel

`ferrocad_core` is an `rlib`. When two Python extension crates are built as
separate `cdylib` targets (`ferrocad.abi3.so` and `Part.abi3.so`), the Rust linker
statically embeds a private copy of every `ferrocad_core` static in **each** shared
library:

```text
BROKEN: two extension modules, two cores

  ferrocad.abi3.so                    Part.abi3.so
  +--------------------------+        +--------------------------+
  | ferrocad_py              |        | ferrocad_part_py         |
  |   -> ferrocad_core (A)   |        |   -> ferrocad_core (B)   |
  |      APPLICATION_A       |        |      APPLICATION_B       |
  |      OBJECT_TYPES_A      |        |      OBJECT_TYPES_B      |
  +--------------------------+        +--------------------------+
```

The copies stay separate for three independent reasons:

1. **Each `.so` contains its own copy.** Each `cdylib` has already resolved the
   `rlib` symbols into its own `.data`/`.bss`; the loader does not merge them.
2. **Rust gives non-`pub` statics internal linkage.** The copies are not exported
   candidates for symbol interposition in the first place (and interposition would
   not merge two already-linked data objects anyway).
3. **CPython loads extension modules with `RTLD_LOCAL`** on POSIX, and PE/COFF DLLs
   have separate data sections on Windows. There is no shared symbol namespace to
   begin with.

The failure modes are silent because every module answers correctly *within its own
core*:

| Symptom | What happens |
| --- | --- |
| **Invisible documents** | `ferrocad.newDocument("D")` registers the document in `APPLICATION_A`; `Part` sees `APPLICATION_B` and reports no active document, or creates an orphan. |
| **Unknown object types** | `Part` registers `Part::Feature` / `Part::Box` in `OBJECT_TYPES_B`; `doc.addObject("Part::Box", ...)` against `APPLICATION_A` raises "unknown type". |
| **Broken type identity** | A `PyObject` wrapping a shape from `ferrocad_core (A)` does not round-trip through a property converter registered in `ferrocad_core (B)`; `Shape` reads back as `None` or the wrong class. |

This is not a PyO3 quirk. It is the normal behaviour of two independently linked
copies of the same Rust crate being loaded into one process.

## 2. State scopes

Every piece of state in FerroCAD belongs to one of four scopes. The first three are
the "three-tier" model; the fourth is the workbench singleton, which exists because
Part owns a geometry backend that is not part of the document core.

| Scope | Current realization | Examples | Rule |
| --- | --- | --- | --- |
| **Process-wide core state** | `ferrocad_core::kernel()` over `ferrocad_core::application()` (`OnceLock<Mutex<Application>>`), `object_registry` (`ObjectTypeRegistry`), `property_types` (`PropertyRegistry` + `PropertyExtensionRegistry`) | Document registry, active document, document observers, object-type behaviours, property factories and extension restore hooks | **Exactly one** per process; a workbench crate must not introduce its own `OnceLock<…Registry>` |
| **Interpreter state** | PyO3 module objects and their classes/exceptions; the `ferrocad_py` caches (see "Known deviations") | `Py<PyType>` handles, custom exceptions, converter/resolver tables | One per interpreter in principle; **currently single-interpreter only** |
| **Document state** | `ferrocad_core::Document`, held as `Arc<Mutex<Document>>` (`DocumentHandle`) | Object graph, property bags, per-document undo stack, recompute DAG | One per open document, owned by the `Application` registry |
| **Workbench singleton** | `ferrocad_part::backend()` (a private `BACKEND: OnceLock<Arc<dyn GeometryBackend>>` behind it), `ferrocad_mesh::register`'s idempotence flag | The OCCT backend, the kernel's element-name/geometry operations | One per process, installed by the composition root; never a second document or object-type registry |

### What must not move into core

- **Python objects.** `Py<PyType>`, exceptions and callbacks are interpreter-owned.
  They must never live in `ferrocad_core`, whose statics outlive any one
  interpreter.
- **GUI-only registries.** Viewport overlays, selection observers and view-state
  listeners belong behind `FreeCADGui`; they must stay inert or absent in
  `--check-init` and headless renders.
- **Per-document caches.** Tessellation caches, prepared shapes and naming maps
  belong on `Document`, not in a global keyed by document name.
- **Workbench-local caches.** They belong to the workbench crate. The Part backend
  is the sanctioned example.

### Known deviations

The rule above describes the target; the current details are worth stating plainly
rather than implying the code is cleaner than it is:

- **The kernel facade is a handle, not a state migration.** `ferrocad_core::kernel()`
  returns `&'static Kernel`; `documents()`, `object_types()`, `properties()` and
  `property_extensions()` return the existing `OnceLock`s. `application()`,
  `object_registry::*` and `property_types::*` remain and reach the same state.
- **`ferrocad_py` keeps Python-object caches in process-wide `OnceLock`s.**
  `DOCUMENTS`, `OBJECT_CACHE`, `OBSERVERS`, the extension converters and the
  sub-element resolvers are all process-global. That is correct for the
  single-interpreter app and for the combined extension, but it is **not**
  subinterpreter-safe. Moving them into PyO3 multi-phase module state (PEP 3121) is
  a proposed cleanup, not current behaviour. The PyO3 modules here are single-phase.
  The display/interaction/manipulator provider tables are a different case: they are
  module-level Python lists in `python/FerroCAD/__init__.py`, process-global for the
  same single-interpreter reason.
- **`ferrocad_part::backend()` is a workbench-level `OnceLock`.** It is the injected
  kernel backend, not a second document/object-type core, and the composition root
  installs it once. The rule is "no new process-wide *core* registries in a
  workbench crate", not "no statics at all".

## 3. The two supported compositions

Only the leaf packaging targets produce a supported multi-module composition:

```text
                       ferrocad_core (rlib)
             Application, object types, property types
                                ^
          +---------------------+---------------------+
          |                     |                     |
    ferrocad_py (rlib)   ferrocad_part_py (rlib)   ferrocad_mesh_py (rlib)
    module `ferrocad`    module `Part`             modules `Mesh`, `MeshPart`
          |                     |                     |
          +----------+----------+----------+----------+
                     |                     |
                     v                     v
          ferrocad_parametric       ferrocad_combined_py
          (edition binary)          (host-Python cdylib)
```

| Composition | Artifact | Registration | Use |
| --- | --- | --- | --- |
| `ferrocad_parametric` | `bin/ferrocad` (packaged) | calls `ferrocad_combined_py::register_builtins()` before the interpreter starts; `append_to_inittab!` for `ferrocad`, `Part`, `Mesh`, `MeshPart` | Desktop edition |
| `ferrocad_combined_py` | `ferrocad_combined.abi3.so`, module `ferrocad_combined` | `install()` builds the four module objects and publishes them in `sys.modules` | Host Python (`PYTHONPATH=python` / tests / scripts) |
| standalone `ferrocad.abi3.so`, `Part.abi3.so`, `Mesh.abi3.so` | separate `cdylib`s | their own `#[pymodule]` init | Isolated experiments only; **not** a supported multi-module composition |

`ferrocad_py`, `ferrocad_part_py` and `ferrocad_mesh_py` still declare
`crate-type = ["cdylib", "rlib"]`; the standalone cdylibs remain buildable. The
supported compositions are the two leaves above, and `ferrocad_combined_py` is the
only supported way to get more than one module in a host interpreter.

### The Python facades are not Rust modules

`FreeCAD` and `FerroCAD` are Python packages in `python/`, layered on the `ferrocad`
extension:

- `ferrocad` is the PyO3 module (`ferrocad_py`), the `App`-level surface.
- `FreeCAD` is the upstream-shaped facade (`python/FreeCAD/__init__.py`).
- `FerroCAD` is the FerroCAD-only extension surface (`python/FerroCAD/__init__.py`).
- `ferrocad_combined` publishes `ferrocad`, `Part`, `Mesh` and `MeshPart`; it does
  **not** publish `FreeCAD` or `FerroCAD`.

`python/FreeCAD/__init__.py` imports `ferrocad_combined` first **only when `Part`
is not a built-in** (`importlib.util.find_spec("Part").loader is
BuiltinImporter`). In the edition binary the four modules are built-ins, so the
facade must not import the on-disk extension and replace the edition's core. In a
host interpreter there is no built-in `Part`, so the combined extension is the
correct source for all four modules.

## 4. Current registration sequence

### Edition binary

`crates/ferrocad_parametric/src/main.rs` registers the modules first, then either
runs the `--check-init` boot or the shell. The relevant part is:

```rust
fn main() {
    // One process, one core: `ferrocad`, `Part`, `Mesh` and `MeshPart` are
    // appended to the interpreter's built-in table before it initialises.
    ferrocad_combined_py::register_builtins();

    if std::env::args().skip(1).any(|arg| arg == "--check-init") {
        // Boot the base app, then import the edition's app script (which imports
        // Part and registers the display/interaction hooks).
        let result = ferrocad::check_init().and_then(|()| {
            pyo3::Python::with_gil(|py| {
                py.import("ferrocad_parametric")
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })
        });
        // print `ferrocad: init OK` / exit 1 on `Err`
        return;
    }

    // ... parse edition args, add contributions, run the shell.
}
```

The registration itself is one list in `ferrocad_combined_py::register_builtins()`:

```rust
pub fn register_builtins() {
    use ferrocad_mesh_py::{Mesh as mesh_module, MeshPart as meshpart_module};
    use ferrocad_part_py::Part as part_module;
    use ferrocad_py::ferrocad as ferrocad_module;
    pyo3::append_to_inittab!(ferrocad_module);
    pyo3::append_to_inittab!(part_module);
    pyo3::append_to_inittab!(mesh_module);
    pyo3::append_to_inittab!(meshpart_module);
}
```

### Host-Python extension

An imported extension cannot call `PyImport_AppendInittab` (the interpreter is
already running), so `ferrocad_combined` calls `install()` instead:

1. `PyModule::new(py, name)` for each of `ferrocad`, `Part`, `Mesh`, `MeshPart`;
2. call the corresponding `#[pymodule]` init on that module object;
3. set `__ferrocad_combined__ = True` and store it in `sys.modules`;
4. repeat for the next module.

`install()` is idempotent: it checks the marker on `sys.modules["Part"]` and
returns if this crate already published the set. It deliberately replaces a module
imported from a *different* `.so` (for example a standalone `Part.abi3.so`),
because that module cannot share this extension's core.

The Python shims make plain imports reach the published modules:

```python
# python/Part.py (same shape for Mesh.py and MeshPart.py)
import sys
import ferrocad_combined            # publishes sys.modules["Part"]
sys.modules[__name__] = sys.modules["Part"]
```

### What each module init registers

| Module | Init does |
| --- | --- |
| `ferrocad` | `set_python_executor_available(true)`; adds the `App` classes and functions |
| `Part` | `register_kernel()`: installs the OCCT `GeometryBackend` (`ferrocad_part::register`), the shape property/list, all Part object types, the `Part::Feature` Placement↔Shape sync, and the link / link-array / solids / feature-python registrations; then adds the `Part` classes and functions |
| `Mesh` | `ferrocad_mesh::register()` and the mesh extension converter; adds `Mesh` classes and functions |
| `MeshPart` | adds `MeshPart.meshFromShape` / `wireFromSegment` |

## 5. Verification

The invariant is checked by these gates:

| Gate | What it proves |
| --- | --- |
| `crates/ferrocad_part_py/tests/part_boot.rs` | `Part` and `ferrocad` in one image: both report the same `_kernel_id`; `Part.makeBox` returns a `Part.Shape`, and `obj.Shape = shape` round-trips through `Part::Feature` with `isSame` |
| `crates/ferrocad_combined_py/tests/combined.rs` | All four modules import and report one `_kernel_id`; a `Part::Feature` and a `Mesh::Feature` live in one document; `type(obj.Shape) is Part.Shape`, `type(obj.Mesh) is Mesh.Mesh`, and `MeshPart.meshFromShape` consumes the Part shape |
| `crates/ferrocad_combined_py/tests/mixed_image.rs` | `install()` refuses an interpreter whose `Part` is a built-in, and does not replace it; a file-based `Part.py` is not mistaken for a built-in |
| `tests/test_packaged_modules.py` | The same composition from a real host CPython with `PYTHONPATH=python`, including `import FreeCAD` before `import Part`, with the `_kernel_id` equality assertion; skips when `ferrocad_combined.abi3.so` is not built |
| `cargo test -p ferrocad_parametric` | The edition's manipulator round trips (fillet and the move gizmo) through the app script and document; the move-gizmo test also asserts one undo restores the placement |
| `ferrocad_parametric --check-init` | The base boot, the edition's app script import, `_kernel_id` equality across `FreeCAD`/`Part`/`Mesh`/`MeshPart`, and a document with a `Part::Feature` holding a `Part.makeBox` |
| `crates/ferrocad_core/src/tests/kernel.rs` | `kernel().documents()` sees a document created through `application()`; object types, property types and extension restore hooks registered through the free functions are visible through `kernel()` |

### Diagnostics in place

- **`_kernel_id` on every module.** Each `#[pymodule]` init adds
  `ferrocad_core::application_id()` (the address of the process-wide
  `Application`), and `python/FreeCAD/__init__.py` and
  `python/FerroCAD/__init__.py` re-export it. `--check-init` and the three test
  targets assert all four values are equal, so a split linkage fails loudly.
- **A document/Box assertion in `--check-init`.** It creates a document, adds a
  `Part::Feature`, assigns a `Part.makeBox`, checks `ShapeType == "Solid"` and
  closes the document.
- **A capsule mismatch test** remains proposed with the modular-wheel ABI in §6.

## 6. Modular wheels and the capsule ABI (proposal)

The E4 slice in [`edition-publishing.md`](edition-publishing.md) is a **wheel that
bundles Part** (`pip install ferrocad[parametric]`). That wheel can ship
`ferrocad_combined.abi3.so` or register the four modules as built-ins; it does
**not** need a new ABI. The capsule below is for a different, not-yet-scoped
scenario:

- independently distributed wheels such as `ferrocad` + `ferrocad-part`;
- out-of-tree Rust workbenches that need the kernel without recompiling it.

In that scenario a satellite extension must not link `ferrocad_core` directly. The
proposal is a versioned C-ABI table handed over in a Python `PyCapsule`:

```text
                       ferrocad_core_capi (proposed rlib)
                    #[repr(C)] FerroCadKernelApi vtable
                        (no pyo3, no Rust ABI)

              +----------------------+----------------------+
              |                                             |
    ferrocad_core_py (cdylib)                     ferrocad_part_py (cdylib)
    owns the core; exports                         imports the capsule and
    PyCapsule "ferrocad.kernel_api.v1"             borrows the core through it
```

The proposal, at a high level:

1. **Version in the capsule name.** `ferrocad.kernel_api.v1`; a breaking change
   ships `.v2`.
2. **Handshake before use.** The satellite checks `abi_version` and
   `struct_size`; a mismatch raises an immediate `ImportError` instead of risking
   UB. Minor versions append fields only at the end.
3. **Core owns all lifetimes.** Satellites receive opaque handles or integer IDs.
   Allocation, deallocation and every document/object operation cross back through
   the vtable into the core.
4. **No Rust types cross the boundary.** The table is `#[repr(C)]`; errors are
   status codes plus a borrowed message; panics are caught at the FFI boundary.
5. **Subinterpreters remain out of scope** for the first version; the capsule is
   process-local.

None of this exists today: there is no `ferrocad_core_capi` or
`ferrocad_core_py` crate. The acceptance criterion for the proposal is a satellite
`.so` that imports `ferrocad`, obtains the capsule, registers an object type, and
has `ferrocad`'s documents see it, with no `ferrocad_core` in the satellite's link
map (`readelf -d` / `otool -L`).

## 7. Relation to the edition-publishing plan

The edition plan's E1/E2 slices (the Linux artifact and the release matrix) and E3
(the macOS/Windows closures) are packaging work; they do not change the state model.
The invariant they must preserve is the one in this note:

- the edition binary and the combined extension each link the `rlib` graph **once**;
- every module resolves the same `ferrocad_core` statics;
- the Python facade never adds a second core.

[`edition-publishing.md`](edition-publishing.md) §8 states the same architecture in
the release plan's terms.
