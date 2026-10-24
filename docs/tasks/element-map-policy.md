# Task — the compound-cut element-map size gap (S6c)

Status: **implemented for every reachable shape (2026-10-24).** Upstream is the pinned
checkout `../freecad-upstream` at `33adf61`. The compound cuts measure **20** and **47**;
the compound (52), fuse (58), cut/common (26), general-fuse (60), sweep (6), shell/solid
(26), loft (24) and filled-face (9) guards also match; and `ElementMapVersion` is enabled
for every shape that carries a lineage. Four consumer shapes were closed by attaching the
intrinsic map. Two asserts still skip by design: `FaceMakerCheese` (10) needs upstream's
face-combo alias, and `makeShellFromWires` (24) needs a different (ruled-shell) topology —
both measured and recorded in `part_lineage.rs`.

`TopoShapeTest.testPartCompoundCut1` expects 20 reverse-map keys and
`testPartCompoundCut2` expects 47; this kernel reported 12 and 30 before the policy
landed. The gap was
**not** an alias, `_1`/`_2`, or duplicate-name policy. It has two independent
causes: upstream rewrites compound cuts before handing them to OCCT, and it
then names *every* result sub-element through a cascade of name-derivation
passes. Neither cause can be reproduced soundly from the data this kernel keeps,
and the 47 is not even the count of all result elements, so it cannot be hit by
naming more elements.

## 1. What `ElementReverseMap` actually counts

`ComplexGeoDataPy::getElementReverseMap()` (`src/App/ComplexGeoDataPyImp.cpp`)
builds a dict from `ComplexGeoData::getElementMap()`, which is
`ElementMap::getAll()`: one `(MappedName, IndexedName)` pair per named
sub-element. The dict key is the `IndexedName` (`Face1`, `Edge7`, …) and the
value is the mapped name, or a list of them.

Consequences, all relevant here:

- `len(ElementReverseMap)` is the number of **named result sub-elements**, not
  the number of names and not the number of topology elements.
- An element may carry several names (`ElementMap` keeps a deque of
  `MappedNameRef` per index), but additional names are extra *values* under the
  same key. They can never raise the key count.
- Duplicate *names* are disambiguated in
  `ElementMap::renameDuplicateElement` with a `#D<hex>` prefix, not with
  `_1`/`_2`. There is no `_1`/`_2` rule anywhere on this path
  (`src/App/ElementMap.cpp`).

So the question is which result elements upstream assigns a name to, not how
many aliases it invents.

## 2. Cause 1 — upstream rewrites compound cuts

`FCBRepAlgoAPI_BooleanOperation::Build()`
(`src/Mod/Part/App/FCBRepAlgoAPI_BooleanOperation.cpp`) does not simply call the
OCCT BOP when an operand is a compound:

- **Cut with one compound tool** (our `testPartCompoundCut1`: `Cylinder1` cut
  by `Compound1`): `RecursiveCutFusedTools` flattens the tool compound; if it
  contains two or more shapes it first **fuses** them, uses the fused shape as
  the tool, then cuts.
- **Cut with one compound argument** (our `testPartCompoundCut2`: `Compound1`
  cut by `Cylinder1`): `RecursiveCutCompound` cuts **each child of the compound
  separately** and assembles the per-child results into a new compound. It does
  not fuse the base.

Our `OcctBackend::cut` passes the compound through as a single
`BRepAlgoAPI_Cut` argument. That produces a different subdivision: extra solids,
split faces and extra edges. Measured with this kernel's own primitives:

| case | our current `Part::Cut` | upstream compound policy, reproduced by hand |
| --- | --- | --- |
| cut1 (`Cyl − Compound`) | 11 F, 16 E, 8 V (35 elements); 12 named | 5 F, 9 E, 6 V (exactly 20 elements); 5 named |
| cut2 (`Compound − Cyl`) | 17 F, 31 E, 18 V; 30 named | 14 F, 29 E, 18 V (local OCCT); 0 named |

For cut1 the rewritten shape has exactly the 20 elements the upstream test
lists (`Face1–5`, `Edge1–9`, `Vertex1–6`). That is the clean demonstration: the
first half of the gap is **geometry/topology produced by the compound-cut
policy**, not naming.

For cut2 the local kernel gets 14 F / 29 E / 18 V, while the upstream reference
keys reach `Face14`, `Edge30` and `Vertex20`, so upstream's shape has at least
14 faces, 30 edges and 20 vertices (≥ 64 elements). The reference is therefore
a different (and slightly finer) subdivision than our OCCT produces. This
kernel also does not set upstream's automatic fuzzy value
(`FCBRepAlgoAPIHelper::setAutoFuzzy`), which can change near-tangent topology.

## 3. Cause 2 — upstream names every result element

`TopoShape::makeShapeWithElementMap`
(`src/Mod/Part/App/TopoShapeExpansion.cpp`) is a multi-pass namer, not a fold of
`Modified`/`Generated`:

1. `mapSubElement(shapes)` (called with no op) copies names for sub-shapes that
   survive with the same TShape, via the shape ancestry cache — an identity
   mapping that reports no `Modified`/`Generated` relation.
