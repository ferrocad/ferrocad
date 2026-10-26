# Kernel state sharing

Status: implemented (2026-10-26; K2a 2026-10-27). The two supported compositions, the
`ferrocad_core::kernel()` facade (`05cf713`, K3), the `_kernel_id` identity
diagnostic (K0, `beca5fb`), the mixed-image guard (K1, `9869d76`) and the
interpreter-scoped `ferrocad_py` caches (K2a) are live. K2c (bucket reclamation and
PEP 3121 multi-phase state) is parked, and the modular-wheel `PyCapsule` C-ABI (K4)
remains an unscheduled proposal gated on whether separate binary wheels are ever
needed. Task record:
[`../tasks/kernel-state-sharing.md`](../tasks/kernel-state-sharing.md) (K0–K4).

Companion to [`repackaging.md`](repackaging.md) (the edition model),
[`distribution.md`](distribution.md) (payload mechanics),
[`occt-bundling.md`](occt-bundling.md) (the kernel closure),
[`edition-publishing.md`](edition-publishing.md) (the Parametric release plan), and
[`occt-integration.md`](occt-integration.md) §4 (where the kernel lives).

`ferrocad_parametric` (the edition binary) and `ferrocad_combined_py` (the
host-Python extension) are two packaging compositions of one invariant: **there is
exactly one process-wide FerroCAD core per process.**

## 1. Why separate `cdylib`s split the kernel

`ferrocad_core` is an `rlib`. When two Python extension crates are built as separate
`cdylib` targets (`ferrocad.abi3.so` and `Part.abi3.so`), the Rust linker statically
embeds a private copy of every `ferrocad_core` static in **each** shared library:

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

Because each `.so` resolves `rlib` symbols into its own `.data`/`.bss` and CPython
loads extensions with `RTLD_LOCAL` (or isolated PE data sections on Windows), each
module operates on an isolated core: documents created in `FreeCAD` are invisible to
`Part`, `"Part::Box"` is unknown to `doc.addObject()`, and shape property
converters fail type-identity checks.

## 2. State scopes and the `kernel()` facade

Every piece of state in FerroCAD belongs to one of four scopes:

| Scope | Realization | Examples | Rule |
| --- | --- | --- | --- |
| **Process-wide core state** | `ferrocad_core::kernel()` (`Kernel` facade over `application()`, `object_registry`, `property_types`) | Document registry, active document, observers, object-type behaviours, property factories and restore hooks | **Exactly one** per process |
| **Workbench singleton** | `ferrocad_part::backend()` (`OnceLock<Arc<dyn GeometryBackend>>`), `ferrocad_mesh::register` flag | OCCT geometry backend, element-name/geometry operations | One per process, installed by the composition root; never a second document/type registry |
| **Document state** | `ferrocad_core::Document` (`Arc<Mutex<Document>>` / `DocumentHandle`) | Object graph, property bags, per-document undo stack, recompute DAG | One per open document, owned by `kernel().documents()` |
| **Interpreter state** | PyO3 module objects, classes/exceptions; `ferrocad_py`'s `Py`-owning tables, keyed by `PyInterpreterState_GetID()`; `python/FerroCAD/__init__.py` provider lists | `Py<PyType>` handles, custom exceptions, document/object/observer caches, display/interaction providers | One bucket per interpreter for the `Py`-owning tables; the converter/resolver/provider/method tables hold no `Py` and stay process-wide |

### The `ferrocad_core::kernel()` boundary (`05cf713`)

`ferrocad_core::kernel()` returns `&'static Kernel`, providing a single entry point
for `documents()`, `object_types()`, `properties()` and `property_extensions()`, while
keeping the existing free functions (`application()`, `object_registry::*`,
`property_types::*`) as thin, non-breaking wrappers. Workbench crates must access
shared registries through `ferrocad_core` and never introduce their own document or
object-type `OnceLock`s.

### What must not move into core

- **Python objects.** `Py<PyType>`, exceptions and callbacks are interpreter-owned;
  they must never live in `ferrocad_core`, whose statics outlive any one interpreter.
- **GUI-only registries.** Viewport overlays, selection observers and view-state
  listeners belong behind `FreeCADGui`; they stay inert or absent in `--check-init`
  and headless renders.
