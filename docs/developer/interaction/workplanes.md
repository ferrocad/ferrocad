# Workplane (`WP`): specification and interaction model

Status: design (2026-10-29). The state model and the `WP` label are built; the three
workflows are specified and not yet complete (see
[`../../project/tasks/workplanes.md`](../../project/tasks/workplanes.md) for the work
list). Companion to [`axis-origin-grid.md`](axis-origin-grid.md) (the triad, the
ground/workplane grids, face frames), [`visual-language.md`](visual-language.md) (tokens
and chrome), [`gizmo-manipulators.md`](gizmo-manipulators.md) (`ManipulatorFrame` and the
G3a handles) and [`selection-gizmos-and-forms.md`](selection-gizmos-and-forms.md) (the
wheel and the interaction state machine).

This note defines what a workplane is, how it differs from the global origin, how it is
represented in the top-left spatial strip, and why it reuses the existing overlay and
manipulator seams rather than adding a rendering backend.

## 0. Principles

1. **There is always an active workplane.** 2D creation tools (`Polyline`, `Create Slab`)
   and planar snaps always project onto a valid plane. "No active workplane" is never a
   user-facing state; with no override the active plane is **World XY** at `(0, 0, 0)`.
2. **The global origin is the permanent datum; the workplane is the movable jig.** Export
   coordinates, document placements and the camera's `Home` stay anchored to world
   `(0, 0, 0)`. The workplane is a transient local construction frame that moves where the
   user is working.
3. **`WP` is a selector, not a visibility toggle.** `Grid` and `Axes` own *whether* the
   grid and triad are visible (and carry the cyan on/off dots). `WP` owns *where* the
   grid, the axis cross and the tool's projection plane sit — so it has no on/off dot.
4. **Geometry-first activation.** Binding to geometry happens in situ (a radial-wheel
   `Set WP`, or the ephemeral face-hover while a tool runs); the `WP` menu is the explicit
   datum selector and the reset control.

## 1. Representation and ownership

Two frames exist, and they are the same frame:

| Type | Shape | Used by |
| --- | --- | --- |
| `ferrocad_stage::Workplane` | `origin`, `u`, `v`, `n` (with `n = u × v`), `kind` (`World`/`Datum`/`Face`) | the plane itself: snapping, `ray_local`, `FaceFrame`, the Python binding |
| `ferrocad_stage::ManipulatorFrame` | `origin`, `basis: [Vector3; 3]` | gizmos and manipulators; the frame the G3a handles solve in |

The mapping is exact: `basis = [plane.u, plane.v, plane.n]`, and
`ManipulatorFrame::of_workplane(&plane)` is the one place it is projected, so a gizmo or
an overlay can solve in the active plane's coordinates without conversion code of its own.

**State.** The viewport holds `ActiveWorkplane { plane: Workplane, label: String }` and it
is **not** an `Option` — the world plane is the default value, which is what makes
principle 1 true in the type system. `label` is presentation only (`"XY"`, `"Face1"`,
`"Datum"`, `"XZ"`), while `plane.kind` stays the semantic source. The earlier shape,
`Option<FaceFrame>` with `None` meaning world, could not distinguish "world" from "none"
and forced the status bar to deny the plane; it was replaced on 2026-10-29.

A `ManipulatorFrame`-typed `custom_workplane: Option<...>` would be equivalent only if the
resolved default lives somewhere; storing the resolved `ActiveWorkplane` removes that
second source of truth.

**Single source of truth.** Ray–plane intersection for creation tools, grid snapping, and
the `Grid`/`Axes` overlays all read that one resolved frame. The math is native and is
never overridden by a mod.

## 2. Canvas visuals: world origin versus active plane

With the default **World XY**, the workplane and the global origin are co-located: one
grid, one axis cross.

With a bound plane:

| Element | Behaviour |
| --- | --- |
| **Grid** | The **same** grid, rotated and moved into the plane: same generator, same steps, same ink, camera-following radius. It is not a clipped local patch. |
| **Axes** | The triad moves to `frame.origin` and its stems are the plane's `U`, `V`, `N`. |
| **Global origin** | The world **ground is not drawn** (there is one grid, and it is the active plane's). The world triad stays at `(0, 0, 0)`, dimmed (`WORLD_TRIAD_DIM = 0.30`), plus the origin node. The absolute zero never disappears, but it never doubles the grid either. |
| **Status bar** | Never a persistent "no active workplane". The line carries transient command output and the hover report. |

The finer 1 mm / 10 mm workplane style (`GridStyle::workplane()`) belongs to the **hover
preview** only: it is a hint that a face can be bound, and it must not make a bound grid
look like a different object from the origin grid.

## 3. The top-left spatial strip

In `[ • Grid ▾ ][ • Axes ▾ ][ WP ▾ ][ • Gizmo ▾ ][ • Snap ▾ ]`:

- **No state dot.** `Grid`, `Axes`, `Gizmo` and `Snap` are capabilities that can be off;
  the workplane cannot. A dot would be permanently lit or permanently dead.
- **The label carries the state**, like the NaviCube's `PERSP`/`ORTHO` indicator:
  - default: `WP: XY` in `FG` (`#D8DBE0`);
  - a standard plane: `WP: XZ` / `WP: YZ` in `FG`;
  - a bound face or custom frame: `WP: Face1` / `WP: Custom` in `CHROME_ACTIVE`
    (`#67D7FF`).
- **One button.** A click anywhere on the button opens the dielectric dropdown; there is
  no primary action to misfire when the plane is already at the default.

### Dropdown

```text
• World XY (Top)
• World XZ (Front)
• World YZ (Right)
• View Plane (Camera)
────────────────────────────
  Set from Selection
  Offset Workplane…
  Align Camera to WP
```

- Rows follow the menu matrix: transparent idle, additive `#FFFFFF14` hover, a
  `#38BDF8` left dot on the current datum, `MUTED` (`#8B919A`) for unavailable rows.
- **Set from Selection** is enabled for a planar face, a flat wire, or three vertices. It
  replaces the ambiguous `Activate workplane` item, which required geometry to already be
  hovered but lived a full-screen pointer-travel away.
- **World XY (Top)** is the reset; it replaces `Clear workplane`.

## 4. The three workflows

### 4.1 Ephemeral auto-workplane (zero clicks, during creation)

While a creation tool is active and the plane is at the default:

1. Hovering a planar face highlights it (`HIGHLIGHT_TINT`) and previews the cursor in that
   face's `(U, V)` frame (the hover grid, after `FACE_DWELL`).
2. The **first click** on the face locks the tool's projection plane to that frame for the
   session.
3. Finishing or cancelling the tool restores the previous plane (usually World XY).

### 4.2 Explicit in-situ binding (the radial wheel)

1. Right-click or `Space` over a planar face (`WheelTarget` with `SelectionAnchor.point`
   and `normal`).
2. Choose **Set WP**.
3. The shell builds the frame, sets the active plane, moves the grid and axes, and the
   pill reads `WP: Face` in `CHROME_ACTIVE`. Opening the wheel does not mutate the
   document selection.

### 4.3 Explicit datum and offset (the `WP` menu)

1. Pick `World XY`, `World XZ`, `World YZ` or `View Plane`.
2. **Offset Workplane…** spawns a single-axis manipulator along `+N` with the floating HUD
   (`Offset ΔZ [ 0.0 ] mm`), moving `frame.origin` along `frame.basis[2]`.
3. **Align Camera to WP** is `view_workplane()`: look down the plane's normal with a
   roll-free up vector (at the default this is the top view).

## 5. Mod and provider policy (no new backend ABI)

1. **Overlays reuse the `Grid` and `Axes` providers.** A mod-supplied grid or axis overlay
   consumes the viewport's frame; moving the active frame moves them. There is no
   `view.workplane.backend` claim.
2. **Handles reuse G3a.** A workbench that wants in-canvas grips to offset or tilt the
   plane declares `axis`/`ring` handles through the existing declarative manipulator seam.
3. **Parametric datums belong to workbenches.** The viewport's workplane is transient: it
   never enters the undo stack or the document tree. A workbench may add a command that
   saves the current frame as a persistent `App::Plane` object.

## 6. Verification

- **Default:** a fresh document reads `WP: XY`, no dot, no "no active workplane" text, and
  `Polyline` clicks land on `Z = 0`.
- **Wheel:** `Set WP` on an unselected face sets the frame without mutating the selection
  and moves the grid and axes on commit; `Esc` restores World XY.
- **Reset:** `World XY` in the menu restores the default frame and returns the label to
  `FG`.
- **Grid identity:** a bound plane's grid uses the same steps as the origin grid, and
  snapping uses those same steps.
- **Ephemeral:** with a tool active, the first click on a tilted face binds it, a
  subsequent click in empty space lands on that plane, and finishing restores the previous
  plane.
