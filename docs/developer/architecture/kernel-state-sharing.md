# Kernel state sharing and extension linking

Status: implemented (2026-10-26; K2a 2026-10-27). The two supported compositions, the
`ferrocad_core::kernel()` facade (`05cf713`, K3), the `_kernel_id` identity diagnostic
(K0, `beca5fb`), the mixed-image guard (K1, `9869d76`) and the interpreter-scoped
`ferrocad_py` caches (K2a) are live. K2c (bucket reclamation and PEP 3121 multi-phase
state) is parked, and the modular-wheel `PyCapsule` C-ABI (K4) remains a deferred
proposal gated on whether separate binary wheels are ever needed. Task record:
[`kernel-state-sharing.md`](../../project/record/tasks/kernel-state-sharing.md) (K0–K4).

Companion to [`../architecture.md`](../architecture.md) (the crate map),
[`python-bindings.md`](python-bindings.md) (the binding design),
[`repackaging.md`](repackaging.md) (the edition model),
[`distribution.md`](distribution.md) (payload mechanics),
[`occt-bundling.md`](occt-bundling.md) (the kernel closure),
[`edition-publishing.md`](edition-publishing.md) (the Parametric release plan), and
[`occt-integration.md`](occt-integration.md) §4 (where the kernel lives).

`ferrocad_parametric` (the edition binary) and `ferrocad_combined_py` (the host-Python
extension) are two packaging compositions of one invariant: **there is exactly one
process-wide FerroCAD core per process.** This note states why separate `cdylib`s
break that invariant, the state scopes, the linking options, the decision, and what
keeps the Rust API ready for the deferred `PyCapsule` boundary.

## 1. Why separate `cdylib`s split the kernel

In upstream FreeCAD (C++), the Python extension modules (`FreeCAD.so`, `Part.so`,
`Mesh.so`) dynamically link against one OS shared library, `libFreeCADApp.so`. When
Python loads the modules, the OS dynamic linker (`ld.so`) maps that library into the
process address space once, so there is one `App::Application::_pcSingleton`:

```text
UPSTREAM C++: shared OS library (libFreeCADApp.so)

  FreeCAD.so ──┐
  Part.so ─────┼──> OS dynamic linker (ld.so) ──> ONE loaded libFreeCADApp.so
  Mesh.so ─────┘                                    └── App::Application::_pcSingleton
```

In FerroCAD, `ferrocad_core` is a pure-Rust `rlib`, and Rust has no stable
cross-`cdylib` ABI. When two Python extension crates are built as independent
`cdylib` targets (`ferrocad.abi3.so` and `Part.abi3.so`), the Rust linker statically
embeds a private copy of every `ferrocad_core` static into **each** shared library:

```text
BROKEN: separate Rust cdylibs, two isolated cores

  ferrocad.abi3.so                    Part.abi3.so
  +--------------------------+        +--------------------------+
  | ferrocad_py              |        | ferrocad_part_py         |
  |   -> ferrocad_core (A)   |        |   -> ferrocad_core (B)   |
  |      APPLICATION_A       |        |      APPLICATION_B       |
  |      OBJECT_TYPES_A      |        |      OBJECT_TYPES_B      |
  +--------------------------+        +--------------------------+
```

Each `.so` resolves the `rlib` symbols into its own `.data`/`.bss` segment, and
CPython loads extensions with `RTLD_LOCAL` (or isolated PE data sections on Windows),
so each module operates on an isolated core:

1. **DOM state split.** Documents created in `FreeCAD` (`APPLICATION_A`) are invisible
   to `Part` (`APPLICATION_B`).
2. **Isolated object and property registries.** `"Part::Box"` and `"Part::Feature"`
   registered in `OBJECT_TYPES_B` are unknown to `doc.addObject()` in
   `OBJECT_TYPES_A`.
3. **Type-identity failures.** Shape and mesh property converters fail type-identity
   (`TypeId` / extension) checks across the dynamic boundary.

## 2. State scopes and the `kernel()` facade

Every piece of state in FerroCAD belongs to one of four explicit scopes:

| Scope | Realization | Examples | Invariant |
| --- | --- | --- | --- |
| **Process-wide core state** | `ferrocad_core::kernel()` (`Kernel` facade over `application()`, `object_registry`, `property_types`) | Document registry, active document, observers, object-type behaviours, property factories and restore hooks | **Exactly one** per process |
| **Workbench singleton** | `ferrocad_part::backend()` (`OnceLock<Arc<dyn GeometryBackend>>`), `ferrocad_mesh::register` flag | OCCT geometry backend, element-name/geometry operations | One per process, installed by the composition root; never a second document/type registry |
| **Document state** | `ferrocad_core::Document` (`Arc<Mutex<Document>>` / `DocumentHandle`) | Object graph, property bags, per-document undo stack, recompute DAG | One per open document, owned by `kernel().documents()` |
| **Interpreter state** | PyO3 module objects, classes/exceptions; `ferrocad_py`'s `Py`-owning tables keyed by `PyInterpreterState_GetID()`; `python/FerroCAD/__init__.py` provider lists | `Py<PyType>` handles, custom exceptions, document/object/observer wrapper caches, display/interaction providers | One bucket per interpreter for `Py`-owning tables; converter/resolver/provider/method tables hold no `Py` and stay process-wide |

### The `ferrocad_core::kernel()` boundary (`05cf713`, K3)

`ferrocad_core::kernel()` returns `&'static Kernel`, providing one authoritative
entry point for `documents()`, `object_types()`, `properties()` and
`property_extensions()`, while keeping the existing free functions (`application()`,
`object_registry::*`, `property_types::*`) as thin, non-breaking wrappers that reach
the same state. Workbench crates must access shared registries exclusively through
`ferrocad_core` and must never introduce shadow document or object-type `OnceLock`s.

### What must not move into `ferrocad_core`

- **Python objects.** `Py<PyType>`, exceptions and Python callbacks are
  interpreter-owned; they must never live in `ferrocad_core`, whose statics outlive
  any one interpreter.
- **GUI-only registries.** Viewport overlays, selection observers and view-state
  listeners belong behind `FreeCADGui`; they stay inert or absent in `--check-init`
  and headless renders.
- **Per-document caches.** Tessellation caches, prepared shapes and naming maps belong
  on `Document`, not in a global keyed by document name.
- **Workbench-local caches.** They belong in the workbench crate; the Part backend is
  the sanctioned example.

### Interpreter scoping and known deviations (K2a / K2c)

- **K2a (live).** `ferrocad_py` scopes its `Py`-owning tables (`documents`, `objects`,
  `observers`) inside a `HashMap<u64, InterpreterState>` keyed by
  `PyInterpreterState_GetID()`, so a second interpreter cannot see the first
  interpreter's `Py` wrapper objects. Converter, resolver, provider and method tables
  hold `(String, fn …)` pairs with no `Py` and remain process-wide.
- **K2c (parked).** Interpreter buckets are not yet reclaimed when an interpreter is
  destroyed, and CPython still refuses the single-phase `#[pymodule]` initialisation
  in a subinterpreter.

## 3. Architectural options

Three approaches were evaluated for upholding the single-core invariant across both the
app shell and standalone Python.

### Option 1: unified static composition (implemented)

Compile `ferrocad_py`, `ferrocad_part_py` and `ferrocad_mesh_py` as `rlib`s and link
them once into the edition binary (`ferrocad_parametric`) for the host-owned path, and
into one combined `cdylib` (`ferrocad_combined.abi3.so`) for the Python-owned path.

- **Pros.** Cargo deduplicates `ferrocad_core` at compile time; zero FFI/marshalling
  overhead; no OS dynamic-linker (`rpath` / `auditwheel` / `delocate`) complexity.
- **Cons.** First-party workbenches are compiled into the combined artifact rather
  than shipped as separate binary wheels.

### Option 2: modular Python extensions with a `PyCapsule` C-ABI (deferred, K4)

Only `ferrocad_py` links `ferrocad_core`, and it exports a versioned `#[repr(C)]`
vtable through a `PyCapsule` (`ferrocad_core_capi`). Separate workbench `.so` wheels
call the core through that vtable.

- **Pros.** Enables independent binary wheels (`pip install ferrocad-part`) and
  out-of-tree Rust binary plugins without OS linker friction: `PyCapsule_Import`
  resolves the vtable in memory at import time, and calls run at native C
  function-pointer speed.
- **Cons.** Requires designing and maintaining an explicit `#[repr(C)]` FFI surface.

### Option 3: OS-linked shared core (`libferrocad_core.so`, rejected as the contract)

Compile `ferrocad_core` as an OS shared library (`cdylib`) mirroring C++
`libFreeCADApp.so`.