- **Per-document caches.** Tessellation caches, prepared shapes and naming maps belong
  on `Document`, not in a global keyed by document name.
- **Workbench-local caches.** They belong to the workbench crate; the Part backend is
  the sanctioned example.

### Known deviations

- **`ferrocad_py` scopes only its `Py`-owning tables by interpreter (K2a).**
  `documents`, `objects` and `observers` live in one
  `HashMap<u64, InterpreterState>` keyed by `PyInterpreterState_GetID()`, so a second
  interpreter cannot see the first's `Py` objects. The converter/resolver/provider/
  method tables hold `(String, fn …)` pairs with no `Py`, so they stay process-wide.
  Buckets are not reclaimed when an interpreter is destroyed, and CPython still
  refuses the single-phase `#[pymodule]` in a subinterpreter; both are K2c.
- **The `Kernel` facade is a handle, not a state migration.** `kernel()` returns the
  existing registries; the free functions remain and reach the same state.

## 3. The two supported compositions

```text
                        ferrocad_core (rlib)
                 kernel(): documents, object_types, properties
                                 ^
           +---------------------+---------------------+
           |                     |                     |
    ferrocad_py (rlib)    ferrocad_part_py (rlib)   ferrocad_mesh_py (rlib)
    module `ferrocad`     module `Part`             modules `Mesh`, `MeshPart`
           |                     |                     |
           +----------+----------+----------+----------+
                      |                     |
                      v                     v
           ferrocad_parametric        ferrocad_combined_py
           (edition binary)           (host-Python cdylib)

```

| Composition | Artifact | Registration & guard |
| --- | --- | --- |
| `ferrocad_parametric` | `bin/ferrocad` | Calls `ferrocad_combined_py::register_builtins()` (`append_to_inittab!` for `ferrocad`, `Part`, `Mesh`, `MeshPart`) before interpreter boot. |
| `ferrocad_combined_py` | `ferrocad_combined.abi3.so` | `install()` builds all four modules against the single linked core and publishes them in `sys.modules`, marking each with `__ferrocad_combined__ = True`. It refuses to overwrite built-ins when imported inside an edition binary. |

### Facade and mixed-image guards

`FreeCAD` (`python/FreeCAD/__init__.py`) and `FerroCAD`
(`python/FerroCAD/__init__.py`) are pure-Python packages layered over `ferrocad`:

1. **Edition guard in `FreeCAD/__init__.py`:** checks
   `importlib.util.find_spec("Part").loader is BuiltinImporter` and skips importing
   `ferrocad_combined` when running inside the edition binary.
2. **Mixed-image guard in `ferrocad_combined_py::install()`:** honours the
   `__ferrocad_combined__` marker first (idempotence), then returns `ImportError`
   when `Part` is already registered via `BuiltinImporter`, so an on-disk `.so`
   cannot introduce a second core into an edition process.

## 4. Verification gates

All gates are active in CI:

- **Observable kernel identity (`_kernel_id`):** exported on `ferrocad`, `Part`,
  `Mesh` and `MeshPart`, and re-exported by `FreeCAD` and `FerroCAD`. The gates
  assert the values are equal across modules.
- **`ferrocad_parametric --check-init`:** boots the edition, imports
  `ferrocad_parametric`, verifies `_kernel_id` identity across all modules, and
  creates, fills and closes a document containing a `Part::Feature` box.
- **Rust and Python suites:** `ferrocad_core` (110 tests, including the `kernel()`
  facade parity), `ferrocad_combined_py` (`combined.rs`, `mixed_image.rs`),
  `part_boot.rs`, and `tests/test_packaged_modules.py` (2/2).

## 5. Deferred: modular wheels and the `PyCapsule` C-ABI (K4)

First-party workbenches (`Part`, `Mesh`) are statically linked into the edition
binary and `ferrocad_combined.abi3.so`, while `.FCStd` files and the Python API serve
as the public interoperability boundary. A `#[repr(C)]` `PyCapsule` vtable
(`ferrocad_core_capi`) is only required if FerroCAD later distributes separate
per-workbench wheels (`pip install ferrocad-part`) or supports out-of-tree binary Rust
plugins.
