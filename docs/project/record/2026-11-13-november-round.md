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
| SimpleCAD integrated | **985 passed, 49 failed, 18 errored, 7 skipped, 88/88 modules** | `cargo test -p ferrocad_part_py --test simplecad_conformance`; 5 Qt groups are classified skips |
| Part API audit | **252 of 408 names (61.8%)**, 21 shared classes, 11 with method gaps | `python3 tools/part_api_audit.py`; 39 functions / 60 classes / 40 object types, contract at 17 source files |
| Rust lib tests | **1 454 passed, 0 failed** across the measured crates | 1 178 in 21 crates plus `ferrocad_shell_core` 276/0 |
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
   errored** (the known `PartDesign` absence). SC3 then classifies the 5 Qt groups as
   skips (**985/49/18/7**), and SC7 adds the draw → push/pull → cut → STL acceptance.

The Part API audit's `ours.object_types.files` 16 → 17 drift is benign: the extra source
file is `crates/ferrocad_part/src/extrusion.rs` (2026-11-11), and registered names did not
change. The contract was re-pinned and `--check` passes.

### 2.2 Other check state

- `ferrocad_shell_core` is **276/0**, stable across repeated default and 32-thread runs.
  The two failures were `boot` and `tree` tests hard-coding the old `Shell` startup
  document; the tests now read the active startup document's name and object count, so
  they pass with the checked-in `Sample.FCStd` boot. A parallel-run SIGSEGV observed
  before this fix no longer reproduces; the crash coincided with those panicking tests in
  the shared-interpreter suite.
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
- **SimpleCAD's remaining 49 failures and 18 errors are not closed**, and `S4.4`–`S4.6`
  are still the last façade slices. SC3 and SC7 (the Qt skip classification and the
  draw → push/pull → cut → STL acceptance) are done.
- **The edition was not published.** `v0.1.4` is still the latest tag; the browser shell,
  the host split and the Rust OCCT boundary are unreleased.
- **The image-path cost was instrumented, not removed.** The software fallback still does
  render + readback + swizzle per frame; the timing/`FERROCAD_TRACE` work makes it
  measurable.

## 5. Same-day close-out (2026-11-13)

The round's open P1 tail was closed after the measurements above, on the same tree:

- **SC4–SC6 landed.** `FreeCADGui.ActiveView.getWorkplane`/`getFacePlane` read the
  viewport's published plane and an exact face frame; the selection-armed push/pull stem
  is verified end to end in one transaction; the preview decision keeps
  `FreeCADGui.Preview` and drops 3-D text, while the imperative-Coin lifecycle
  (`getChild`/`insertChild`/`removeChild`/`removeAllChildren`/`findChild`, `isOfType`) is
  complete enough for an overlay.
- **SC8 closed SC0.** All 49 failures are attributed: 16 boolean/topology partition, 5
  document undo/transaction/labels, 10 Part query strictness/tolerance, 6 shape
  identity/ownership/BRep export (37 P2), 11 surface/freeform (P3), and 1 documented
  test assumption (exact `BoundBox`). The full table is in
  [`../research/simplecad-bringup.md`](../research/simplecad-bringup.md) §7.
- **The baseline did not move:** **985 / 49 / 18 / 7 over 88/88**, SC7 green. The
  paragraph above that calls S4.4–S4.6 the last slices is the state at the round's
  start, not the close; **P1 is closed**.
- **P6.2 took the image path off the UI thread.** The GPUI fallback renders into a BGRA
  target (no per-frame RGBA→BGRA swap), and a worker owns the blocking readback wait/map;
  at 2336×1388 the debug swizzle was **368.57 ms** and is now **0.00 ms**, while the
  UI-thread readback cost fell from **27–44 ms** to **0.2–0.5 ms**. The draw itself is
  unchanged. The off-thread polling exposed and fixed a hover-preview bug that could wipe
  a selection's gizmo when a pre-selection preview dissolved.

