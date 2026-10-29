# Workplane (`WP`): specification and interaction model

Status: design (2026-10-29). The state model, the `WP` menu, the three-point pick, the
offset dialog and `Save as datum plane` are built, and **W1 — the visual hierarchy** (§7–§12:
selection, triads, grid extent, hover bloom) is built. The datum-plane object (`W3`), the
offset gizmo (`W2`) and the Coin-grid uniform (`W4`) are specified and parked — see
[`../../project/tasks/workplanes.md`](../../project/tasks/workplanes.md) §8 for the work
list. Companion to [`axis-origin-grid.md`](axis-origin-grid.md) (the triad, the
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

## 7. Selection: what a click means (verified 2026-10-29)

There are no selection **modes**, and none are added. The whole state machine is:

| Gesture | What it selects |
| --- | --- |
| hover | **nothing** — `HoverReport { target, through }` plus `hover_anchor()`; the shell tints the thing under the pointer and logs `clicked through: …` |
| left click on a named part | that sub-element (`Face4`, `Edge6`) |
| left click on an unnamed part | the whole object (a mesh with no part names) |
| double click | the whole body (`click_target` promotes at `click_count >= 2`) |
| tree click | the whole body (the tree has no sub-element rows) |
| Ctrl/Shift+click | toggles whole objects by name; **no multi-element selection** |
| right-click | a *context target* for the wheel; the selection is untouched until a verb runs |

So the answer to “does hover select a face while a click selects the object?” is **no**:
hover only reports; a single click selects the face; a double click selects the body.
Both editions report `FaceN`, because the stage's part names are the kernel's element
names (the toy names one part per box face; `Part` passes `Shape.mesh()["parts"]`).

Why this matters for the rules below: everything they key on — “the selection has an
element”, “a manipulator claims it”, “a session verb is active” — already exists in this
one state. No mode switch is needed to know whether the user is working on a body or a
face; the double click *is* the switch.

Deliberately not added: a body/face/edge/vertex filter toolbar, marquee selection,
multi-element selection and named selection sets. Every shipped consumer of a sub-element
(`Set WP`, push/pull, fillet) takes exactly one, and the tree covers “select the body when
the face keeps winning”. These are limits, not omissions.

## 8. One active triad: the suppression hierarchy

The overload in a dense scene is four **roles** at one anchor, not four styles. The
hierarchy is inferred from (a) the active plane's `WorkplaneKind` and (b) what claims the
selection. It adds no setting, no token and no user-visible switch.

| Role | Drawn when | Anchor | Ink |
| --- | --- | --- | --- |
| Active axis cross (`U/V/N`) | `• Axes` on | active plane origin | the axis tokens; **no `N` for a face** (§8.2) |
| World origin ghost | `• Axes` on and a non-default plane active | world `(0,0,0)` | `WORLD_TRIAD_GHOST` = 0.15, 12 px stems, no labels, plus the origin node |
| Body transform triad | no element-anchored handle claims the selection, **or** a Move/Rotate session (§8.3, §8.4) | the body datum | `GizmoStyle` |
| Sub-element handle | a registered manipulator claims the selection's element | the element | `GizmoStyle` |
| Grid | `• Grid` on | the active plane | the grid tokens; extent per §9 |

### 8.1 One primary cross

At the default plane, the world triad **is** the active cross, so there is one. When a
non-default plane is active, the world triad recedes to `WORLD_TRIAD_DIM` (30 %) and the
active plane's cross is the only full-strength one; the origin node stays, so absolute
zero is still marked. The value is derived from the existing `AxisStyle`
(`scaled_opacity`), so nothing new is styled and there is no setting. The same constant
is used for the hover preview, so one rule covers both cases: a local layer makes the
world cross recede. `• Axes` still owns the whole layer — turning it off removes the world
cross, the active cross and the origin node together.

### 8.2 No normal stem on a face

A face-bound plane draws a **2D `U/V` bracket** (two stems and two tags) in the face and
no `+N` stem. Two reasons: the solid already says which side is outside (so `N` is
redundant there), and the normal direction is exactly what a face manipulator owns — the
push/pull stem and the offset HUD. Two arrows on one line is the collision.

Planes with no surface behind them — world standards, `View`, a 3-point frame, and an
offset of any of those — keep `U/V/N`, because nothing else shows which side is `+N`.
`AxisStyle` is unchanged (same ink, width, opacity, tags); the active layer simply does
not emit the third stem. An offset of a *face* plane keeps the face kind, so it stays a
`U/V` bracket; the offset's sign is shown by the HUD/gizmo while it is being changed, and
by the amount in the pill afterwards.

### 8.3 The body triad yields to a sub-element handle

A selection with an element shows **only** the handles that claim that element (Part:
push/pull on a face, the dress-up arc on an edge). The shared body triad is armed only
when *no* element-anchored handle exists. The test is “is there a handle for this
element”, not “is an element selected”, so a face of an object with no workbench grips
still gets the body triad — no click is ever left with zero handles.

Only the **shared** triad (the one at the body datum, `ShapeItem::anchor`) yields. A
workbench gizmo anchored elsewhere — the toy's height grip on the top centre — is
untouched. The toy is the reference case for both branches: a click on the **top** face
arms `HeightGrip` (and the triad yields); a click on a side or bottom face claims nothing
(a height handle dragged from a side face would change a dimension the user did not point
at), so the triad stays and the body can be moved or rotated from that face. A double
click always selects the whole body.

### 8.4 A verb is an explicit request

Move and Rotate arm the body triad for the selected **body** even while a face is
selected: from the radial wheel, or `Ctrl+Shift+M` / `Ctrl+Shift+R` (a global
accelerator, so it does not need a visible button). When
the session ends or the selection changes, the default (suppressed) state returns. The
suppression is therefore a *default*, not a lock — which is what keeps body transforms
reachable from any selection. The Move HUD keeps anchoring at the body datum, the same
point the triad uses.

### 8.5 The hover bloom is tool-scoped

The hover bloom (a local grid on the face under the pointer) appears only when a
**creation tool** is active **and** the active plane is still the default — because that
is when it previews something a click will actually do (bind the plane for the session).
Once the tool's first click binds a face, the plane is no longer the default, so the bloom
stops and later hovers during the session add no competing grids. With no tool, hovering
keeps the face *tint* (the click feedback) and the `HoverReport`, but adds no second grid;
`Set WP` in the wheel is one gesture away and needs no preview. The predicate is inferred
from the tool and plane state — nothing is configured.

## 9. Grid extent: one generator, inferred extent

The grid stays one generator; only its **extent** is inferred from what the plane is bound
to:

- **A plane bound to a finite surface** (a face today; a datum plane once it exists) gets
  a face-sized patch: `radius = max(3 × in-plane radius, 2 × major step)`. The world
  clamp (`RADIUS_RANGE`, 500–5000 mm) is **not** applied to it — it exists so the world
  ground does not collapse when zoomed in, and applying it would force a 40 mm face to
  carry a 500 mm grid.
- **Every other plane** (world, standard, `View`, 3-point, an offset of those) keeps the
  camera-following extent, `GridSpec::radius_for(camera)` — unchanged.
- **The fade is already there.** `grid::ground` fades alpha smoothly from
  `FADE_START × radius` to `0` at `radius`, so a smaller radius *is* the localized patch.
  No shader change, no `GridStyle` field, no new setting.
- **What the identity keeps:** same generator, same minor/major steps, same ink, same
  `1:2` ratio, same snapping steps. Only the extent says “this grid belongs to that
  face”, which is exactly the information a wall-of-grid was hiding.

State: `ActiveWorkplane` carries the bound surface's in-plane radius (`FaceFrame.radius`,
already computed when the face resolves) so the grid rebuilds without re-resolving the
stage; it survives `Offset` (the patch stays face-sized). This is inferred state, not a
preference — the pill never mentions it. The hover preview uses the hovered face's radius
with the same rule.

