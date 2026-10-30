# Round log — 2026-10-29/30: the workplane hierarchy, datum planes and the grid

Status: record. Closes the **workplane and grid** work of
[`TODO-29-10-2026.md`](TODO-29-10-2026.md); the specs are
[`../tasks/workplanes.md`](../tasks/workplanes.md) and
[`../tasks/polyline-vertex-editing-and-active-tool-hud.md`](../tasks/polyline-vertex-editing-and-active-tool-hud.md),
the surface contracts are [`../../developer/interaction/workplanes.md`](../../developer/interaction/workplanes.md)
§7–§12 and [`../../developer/interaction/axis-origin-grid.md`](../../developer/interaction/axis-origin-grid.md),
and the one deferred decision is [`../tasks/stroke-units.md`](../tasks/stroke-units.md).

## Goal

The backlog was opened SimpleCAD-first, but the round's visible defects were in the
spatial reference: a workplane that could only be changed through a menu that first
required a face under the pointer, a grid whose subdivisions moved when the camera tilted,
and a tool layer that could not finish what it started. The acceptance was behavioural —
**a plane that is invisible at the world origin and obvious when it is not**, a grid whose
drawn pitch is the pitch the user set, and a polyline that commits.

## Delivered

| Slice | What landed | Tests |
| --- | --- | --- |
| **W1 — plane state** | `Workplane::world()` as a real state; `WP` becomes a selector (`world:xy\|xz\|yz`, `camera`, `selection`, `offset` + mm, `set` + frame); sticky/ephemeral binding; a creation tool binds the face it starts on; `Set WP` in the radial wheel; `Pick 3 points…` | `ferrocad_stage` workplane suite; shell viewport tests |
| **W2 — datum planes** | `Part::DatumPlane` (`crates/ferrocad_part/src/datum.rs`) with attachment; `Save as datum plane` creates it; the offset stem (`ferrocad_stage::workplane_stem`) with a session and readout | `test_datum_plane.py` 10; `ferrocad_part` datum tests |
| **W3 — program frame** | `graphics::WorkplaneFrame`, `ViewSpec.workplane`; `Frame::workplane_steps` and `grid_stroke`; the WGSL reads both, so the Coin grid follows the active plane without a mod parameter | `a_face_workplane_clamps_grid_radius_to_face_extent`, `the_grid_stroke_reaches_the_layer_and_the_program_frame` |
| **W4 — grid** | Adaptive spacing (`effective_steps`), `MinorStyle` (dots/lines), major/minor stroke widths in device pixels, one `#94A3B8` ink; `FerroCAD.Grid`, both grid mods and the shared shader | `test_grid_api.py`, `test_grid_overlay_coin.py`, `test_grid_overlay_fork.py`, `test_grid_provider_facade.py` — **62 tests** |
| **W5 — polyline** | In-flight vertex editing, one active-action HUD, `Tool::finish` returning a *creating* `InteractionCommit` | `ferrocad_stage` tool suite |
| **W6 — shell** | Provider bootstrap after startup, `SetResult.view` applied instead of dropped, `refresh_providers`, the offset-stem session/HUD, a new tree suite | `ferrocad_gpui` 239 lib tests |
| **P1.3** | `Shape.project` is upstream's normal projection (`BRepAlgo_NormalProjection`) | `part_api_gaps.rs` on a plane and a cylinder |

## The four grid defects

All four were reported from use, not from a test, and each had a different mechanism. The
pattern is worth keeping: *a symptom that reads like a style problem was a geometry-budget
problem in three of the four cases.*

| Symptom | Mechanism | Fix | Regression test |
| --- | --- | --- | --- |
| “Spacing changes as I tilt” | `effective_steps` promoted **both** levels when the projected spacing crossed the cull; tilting compresses the plane, so a tilt could move the whole lattice | Only the **minor** is coarsened (up the 1-2-5 ladder, `coarser_step`); the **major** climbs decades only when it is itself unresolved (`MAX_PROMOTIONS = 12`) | `a_zoomed_out_view_coarsens_the_minor_level`, `an_adaptive_step_climbs_until_it_is_resolved` |
| “Opacity only dims the minor grid” | Wide strokes were emitted as triangles through the opaque `REPLACE` pipeline, which ignores the vertex alpha that carries the radial fade and the opacity multiplier | Wide strokes ask for `BLENDED_DEPTH`, the renderer's blended scene-triangle variant | `a_wide_stroke_is_an_in_plane_quad`, `a_wide_dot_is_an_in_plane_square` |
| “No minor lines” | `minor == major` has no rows of its own — every row at that step *is* a major row — so `minor_style` and `minor_width` silently did nothing | The facade keeps the pair a **decade apart**: writing the major equal to the minor lowers the minor, and a stored degenerate pair is healed on read | `a_minor_level_equal_to_the_major_draws_nothing` (generator), `test_a_major_equal_to_the_minor_lowers_the_minor_a_decade` (facade) |
| “1 mm drawn at 5 mm”, and “still coarsens as I zoom out with adaptive off” | The **extent** budget coarsened the **pitch**: a 500 mm radius floor made a small part's adaptive step ~5 mm, and the 112-column dot budget coarsened a configured step to fit the vertex count | The radius floor is 1 mm (`RADIUS_RANGE = (1, 5_000)`), the generator is linear (a capped `FADE_SEGMENTS` per line, not one vertex per cell), and the minor level is **clipped in extent** (`minor_radius`: 512 columns of lines, 112 of dots) rather than coarsened or dropped | `the_radius_follows_the_camera_and_is_only_capped_far_away`, `a_dense_minor_level_is_clipped_not_refused`, `the_dot_field_fits_its_point_budget`, `adaptive_spacing_follows_the_extent_and_off_uses_the_configured_pair` |

