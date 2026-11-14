# Transactions and undo

Status: design note (2026-11-16).

A document edit is atomic: `openTransaction(name)` … `commitTransaction()` becomes **one**
undo step, and `abortTransaction()` puts every value back before the operation returns. That
is only true if every value the edit writes is recorded. This note is the rule for which
writes are recorded, and the two ways a write escaped it.

## The rule

`Document::set_property` records a `Change` and then calls the object type's
`on_property_changed`. A type may react to that write by changing a **partner** property —
it keeps derived state in sync without a recompute:

| Write | Mirror |
| --- | --- |
| `Placement = p` on a `Part::Feature` | the stored `Shape` is re-baked by `p · old⁻¹`, so `obj.Shape` is world-space geometry |
| `Shape = s` on a `Part::Feature` with a placement | either the object adopts `s`'s own placement, or `s` is re-baked by the object's |
| `Placement = p` on a `Mesh::Feature` | the mesh transform follows, so `obj.Mesh.BoundBox` moves with it |

The mirror must go through `Document::set_property_derived`. It records the change in the
same transaction but does not touch the object, fire observers, or call the hook again — so
the pair cannot recurse and one undo restores both halves.

| Call | Records | Touches | Fires the hook |
| --- | --- | --- | --- |
| `set_property` | yes | yes | yes |
| `set_property_derived` | yes | no | no |
| `set_property_raw` | no | no | no |

`set_property_raw` remains right for **computed output**: an `execute` writing the `Shape`
it just built is the result of a recompute, not an edit, and must not add an undo step.

`Label` is not a property-container entry — it is a field on the document object — so
`Document::set_label` records `Change::Label` itself. An operation that names its result
(`Union`, `Block · 1`) is renaming as part of the edit, and a refused operation must not
leave its result renamed.

## What the two escapes looked like

Both were invisible until an operation actually needed the other half.

1. **The mirror was raw.** A `Placement` write re-baked `Shape` through `set_property_raw`.
   Committing looked right, because both properties held the new value. Undo reverted the
   placement through `restore_property_value`, which re-runs the hook — but the *stored
   geometry* was still the placed one, so `Shape.isSame(before)` was false and a body that a
   failed rotate had touched stayed where the abandoned transaction put it. In SimpleCAD's
   wording, the rotation was not atomic.
2. **The label was never recorded.** `set_label` assigned the field directly. The shape came
   back after undo; the name did not. Two document tests and the whole union document class
   read exactly that: `'Union' != 'First'`.

Recording the mirror also fixes the *redo* direction for free, because the two changes are
ordered in the log and replay in the same order.

## Restoring a value

`restore_property_value` re-runs `on_property_changed` after writing, so an undo reproduces
the side effect a normal write has. It also marks a non-`Output` property's object for
recompute, so a feature's computed `Shape` is rebuilt instead of staying stale. A label
restore deliberately does **not** touch the object: a name is not derived state.

## What this does not do

- **No whole-object snapshots.** Upstream serialises the object into the transaction; we
  record the individual changes, so a new kind of state needs its own `Change` variant. The
  variants are property, label, property slot, object add/remove and expressions.
- **No undo across documents.** A transaction belongs to one `Document`.
- **`Shape` re-baking drops the lineage.** A rigid move replaces the sub-shape handles, so
  the stored element map is dropped and positional names resolve instead; this is the same
  contract `Shape.translate`/`rotate` have.

## Tests

- `crates/ferrocad_core/src/tests/transactions.rs`:
  `abort_and_undo_restore_a_label`, `a_label_rewrite_records_no_transaction`,
  `a_derived_property_write_is_reverted_with_its_primary`.
- `crates/ferrocad_part_py/tests/part_shape_ops.rs`:
  `a_placement_move_undoes_its_shape_and_a_label_undoes_with_it` (an aborted move, a
  committed move, and a rename, each round-tripped through undo and redo).