## 10. Datum planes: a document object with a handle (parked)

A datum plane is a **document object** that gets a manipulator when selected, not a
special always-on gizmo. It:

- lives in the tree with a name, `Visibility` (already a persisted view property that the
  display provider honours) and one undo step per edit;
- renders as a bounded planar item — a translucent fill from the theme's glass token at
  low alpha with the glass/top rim, through the existing `with_blended_surface()`; no new
  token, no shader;
- is picked through its whole area. A rim-only pick (an earlier idea) would make the
  middle unclickable, which contradicts “click it to move it”; hiding is one keystroke
  and the tree row always selects it, even behind geometry;
- when selected, arms a normal/angle manipulator through the same workbench seam push/pull
  uses (`+N` drag offsets it, a ring tilts it); both are placement edits, one transaction
  each;
- is a valid `Set WP` target, because it is a plane with one named face (`Face1`).

Why not a gizmo: a gizmo has no lifetime, no tree row, no name, no visibility, no undo, no
save, and there can be many planes. FreeCAD already types this as
`App::Plane`/`PartDesign::Plane`; matching it keeps the kernel model.

Interim: `Save as datum plane` already creates a `Part::Feature` face at the live frame —
pickable, renameable, saveable and bindable today. The full model still needs the
`Part::DatumPlane` type, its glass rendering, the normal/tilt handle, and the tree's
`Space`-to-hide shortcut.

## 11. Reachability: every workflow survives the hierarchy

| Workflow | Entry | Why the hierarchy cannot strand it |
| --- | --- | --- |
| Set WP from a face | wheel `Set WP`; menu `Set from selection` | display-only rules |
| Set WP from 3 points | menu `Pick 3 points…` | a tool, not display |
| Standards / view / offset / reset / align | `WP` menu | untouched |
| Draw on a face | a creation tool's first click | the bloom previews the bind (§8.5) |
| Move / rotate the body | body click → triad; face click → double click, or wheel `Move`/`Rotate`, or `Ctrl+Shift+M`/`R` | verb override (§8.4) |
| Push/pull a face | face click → the stem | §8.2 frees the normal, §8.3 drops the triad |
| Fillet/chamfer an edge | edge click → the arc | §8.3 |
| Adjust the toy slab's height | click its **top** face | §8.3: the grip claims it, so it is the only handle |
| Move/rotate the toy slab from a side face | click a side/bottom face | §8.3 fallback: nothing claims it, so the triad stays |
| Save a datum plane | menu / wheel | unchanged |
| Bind WP to a datum plane | select it → `Set from selection` | one named face |
| Hide/show an object | `Visibility` property now; tree `Space` parked | the tree is always reachable |
| Snap (grid / vertex / edge / face) | `Snap` menu | independent of the drawn layers |
| Navigate | orbit / pan / zoom | the grid patch no longer walls the view (§9) |

## 12. What this deliberately does not add

- **No appearance configuration.** No per-plane style, no `WP` appearance submenu, no
  axis-visibility switches: every rule above reads the plane's kind, the selection's
  granularity and whether a handle claims it.
- **No new tokens.** The ghost and face-bracket values are derived from `AxisStyle`; the
  datum glass reuses the theme's glass inks.
- **No selection modes**, marquee or multi-element selection (§7).
- **No new grid renderer or shader.** The extent is a `GridSpec.radius`; the fade already
  exists (§9).
- **No `view.workplane.backend` claim** (§5).