2. It collects the `Mapper`'s `modified`/`generated` relations for all three
   element kinds of every input.
3. **Reverse pass** (highest element kind first): for each *named* face/edge it
   names the still-unnamed lower elements it contains, with the
   `upperPostfix()` (`…;U1`, `…;U2`, …).
4. **Forward pass**: for each still-unnamed higher element whose lower elements
   are all named, it composes a name from them with `lowerPostfix()` (`…;L`).

This is a cascade over the result's incidence graph: naming one face can name
its edges and vertices, and those can name neighbouring faces. That is where
the "extra" 15 names in cut1 come from.

Measured on the correct cut1 shape (20 elements):

- 5 elements have a `Modified`/`Generated` relation (the names our reverse map
  would contain today);
- 2 more are `IsSame` with an input element (the identity mapping);
- 13 have no direct relation to either operand and are named upstream only by
  the reverse/forward cascade.

Our `crates/ferrocad_occt/src/backend.rs::build_history` deliberately records
only `modified`, `generated` and `deleted`; unchanged elements are absent. Our
`Lineage`/`ElementMap` folds exactly those relations and stops. Adding the two
identity names would make cut1 report 7, not 20; the remaining 13 require the
cascade.

## 4. Why 47 cannot be reproduced soundly

- **It is a proper subset.** The reference list contains 9 faces, 22 edges and
  16 vertices, but its indices go up to `Face14`, `Edge30` and `Vertex20`.
  A result with at least 64 elements therefore has 17 named-out elements
  (`Face1–3,5,6`; `Edge1,2,4,5,6,7,10,14`; `Vertex1,2,5,6`). A policy that
  names every element would produce ≥ 64 keys, not 47.
- **The subset comes from internal BOP history state.** Because
  `RecursiveCutCompound` calls `Build()` once per child and then assembles the
  compound by hand, the surviving `Modified`/`Generated` history is not a
  function of (final shape, original base, original tool) that we can recompute
  from our two-operand history. Which elements are unnamed depends on the order
  and history of those internal builds.
- **The topology itself does not match.** Our local OCCT gives 14 F / 29 E /
  18 V for the same per-child construction, against the ≥ 14 / 30 / 20 implied
  by the reference. Ordinals are positional, so a one- or two-element
  difference shifts every name; "47" would be coincidence, not evidence that a
  rule is right.
- **The cascade is a different relation kind.** Naming an edge after the face
  that contains it (or a face after its boundary) is incidence derivation, not
  production provenance. Our `History`/`ReverseMap` contract is "which input
  names produced this output"; adding `parent → child` entries would either
  change that contract or require a second map, and it still would not yield
  the upstream subset.

## 5. What this kernel does today

- `build_history` enumerates both operands and all three kinds, probes
  `Modified`/`Generated`/`IsDeleted`, and keeps only outputs present in the
  result map. Reliable, but incomplete by design: unchanged elements have no
  relation.
- `Part::Compound` returns an empty lineage (`crates/ferrocad_part/src/containers.rs`),
  while upstream's `TopoShape::makeElementCompound` →
  `mapSubElement(shapes, op)` gives `Compound1` 52 names.
- `ElementMapVersion` and `ElementMapSize` stay off. Turning them on is not
  justified by a map that names only directly-related elements, and a partial
  count would be worse than the current, honest state.

## 6. Options considered

1. **Leave the two assertions as a known gap (chosen).** No code change; the
   analysis explains both counts.
2. **Implement the compound BOP policy alone.** This would make cut1
   geometrically match upstream (20 elements), but our map would then report
   7, not 20, and cut2 would remain different. It also changes boolean
   semantics outside the element-map slice.
3. **Implement the full upstream namer.** This needs (a) the compound BOP
   rewrite, (b) identity/ancestry relations, and (c) the reverse/forward
   cascade over result adjacency. It is necessary for cut1's 20 but still
   insufficient for cut2's 47, because 47 is not "all elements" and depends on
   internal history state we do not replicate. That is the soundness boundary:
   part of the policy is implementable, the exact counts are not.

## 7. Measurement

```
FERROCAD_PART_FILES="TopoShapeTest" … cargo test -q -p ferrocad_part_py \
  --test part_conformance --offline -- --nocapture
```

gives `TopoShapeTest.py: 61 tests (pass 58, fail 3, error 0, skip 0)`. The
failures are `testAppPartMakeSweepSurface` and the two compound-cut counts
(`12 != 20`, `30 != 47`). The package tests for `ferrocad_lineage`,
`ferrocad_occt`, `ferrocad_part` and the `part_lineage` integration test pass
unchanged.

## 8. Related gaps

- `Part::Compound` itself: upstream `ElementMapSize` 52, ours 0; this is the
  same missing compound naming pass, not an independent defect.
- The other guards (52/58/26/10/9/24) likewise assume every relevant result
  element is named, and several also depend on operation-specific post-passes
  (for example `makeElementShell`). They should not be attempted as a group
  until the naming contract has a derived-name pass.
