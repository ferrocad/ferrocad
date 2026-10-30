# Workplanes: one model, three ways in

Status: task spec (2026-10-29). **Built**: the state model, the three tiers' entry points,
`Pick 3 points`, `Save as datum plane` and the visual hierarchy are in the tree (§7); the
four slices and their remaining gaps are recorded in §8.
Companion to
[`../../developer/interaction/selection-gizmos-and-forms.md`](../../developer/interaction/selection-gizmos-and-forms.md)
(the context wheel, §2.2), [`../../developer/interaction/gizmo-manipulators.md`](../../developer/interaction/gizmo-manipulators.md)
(frames and the triad), [`../../developer/interaction/visual-language.md`](../../developer/interaction/visual-language.md)
(the accent/glass tokens), [`polyline-vertex-editing-and-active-tool-hud.md`](polyline-vertex-editing-and-active-tool-hud.md)
(the active-action surface this shares a slot with) and
[`../TODO-31-10-2026.md`](../TODO-31-10-2026.md) P4 SK4.

The workplane should be **invisible when it is the world plane and obvious when it is
not**. Today it is the reverse: the default state is unrepresented, the only way to
change it is a menu that first requires a face under the pointer, and the tools that
draw on it say nothing about which plane they are using.

## 0. What the tree has today (verified 2026-10-29)

The math and most of the rendering are already there; the state model and the entry
points are not.

- **The plane type is complete.** `ferrocad_stage::Workplane { origin, u, v, n, kind }`
  with `Workplane::world()` (`origin = 0`, `u = +X`, `v = +Y`, `n = +Z`),
  `Workplane::new(origin, u, v, kind)` (normalized, `n = u × v`), and
  `Workplane::ray_local(origin, direction) -> Option<[u, v]>` — the ray/plane hit the
  tools need (`crates/ferrocad_stage/src/workplane.rs`).
- **`WorkplaneKind` already distinguishes the three sources:** `World`, `Datum(String)`,
  `Face { entity, element }` (`workplane.rs:19`). Only `World` and `Face` are ever
  produced; `Datum` appears in tests and the Python binding, because there is no datum
  plane object in the document yet.
- **The grid is one generator, parameterised by a plane.** `GridSpec { workplane, minor,
  major, radius }` (`crates/ferrocad_stage/src/grid.rs:85`); `GridSpec::world(radius)` is
  just the same struct with `Workplane::world()`. Every vertex is emitted through
  `spec.workplane.point(a, b)`, so a bound plane's grid is the *same* mesh generator and
  the *same* shader, rotated and offset — not a second “local patch” renderer.
- **It is a camera-following disc, not an infinite plane.** `radius` is
  `2.5 × eye distance`, clamped to `RADIUS_RANGE` (`GridSpec::radius_for`), for the world
  and for a bound plane alike. “Infinite” is the illusion the radius rule produces.
- **The ghost triad already exists.** With a plane active the world ground is dimmed to
  `WORLD_DIM = 0.25` and the world triad to `WORLD_TRIAD_DIM = 0.30` (`grid.rs:162–165`) —
  i.e. the “muted world-origin anchor” is already the shipped behaviour, not a new idea.
- **The style differs, not the geometry.** `GridStyle::workplane()` uses 1 mm minor dots
  and 10 mm majors, while the world uses 10 mm / 100 mm (`grid.rs:22–25,149`). That is the
  only reason a bound grid *looks* unlike the origin grid.
- **The viewport holds `Option<FaceFrame>`, not a plane.** `set_workplane(Option<FaceFrame>)`
  is documented as **sticky** ("only replaced by another activation or cleared by `Esc`"),
  with `activate_hovered_workplane()`, `clear_workplane()` and `view_workplane()`
  (`inventor_view.rs`). `None` means "world", and the grid, axes, origin node and snap
  context all fall back to `Workplane::world()` — so the *rendering* already treats the
  world plane as active; only the state has no name for it.
- **Hover previews already exist.** A hovered planar face gets a local grid after
  `FACE_DWELL` ("the bloom arrives without pointer motion"), and `snap_context()` uses
  `frame.plane` when a workplane is active, else `snap_grid.map(SnapContext::world)`.
