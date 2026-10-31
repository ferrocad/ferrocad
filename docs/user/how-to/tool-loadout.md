# Switch tools with the loadout

FerroCAD's pointer carries **one tool at a time**. The bar at the bottom-left of
the 3-D view names the hand you have equipped, and `1`, `2`, `3` and `0` switch it:

| Slot | Key | A drag with this hand | `Select ▾` default |
| --- | --- | --- | --- |
| **Move** | `1` | moves or rotates the selected body with the transform triad | Body |
| **Push/Pull** | `2` | extrudes or pockets the face under the pointer | Face |
| **Fillet** | `3` | fillets or chamfers the edge under the pointer | Edge |
| **Inspect** | `0` | selects and inspects; no handles appear, and the highlighted scope follows the pointer | Body |

The active slot is lit in the bar. Push/pull and fillet appear only when the
edition provides those controls (the Part workbench does); Move and Inspect are
always there. Pressing `1` while Move is already equipped toggles it between
Move and Rotate; from another pill, `1` equips Move with its arrows. `0` is
Inspect, its own selection hand, not a paused Move.

## Move a body without selecting it first

With **Move** equipped and nothing selected, hover a body. The triad
materialises at the body's origin in a dimmed *idle* ink — it is a preview, not a
selection. Press one of its handles and drag: the body becomes the selection and
the move opens in the same gesture. Releasing keeps the move pending, so you can
check it before committing.

If you click the body's interior instead, the body is selected and the triad
stays after the pointer leaves. A hover preview never survives on its own.

## Pick faces and edges deliberately

The header's `Select: Body ▾` control (between `Gizmo ▾` and `Snap ▾`) is the
**topology filter**. The Move hand reports the whole body even when you click a
face, which is what lets you grab and move it immediately. Equipping Push/Pull or
Fillet switches the filter to Face or Edge, so the sub-element is kept.

You can also override the filter yourself: choose `Auto`, `Body`, `Face` or
`Edge` in the `Select ▾` menu without changing the hand. The label turns cyan
while the filter differs from the hand's default. A script that selects a
sub-element reports the same filtered scope the window does.

With **Inspect** (`0`) there is no triad to tell you what a click would grab, so
the viewport highlights it instead: hovering a body paints the whole body under
`Body`, and a `Face`/`Edge` filter paints just that sub-element. The latched
selection keeps its highlight when the pointer moves away. That is the same
filtered scope a click will report. Inspect also docks a Body/Face/Edge switcher
just above the loadout bar; it reads and writes the same filter as `Select ▾`, so
the header and the in-context pill can never disagree.

## Keep, commit or roll back a move

- **Release** does not commit. The object stays previewed and the 3-D measuring
  tape shows the pending distance: each dashed leg carries its own number.
