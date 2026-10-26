# Task — kernel state sharing

Status: task (2026-10-26). The split-kernel bug is fixed; **K0–K1 are the open
hardening work**, K2–K3 are debt, and K4 is a gated proposal. Design authority:
[`../architecture/kernel-state-sharing.md`](../architecture/kernel-state-sharing.md).
Backlog: [`../TODO-26-10-2026.md`](../TODO-26-10-2026.md) P6.

## Why this is a task

`ferrocad_combined_py` now links `ferrocad`, `Part`, `Mesh` and `MeshPart` into
one extension, and `ferrocad_parametric` registers the same set as built-ins. That
fixes the observed bug: before it, `PYTHONPATH=python` loaded 17 of SimpleCAD's 88
test modules (61 missing `Part`, 10 missing `Mesh`), while the in-process harness
hid the gap.

What remains is the difference between "the composition happens to be correct" and
"a regression cannot land":

- nothing exposes the kernel identity, so a split cannot be asserted;
- `ferrocad_combined_py::install()` can replace an edition's built-in `ferrocad`
  when the on-disk extension is imported in the app;
- the PyO3 caches are process-wide, so the modules are single-interpreter only;
- the process-wide registries are several `OnceLock`s, not one documented boundary;
- the modular-wheel capsule does not exist, and may never need to.

## Current state (verified 2026-10-26)

| Piece | Where | State |
| --- | --- | --- |
| Documents / active document / observers | `ferrocad_core::application()` (`OnceLock<Mutex<Application>>`) | Implemented; process-wide |
| Object-type behaviours | `ferrocad_core::object_registry` (`OnceLock<Mutex<BTreeMap<...>>>`) | Implemented; process-wide |
| Property factories / restore hooks | `ferrocad_core::property_types` (`OnceLock` registry + extensions) | Implemented; process-wide |
| Geometry backend | `ferrocad_part::backend()` behind a private `BACKEND: OnceLock<...>` | Implemented; one per process, installed by the composition root |
| Python-object caches | `ferrocad_py` `DOCUMENTS`, `OBJECT_CACHE`, `OBSERVERS`, `CONVERTERS`, `RESOLVERS` | Implemented; process-wide, single-interpreter only |
| Display/interaction providers | module-level Python lists in `python/FerroCAD/__init__.py` | Implemented; process-wide by the same assumption |
| Module identity diagnostic | `ferrocad_core::application_id()`, `_kernel_id` on every module | Implemented (K0) |
| Mixed-image guard | `ferrocad_combined_py::install()` refuses built-ins | Implemented (K1) |
| Kernel facade | — | **Missing** (proposal) |
| Capsule ABI | — | **Missing** (proposal) |

The existing gates are `crates/ferrocad_part_py/tests/part_boot.rs`,
`crates/ferrocad_combined_py/tests/combined.rs`,
`tests/test_packaged_modules.py`, `cargo test -p ferrocad_parametric`, and
`ferrocad_parametric --check-init`. The first three prove document/type sharing;
none of them asserts a process identity.

## Slices

| Slice | Size | Work | Acceptance | Depends on |
| --- | --- | --- | --- | --- |
| **K0** | S | Observable kernel identity: `_kernel_id` on all four modules, re-exported by `FreeCAD`; extend `--check-init` with identity + document/Part assertions | All gates assert `FreeCAD._kernel_id == Part._kernel_id == Mesh._kernel_id == MeshPart._kernel_id`; `--check-init` creates and closes a document with a `Part::Feature` | — |
| **K1** | S/M | Mixed-image guard and composition matrix: `install()` refuses to replace an edition's built-ins; CI runs `--check-init` and the packaged test | Importing the on-disk combined extension in an edition fails cleanly; CI red if `Part` is missing or the IDs differ | K0 |
| **K2** | L | Interpreter-scoped Python state: move the `ferrocad_py` caches out of `std::sync::OnceLock`; key them by `PyInterpreterState_GetID()` or use PEP 3121 multi-phase module state | Two subinterpreters do not share `Py` objects; single-interpreter tests and SimpleCAD unchanged | K0 |
| **K3** | M | `ferrocad_core::Kernel` facade: one handle for `documents()`, `object_types()`, `properties()`; free functions stay as wrappers; no new workbench registries | `kernel().documents()` sees `application()` documents; no behaviour change; new workbench code uses the facade | K0 |
| **K4** | XL | Modular-wheel capsule ABI: `ferrocad_core_capi` + a versioned `PyCapsule`; satellite modules borrow the core instead of linking it | A satellite `.so` has no `ferrocad_core` in its link map and can register an object type; a version mismatch raises `ImportError` | Decision below |