- **The dropdown is the command registry.** `WP` is a `split_button` with **no provider
  rows** (`chrome.rs:378`); the menu is filled by the three Python commands whose
  `FerroCAD_Surface` is `"workplane"`: `App_ActivateWorkplane` (`activate`),
  `App_ClearWorkplane` (`clear`) and `App_ViewWorkplane` (`view`)
  (`python/ferrocad_shell/__init__.py`). A `ViewRequest` carries exactly those three
  verbs and rejects anything else (`crates/ferrocad_gpui/src/view.rs`).
- **The status string is a command result, not state.** `"no active workplane"` is the
  status-line message for `Clear`/`View` when nothing is active
  (`crates/ferrocad_gpui/src/shell/dispatch.rs:498–510`) — it is not a persistent readout.
  The persistent caption is the static `WP` label, which never says *which* plane.
- **Frames and the gizmo are disconnected.** `ManipulatorFrame { origin, basis }` and
  `TransformGizmo::set_frame` exist (`crates/ferrocad_stage/src/gizmo.rs`), and
  manipulators use them (`push_pull` sets a frame from a face centroid and normal), but
  nothing maps the active workplane onto the gizmo.
- **Python can build a plane but not activate one.** `ferrocad_stage.Workplane` /
  `WorkplaneKind` are exposed with `(u, v)` ↔ world mapping
  (`crates/ferrocad_stage_py/src/workplane.rs`), so the *math* is scriptable today. The
  viewport's active plane is only reachable through the three view verbs above; there is
  no "set this frame" call, and `FreeCADGui`'s only `setWorkplane` is Draft's own
  `WorkingPlane` state (`python/FreeCADGui/draft.py:509`).

## 1. The state problem

`Option<FaceFrame>` has three meanings packed into two values:

| Intent | Today |
| --- | --- |
| "the world XY plane is active" (the default, and what the grid draws) | `None` |
| "a face is active" | `Some(frame)` |
| "no plane at all" | `None` — indistinguishable from the world |

Because there is no value for "world", the UI cannot say `WP: XY`, the `WP` pill cannot
show a dot, and `Clear` has to print a denial when it is already at the default. The fix
is a state change, not copy: make the world plane a first-class active plane and give
`Datum` and a custom frame somewhere to live.

## 2. Three ways in

### 2.1 Tier 1 — ephemeral plane during a creation tool

**Landed (2026-10-29, see §7).** A creation tool that starts on a planar face binds that
face for the session, and the plane it started from is restored when the tool ends:

- While a creation tool is active and the pointer is over a **planar face**, the hovered
  frame becomes the tool's provisional plane (the grid already blooms there).
- The **first press** on that face binds the plane *for the session*; later clicks in
  empty space project onto that frame rather than onto world XY. Only the first press
  binds — a later click on a different face must not slide the plane out from under the
  points already placed.
- Finishing or cancelling the tool returns the active plane to what it was before the
  session (usually world), unless the user bound it explicitly in Tier 2. An explicit
  binding is sticky and a click never overrides it.
- The tool's `SnapContext` already carries a `Workplane`; this tier chooses it once per
  session rather than reading the viewport's global one for every event.

The binding is **per tool session** (not per click): one plane for the whole shape, and
`Polyline` finishing returns the workplane to World XY.

### 2.2 Tier 2 — explicit binding from the context wheel

**Landed (2026-10-29, §7.4).** `Set WP` is a wheel verb for the selected planar face:

- `App_SetWorkplane` (`Set from selection` in the `WP` menu, `Set WP` in the wheel) calls
  `set_workplane_from_element(entity, element)`, which resolves the face's own centroid
  and normal rather than the click point, then `set_workplane(Some(frame))`.
- The binding is **sticky**: it survives pointer moves and empty-space clicks.
- The grid, axes and origin node move to the frame (they already render at the active
  plane; the animation is presentation).
- **Align camera to WP** is `App_ViewWorkplane` → `view_workplane()`, which looks down the
  normal with a roll-free up vector.

### 2.3 Tier 3 — the `WP` dropdown

**The datum rows and the creating rows both landed (2026-10-29, §7.3, §8).**
The menu is the plane choice, not the plumbing:

