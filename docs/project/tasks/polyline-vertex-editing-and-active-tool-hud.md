# Polyline vertex editing and one active-action HUD

Status: task spec (2026-10-29). **Built on 2026-10-29**: the tool-side vertex editing
(§3.1–§3.5), the finish/commit path and the active-action HUD (§2) have landed; numeric
entry and the hover rim remain — see §6. Companion to
[`../../developer/interaction/selection-gizmos-and-forms.md`](../../developer/interaction/selection-gizmos-and-forms.md)
(the form/gizmo state machines, §5.2 and §6.1 below),
[`../../developer/interaction/visual-language.md`](../../developer/interaction/visual-language.md)
(the dielectric-glass contract), [`../../developer/interaction/transform-ux.md`](../../developer/interaction/transform-ux.md)
(the gadget/HUD split) and [`../TODO-29-10-2026.md`](../TODO-29-10-2026.md) P4 SK4.

Two UX defects, one root cause each:

1. **A placed vertex cannot be adjusted until the polyline is finished**, so a six-point
   shape has to be cancelled and redrawn to fix point 3.
2. **"An action is running" is drawn by two different widgets** — the tool strip and the
   floating form — with different placement, materials and button hierarchy, and the
   strip styles its destructive button as the primary action.

## 0. What the tree does today (verified 2026-10-29)

Read from this checkout; the descriptions in the brief that follow are checked against it.

- **`PolylineTool` appends on every press.** `on_event` pushes `snap.or(event.world())` on
  each left `Down` (`crates/ferrocad_stage/src/tool.rs:206`). There is no press origin, no
  travel threshold, no hit test against the tool's own vertices, and no per-point edit or
  delete. `cancel()` clears all three `ShapeItem` slots; `status()` reports label, point
  count and a hint (`tool.rs:226`).
- **`PointerEvent` has no drag state and no projection.** `kind` is
  `Move`/`Down`/`Up`, `position` is device pixels, `hit` is the scene pick
  (`crates/ferrocad_stage/src/input.rs:34`). `button` is only set on `Down`/`Up`, so a tool
  that wants "press and drag" must remember the press itself. There is no view/projection
  available to a tool, so it cannot project its own points to screen space.
- **The `Tool` trait has three jobs, not four.** `on_event`, `items`, `status`, `cancel`
  (`tool.rs:79`). There is no `finish`: `Enter` ends the session in the host
  (`crates/ferrocad_gpui/src/shell/dispatch.rs:599`), and — per `TODO-29` — it currently
  produces no `InteractionCommit`, so finishing draws nothing.
- **Tool visuals are never selectable**, which is the property to preserve:
  `wire_item` sets `with_selectable(false)` (`tool.rs:243`).
- **The strip is an opaque flex row above the canvas with an accent Cancel.** `tool_strip`
  (`crates/ferrocad_gpui/src/shell/render.rs:11`) uses `bg(PANEL_ALT)` rather than the
  floating-HUD `WHEEL_GLASS`, and has a single **Cancel** button painted `bg(ACCENT)` — the
  primary accent on an abort action (`render.rs:45`). There is no **Finish** button; the
  hint says "Enter to finish". It is added as a child of the viewport *column* above the
  canvas (`shell.rs:1169`), so it does push the canvas down when it appears.
- **The action dock is a separate element inside the canvas** (`chrome::action_dock`,
  `shell.rs:1224`), absolutely positioned at the bottom-centre — the slot the strip should
  use.
- **`PointerEvent` is mirrored in the Python binding** (`crates/ferrocad_stage_py/src/input.rs:90`)
  and constructed in `crates/ferrocad_viewport/src/inventor_view.rs:1436,2565`, so any new
  field is a three-crate change.

**Reconcile before building.** The attached capture shows a bottom-centre glass pill with
`Cancel` + `Finish`; this tree renders an opaque panel with `Cancel` only, in the viewport
rather than the dock slot. Either the capture is a newer local build or it is the target
design. Step 0 is to capture the current build and record which it is, because the rest
of §2 assumes "replace what is there", not "restyle the capture".

## 1. Why the vertex case is a state conflict, not a missing feature

The rules are already written down, and they are right:

- **Pointer layering** puts the active tool above the scene and below the gizmo
  (`selection-gizmos-and-forms.md` §6.1), so the tool owns the pointer while it runs.
- **Form states** cancel on a "competing interaction" (§5.2), so an in-progress tool
  cannot hand the pointer to the document-level `Move` form.

Therefore moving point 3 of an open polyline is **not** a `Move` command. It is a
sub-state of the polyline tool, which is exactly how the tools in Rhino, Fusion's Forms and
Plasticity behave: the same tool owns the vertices before and after they are placed.

That also keeps the document clean: nothing is in `FreeCADGui.Selection`, no gizmo arms,
and the one transaction happens at Finish.

## 2. One active-action surface

### 2.1 Placement and material

- **Swap, do not stack.** While a tool or a floating form is active, the bottom-centre
  slot shows the active surface; the idle action dock (`Create Slab · Polyline · Search`)
  is not rendered. The user cannot start a second command from a dock that would have to
  cancel the first.