### 5.1 The manipulator finish pair (after close)

Dragging the Push/Pull stem or the Fillet scrubber applied the edit on release, while the
Move hand kept its `✓`/`✗` step. The asymmetry was real, but it was **not** a lost
feature: the drag/commit path has written immediately since the M0 split, and no earlier
build carried a pending stage for a manipulator. The fix adds the stage the Move form
already had rather than restoring one.

- **The shell holds the commit.** `ViewportEvent::Interaction` no longer routes straight to
  `on_interaction`: a released stem or scrubber parks its `InteractionCommit` in
  `Shell::pending_interaction`, the ghost stays at the pending value, and the status line
  names it (`extrude 3.00 mm pending — ✓ to apply, ✗ to discard`).
- **One decision, one writer.** `✓`/`Enter` spend it through the same `on_interaction` →
  app-script `interaction` hook (one transaction); `✗`/`Esc` drop it and re-arm the handle
  at rest. The pair is the Move form's own strip (`dialog-accept` / `dialog-cancel`),
  placed beside the manipulator's dimension pill instead of the triad.
- **The context gate learned it.** `InteractionContext::PendingInteraction` refuses a pick,
  a hand switch, a tool and editing commands, and the refusal names the held value.
- **Exceptions.** The `+N` workplane offset stem keeps its own HUD and immediate apply
  (transient viewport state, no document object, no undo step); a headless capture
  (`Shell::probe_manipulator`) drives the release **and** `✓`, so the screenshot workflows
  still show the committed solid.
- **Verification.** 4 new `ferrocad_shell_core` shell tests (a release holds; `✓` writes
  once; `✗`/`Esc` write nothing; `Enter` applies; a hand switch is refused). The crate's
  lib suite is **280 passed**; `ferrocad_viewport --lib` **230 passed**;
  `ferrocad_part_gui` **30 + 1 doctest**.

**The follow-up the pending stage made visible (same day).** Holding a release exposed
three gaps that the immediate write had hidden:

- **The ghost vanished on release.** The viewport drew a manipulator's `preview_items`
  only while `is_dragging()`, so the value under decision disappeared the moment it became
  a decision. `Manipulator::is_pending` (new, default `false`) keeps the preview drawn
  while the host holds the commit; the stem and the scrubber report it from `end_drag`.
- **The push/pull ghost was a floating plane.** It is now the face **swept** to the pending
  depth — the cap at the offset plus the walls its border traces (`border_edges`, by
  position, because the tessellation gives each triangle its own vertices) — so the volume
  and the direction are visible without a kernel call. `ferrocad_part_gui`
  **33 passed**: the prism's vertex/triangle counts, the ghost surviving the release, and
  the scrubber's swath surviving its own.
- **A second drag restarted at zero.** `PushPullStem::drag_to` assigned the solver's
  per-drag travel straight to the distance, so grabbing a held 3 mm stem and pulling reset
  the value. It now adds the distance at press, which makes the second (and later) drags a
  continuation; the first drag is unchanged because its press distance is 0. Adjusting is
  also covered on the shell side: a second release replaces the held commit
  (`a_second_release_replaces_the_held_commit`), so `✓` applies the latest value.
- **Result.** `ferrocad_shell_core` **281**, `ferrocad_viewport` **231**, `ferrocad_part_gui`
  **33 + 1 doctest**, and the edition's `push_pull_round_trip` / `manipulator_round_trip`
  still green.

### 5.2 The manipulator's numeric twin

The same session also closed the gap that holding a value exposed: a released stem could
only be adjusted by dragging it again. The twin is the same pending edit typed instead.