```text
[ • WP: XY ▾ ]
  World XY (Top) · World XZ (Front) · World YZ (Side) · View plane (Camera)
  ─────────────────────────────────────────────────────────────
  Set from selection · Align camera to WP
  Offset workplane… · Pick 3 points…
```

`Save as datum plane` joins the same group when the Part workbench registers it (it is
the Part module's command, not the shell's).

- The three world planes are `Workplane::standard(StandardPlane)` frames with
  `WorkplaneKind::World`; the label (`XY`/`XZ`/`YZ`) comes from the plane, and only `XY`
  is the default.
- **View plane** is the camera's own basis (`u` = screen right, `v` = screen up, `n`
  toward the viewer) through the point the camera looks at, with `WorkplaneKind::Custom`
  and the label `View`.
- **Set from selection** is Tier 2's command (it lives in the menu too).
- `Activate workplane` and `Clear workplane` stay **callable** (`FerroCAD_Surface:
  "hidden"`) but are no longer menu rows: a menu click moves the pointer off the face, so
  the hover verb could only fail there, and `World XY (Top)` already is the reset.
- **Pick 3 points… — built.** A native `workplane_points` tool: left-click plots a
  point, Backspace drops the last one, Enter makes the picked triangle the plane.
  `Workplane::new(origin, p1-p0, p2-p0)` rejects degenerate input, so a collinear pick
  leaves the active plane alone.
- **Offset workplane… — built.** It moves the frame along `n` with the 1D normal stem and
  the floating HUD (`Offset ΔN [ 0.0 ] mm`), two-way and one session per window.

### 2.4 The grid: one generator, no patch

**Yes — keep one grid, and the tree already does.** A bound workplane reuses
`GridSpec`/`GridStyle` and the same mesh generator; it is not a second overlay. Three
decisions remain, and none of them needs a new renderer:

1. **Radius rule.** Use `GridSpec::radius_for(camera)` for a bound plane exactly as for the
   world, so the two are indistinguishable apart from the transform. No clipped patch
   around the face.
2. **Step size.** Today `GridStyle::workplane()` switches to 1 mm/10 mm while the world
   stays at 10 mm/100 mm — that is what makes a bound grid look like a different object.
   Either keep the world steps (truly identical), or keep the finer steps *and* say so in
   the pill (`WP: Face3 · 1 mm`). Preference: keep the world steps, and let zoom reveal
   detail through the existing `MINOR_PIXELS` decade rule. **Superseded:** the shipped
   rule is `FerroCAD.Grid.adaptive` — the step fits the extent the layer covers (a face
   patch, or the ground disc), so the two grids agree in density without a pill label.
   See [`../../developer/interaction/workplanes.md`](../../developer/interaction/workplanes.md) §9.
3. **The world layer.** Do **not** keep the ground: it would be a second grid. The world
   ground is not drawn while a non-default plane is active; the triad stays, dimmed
   (`WORLD_TRIAD_DIM = 0.30`), with the origin node, so absolute zero is still locatable.
   (`WORLD_DIM` survives only for the hover preview, where the world plane is still the
   **active** plane and the preview is a hint on top of it.)

Because the grid is a spec, switching planes is a state swap, not a mode. If the swap is
animated (120 ms, per the brief), animate the spec's plane and radius; the camera only
moves when `view_workplane()` is invoked.

## 3. The `WP` pill and the status bar

- **The pill shows the plane, not the plumbing:** `WP: XY` when world, `WP: Face3` in
  cyan `CHROME_ACTIVE` when bound, `WP: View` for the camera plane. There is no state
  dot: `Grid`/`Axes`/`Gizmo`/`Snap` can be off, while a workplane always exists.
- **No primary action.** A click anywhere on the control opens the datum menu; the
  label is the state, and `World XY (Top)` is the reset. (An earlier draft toggled the
  body back to world; that is a second, hidden meaning on the same click.)
- **The status line keeps the hover report.** `"no active workplane"` is gone; a `Clear`
  at the world default reports “already World XY” rather than failing.

## 4. Wiring: plane → frame → gizmo → tools

Everything here already has the pieces; this is the map to connect them.