- **Material.** Use the dielectric HUD token (`WHEEL_GLASS` + `DOCK_BORDER`, as the
  floating form and the wheel already do), not `PANEL_ALT`.
- **No layout shift.** The strip is currently a flex row above the canvas, so it pushes
  the canvas down. Move it into the canvas as an absolutely positioned overlay (the action
  dock and the viewport header already work that way) and assert that the viewport rect
  is identical with and without an active tool.
- **Two placements, one rule.** Freeform picking tools (Polyline, Measure, Arc) dock
  bottom-centre, because the pointer travels over the whole canvas. Gizmo-bound forms
  (Move, Rotate, Scale) anchor to the gizmo (§5.1 `float`), because the fields are read
  against the triad. Decide once and record it in `visual-language.md`.

### 2.2 Button hierarchy

- **Cancel** is neutral/muted (`PANEL` on `WHEEL_GLASS`), never `ACCENT`.
- **Finish** is the primary action (`ACCENT`), and it is a real button, not a hint. A
  trackpad user must be able to end a six-point polyline without finding `Enter`.
- Both have keyboard parity: `Esc` = Cancel, `Enter` = Finish, and the buttons call the
  same handlers (`cancel_tool`, the finish path) so the two cannot drift.

### 2.3 What the tool strip shows

One pill that morphs with the tool's state rather than a second widget:

| State | Content |
| --- | --- |
| Appending | `Polyline · 6 points · click to add · drag a point to adjust` + Cancel/Finish |
| Adjusting point 3 | `Polyline (point 3) · X [ 24.50 ] · Y [ −12.00 ]` + Delete point/Done, with Cancel/Finish still available |

Both are `ToolStatus` shapes today except the fields; the numeric part is what needs the
new sub-state (§3.4).

## 3. Editing a placed vertex inside the tool

### 3.1 Gesture separation

The tool already owns pointer hit-testing (§6.1). Add a press/drag state machine:

| Pointer | Gesture | Result |
| --- | --- | --- |
| Empty workplane or snap target | click (< 5 px travel) | append point *N+1* (today's behaviour) |
| Point 0 | click | close the loop, finish |
| Any placed point | hover | highlight the vertex (hover rim; the tool's items stay non-selectable) |
| Any placed point | press + drag | slide it in-plane, rubber-banding both adjacent segments live |
| Any placed point | click (no drag) | make it the *active point*: numeric X/Y entry, Delete removes it |
| Anywhere | `Backspace` / `Ctrl+Z` | remove / restore the last point (tool-local undo) |

The 5 px threshold is measured in device pixels from the remembered press position. It is
the only part that needs new state on the tool; everything else is a hit test and an index.

### 3.2 Why the tool must own it

Doing it in the document-level `Move` would violate §6.1 (the tool owns the pointer), would
need a `Selection` entry for geometry that is not a document object, and would cancel the
tool under §5.2's competing-interaction rule. Tool ownership is the only option that keeps
undo at exactly one transaction.

### 3.3 The hit-test gap (decide first)

The tool sees device-pixel positions and scene hits, but no projection, so it cannot tell
whether the pointer is within ~10 px of a placed vertex. Three options, in order of
preference:

1. **Give the event a projection handle.** Extend `PointerEvent` (or the session the host
   hands the tool) with the frame's view-projection, so the tool projects its own points.
   Self-contained, no host-side per-tool logic, and reusable by every future tool.
2. **Host-side tool hit test.** Add `fn hit_test(&self, position: [i32; 2]) -> Option<usize>`
   to `Tool`; the host projects the tool's points and reports the nearest within the radius.
   Keeps projection out of the tool but puts tool knowledge in the host.
3. **World-space radius.** Wrong at every zoom except one; do not.

Option 1 is the recommendation: it also unlocks measure/arc snapping to tool geometry
without a second mechanism.

### 3.4 Numeric entry

- The X/Y fields format with the session's units (mm today).
- Typing moves the vertex and re-rubber-bands immediately; unparsable text is zero while
  typing, never an error (the form rule, §5.2).
- `Tab` moves between the fields; `Enter` returns to appending (not Finish) when a point is
  active, unless the appending state would append a duplicate.

### 3.5 Finish semantics

`Finish` and `Enter` share one handler (`Shell::finish_active_tool`), and the tool hands
its geometry over on the way out: `Tool::finish() -> Option<InteractionCommit>`. The
polyline returns a **creating** commit — `creates: true`, `manipulator: "polyline"`,
`action: "create"`, `values` are the world-space points — which the viewport exposes
through `take_interaction_commit` and the shell routes without resolving an entity: the
edition's `interaction` hook receives `obj = None` and creates the object in one
transaction. Fewer than two points returns `None`, so a stray click cannot create an
empty object.

## 4. Acceptance

- **`ferrocad_stage` unit tests.** Dragging a placed vertex changes its coordinate and
  leaves `points.len()` unchanged; releasing with < 5 px travel selects instead of
  appending; a click on empty space still appends; `Backspace` removes the last point and
  the tool-local undo restores it; `cancel()` still clears everything; no emitted
  `ShapeItem` is selectable.
- **Shell/HUD tests.** The action dock is not rendered while a tool is active; the strip
  uses the glass token; Cancel is neutral and Finish is the accent; Finish and `Enter`
  call the same handler; the viewport rect is identical with and without the strip.
- **Numeric test.** Typing `X`/`Y` for point 3 moves it, the adjacent segments follow, and
  the committed wire (once §3.5 lands) contains the edited point.
- **No regression:** `tools/conformance.py --simplecad` and the Part suites are untouched
  by this work; `ferrocad_gpui` and `ferrocad_stage` test counts must not drop.

## 5. Open decisions

1. **Capture or code?** Reconcile the attached pill screenshot with `render.rs` (§0).
2. **`Tool` contract:** projection handle (option 1) vs host-side `hit_test` (option 2).
3. **`Tool::finish`:** add it to the trait, or keep finishing in the host dispatch? The
   tool already owns `cancel`, so symmetry argues for `finish`.
4. **Placement for gizmo forms:** keep the HUD anchored to the gizmo, or move every active
   surface into the bottom-centre slot? The former has better eye travel; the latter has
   one predictable place.
5. **Does the strip become a `forms`-style HUD** (reusing focus/field machinery) or stay a
   bespoke element with two buttons?

## 6. What landed (2026-10-29)

Built by two parallel agents against disjoint write scopes; verified centrally.

**Tool side (`ferrocad_stage`, `ferrocad_stage_py`, `ferrocad_viewport`).**

- `PointerEvent` carries `projection: Option<ViewProjection>` — a `Copy` world→device
  handle (`Matrix4` + viewport size) built from the camera in
  `inventor_view.rs`. This is **decision 2, option 1**: the tool projects its own points,
  no host-side per-tool hit test and no world-space radius. Both constructors attach it;
  the Python mirror sets `None` (no camera), so a scripted tool falls back to appending.
- `PolylineTool` gained hover / active-point / press-drag state, a screen-space vertex
  hit test (`VERTEX_PIXELS = 8`, a 16 px target), a 5 px `DRAG_THRESHOLD_PIXELS`, and a
  tool-local removal stack. Press on a vertex never appends; a click selects it; a drag
  slides it with the next-point band hidden; `Backspace`/`Delete` removes the active (else
  last) vertex and `Ctrl+Z` restores it.
- `Tool::on_key(key, ctrl) -> bool` (default `false`) is the narrow channel for those
  edits; `inventor_view::handle_key` forwards an unowned key to it and rebuilds the
  transients when consumed.

**HUD side (`ferrocad_gpui`).**

- `tool_strip` is an absolutely positioned bottom-centre pill in the canvas
  (`WHEEL_GLASS` + `DOCK_BORDER`), in the same slot as `action_dock`; the shell renders
  one or the other, never both.
- **Cancel** is neutral (`PANEL`); **Finish** is the accent action and calls
  `Shell::finish_active_tool`, the same handler the shell's `Enter` path now uses for any
  active tool (previously Python-backed tools only).

**Verified.** `ferrocad_stage` 185 passed, `ferrocad_gpui --lib` 216, `ferrocad_viewport`
160, `ferrocad_toy` 18 plus its round trips (including the new polyline creation), and
`tools/run_tests.py --extension Part --extension Mesh --extension MeshPart` at 714 passed
/ 3 skipped / 0 errors. The shell test `an_active_tool_overlays_the_canvas_without_moving_it`
asserts the canvas rect is identical with and without a session.

**The finish path.** `Tool::finish()` returns `Option<InteractionCommit>`;
`InventorViewport::finish_tool` stores it and the shell drains it through
`take_interaction_commit`. `InteractionCommit` gained `creates: bool` (built with
`::creating()`), which tells the host to route the commit without an entity lookup.
`ferrocad_shell.interaction` accepts an empty object name for exactly that case, and the
toy edition handles `polyline`/`create` by adding an `App::FeaturePython` with a `Points`
list in one transaction — displayed as an edge-only wireframe mesh. Finishing with fewer
than two points still commits nothing.

**Decisions taken:** 2 → option 1 (projection on the event); 3 → `Tool::finish` added
(the tool owns the geometry until it ends); 4 → freeform tools dock bottom-centre, gizmo
forms stay anchored; 5 → the strip stays a bespoke element (no field machinery yet).

**Still open.** Numeric X/Y entry (§3.4), the hover rim geometry, and “click point 0 to
close the loop” are not built.

## 7. Non-goals

- Turning the polyline into a constrained sketch (that is P4 SK1–SK6; this is the tool's
  pre-commit editing only).
- Editing a **committed** wire's vertices in place; after Finish the normal
  `SelectionAnchor` + gizmo flow applies, per §6.1.
- A general Qt-free widget toolkit (P1.2's `QApplication` bucket).
