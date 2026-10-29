# Workplanes: one model, three ways in

Status: task spec (2026-10-29). **Partially built on 2026-10-29**: the state model, the
same-grid change and the `WP` pill have landed (§7); the three tiers' entry points remain.
Companion to
[`../../developer/interaction/selection-gizmos-and-forms.md`](../../developer/interaction/selection-gizmos-and-forms.md)
(the context wheel, §2.2), [`../../developer/interaction/gizmo-manipulators.md`](../../developer/interaction/gizmo-manipulators.md)
(frames and the triad), [`../../developer/interaction/visual-language.md`](../../developer/interaction/visual-language.md)
(the accent/glass tokens), [`polyline-vertex-editing-and-active-tool-hud.md`](polyline-vertex-editing-and-active-tool-hud.md)
(the active-action surface this shares a slot with) and
[`../TODO-29-10-2026.md`](../TODO-29-10-2026.md) P4 SK4.

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

The visual half exists (hover face + local grid). What is missing is binding:

- While a creation tool is active and the pointer is over a **planar face**, the hovered
  frame becomes the tool's provisional plane (the grid already blooms there).
- The **first click** on that face binds the plane *for the session*; later clicks in
  empty space project onto that frame rather than onto world XY.
- Finishing or cancelling the tool returns the active plane to what it was before the
  session (usually world), unless the user bound it explicitly in Tier 2.
- The tool's `SnapContext` already carries a `Workplane`; this tier is a matter of
  choosing it per session rather than reading the viewport's global one.

Design question: is the binding *per tool session* (my recommendation) or per click? Per
session matches "the moment Polyline finishes, the workplane returns to World XY" and
keeps one plane for the whole shape.

### 2.2 Tier 2 — explicit binding from the context wheel

The context wheel already has a per-selection target (`selection-gizmos-and-forms.md`
§2.2). Add **Set WP** as a wheel verb on a planar face:

- Calls `set_workplane(Some(frame))` — already sticky, so it survives pointer moves and
  empty-space clicks.
- The grid, axes and origin node animate to the face (they already render at the active
  plane; the animation is presentation).
- **Align view to WP** is `view_workplane()`, which already looks down the normal with a
  roll-free up vector; wire it to a wheel verb and to double-clicking the `WP` pill.

### 2.3 Tier 3 — the `WP` dropdown

Replace the three plumbing commands with plane choices (the commands stay reachable, but
they are no longer the menu):

```text
[ • WP: Top (XY) ▾ ]
  Top (World XY) · Front (World XZ) · Right (World YZ) · View plane
  ─────────────────────────────────────────────────────────────
  Pick face… · Pick 3 points… · Offset workplane… · Align camera to WP
```

- The three world planes are `Workplane::new(...)` frames with `WorkplaneKind::World` (or
  a new `Standard` kind so the label can name them).
- **View plane** is a frame from the camera's view direction at the current pivot.
- **Pick face…** is the existing activate-hovered path, made explicit rather than
  implicit.
- **Pick 3 points…** is `Workplane::new(origin, p1-p0, p2-p0)`, which already rejects
  degenerate input.
- **Offset workplane…** moves the frame along `n` and should use the single-axis `Z`
  gizmo with the floating HUD (`Offset ΔZ [ 0.0 ] mm`), like the existing transform form.

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
   detail through the existing `MINOR_PIXELS` decade rule.
3. **The world layer.** Keep it, dimmed: `WORLD_DIM = 0.25` on the ground and
   `WORLD_TRIAD_DIM = 0.30` on the triad already produce the “muted absolute datum” the
   brief asks for, so the world origin stays locatable without a new element.

Because the grid is a spec, switching planes is a state swap, not a mode. If the swap is
animated (120 ms, per the brief), animate the spec's plane and radius; the camera only
moves when `view_workplane()` is invoked.

## 3. The `WP` pill and the status bar

- **The pill shows the plane, not the plumbing:** `WP: XY` when world, `• WP: Face3 ×`
  when bound (cyan `#38BDF8` dot for non-world, per the accent table), `WP: Datum…` once
  datums exist.
- **The main body toggles** back to the world plane when a custom plane is active — the
  same "label is the switch" rule as Grid/Axes/Snap — and the `×` clears to world.
- **The status line keeps the hover report.** `"no active workplane"` disappears with
  the state change; `Clear` at the world default becomes a no-op rather than a message.

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

1. **State type:** `Option<FaceFrame>` → an enum (`World | Datum(name) | Face{..} |
   Custom{frame}`) or `Option<Workplane>` with `WorkplaneKind` doing the naming? The
   second reuses the existing kind and needs no new enum.
2. **Ephemeral binding:** per tool session (recommended) or per click.
3. **Does the `WP` pill body toggle to world or clear?** They are the same thing once
   world is representable — decide the wording.
4. **Datum planes:** `WorkplaneKind::Datum` exists with no document object behind it. Add
   `Part::DatumPlane` as part of this task, or keep the dropdown to faces/world/view
   until P4 SK1's constraint work needs datum objects?
5. **The Python verb:** extend `ViewRequest` with `{"workplane": {"set": {...}}}` (no new
   public class) or expose `FreeCADGui.ActiveDocument.ActiveView` workplane accessors?
   The first keeps one validation path.
6. **The status text.** Confirm the `"no active workplane"` the brief cites: in this
   tree it is the Clear/View result message (`dispatch.rs:498–510`), not a persistent
   readout. If a newer build has a persistent one, fix that too.
7. **Grid steps.** Keep the world steps on a bound plane (identical grid) or keep the
   finer `GridStyle::workplane()` steps and label them in the pill?
8. **World layer.** Keep it dimmed behind a bound plane (recommended — it is the absolute
   datum) or hide it while a plane is active?

## 7. What landed (2026-10-29)

The state model and the pill — the prerequisite for all three tiers.

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

**Still open:** the dropdown's standard planes (`XZ`/`YZ`), view plane, 3-point pick and
offset HUD; the ephemeral binding during a tool (§2.1); the wheel verb (§2.2); the Python
“set” verb (§4); and the datum-plane object question.

## 8. Non-goals

- Datum-plane *objects* with expressions and attachment (that is `Part::Datum` and the
  constraint work; this task only needs a frame and a label).
- A full sketch solver on the plane (P4 SK1–SK6). This task makes the plane a good place
  to draw; it does not constrain what is drawn.
- Multi-plane documents (one active plane at a time, as today).