| Consumer | Today | Target |
| --- | --- | --- |
| Grid / axes / origin | render at `active_workplane.plane` or `Workplane::world()` | unchanged, driven by the named state |
| Snap | `SnapContext` from the plane, else `snap_grid` | unchanged; the ephemeral tier supplies the plane |
| `TransformGizmo` | frame from the manipulator (`push_pull` uses a face frame) | `set_frame(ManipulatorFrame { origin: wp.origin, basis: [wp.u, wp.v, wp.n] })` when a transform is created or moved in workplane coordinates |
| Creation tools | `event.world()` / scene hit / snap | ray through the pointer onto the active plane via `Workplane::ray_local` |
| Python | `Workplane` math only | a `"set"` verb carrying an origin + two axes (or a `FreeCADGui.activeWorkplane` property) |

## 5. Acceptance

- **State:** the world plane is representable and reported (`WP: XY`); setting a face,
  a world plane, a view plane and a 3-point frame each round-trip through the viewport
  state and a `.FCStd`-free session; `Clear` from world is a no-op.
- **Ephemeral tier:** with `Polyline` active, hovering a tilted face blooms the local
  grid, the first click binds it, the second point in empty space lands on that plane
  (assert the world coordinate has the expected `n` component), and finishing restores
  the previous plane.
- **Explicit tier:** the wheel verb activates the face under the cursor; grid, axes and
  origin move to it; `view_workplane` looks down its normal.
- **Dropdown tier:** each entry sets the expected frame; offset moves the plane exactly
  Δ along `n`; a degenerate 3-point pick is rejected, not silently accepted.
- **Gizmo:** a transform begun while a non-world plane is active reports a frame whose
  `basis[2]` equals the plane normal.
- **No regressions:** the existing viewport tests for sticky workplanes, the hover
  dwell, snap contexts and the grid keep passing; the `WP` command registry entries stay
  callable even after the menu changes.

## 6. Open decisions

1. **State type — decided (§7.1).** `ActiveWorkplane { plane: Workplane, label }`, with
   `WorkplaneKind` doing the provenance naming and `is_default()` as the only “unbound”
   test.
2. **Ephemeral binding:** per tool session (decided, §2.1) — one plane for the whole
   shape, first press only.
3. **The `WP` pill's click — decided (§7.1, §3).** There is no primary action and no
   state dot: a click anywhere opens the datum menu, and `World XY (Top)` is the reset.
   A body-click toggle would be a second, hidden meaning on the same control.
4. **Datum planes:** `WorkplaneKind::Datum` exists with no document object behind it. Add
   `Part::DatumPlane` as part of this task, or keep the dropdown to faces/world/view
   until P4 SK1's constraint work needs datum objects?
5. **The Python verb:** extend `ViewRequest` with `{"workplane": {"set": {...}}}` (no new
   public class) or expose `FreeCADGui.ActiveDocument.ActiveView` workplane accessors?
   The first keeps one validation path.
6. **The status text.** Confirmed: the tree has no persistent `"no active workplane"`;
   the phrase was only ever a command result, and that is now gone (§7.1).
7. **Grid steps — decided (§7.1).** A bound plane keeps the world steps; the finer
   `GridStyle::workplane()` style survives only as the hover preview.
8. **World layer — decided (§7.5).** The world *ground* is hidden behind a bound plane
   (one grid); the world *triad* stays, dimmed.

## 7. What landed (2026-10-29)

### 7.1 The state model and the pill

The prerequisite for all three tiers.

- The viewport's plane is `ActiveWorkplane { plane, label }`, **not** `Option<FaceFrame>`:
  it starts at world `XY` (`label: "XY"`) and is never absent. `set_active_workplane`
  takes an explicit plane; `set_workplane(Some(frame))` labels the frame from its
  `WorkplaneKind` (`Face1`), and `set_workplane(None)` / `clear_workplane()` reset to
  world. `active_workplane()` returns `&ActiveWorkplane`; `is_default()` is the pill's
  dot test.
- **The grid is the origin grid, moved.** The active plane uses the same generator, the
  same steps (`grid_minor`/`grid_major`), the same ink and the camera-following radius;
  snapping uses those steps too, so the drawn grid and the snap agree. The finer 1 mm/10 mm
  style now survives only in the *hover* preview, where it is a hint about the face.
