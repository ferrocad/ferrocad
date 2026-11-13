# The November round — 2026-11-01 … 2026-11-12

Status: round record (written 2026-11-13). This is the compact measurement and what
changed while the 2026-10-31 backlog was open. The backlog itself is
[`TODO-31-10-2026.md`](TODO-31-10-2026.md); the successor is
[`../TODO-13-11-2026.md`](../TODO-13-11-2026.md). The task-level detail stays in the
specs under [`../tasks/`](../tasks/README.md) and the design authority in
[`../../developer/architecture/host-backends.md`](../../developer/architecture/host-backends.md).

## 1. What the round established

| Date | Work | What it changed |
| --- | --- | --- |
| 2026-11-01 | **The OCCT boundary is Rust** | `ferrocad_part_cxx` is gone: `ShapeFix`, `ChFi2d`, topology, boolean history and the geometry helpers are Rust over `ferrocad_occt_cxx`'s sys bindings. `ferrocad_occt` contains no C++; the remaining C++ is the published sys crate. |
| 2026-11-02/03 | **The GPUI wasm seam** | A GPUI + wgpu + WebGL2 window draws a real `Part` document in the browser, and the prebuilt OCCT kernel links into the wasm module. |
| 2026-11-04 | **The browser shell (M0–M6)** | The shell is split into `ferrocad_shell_api` / `_core` / `_python` + the browser host; the real shell runs in Chrome with a Rust backend, a live 3D viewport, a `Part::Box` + `Part::Fillet` document, editing, `.FCStd` open/save and a headless smoke check. |
| 2026-11-04/05 | **Hosts and backends (S1–S8)** | `ScriptHost`, the host `FilePicker`, `ferrocad_shell_rust`, one display path, shared property presentation, the command catalogue as data, and the two script halves (`JsScriptHost`, `PywasmScriptHost`) over `RustAppBackend`. |
| 2026-11-06/08 | **Stage, timing and interaction polish** | The origin is a transform rather than baked geometry, a move rewrites positions, the presenter/frame timing is measured, and the drag asks for the next frame. |
| 2026-11-08/09 | **Two more runtimes and the mod chain** | `rquickjs` on the desktop and RustPython on the worker; one `Runtime` and an ordered host chain; wasm workbench bundles and the `ferrocad_addon` ABI. |
| 2026-11-10/11 | **UI surface and documentation** | UI quickstart, interaction examples, controls/tool figures and GIFs; the icon catalogue and a curated icon set; fetched mod packs in the browser; grid settings without an interpreter. |
| 2026-11-12 | **Backend parity closes (B0–B21)** | One `AppBackend` (`RustAppBackend`) plus a `ScriptHost`; every shown command is a real `FreeCADGui` command; the RustPython half runs the shared `python/` façade, writes documents, builds `Part`/`Mesh`, loads forms and draws a Coin scene; packs carry their `Resources/`. |

The structural result: there is **one document implementation, four runtime configurations**
(CPython, RustPython, JS, none), and the browser and desktop install the same
`RustAppBackend`. The Part engine, the shell and the Python façade no longer have separate
copies per platform.

## 2. Measurements at close (2026-11-13)

Upstream `../freecad-upstream` @ `33adf61`; SimpleCAD `../simplecad` @ `9180c4b` (v0.38.0).
The Part run is in-process from the current tree. The SimpleCAD parent runs in-process; its
isolated `shape_worker.py` children import the packaged `python/` extension, so that
number also depends on the built combined extension. The Rust rows are lib tests only.

| Measure | Value | Command / note |
| --- | --- | --- |
| Part conformance | **531 passed, 0 failed, 0 errored, 12 skipped, 0 load failures** | `FERROCAD_UPSTREAM=… cargo test -p ferrocad_part_py --test part_conformance -- --nocapture` |
| Draft conformance | **50 passed, 0 failed, 0 errored, 1 skipped, 0 load failures** | same harness with `--test draft_conformance`; the skip is the absent BIM module |
| Draft M2 | **27 passed, 0 failed, 0 errored, 0 load failures** | `test_creation` 26 + `test_array` 1 |
| SimpleCAD integrated | **985 passed, 49 failed, 23 errored, 2 skipped, 88/88 modules** | `cargo test -p ferrocad_part_py --test simplecad_conformance` |
| Part API audit | **252 of 408 names (61.8%)**, 21 shared classes, 11 with method gaps | `python3 tools/part_api_audit.py`; 39 functions / 60 classes / 40 object types, contract at 17 source files |
| Rust lib tests | **1 452 passed, 2 failed** across the measured crates | 1 178 in 21 crates plus `ferrocad_shell_core` 274/2 (serial) |
| Book | **48 chapters** | `python3 tools/build_book.py --check` |