- **`Enter`** (or the form's `✓`) commits the pending move as one undo step and resets
  the fields; a form with nothing pending commits nothing (and shows no `✓`/`✗` at all).
- **`Esc`** (or the form's `✗`) rolls the preview back without writing anything, so it
  spends no undo entry. With nothing pending, `Esc` clears the selection instead — the
  form and the triad go with it.
- **Rotation follows the same rule, one axis at a time.** The wheel's `Rotate` verb switches
  the Move hand to the rings, and its bar reads `1 Rotate`. A ring drag leaves a pending
  rotation and the same flat `✗ │ ✓` strip appears beside the rings; a 3-D protractor draws
  the swept glass sector, a dashed start spoke, the live spoke and the 15° detents. The
  docked rotate bar shows `ΔθX`, `ΔθY` and `ΔθZ`, but only the pending ring's cell holds an
  angle: a rotation is an axis and an angle, not three independent coordinate fields.
  While a step is pending, **the other two rings are locked** — they stay visible but are
  muted, refuse clicks (no accidental orbit or deselection) and their bar cells show `—`.
  Finish with `✓`/`Enter` or roll back with `✗`/`Esc`; the lock is what guarantees that
  cancel always means exactly the rotation on the protractor, and that no ring switch
  commits behind your back. Dragging the same ring again adds to that angle (three 30°
  drags show 30°, 60°, then 90°). `WLD` keeps the rings aligned with the world grid, `LOC`
  turns them with the body's committed orientation, and `WP` follows the active
  workplane; the badge cycles the three and is inert while a step is pending.
  The same switch orients the **Move arrows**, so `LOC` translates along the body's own
  axes, and the `Δ` fields are read in that frame (in `LOC` an absolute coordinate is
  measured from the body's origin).
  `Reset 0°` commits the rotation that returns the body to world alignment and is
  likewise disabled while a step is pending. The button names the stored orientation:
  `Reset (Z 45°)` for a pure axis, `Reset (21.4°)` for a compound tilt (the shortest total
  angle, never Euler fractions), and it dims when the body is already at `0°`. `Tab` focuses the active `Δθ` cell; when
  nothing is pending, clicking another cell (or typing `X`, `Y` or `Z` in the field)
  chooses the ring the typed angle will use. A ring rotates the body the triad sits on;
  drag the centre disc first to re-pivot onto a vertex or edge midpoint.
- **A pending move or rotation locks the workspace.** While the `✗`/`✓` buttons are
  showing, switching tools, changing `Select ▾` or the handle set, selecting another
  object, dragging a different body, or starting an editing command (`Create Box`) is
  **blocked** (the status line says "move pending" or "rotation pending"; the tool bar
  dims), so nothing commits and nothing is lost by a stray click. View commands such as
  `Grid` or `Axes` still work. Finish with `✓` or `✗` and the gesture goes through.

## Keys at a glance

| Key | Effect |
| --- | --- |
| `0` / `1` / `2` / `3` | equip Inspect / Move / Push/Pull / Fillet; while Slot 1 is active, `1` toggles Move ⇄ Rotate |
| `Tab` | focus the Move form's field for the axis you last dragged (then cycle); on the rotate form, focus `Δθ` |
| a digit other than `0`–`3`, `-` or `.` | focus the remembered field and start the number |
| `=` | switch the translation form between offsets (`ΔX`) and coordinates (`X`); cycle the rotate rings (`WLD` → `LOC` → `WP`) when no rotation is pending |
| `Enter` | commit the pending move, or accept an open dialog |
| `Esc` | roll back the pending move; at rest, clear the selection; or close the open surface |
| `Space` | open the object-verb wheel |

With **Move** equipped and something selected, the coordinates are always on screen: a
`Δ`/`XYZ` switch and the `ΔX`, `ΔY` and `ΔZ` fields, docked at the bottom-left, just
above the loadout bar. A separate flat `✗ │ ✓` strip appears in the open space beside
the triad (it stays away from the arrows and the measuring tape); `✓` is accented cyan,
`✗` turns red on hover. It appears **only while a move is pending** (a drag or a typed
number) and hides while you are dragging: at rest there is nothing to finish, and the
3-D ruler is not drawn either.
The fields start **unfocused**: their numbers follow the triad and the 3-D measuring
tape live, but `0`–`3` still equip tools. `Tab` — or a digit other than `0`–`3`,
`-`, `.`, a click in a field, or a click on a ruler tag — puts the keyboard in
a field. Once a field is focused its digits belong to it. `✓`/`Enter` commits and
resets the fields; `✗`/`Esc` rolls back. Neither closes the coordinates. Starting a
`Move` command (`Space` wheel or the bar) leaves the pointer for the Move hand.

Click the `Δ` switch (or press `=`) to show **coordinates** instead of offsets: click the
`X` field and type `0` to snap the pivot onto the YZ plane, and a typed coordinate is
still one ordinary move. Switching `WLD`/`LOC`/`WP` keeps the same point and rewrites the
fields in the new frame, so the number changes but no `✓`/`✗` appears. Type `=15` in a
field to mean "X = 15" for that entry without switching the whole form.

With the **rotate** rings, the same dock row becomes `ΔθX ΔθY ΔθZ` plus the `WLD/LOC/WP`
switch and a `Reset 0°` button. Only the active ring's cell holds an input. `Tab` focuses
it; while
nothing is pending, clicking another cell (or typing `X`, `Y` or `Z` while the caret is
in the field) selects that ring. Once a step is pending the siblings show `—`, and the
frame switch, `Reset 0°` and the sibling cells are disabled until you apply or cancel.

## Notes

- The loadout is **view state**: it belongs to the window, not the document, and
  does not change what is saved.
- `Gizmo ▾` still chooses the Move hand's mode (Translate or Rotate). The wheel's
  `Move` and `Rotate` verbs also equip the Move hand.