- `view_workplane()` always succeeds: at the default it is the top view (down `-Z`).
- The `WP` control is a **selector, not a toggle**: it has no state dot at all, its label
  reads `WP: XY` / `WP: Face1` in normal ink and switches to `CHROME_ACTIVE` when the plane
  is non-default, and a click anywhere on it opens the datum menu (there is no primary
  action to misfire at the default). The shell reads the plane from the viewport entity
  while rendering, so there is no mirrored state to go stale.
- `ManipulatorFrame::of_workplane` converts the plane to the gizmo/overlay frame
  (`basis = [u, v, n]`), so the overlays and future handles can solve in the plane.
- Status messages no longer deny a plane: `Clear` reports “workplane reset to World XY”
  (or “already World XY”), `Activate` names the plane, `View` names what it looks down.

Verified: `ferrocad_stage` 186 (the frame conversion), `ferrocad_viewport` 163 (three new
tests: default plane, face label, no-op reset), `ferrocad_gpui --lib` 216, plus shell
assertions that the default pill has no state dot and that a click on its label opens
the menu.

### 7.2 Tier 1 — the ephemeral session plane

The first click of a creation tool on a planar face binds that face for the session, with
no explicit activation step:

- The viewport holds `ephemeral_restore: Option<ActiveWorkplane>`, the plane to put back
  when the session ends. It is set only by the first press, only while the active plane
  is the default and only while the tool has placed no point yet (`status().points == 0`),
  so a later click on another face cannot move the plane under the points already placed.
- `set_tool(None)`, `cancel_tool` and `finish_tool` all call `restore_ephemeral_workplane`,
  so `Esc`, `Enter` and the HUD's **Finish**/*Cancel* all return to the pre-session plane.
  An explicit binding never sets `ephemeral_restore`, so it survives a tool session.
- Face resolution is shared with the hover bloom (`face_frame_for`), so the click and the
  dwell preview can never disagree about what “the face under the pointer” is.

The pressed point and everything after it project onto the bound frame, because
`snap_context()` reads the active plane; the hover preview stops showing the face already
bound. The `WP` pill follows automatically: the shell reads the plane from the viewport
while rendering, so it shows `WP: Face1` for the session and returns to `WP: XY` when the
tool ends.

Verified: `ferrocad_viewport` 167 (four Tier 1 tests: bind-and-restore; a later click does
not rebind; an empty-space start never binds; an explicit plane survives a session),
`ferrocad_gpui --lib` 216, `ferrocad_toy` 18 + round trips.

### 7.3 Tier 3 — the datum rows

`WP ▾` now lists planes, not plumbing:

- `ferrocad_stage::StandardPlane` (`Top`/`Front`/`Right`) plus
  `Workplane::standard(...)` build the three axis planes at the origin (`XY`, `XZ` with
  `n = -Y`, `YZ` with `n = +X`). `WorkplaneKind::World` means “a world-standard plane”,
  so the enum gains no variant for them; a new `WorkplaneKind::Custom` covers frames with
  no document object behind them, and the Python mirror gains `WorkplaneKind.Custom`.
- `ActiveWorkplane::standard(...)` labels them `XY`/`XZ`/`YZ`; only `XY` is `is_default()`.
- `set_view_plane()` reads the camera's own basis out of the view matrix's rows (screen
  right/up/toward-viewer), puts the plane through the point the camera looks at, and
  labels it `View` with `WorkplaneKind::Custom`.
- `set_workplane_from_element(entity, "Face6")` resolves the face by element name and
  reuses `face_frame_for`, so the explicit binding and the hover bloom agree.
- `WorkplaneAction` gained `Standard`, `Camera` and `Selection`, all parsed from the same
  one string a Python command returns (`"world:xz"`, `"camera"`, `"selection"`).
- New registry rows: `App_WorkplaneXY`/`XZ`/`YZ` (`World XY (Top)` …),
  `App_WorkplaneCamera` (`View plane (Camera)`) and `App_ViewWorkplane` (`Align camera
  to WP`). `App_ActivateWorkplane` and `App_ClearWorkplane` stay registered but are
  `"hidden"`: a menu click moves the pointer off the face, so the hover verb could only
  fail there, and `World XY (Top)` is the reset.
