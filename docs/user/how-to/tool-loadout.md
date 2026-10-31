# Switch tools with the loadout

FerroCAD's pointer carries **one tool at a time**. The bar at the bottom-left of
the 3-D view names the hand you have equipped, and `1`, `2` and `3` switch it:

| Slot | Key | A drag with this hand | `Select ▾` default |
| --- | --- | --- | --- |
| **Move** | `1` | moves or rotates the selected body with the transform triad | Body |
| **Push/Pull** | `2` | extrudes or pockets the face under the pointer | Face |
| **Fillet** | `3` | fillets or chamfers the edge under the pointer | Edge |

The active slot is lit in the bar. Push/pull and fillet appear only when the
edition provides those controls (the Part workbench does); Move is always there.

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

## Keep, commit or roll back a move

- **Release** does not commit. The object stays previewed and the 3-D measuring
  tape shows the pending distance: each dashed leg carries its own number.
- **`Enter`** (or the form's `✓`) commits the pending move as one undo step and resets
  the fields; a form with nothing pending commits nothing (and shows no `✓`/`✗` at all).
- **`Esc`** (or the form's `✗`) rolls the preview back without writing anything, so it
  spends no undo entry. With nothing pending, `Esc` clears the selection instead — the
  form and the triad go with it.
- **A pending move locks the workspace.** While the `✗`/`✓` buttons are showing,
  switching tools, changing `Select ▾` or the handle set, selecting another object,
  dragging a different body, or starting an editing command (`Create Box`) is **blocked**
  (the status line says "move pending"; the tool bar dims), so nothing commits and nothing
  is lost by a stray click. View commands such as `Grid` or `Axes` still work. Finish with
  `✓` or `✗` and the gesture goes through.

## Keys at a glance

| Key | Effect |
| --- | --- |
| `1` / `2` / `3` | equip Move / Push/Pull / Fillet; `1` again on Move pauses the hand |
| `Tab` | focus the Move form's field for the axis you last dragged (then cycle) |
| a digit, `-` or `.` | focus the remembered field and start the number |
| `=` | switch the form between offsets (`ΔX`) and coordinates (`X`) |
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
tape live, but `1`, `2` and `3` still equip tools. `Tab` — or a digit other than
`1`–`3`, `-`, `.`, a click in a field, or a click on a ruler tag — puts the keyboard in
a field. Once a field is focused its digits belong to it. `✓`/`Enter` commits and
resets the fields; `✗`/`Esc` rolls back. Neither closes the coordinates. Starting a
`Move` command (`Space` wheel or the bar) brings a paused hand back.

Click the `Δ` switch (or press `=`) to show **coordinates** instead of offsets: typing
`0` in `X` snaps the pivot onto the YZ plane, and a typed coordinate is still one
ordinary move. Type `=15` in a field to mean "X = 15" for that entry without switching
the whole form.

## Notes

- The loadout is **view state**: it belongs to the window, not the document, and
  does not change what is saved.
- `Gizmo ▾` still chooses the Move hand's mode (Translate or Rotate). The wheel's
  `Move` and `Rotate` verbs also equip the Move hand.