- **Pros.** Allows native C++ dynamic linking if exposed through `cxx`.
- **Cons.** High cross-platform wheel-packaging friction (`rpath`, `@loader_path`,
  Windows DLL search paths). Native C++ FreeCAD workbench compatibility is explicitly
  out of scope.

The decision is **Option 1 now, Option 2 when independent binary distribution is a
concrete requirement, and Option 3 only as an optional backing detail of Option 2.**
The kernel and its registries stay private to `ferrocad_core`, and the internal Rust
API is kept capsule-ready so that the later step is localised to the binding crates.

## 4. The two supported compositions

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

| Composition | Artifact | Registration and mixed-image guard |
| --- | --- | --- |
| **`ferrocad_parametric`** (host-owned) | `bin/ferrocad` | Calls `ferrocad_combined_py::register_builtins()` (`append_to_inittab!` for `ferrocad`, `Part`, `Mesh`, `MeshPart`) before booting the embedded CPython interpreter. |
| **`ferrocad_combined_py`** (Python-owned) | `ferrocad_combined.abi3.so` | `install()` builds all four modules against the single linked `ferrocad_core` and publishes them in `sys.modules`, marking each with `__ferrocad_combined__ = True`. It refuses to overwrite built-ins when imported inside an edition binary. |

### Facade and mixed-image guards (K1, `9869d76`)

`FreeCAD` (`python/FreeCAD/__init__.py`) and `FerroCAD`
(`python/FerroCAD/__init__.py`) are pure-Python packages layered over `ferrocad`. Two
guards prevent an on-disk `.so` from accidentally loading a second copy of
`ferrocad_core` into an edition process:

1. **Edition guard in `FreeCAD/__init__.py`.** Checks
   `importlib.util.find_spec("Part").loader is BuiltinImporter` and skips importing
   `ferrocad_combined` when running inside the edition binary.
2. **Mixed-image guard in `ferrocad_combined_py::install()`.** Honours the
   `__ferrocad_combined__` marker first (for idempotence), then raises `ImportError`
   if `Part` is already registered via `BuiltinImporter`.

## 5. Verification gates

All kernel-sharing invariants are enforced in CI:

- **Observable kernel identity (`_kernel_id`, K0, `beca5fb`).** Exported on
  `ferrocad`, `Part`, `Mesh` and `MeshPart`, and re-exported by `FreeCAD` and
  `FerroCAD`. The gates assert that the values are equal across all loaded modules.
- **`ferrocad_parametric --check-init`.** Boots the edition binary, imports
  `ferrocad_parametric`, verifies `_kernel_id` equality across all modules, and
  creates, populates and closes a document containing a `Part::Feature` box.
- **Automated suites.** `ferrocad_core` (110 tests, including `kernel()` facade
  parity), `ferrocad_combined_py` (`combined.rs`, `mixed_image.rs`), `part_boot.rs`
  and `tests/test_packaged_modules.py` (2/2).

## 6. Deferred: modular wheels and the `PyCapsule` C-ABI (K4)

### Policy on C++ interoperability and modular binaries

- **Native C++ module coexistence is out of scope.** FerroCAD does not target binary
  C++ ABI compatibility with upstream FreeCAD's `libFreeCADApp.so` (via `cxx` or C++
  shims). `.FCStd` files, standard CAD formats (STEP/BREP/STL), IPC and the Python
  API are the public interoperability boundaries.
- **When K4 (`ferrocad_core_capi`) triggers.** A `#[repr(C)]` `PyCapsule` vtable is
  only required if FerroCAD later distributes separate per-workbench wheels
  (`pip install ferrocad-part`) or supports out-of-tree binary Rust plugins. If an
  OS-level `libferrocad_core.so` is ever introduced, it stays an internal backing
  detail behind the `PyCapsule` contract.

### Keeping the Rust API capsule-ready

To ensure K4 remains a localised addition rather than a cross-workspace rewrite:

1. **Route all core registry access through `ferrocad_core::kernel()`.** Never add
   secondary document or type registries outside `ferrocad_core`.
2. **Keep geometry and domain crates pure.** `ferrocad_types`, `ferrocad_curves`,
   `ferrocad_lineage`, `ferrocad_geom` and `ferrocad_occt` must never depend on
   `ferrocad_core`.
3. **Centralise extension and property type seams.** Keep `Property::Extension`
   downcasting and converter registration behind
   `ferrocad_core::kernel().properties()` and `property_extensions()`, so lowering
   the boundary to explicit type tags over a `#[repr(C)]` vtable requires no changes
   to workbench domain logic.