- The `WP` popover groups under `chrome::WORKPLANE_GROUP` (`"App_Workplane"`), not under a
  primary command: `WP` is a selector with no primary action, so there is no command to
  hide the rows behind. `chrome::WORKPLANE` (`"App_ViewWorkplane"`) stays the control's
  registry gate.

### 7.4 Tier 2 — `Set WP`

`App_SetWorkplane` is one command with two homes: `Set from selection` in the `WP` menu
and `Set WP` in the radial wheel. It uses the **selected** planar face's own centroid and
normal (not the click point), so the frame is stable however the pointer arrived, and it
is sticky like any explicit binding. `IsActive()` requires a sub-element selection, so the
wheel still falls back to the dock verbs over empty space.

The wheel had no way to host a command that belongs to another surface, and moving this
row to `Surface::Radial` would hide it from the menu. `FerroCAD_Wheel: true` (forwarded as
`CommandInfo::wheel`) offers a command in the wheel **in addition to** its own surface;
`radial_slots` includes `radial`-surface commands *or* `wheel: true` ones.

Verified: `ferrocad_stage` 187 (the three standards' axes and right-handedness),
`ferrocad_viewport` 172 (three new tests: standard planes, view plane, face-by-element),
`ferrocad_gpui --lib` 218 (the datum rows render in the popover, `Set WP` is flagged for
the wheel, and a GPU-gated end-to-end test drives `App_WorkplaneXZ` →
`App_SetWorkplane` → `App_WorkplaneXY` through a real viewport).

### 7.5 Fixes from the first hands-on pass (2026-10-29)

Four things the first real session found:

1. **Two grids.** The world ground was still drawn (dimmed to `WORLD_DIM`) under the
   active plane's grid, so selecting a plane looked like *adding* a grid rather than
   moving it. `sync_grid` now hides the world ground entirely while a non-default plane
   is active: one grid, at the plane. The world **triad** still recedes to
   `WORLD_TRIAD_DIM` and the origin node stays, so absolute zero is still marked. The
   hover preview keeps the world ground (the default plane is still active then) and
   dims it.
2. **No origin control.** `ActiveWorkplane` gained `offset` (mm along the plane's own
   normal) and `display_label()` (`XY +25`, `Face3 -4`); `offset_workplane(delta)` moves
   the origin and accumulates the offset, and `is_default()` is only true at zero offset
   on world `XY`. `App_OffsetWorkplane` is the menu row: it opens a one-field quantity
   dialog and returns `{"view": {"workplane": "offset", "workplane_offset": mm}}`;
   `ViewRequest` validates the pair (a missing or non-finite value is refused), and the
   pill shows the shifted label.
3. **No face to bind in the toy edition.** `Set from selection` needs a **named**
   sub-element, and the toy slab's mesh tagged every triangle as one untagged part, so
   there was never a `FaceN` to bind (the Part edition's `Shape.mesh()` does name
   faces, which is why it works there). The toy slab now emits one part per box face
   (`Face1`…`Face6`, with the matching `triangle_parts`/`segment_parts`), so the hover
   bloom, Tier 1, `Set from selection` and the wheel's `Set WP` all work in the sample
   edition too. A single click now selects the face; a double click still promotes to
   the whole body. The toy's height grip and move gizmo accept a face pick as well (a
   box's height and placement are properties of the body), so the handles do not
   disappear when the click lands on a face.
4. **The wheel could not offer a non-radial command.** Covered in §7.4
   (`FerroCAD_Wheel`).

Verified: `ferrocad_viewport` 172 (the one-grid packet count and the offset's origin,
label and default state), `ferrocad_gpui --lib` 218 (a full face-selection →
`App_SetWorkplane` chain over a real stage item, and `App_OffsetWorkplane` in the
popover), `ferrocad_toy` 18.

**Known limitation (Coin grid).** A mod-authored grid overlay is a world-ground shader;
its ABI has no workplane uniform
([`grid-overlay-plan.md`](../../developer/workbenches/grid-overlay-plan.md) §3.3 keeps the
workplane out of `FerroCAD.Grid` deliberately). With the Coin grid active the native
*plane* grid still draws at the plane, but the overlay itself stays at world `XY`, so the
view can show a mod grid at the origin plus the native grid at the plane. Making the
overlay follow the plane needs a `Frame` uniform (the `inv_view_proj` block already
exists for programs) and a mod-side update; it is not a native-grid bug.

**Still open after §7.5:** an `Offset workplane…` **gizmo** (the menu row and its
one-field dialog landed here; the 1D axis handle + floating HUD did not) and a proper
datum-plane object type (a `Part::Feature` face stands in for it, §7.6).

### 7.6 `Pick 3 points`, an arbitrary frame, and the datum plane (2026-10-29)

- **`App_Workplane3Points` → the native `workplane_points` tool.** Three left clicks
  (snap-preferred) plot the points; `Backspace` drops the last; `Enter` resolves the
  triangle into the active plane; `Esc` cancels. The frame travels through two new
  defaulted `Tool` methods: `workplane()` (what the picker resolved) and
  `picks_workplane()` (this tool's points are input, not a sketch). `finish_tool` reads
  `workplane()` before ending the session and applies it with `set_workplane_frame`
  **only on finish** — `cancel_tool` never applies it, and the ephemeral restore cannot
  fight it (that restore only fires for a session that bound a face).
  `picks_workplane()` also excludes the tool from the first-press ephemeral binding;
  without it the second and third picks would be flattened onto the first face's plane.
  The tool ships in `ferrocad::base_contributions()`, so every edition has it, and the
  command is a `workplane`-surface row (`Pick 3 points…`).
- **An arbitrary frame from a request.** `ViewRequest` gained
  `{"workplane": "set", "workplane_frame": {"origin": [x,y,z], "u": [...], "v": [...]}}`.
  The parser checks shape and finiteness (three components each; an orphan `frame` and a
  missing one are refused); whether the axes actually span a plane is
  `InventorViewport::set_workplane_frame`'s call, and a degenerate frame is reported as
  such. This is the script/mod path to a frame, and the same seam the picker uses.
- **Selectable datum plane.** `App_SaveDatumPlane` (`Save as datum plane`, short
  `Save WP`, in the `WP` menu and the radial wheel) is a document command with a
  transient half: the shell reads the **live** frame and calls the entry module's
  `add_datum_plane(payload)`, which builds a 100 mm square face **from the frame's own
  points** (no rotation math) in one `"Datum plane"` transaction as a `Part::Feature`,
  cyan, `ShadedWithEdges`. Its mesh names the face `Face1`, so `Set from selection` /
  `Set WP` binds it immediately; it is pickable, movable, undoable and saved with the
  document. The Python half lives in the edition that links `Part`
  (`ferrocad_parametric`); the shell names no workbench, only the command id. A headless
  run reports “the shell owns this command”, and a build whose edition has no
  `add_datum_plane` reports the bridge's error rather than failing the command.
  The real object type (`Part::DatumPlane` with attachment/expressions) remains open;
  this is a plain face, which is exactly what `Set WP` needs.

Verified: `ferrocad_stage` 195, `ferrocad_viewport` 176, `ferrocad_gpui --lib` 222,
`ferrocad_toy` 18, Python `tools/run_tests.py --extension Part` 721 passed / 3 skipped
(7 new datum-plane tests, and the new module is in the extension-set contract),
`cargo check -p ferrocad`.

## 8. Reflection (2026-10-29): visual hierarchy and parked work

**The hierarchy rules are design; slices 1–4 below have since been built, with the gaps
named in each.** The full rationale is in
[`../../developer/interaction/workplanes.md`](../../developer/interaction/workplanes.md)
§7–§12.

Decided in the reflection:

- **Selection needs no new mode.** Hover reports only; one click selects the sub-element;
  a double click selects the body; a tree click selects bodies. Every suppression rule
  keys on that existing state, so no body/face filter is added.
- **One active axis cross.** A bound plane's cross is the only full-strength one; the
  world origin becomes a 15 %, 12 px ghost with no labels (the origin node stays).
- **A face-bound plane draws a 2D `U/V` bracket**, not `U/V/N`: the normal belongs to the
  face manipulator. Floating planes keep `N`.
- **The body triad yields to an element-anchored handle** (Part face → push/pull only;
  Part edge → arc only). Move/Rotate arm it explicitly, so suppression is a default, not
  a lock. A face with no handle keeps the triad.
- **A face-bound grid is a face-sized patch** (`max(3 × face radius, 2 × major step)`) on
  the face's **own** step (about ten major cells across it, 1-2-5 ladder), not a wall.
  Only the `GridSpec.radius` changes; the generator's existing fade is
  the edge. No shader, style or setting is added.
- **A datum plane is a document object**, not a gizmo: tree row, `Visibility`, undo,
  save, whole-area pick, and a normal/tilt handle when selected.

The four slices, in order (each independently shippable):

1. **Triad/grid hierarchy — landed 2026-10-29.** `ActiveWorkplane`
   carries an inferred `extent`; a face plane emits a 2D `U/V` bracket; the world cross
   recedes to `WORLD_TRIAD_DIM`; `active_grid` uses `max(3 × face, 2 × major)` on the
   plane's derived step (`nice_step(0.3 × extent)`, about ten major cells); the hover
   bloom is tool-scoped; and the shell drops the shared
   body triad when a manipulator claims the element, unless a `Move`/`Rotate` verb asked
   for it. Acceptance tests: `a_face_workplane_emits_uv_without_normal_stem`,
   `a_face_workplane_clamps_grid_radius_to_face_extent`,
   `hovering_a_face_while_idle_does_not_bloom_a_grid`,
   `hovering_a_face_with_an_active_tool_blooms_only_at_world_default`,
   `a_claimed_sub_element_suppresses_the_shared_body_triad`,
   `an_unclaimed_sub_element_keeps_the_body_triad`,
   `a_move_verb_arms_the_body_triad_on_a_face_selection`. The toy now claims its **top**
   face only (a side or bottom face is the fallback case), and `b811401` is kept.
2. **`Offset Workplane…` as a gizmo — built (2026-10-29).** A 1D axis
   stem along `+N` plus the floating HUD; the menu row and its one-field dialog landed in
   §7.5. The field and the drag are two-way (the field follows every tick, typing moves
   the plane live), the session is a `Shell` field rather than process-global state, and
   `workplane_stem` reaches Python. Evidence: `offsetting_a_workplane_drags_and_types_along_normal`
   and `a_tracked_stem_reports_its_live_total_each_tick`.
3. **`Part::DatumPlane` — landed (2026-10-29), with gaps.** A
   persistent object with `Placement` and `Size`, a derived `Face1`, the `"glass"`
   display opt-in and `Visibility`; `Save as datum plane` (`App_SaveDatumPlane`) creates
   it, superseding §7.6's `Part::Feature` face. Gaps: there is no bespoke normal/tilt
   handle — a selected datum plane is moved and rotated by the standard gizmo through its
   `Placement` — and tree `Space`-to-hide works only on the Dev Tools key path, because
   the main window keeps `Space` for the object-verb wheel. Attachment and expressions
   stay a non-goal (§9).
4. **Coin-grid workplane uniform — built (2026-10-29).**
   The shader, the `Frame` layout and the host transport are done: `Frame` is 256 B, with
   `workplane_origin` (`w` = patch radius), `workplane_u`, `workplane_v` and
   `workplane_n` at offsets 160/176/192/208, `workplane_steps` at 224 and `grid_stroke` at
   240; `ferrocad_graphics::WorkplaneFrame` +
   `write_frame_with_workplane` carry it, `ViewSpec::workplane` selects it, and the
   viewport fills it from
   `ActiveWorkplane` with the same `max(3 × extent, 2 × major)` radius (and the same
   derived step) the native grid uses (an unbound plane sends the camera-following ground
   extent, `radius_for`, never `0`). Acceptance:
   `coin_grid_mod_tracks_the_active_workplane_frame` (viewport) and
   `a_view_workplane_reaches_the_program_frame` (wgpu byte layout).

## 9. Non-goals

- Datum-plane *objects* with expressions and attachment (that is `Part::Datum` and the
  constraint work; this task only needs a frame and a label).
- A full sketch solver on the plane (P4 SK1–SK6). This task makes the plane a good place
  to draw; it does not constrain what is drawn.
- Multi-plane documents (one active plane at a time, as today).