### 2.1 One regression and one harness gap (both fixed 2026-11-13)

1. **Part 531 → 526 → 531.** The five `ColorPerFaceTest` cases pass
   `ViewObject.RootNode` to the **system** `pivy.coin` `SoSearchAction`. B20 changed
   `RootNode` from nodes constructed through the imported `pivy.coin` to
   `ferrocad_inventor::view_root` adapted by `ferrocad_inventor_py::to_py`, so the system
   action rejected the node with `TypeError: Wrong number or type of arguments for
   overloaded function 'SoAction_apply'`. Fixed by building the root through the imported
   module (`view_root_via_coin`) while keeping `view_root` as the single tree body; Part is
   back to **531/0/0/12**.
2. **SimpleCAD 985/49/23 → 975/49/33 → 985/49/23.** Not a kernel regression. SimpleCAD
   runs offset/surface operations in an isolated `scripts/shape_worker.py` **subprocess**;
   the in-process harness registered `FreeCAD`/`Part` as built-ins but never exported
   `PYTHONPATH`/`LD_LIBRARY_PATH`, so every worker exited 1 with
   `ModuleNotFoundError: FreeCAD`, which SimpleCAD reports as
   `The kernel stopped unexpectedly on this geometry (code 1)`. The test now exports the
   repo `python/` tree and `.occt/prefix/lib`; `test_surface_ops` is **15 passed / 1
   errored** (the known `PartDesign` absence).

The Part API audit's `ours.object_types.files` 16 → 17 drift is benign: the extra source
file is `crates/ferrocad_part/src/extrusion.rs` (2026-11-11), and registered names did not
change. The contract was re-pinned and `--check` passes.

### 2.2 Other check state

- `ferrocad_shell_core` lib tests pass **274/2** when run serially. The default parallel run
  ends in a **SIGSEGV** after the tests, because several tests share the process-wide
  document registry; the two serial failures are
  `shell::tests::boot::shell_boots_lists_the_sample_document_and_is_renderable` and
  `shell::tests::tree::a_document_row_switches_the_shell` (a `Sample` document where the
  test expects `Shell`).
- `python3 tools/api_docs.py --check` (9 pages), `tools/userdocs.py --check` (8 pages, 6
  generated images) and `tools/build_book.py --check` (48 chapters) are all current after
  the 2026-11-13 regeneration.
- `/home` is **89 % full with 77 GB free**; `target/` is 19 GB. The old 99 %/10 GB warning
  no longer applies, but the build cap is still worth keeping.

## 3. What this round did not do

- **Draft breadth did not move.** The `--draft` group is still 50/0/0/1; `Import.readDXF`,
  `TechDraw::DrawPage` and the `test_draftgeomutils` reproduction are unchanged.
- **The Sketcher still has no constraint model and no solver.** PlanGCS is still only
  vendored upstream; `SK1` has not started.
- **SimpleCAD's remaining 49 failures and 23 errors are not closed**, and `S4.4`–`S4.6`
  are still the last façade slices.
- **The edition was not published.** `v0.1.4` is still the latest tag; the browser shell,
  the host split and the Rust OCCT boundary are unreleased.
- **The image-path cost was instrumented, not removed.** The software fallback still does
  render + readback + swizzle per frame; the timing/`FERROCAD_TRACE` work makes it
  measurable.

## 4. Where the detail lives

- [`tasks/backend-parity.md`](tasks/backend-parity.md) — the B0–B21 ledger, its
  close-out (§9), and the deferrals table (§7).
- [`tasks/host-backends.md`](tasks/host-backends.md) — S1–S8, the four runtimes and
  the decisions D1–D11.
- [`tasks/wasm-shell.md`](tasks/wasm-shell.md) — M0–M6, the browser shell.
- [`tasks/transform-session.md`](tasks/transform-session.md) and
  [`tasks/tool-loadout.md`](tasks/tool-loadout.md) — the interaction work whose
  A1–A3/U1–U6 stages landed on 2026-10-31.
- [`../tasks/opening-a-large-model.md`](../tasks/opening-a-large-model.md) — the measured
  open/sync budget and the remaining stages.
- [`../../developer/architecture/host-backends.md`](../../developer/architecture/host-backends.md)
  — the stable architecture the round produced.
