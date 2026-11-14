# Part boolean and compound containers

Status: design note (2026-11-14).

`Part::Fuse`, `Part::Cut`, `Part::Common`, `Part::MultiFuse`, `Part::MultiCommon` and
`Part::Compound` live in `crates/ferrocad_part/src/containers.rs`. They are thin: the
document properties go in, the injected `GeometryBackend` does the work, and the result
is one `Part::PropertyPartShape` carrying both the geometry and its lineage. This note
records the rules those six objects follow, because several of them are upstream
behaviours that are easy to "simplify" away.

## Properties

| Type | Links | Inputs |
| --- | --- | --- |
| `Part::Fuse` / `Cut` / `Common` | `Base`, `Tool` | two |
| `Part::MultiFuse` / `MultiCommon` | `Shapes` (`LinkList`) | one or more |
| `Part::Compound` | `Links` (`LinkList`) | one or more |

All six carry `Placement` and an output `Shape`. `MultiFuse`/`MultiCommon` also carry
`Refine`; `MultiCommon` carries `Behavior` as well.

**The boolean does not apply its operands' placements.** A primitive bakes its own
`Placement` into the `Shape` it stores on recompute (`Part::Box::execute` and friends
call `GeometryBackend::place`), so a linked shape is already in world coordinates. The
boolean's own `Placement` is read by the display hook, not by `execute`, exactly as a
`Part::Feature`'s is.

## `Refine`

Upstream declares `Refine` on every boolean and defaults it to the `RefineModel`
preference, which is `true` (`FeaturePartBoolean.cpp:63`, `FeaturePartFuse.cpp:93`,
`FeaturePartCommon.cpp:101`). FerroCAD follows that default; an object restored from a
pre-0.17 document has no `Refine` property and reads as off, which is what upstream's
`Restore` does for those files.

The refine step is `GeometryBackend::refine`: the kernel's unifier
(`ShapeUpgrade_UnifySameDomain`) plus the relations that name the surviving elements.
`clean` is the same call without the history, and is what `Shape.cleaned` /
`Shape.removeSplitter` use.

The history is **geometric**, not topological: the unifier rebuilds every sub-shape it
keeps, so `IsSame` is false even for an element it did not touch, and the bridge has no
`Modified` map for it. Matching therefore compares a face's area and centre of mass (an
edge's length and midpoint; a vertex's point). An input element found exactly once in the
result becomes an identity relation, so a name recorded before the refine still resolves
after it; an element the unifier merged into a larger one, or removed, is retired. Naming
a merged face after either of its parents would be an invention, which the lineage layer
refuses to make.

Consequence worth knowing: a refine that merges nothing is free of name churn — every
face, edge and vertex keeps its name through the rebuild. A refine that *does* merge
faces loses the merged inputs' names (their geometry is no longer an element).

## `Behavior` (`MultiCommon` only)

`{"CommonOfAllShapes", "CommonOfFirstAndRest"}`, default `CommonOfAllShapes`, group
`Compatibility`, hidden — upstream's `FeaturePartCommon.cpp` declaration. The choice is
not cosmetic:

- **`CommonOfAllShapes`** intersects every shape. With exactly one linked shape that is
  a compound, the compound is **expanded into its children** first
  (`FeaturePartCommon.cpp:141`), so the common runs over the members.
- **`CommonOfFirstAndRest`** never expands: upstream hands the whole argument list to one
  kernel operation, so a lone compound is intersected as one shape (the common of one
  shape is that shape).

## Flattening

Upstream expands a **single** linked compound into its direct children, repeatedly while
the list is still one compound (`FeaturePartFuse.cpp:121`):

```text
shapes = links
while len(shapes) == 1 and shapes[0] is a compound:
    shapes = direct children of shapes[0]
```

`GeometryBackend::children` is that `TopoDS_Iterator` step. A compound that sits among
several arguments is left alone — the kernel takes it as one shape.

`MultiFuse` then refuses fewer than two shapes with upstream's
`"Not enough shape objects linked"`; `MultiCommon` accepts one (see `Behavior`).

## Lineage

The per-shape `History` (face/edge/vertex relations and their identities) is folded
forward into one `Lineage`: the first fold seeds it, each further fold applies on top, and
the refine folds last. `Part::Compound` records none — `make_compound` reports no
relations, and inventing names for sub-shapes the kernel did not relate would be worse
than an empty map.

Upstream additionally stores a `Part::PropertyShapeHistory` (`std::vector<ShapeHistory>`,
one per input) on `MultiFuse`/`MultiCommon`. FerroCAD does not: the lineage beside the
shape carries the same information in a kernel-independent form, which is the project's
standing decision that topological naming is a payload-generic lineage layer rather than a
kernel object.

## The commands

`python/ferrocad_parametric/__init__.py` registers `Part_Fuse`, `Part_Cut`, `Part_Common`
and `Part_Compound` the way upstream's `Part/Gui/Command.cpp` does, over
`BOPTools.BOPFeatures`:

| Command | Object | Selection |
| --- | --- | --- |
| `Part_Fuse` | `Part::MultiFuse` named `Fusion` | ≥ 2 shapes, or one compound holding ≥ 2 |
| `Part_Cut` | `Part::Cut` named `Cut` | exactly 2; the first is `Base` |
| `Part_Common` | `Part::MultiCommon` named `Common` | ≥ 2 shapes, or one compound |
| `Part_Compound` | `Part::Compound` named `Compound` | ≥ 1 |

Each runs as one transaction, copies the first operand's `DisplayMode`/`Color` (the
booleans), and hides the operands behind the result (the booleans only — a compound
leaves its links visible). `Part_Compound` keeps selection order and drops repeated picks.

One deliberate difference: upstream warns before a boolean over non-solids and waits for a
modal answer. A headless run has to answer for the user, so the command proceeds and names
the non-solid operands in its status instead of refusing.

## Open questions

- **A boolean result is a `Compound` wrapping one `Solid`.** `backend.fuse` returns
  whatever `BRepAlgoAPI_Fuse::Shape()` gives, and in this OCCT the result of fusing two
  overlapping boxes is a compound of one solid. Upstream's own answer is not verified
  here; nothing in the conformance suite pins it, and SimpleCAD unwraps
  (`result.Solids[0]`). Do not "fix" it by hand without checking upstream first — a
  single-solid unwrap would change every boolean's `ShapeType`.
- **Tree nesting.** A `Part_Compound`/`MultiFuse` shows as one tree row; upstream's
  `getSubObjects` nesting of the operands is not implemented.

## Tests

- `crates/ferrocad_part/src/lib.rs`: `refine_unifies_the_seam_a_boolean_leaves`,
  `a_lone_compound_multi_common_flattens_to_its_children`,
  `a_lone_compound_multi_fuse_flattens_to_its_children`, `a_multi_fuse_needs_two_shapes`,
  `booleans_store_a_positional_lineage`, `multi_booleans_fold_their_linked_boxes`.
- `crates/ferrocad_occt/src/backend.rs` (`provenance_tests`):
  `a_refine_keeps_every_name_it_does_not_merge`, `a_container_names_its_own_elements`.
- `tests/test_part_booleans.py`: the four commands end to end (selection rules, wiring,
  hidden operands, one undo step), `Refine` on/off, and `Behavior`.