### K0 — Observable kernel identity

Add one process-unique value that every module can compare. The cheapest stable
value is the address of the process-wide `Application`:

```rust
// crates/ferrocad_core/src/application.rs
pub fn application_id() -> usize {
    application() as *const Mutex<Application> as usize
}
```

- Export it from `ferrocad_core` and add `_kernel_id` in the `ferrocad`, `Part`,
  `Mesh` and `MeshPart` `#[pymodule]` inits.
- Re-export it from `python/FreeCAD/__init__.py` (`_kernel_id = _fc._kernel_id`).
- Extend the edition's `--check-init` (after it imports its app script) to assert
  equality and exercise a document:

  ```python
  import FreeCAD, Part, Mesh, MeshPart
  assert FreeCAD._kernel_id == Part._kernel_id
  assert Part._kernel_id == Mesh._kernel_id == MeshPart._kernel_id
  name = "__ferrocad_check_init__"
  if FreeCAD.getDocument(name) is not None:
      FreeCAD.closeDocument(name)
  doc = FreeCAD.newDocument(name)
  boxed = doc.addObject("Part::Feature", "Box")
  boxed.Shape = Part.makeBox(1, 1, 1)
  assert boxed.Shape.ShapeType == "Solid"
  FreeCAD.closeDocument(name)
  ```

- Add the identity assertion to `combined.rs`, `part_boot.rs` and
  `tests/test_packaged_modules.py`.

**Files:** `crates/ferrocad_core/src/application.rs` (+ `lib.rs` export),
`crates/ferrocad_py/src/lib.rs`, `crates/ferrocad_part_py/src/lib.rs`,
`crates/ferrocad_mesh_py/src/lib.rs` and `meshpart.rs`,
`crates/ferrocad_parametric/src/main.rs`, `python/FreeCAD/__init__.py`, the three
test files.

**Risk:** `ferrocad_mesh_py` and `ferrocad_part_py` already depend on
`ferrocad_core`; the facade is the only new Python surface. Use a plain integer,
not a Python object, so the value is safe in every interpreter.

### K1 — Mixed-image guard and composition matrix

`ferrocad_combined_py::install()` currently replaces `sys.modules["ferrocad"]`
when it finds a module without its marker. That is right for a host interpreter
and wrong for the edition, which has already registered `ferrocad` (and `Part`)
as built-ins.

- Detect the edition image with the same rule the `FreeCAD` facade uses:
  `importlib.util.find_spec("Part").loader is BuiltinImporter`.
- If `Part` is a built-in, `install()` must not replace anything: return a clear
  `ImportError` telling the caller the edition already owns the modules (or a
  no-op if the four modules are already present and marked).
- Add a unit test for the predicate and a packaging test that the edition's
  `--check-init` runs with the on-disk extension present.
- CI: in the geometry job, after the combined/packaged test, build
  `ferrocad_parametric` and run `--check-init` with the staged `python/` and
  `LD_LIBRARY_PATH` (the same environment the release check uses).

**Acceptance:** the edition boots with the four modules, `--check-init` passes,
and a deliberate `import ferrocad_combined` in that image cannot create a second
core.

### K2 — Interpreter-scoped Python state

The `ferrocad_py` caches hold `Py<...>` values in process-wide `OnceLock`s. That
is safe with one interpreter and unsafe with two: the second interpreter can see
a `PyObject` it does not own.

- Evaluate keying the existing caches by `PyInterpreterState_GetID()` (the smallest
  diff, and it keeps the single-phase `#[pymodule]`) against PEP 3121 multi-phase
  module state (where CPython allocates a per-module struct on each
  `PyModuleObject`). Note: `pyo3::sync::GILOnceCell` / `PyOnceLock` is
  GIL-synchronized but still **process-global**, so it does not isolate
  subinterpreters on its own.