- **The seam is three defaulted methods on `Manipulator`.** `numeric_fields()` describes
  the numbers (`NumericField { label, unit, value, decimals }`, in the commit's order),
  `set_values()` adopts typed ones (clamping and snapping with the manipulator's own
  rules) and `pending_commit()` is the commit a release would have produced. The host
  never interprets a number, so a mod's declared handle can join later.
- **The row is a view.** `shell::manipulator_form` builds one `TextInput` per field,
  reconciles it with the viewport's topmost numeric manipulator on every paint, and
  mirrors values only into **unfocused** fields — so a drag ticks the numbers and typing
  is never rewritten under the caret. It docks where the Move form does and appears as
  soon as the handle is armed, so a value can be typed with no drag at all. `Tab` opens a
  field, `Enter` applies, `Esc` rolls back, a non-slot digit seeds it, and `0`–`3` still
  switch hands.
- **One writer.** A typed value goes `set_values` → `pending_commit` → the same
  `hold_interaction` a release uses, so the ghost, the readout, the `✗`/`✓` pair and the
  status line all describe it, and `✓` spends exactly one transaction.
- **Zero is not an edit.** A stem typed back to `0` clears the held commit instead of
  offering a zero-length extrusion; the ghost goes with it.
- **Measured.** The four new tests are `ferrocad_part_gui`'s
  `the_numeric_twin_sets_the_distance_and_commits_it` and
  `the_numeric_twin_sets_the_radius_and_the_chamfer_distances` (suite **35**),
  `ferrocad_viewport`'s `a_typed_value_reaches_the_manipulator_and_its_commit` (suite
  **232**), and `ferrocad_shell_core`'s `the_manipulator_numeric_row_types_a_held_value`
  (suite **286**, which also checks that `Esc` restores the field to the re-armed
  handle's value). The suite totals are measured on a tree that also carries the in-flight
  geometry-sharing work; three of *its* viewport tests (incremental stage bounds and the
  transform-only packet patch) fail there and are not this change.
- **Still open.** Model-aware bounds (a pocket deeper than the body, a radius wider than
  the edge) need the kernel, and an existing `Part::Extrusion`/`Part::Fillet` is not yet
  re-openable as a live handle.

### 5.3 The handle rides the ghost

The last of the session's three follow-ups was spatial: the stem stayed anchored at the
face centroid while the ghost swept outward, so the handle sat behind the surface being
dragged.

- **The stem's base is the ghost's centroid** (`centroid + N·distance`), and its tip,
  hit test, readout anchor and screen scale all build from it. Since the stem is a fixed
  screen length past the ghost, the pointer's travel and the ghost's travel cancel — the
  handle stays under the pointer while a drag runs, which is the Move triad's rule applied
  to a sweep.
- **The solver is untouched.** It still measures from its press frame at the face; a delta
  along the axis is the same wherever it is measured, so the value mapping did not change.
- **`set_anchor` moves the sweep**, not the ghost: the host's "this object is previewed
  elsewhere" hook translates the face and the handle by one delta and leaves the distance
  alone. Without that, a script preview would have written the ghost point into the face
  origin.
- **Covered** by `the_stem_rides_the_ghost` (typed, dragged, and the `set_anchor`
  round-trip) and `a_drag_keeps_the_handle_with_the_pointer`; `ferrocad_part_gui` is **37
  passed**, and the edition's `push_pull_round_trip` still commits the same distance.

### 5.4 Bounds the geometry can actually vouch for

"Min/max that won't break the model" has two halves. This slice is the half that needs
no kernel; the kernel half followed in §5.5.

- **`NumericField` grew `min`/`max`/`hint`**, and the distinction is the feature: `min` and
  `max` are **hard** (the manipulator clamps to them, on a drag or a typed value, and the
  row prints the range), while `hint` is a **note** that must not clamp.
- **The dress-up scrub was given a conservative ceiling — since removed.**
  `dress_up_ceiling` was half the body's smallest extent, applied by the factory through
  `set_bounds(MIN_RADIUS, ceiling)`. It was wrong (see §5.5): a single edge takes a fillet
  far past half the thin extent, so the stage now publishes only the floor it can prove
  and the ceiling comes from the kernel.
- **A push/pull distance publishes no hard bound, on purpose.** An extrusion is unbounded
  and a cut deeper than the body is a through-cut — still a solid. What the stem can
  measure is the body's thickness **behind** the face, which is published as the hint
  (`body 8.00 mm behind this face`): it says where a pocket stops being a pocket without
  forbidding the through-cut.
- **A clamp never leaves the row lying.** When a hard bound changes a typed value the shell
  rewrites the focused field to what the manipulator took (`40` → `10.00`), and the status
  line already names the accepted number; an in-range value is left exactly as typed so a
  caret is never moved mid-word. The scrubber publishes three decimals, because its floor
  is `0.001` and two would print the bound as `0.00`.
- **Covered** by `the_numeric_twin_reports_the_body_behind_the_face`,
  `the_scrubber_publishes_its_floor_and_takes_the_kernel_ceiling` and the shell's
  `the_manipulator_numeric_row_types_a_held_value` (range chip, clamp, rewrite).
  `ferrocad_part_gui` is **39 passed**; `ferrocad_viewport` **235** and
  `ferrocad_shell_core` **286** stay green, as do both edition round-trips.

**The second half (§5.5) landed the same day**, and building it exposed the ceiling above
as a false bound.

### 5.5 The kernel-backed range, and a ceiling that was wrong

The "second half" of item 4 is built, and building it proved the first half's ceiling was
a mistake.

- **`dress_up_ceiling` is gone.** Half the body's smallest extent is not a bound on one
  edge's fillet: a 10 mm cube's `Edge1` completes at ~9.92 mm, and a `30×30×3` box takes
  29.625 mm on that edge — both far past half the thin extent. The stage still publishes the
  floor it can prove (`MIN_RADIUS`) and nothing more; the factory arms with
  `set_bounds(MIN_RADIUS, None)`.
- **The kernel answers the ceiling.** `ScriptHost::manipulator_probe(object, element,
  manipulator)` is a defaulted seam with four answers: `None` (no opinion), `Ok(None)`
  (asked, no bound), `Ok(Some(probe))` (`{values, min, max, note}`), and `Err` (a provider
  failed; reported once and treated as no opinion). `HostChain` routes it with
  `every().find_map(…)`, so the first host with an answer wins.
- **The edition's provider is a real search.** `ferrocad_parametric` registers
  `FerroCAD.register_manipulator_probe`, which asks providers in registration order and
  skips a raising one. The fillet probe grows a guess (a body extent / 8) by doubling
  until `Shape.makeFillet` fails, then bisects six times. A result that comes back
  **invalid without raising** counts as a failure, because OCC returns those. The floor is
  `1e-3`, the cap is twice the body's largest extent, and if nothing fails below the cap
  the probe reports **no ceiling** rather than inventing one.
- **The probe is deferred, then cached.** `sync_manipulator_probe` runs after the numeric
  row syncs; a request waits one executor tick plus 120 ms — so a hand switch, a
  re-selection or a commit collapses into one probe — and then runs on the **UI thread**,
  because the document half is not safe to enter from a worker. The answer is cached per
  `(object, element, manipulator)`, **including “no opinion”**, and cleared on any document
  edit or replacement.
- **A late answer cannot rewrite the wrong handle.** It is dropped if the armed handle
  changed; otherwise the range goes into the viewport, a held commit the range clamped is
  re-read so `✓` applies the allowed value, and only a field whose text disagrees is
  rewritten.
- **What stays unbounded.** A push/pull distance: the provider declares no opinion for it,
  because an extrusion has no ceiling and a deeper cut is a through-cut. The editable
  surface's own floor remains the only hard minimum.
- **Measured.** `ferrocad_parametric --test manipulator_probe` **2 passed** (the reported
  max succeeds and 1.25× it fails; a push/pull question gets no bound), the provider
  registry `tests/test_manipulator_probe.py` **5 passed**, and the shell test installs a
  provider returning `max: 4.0` and asserts the row, the re-held commit (`4.0`) and the
  rewritten text. `ferrocad_shell_core` **286**, `ferrocad_viewport` **235**,
  `ferrocad_part_gui` **39**, and both edition round-trips green.

Two workbench tests that assumed `bootstrap(name)` names the document were made
name-agnostic — the startup asset names it — which restores the extension-set contract
check (`run_tests.py --check`: 49 modules, 42 base and 7 workbenches).

### 5.6 Changing what is already there (item 5)

A second gesture on a face an earlier operation produced used to stack another feature over
the body. The probe now also carries the value the selection **already** carries, and the
commit edits that feature.

- **The probe is `{values, min, max, note}`.** `ManipulatorProbe` grew `values`, which the
  shell seeds into the armed handle before it applies the range. `Manipulator::seed_values`
  is defaulted `false`; `PushPullStem::seed_distance` moves the stem to the existing cap
  **without** making it pending, and `DressUpScrubber::seed_values` sets the radius. A seed
  holds nothing for `✓` (the shell skips it while a commit is held), so opening an existing
  operation is not itself an edit.
- **Which operation is identified geometrically.** A push/pull cap is the extrusion's base
  face translated by the signed length along its normal (`_is_extrusion_cap`); a fillet's
  round is the cylinder of the feature's own `Radius` whose axis parallels a filleted edge
  (`_dress_up_target`). Neither check needs a name, so this works on a document saved
  before the kernel recorded provenance.
- **The commit edits the feature.** `_edit_extrusion` reuses the `Part::Extrusion` (and the
  boolean while the sign stays); crossing zero builds the other boolean over that same
  extrusion and retires the old one, so one face still has one operation. `_edit_dress_up`
  sets `Radius`/`Distance` on the picked feature. Both are one transaction.
- **A fillet's round is a face, and the scrubber needs an edge.** The edition declares an
  axis grip through `manipulator_specs` instead: at the face centroid, along its normal,
  labelled `R`, seeded with the radius and speaking the same `dress_up`/`fillet` commit.
  Declared handles gained the numeric twin (`label` on the wire, `numeric_fields`,
  `set_values`, `seed_values`, `pending_commit`), so a mod-declared handle types and
  clamps like a native one.
- **The Part layer had to make names mean what they say.** `Part::Fillet`/`Chamfer` used to
  drop the kernel's history (their map was positional-and-self); they now keep it.
  `complete_history` adds identity relations for the elements a dress-up did not report, so
  a partial map no longer makes every other name look retired. And
  `GeometryBackend::element_kind` lets `resolve_element_in` see that a map entry points at
  a different **kind** — a fillet records the consumed edge under the round it generated —
  falling back to the shape's own naming rather than handing the kernel a face where an
  edge was asked for.
- **Measured.** `ferrocad_parametric --test manipulator_probe` **3 passed** (the bisection,
  the unbounded push/pull, and the re-open: a fillet face edited in place, an extrusion cap
  seeded at 5 and edited to 7 with the object count unchanged, then flipped to a pocket
  that reuses the extrusion), the shell's new
  `a_probe_seeds_the_handle_with_the_existing_value` (seeded value shown, nothing held),
  and `ferrocad_part`'s `a_fillet_keeps_the_provenance_of_the_face_it_creates`.
  `ferrocad_shell_core` is **288**, `ferrocad_part` **55**, `ferrocad_part_gui` **39**,
  `ferrocad_viewport` **235**, `ferrocad_stage` **259**, `ferrocad_shell_rust` **58**, and
  the three edition round-trips green.
- **The boot sample is honest about one thing.** Its persisted element map predates the
  history fix, so the kernel range for the sample's existing fillet appears only after the
  first recompute (the seed and the grip work immediately, because they are geometric). The
  file is a build input that is not itself versioned yet; regenerating it gives it the
  provenance-carrying map.

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
