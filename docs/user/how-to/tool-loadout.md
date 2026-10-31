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
  tape shows the pending distance.
- **`Enter`** commits the pending move as one undo step.
- **`Esc`** rolls the preview back without writing anything, so it spends no
  undo entry.
- **Switching the hand or the filter**, selecting another object, or starting a
  command **commits** a pending move first, then continues. You cannot lose a
  drag by changing what you are working on.

## Keys at a glance

| Key | Effect |
| --- | --- |
| `1` / `2` / `3` | equip Move / Push/Pull / Fillet |
| `Tab` | open the Move pill's ΔX/ΔY/ΔZ fields (or cycle them) |
| `Enter` | commit the pending move, or accept an open dialog |
| `Esc` | roll back the pending move, or close the open surface |
| `Space` | open the object-verb wheel |

`1`, `2` and `3` are the tool keys. To type a numeric value that starts with one
of them, press `Tab` first (or click the field): once the Move pill's fields are
open, digits type into them.

## Notes

- The loadout is **view state**: it belongs to the window, not the document, and
  does not change what is saved.
- `Gizmo ▾` still chooses the Move hand's mode (Translate or Rotate). The wheel's
  `Move` and `Rotate` verbs also equip the Move hand.