- Split the work: K2a documents/object cache; K2b observers/converters/resolvers;
  K2c a subinterpreter test (skipped when the runtime lacks `_xxsubinterpreters`).
- Keep the Rust-side process state (`Application`, object types, property types,
  the Part backend) where it is; only `Py` objects move.

**Non-goal:** supporting multiple *independent kernels* in one process. The
invariant remains one core; only the interpreter-owned wrappers become scoped.

### K3 — `Kernel` facade

The design note calls the process-wide registries "kernel state"; today they are
three unrelated `OnceLock`s. Add a facade rather than moving state:

```rust
// crates/ferrocad_core/src/kernel.rs (proposal)
pub struct Kernel;

pub fn kernel() -> &'static Kernel;

impl Kernel {
    pub fn documents(&self) -> &'static Mutex<Application>;
    pub fn object_types(&self) -> &'static ObjectTypeRegistry;
    pub fn properties(&self) -> &'static PropertyRegistry;
}
```

- Keep `application()`, `object_registry::*` and `property_types::*` as thin
  wrappers so the migration is non-breaking.
- New workbench code (and the registration hooks) takes/uses `kernel()`; existing
  code can move incrementally.
- Add a test that `kernel().documents()` sees a document created through
  `application()`, and a review rule that a workbench crate does not introduce a
  new `OnceLock<...Registry>`.
- This is a naming/ownership boundary, not a state migration. Moving the
  `OnceLock`s inside `Kernel` is optional later.

### K4 — Modular-wheel capsule ABI (gated)

The published E4 slice is a **bundled** wheel. That wheel can ship
`ferrocad_combined.abi3.so` or register built-ins; it needs no new ABI. The
capsule is only for **separately distributed** wheels (`ferrocad` +
`ferrocad-part`) or out-of-tree Rust workbenches.

- **Decision gate:** do we intend to ship separate wheels? If not, close K4 as
  won't-do and keep the bundled wheel on the combined extension.
- If yes: add `ferrocad_core_capi` (a `#[repr(C)]` vtable, no PyO3, no Rust ABI),
  a `ferrocad_core_py` owner, and a capsule handshake (`abi_version`,
  `struct_size`, `ImportError` on mismatch). Satellite modules must not link
  `ferrocad_core`.
- Acceptance: a satellite `.so` whose `readelf -d`/`otool -L` output has no
  `ferrocad_core` can import the capsule, register `"Vendor::Thing"`, and have a
  document created by `ferrocad` see it; a mock `abi_version = 999` fails cleanly.

## Suggested order

1. **K0** — it is small and makes every later slice measurable.
2. **K1** — close the only remaining path to a mixed image in the app.
3. **K3** — the facade is low-risk and gives K2 a place to live.
4. **K2** — the largest refactor; land it only when a second interpreter is
   actually needed or when a cache bug points at it.
5. **K4** — only after the modular-wheel decision.

## As built

**K0 and K1 landed 2026-10-26.**

- **K0 — identity.** `ferrocad_core::application_id()` returns the address of the
  process-wide `Application`. Every `#[pymodule]` init (`ferrocad`, `Part`, `Mesh`,
  `MeshPart`) adds it as `_kernel_id`; the `FreeCAD` and `FerroCAD` facades
  re-export it. `--check-init` now asserts equality across all four modules and
  creates, fills and closes a `Part::Feature` document; `part_boot`, `combined`
  and `tests/test_packaged_modules.py` assert the same identity in their own
  compositions. The packaged test imports `Part` first so it cannot pick up a
  standalone `ferrocad.abi3.so` as a second core.
- **K1 — mixed-image guard.** `ferrocad_combined_py::install()` first honours its
  idempotence marker, then returns `ImportError` when `Part` resolves to
  `BuiltinImporter` (the edition image) instead of publishing over the built-ins.
  `crates/ferrocad_combined_py/tests/mixed_image.rs` covers the built-in and
  file-module cases. CI's geometry job builds `ferrocad_parametric` and runs
  `--check-init` right after the packaged test, with the on-disk extension present.

K2–K4 remain open; K4 still waits on the modular-wheel decision.
