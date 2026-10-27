# Task — kernel state sharing

Status: closed task spec (moved to `record/tasks/` on 2026-10-27). Original status: **closed / steady-state** (2026-10-26; K2a landed 2026-10-27). **K0, K1,
K3 and K2a are implemented and green** (`beca5fb`, `9869d76`, `05cf713`). **K2b**
turned out to need nothing (the converter/resolver/provider tables hold no `Py`
values), and **K2c** (bucket reclamation, multi-phase module state) is parked.
**K4** (modular-wheel `PyCapsule` ABI) is gated out unless separate per-workbench
wheels are scheduled.

Design authority: [`../architecture/kernel-state-sharing.md`](../../architecture/kernel-state-sharing.md).
Backlog: [`TODO-26-10-2026.md`](../TODO-26-10-2026.md) §0.4 / P6.

## Slice summary

| Slice | Work | Status |
| --- | --- | --- |
| **K0** | Observable kernel identity (`_kernel_id` across `ferrocad`, `Part`, `Mesh`, `MeshPart`, `FreeCAD`, `FerroCAD`) + `--check-init` document/`Part::Feature` round-trip | **Done** (`beca5fb`) |
| **K1** | Mixed-image guard (`install()` refuses to overwrite `BuiltinImporter` modules) + packaged/edition CI gates | **Done** (`9869d76`) |
| **K3** | `ferrocad_core::kernel()` facade over `application()`, `object_registry` and `property_types`, with unit tests | **Done** (`05cf713`) |
| **K2** | Interpreter-scoped Python state in `ferrocad_py`: the `Py`-owning tables (`documents`, `objects`, `observers`) keyed by `PyInterpreterState_GetID()` | **K2a done**; K2b had no `Py` state; K2c (reclamation, PEP 3121) parked |
| **K4** | Modular-wheel `PyCapsule` ABI (`ferrocad_core_capi`) | **Parked** (not needed while E4 uses `ferrocad_combined`) |

## As built (2026-10-26)

- **K0 — identity.** `ferrocad_core::application_id()` returns the address of the
  process-wide `Application`. Every `#[pymodule]` init (`ferrocad`, `Part`, `Mesh`,
  `MeshPart`) adds it as `_kernel_id`; the `FreeCAD` and `FerroCAD` facades re-export
  it. `--check-init` asserts equality across all four modules and creates, fills and
  closes a `Part::Feature` document; `part_boot`, `combined` and
  `tests/test_packaged_modules.py` assert the same identity in their own
  compositions. The packaged test imports `Part` first so it cannot pick up a
  standalone `ferrocad.abi3.so` as a second core.
- **K1 — mixed-image guard.** `ferrocad_combined_py::install()` first honours its
  `__ferrocad_combined__` idempotence marker, then returns `ImportError` when `Part`
  resolves to `BuiltinImporter` (the edition image) instead of publishing over the
  built-ins. `crates/ferrocad_combined_py/tests/mixed_image.rs` covers the built-in
  and file-module cases. CI's geometry job builds `ferrocad_parametric` and runs
  `--check-init` right after the packaged test, with the on-disk extension present.
- **K3 — `Kernel` facade (`05cf713`).** `ferrocad_core::kernel()` provides a single
  `Kernel` handle (`documents()`, `object_types()`, `properties()`,
  `property_extensions()`) over the existing `OnceLock` registries while preserving
  the free functions for compatibility. `src/tests/kernel.rs` asserts that a
  document, object type, property type and restore hook registered through the free
  functions are visible through the facade. The `ferrocad_core` suite passes 110/110.
- **K2a — interpreter-scoped caches (2026-10-27).** `ferrocad_py` keeps one
  `InterpreterState` bucket per `PyInterpreterState_GetID()` for the three tables that
  own `Py` values (`documents`, `objects`, `observers`). The converter/resolver/
  provider/method tables hold no `Py` and stay process-wide (documented in the
  module). A test asserts the bucket key equals CPython's reported interpreter id and
  that a second interpreter cannot see the first's document; CPython refuses the
  single-phase `#[pymodule]` in a subinterpreter today, which the test records.

## Deferred

- **K2c.** Buckets are not reclaimed when an interpreter is destroyed (safe, but grows
  with many subinterpreters), and true multi-interpreter module support needs PEP 3121
  multi-phase state in PyO3. Neither is a current product requirement.
- **K4** is a decision gate, not an implementation backlog item: the bundled wheel
  does not need a capsule, and no separate per-workbench wheels are scheduled.