The last row is the one that took a second pass. With `adaptive` **off** the two configured
numbers must be what is drawn, so no *extent* rule may touch them — not the radius, not the
vertex budget. (The one exception is the ~4 px screen-space cull, which promotes a level
that cannot be resolved at all; that is what it is for.) Clipping the level's *extent*
(where it fades out) is the only lever that leaves the pitch alone, and in practice it is
invisible because it only bites once the step is far below `MINOR_PIXELS`.

## Deferred decisions

1. **Strokes are device pixels, not logical pixels.** The viewport's stroke widths (and the
   grid's `major_width`/`minor_width`) are device pixels because the gpui interop works in
   device pixels. A logical-pixel/UI-scale option was considered and rejected for now; the
   reasons and the reopening plan are in [`../tasks/stroke-units.md`](../tasks/stroke-units.md).
2. **`RADIUS_RANGE` is `(1, 5_000)` mm**, so a 1 mm part is not forced onto a 500 mm disc.
   The upper bound still clips a far zoom rather than growing without limit.
3. **The external mod still lags.** `ferrocad-grid-overlay` carries an
   `InitGui.py` with the old six knobs and its own backend id; its shader is byte-identical
   to the in-tree one again, but porting the settings UI is a separate change.
4. **The shared grid pair is healed, not refused.** A programmatic write of `minor == major`
   is accepted and corrected rather than rejected, because a parameter file written by an
   older version must still open.

## Measurements

| Suite | At close (2026-10-30) |
| --- | --- |
| Rust lib, 14 crates | **1169 passed, 0 failed** — gpui 239, stage 215, viewport 198, wgpu 61, part 49, graphics 38, inventor 25, toy 18, widgets 65, mesh 63, occt 63, core 89, curves 33, geom 13 |
| Python, extension sets | **687 / 8** base, **738 / 3** with Part |
| Grid + datum-plane Python suites | **72 passed** (four grid suites and `test_datum_plane`) |
| SimpleCAD integrated tree | **969 passed, 64 failed, 26 errored, 2 skipped**, 88/88 modules — unchanged from the opening re-measure |
| Part API audit | **252 / 408 (61.8%)**; object types **40** (was 39), the gain being `Part::DatumPlane` |
| Docs | book 35 chapters, 8 generated user pages / 6 generated images (10 image files), 7 general figures |

The SimpleCAD tree being *unchanged* is the useful result: the round added a workplane
state, a document object type and a grid rewrite, and none of it moved the compatibility
baseline. The Part audit moved by exactly the one type it should have.

## Deviations and notes

- **The `parts/` count moved without a code change.** The Part contract pinned file and
  line numbers, so the re-pin after `Part::DatumPlane` also refreshed the line numbers of
  the `not_implemented` markers that had shifted in `curves.rs`. The contract diff is one
  name, one constant and those offsets — no behavioural claim.
- **`minor == major` is legal at the generator level.** `grid::build` draws nothing for the
  minor level in that case, and the test asserts it; the healing lives in the facade rather
  than in the generator, so a Rust caller still sees exactly what it asked for.
- **The workplane grid spec that said 0.5–5 m and “one decade” was stale.** The code moved
  to a 1 mm floor and repeated promotion; [`axis-origin-grid.md`](../../developer/interaction/axis-origin-grid.md)
  §3.2 was corrected in this close-out.

## Pointers

- The closed backlog is [`TODO-29-10-2026.md`](TODO-29-10-2026.md); the open remainder is
  [`../TODO-30-10-2026.md`](TODO-30-10-2026.md).
- The specs are still active (each has a named tail):
  [`../tasks/workplanes.md`](../tasks/workplanes.md) §8,
  [`../tasks/polyline-vertex-editing-and-active-tool-hud.md`](../tasks/polyline-vertex-editing-and-active-tool-hud.md) §6.
- The visual contract is [`../../developer/interaction/workplanes.md`](../../developer/interaction/workplanes.md);
  the grid's rules are [`../../developer/interaction/axis-origin-grid.md`](../../developer/interaction/axis-origin-grid.md)
  §3.
