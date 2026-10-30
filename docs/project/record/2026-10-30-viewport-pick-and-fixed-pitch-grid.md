# Round log — 2026-10-30: the pick pass, the image path, and a pinned grid pitch

Status: record (code round). Companion to
[`2026-10-30-fan-out-todo-30.md`](2026-10-30-fan-out-todo-30.md), which audited the backlog
read-only; this one is what the tree changed afterwards. The backlog at the time was
[`TODO-30-10-2026.md`](TODO-30-10-2026.md); the open work carried to
[`../TODO-31-10-2026.md`](../TODO-31-10-2026.md).

## What prompted it

The round started from the 3-D text workflow and was driven by what using it exposed: a
ShapeString that would not move, a viewport that was slow with no explanation, a washed-out
Part grey, and a grid that re-spaced when the camera pulled back with spacing pinned.

## The four defects

| Symptom | Mechanism | Fix | Regression test |
| --- | --- | --- | --- |
| "The shape will not move" | `Part::Fuse/Cut/Common/MultiFuse/MultiCommon/Compound` had no `Placement` property, so there was nothing to drag **and** the move commit threw | `Placement` on all six (`8b2f8ef`) | `ferrocad_part` container tests |
| A boolean's display moves but its operands do not | A parametric boolean does not recompute from its operands' placements — only from their `Shape`s | Documented as intended, not "fixed" | — |
| 3 validation errors per frame, picking does nothing | `pick::NORMAL_FORMAT` was `Rgba32Float`, which no adapter we run can render to, so the pick pipeline failed validation silently every frame | `Rgba16Float` (11 bits of signed mantissa is plenty for a unit normal) plus `pick::read_normal`, which decodes f32 / f16 / snorm8 so the pick block size no longer has to match the colour target; the readback staging buffer is reused and dropped only on resize; `image()` clones the colour handle instead of the buffer | `ferrocad_wgpu` 63 (the format ladder and the golden two-tone), trace run shows 0 validation errors |
| "Still coarsens as I zoom out with adaptive off" | `grid::effective_steps` promotes the major by decades until it draws, and the viewport ran it for a **configured** pair too. At 30 m a configured 10/100 became 500/1000 (`grid.rs`'s own test asserted exactly that) | `grid::fixed_steps` returns a configured pair unchanged; `grid::resolve_fade` takes a level's ink to `0` between `MINOR_PIXELS` and half of it; the viewport dispatches on `policy.adaptive` in one place (`drawn_steps`) and applies the fade in `with_resolution` | `a_fixed_pitch_dissolves_instead_of_re_spacing` (stage), `a_configured_pitch_dissolves_instead_of_re_spacing` (viewport, on the drawn packet's alpha) |

Why a fade rather than "just stop promoting": a sub-pixel lattice is moiré. The pitch is the
user's setting, so the *ink* is what has to give — and that is not a new law, it is the one
the Coin shader already implements (`line_cov`'s `1 - smoothstep(0.25, 0.5, cells)`, i.e. the
same 4 px → 2 px band). Both backends now dissolve a level at the same zoom, so the native
grid and the mod stop disagreeing at distance.

The extent rules were already innocent: `minor_radius` clips the minor level's *extent*
(512 line columns, 112 dot columns) rather than its pitch, and the disc follows the eye
distance with a 1 mm floor.

## The Move session

| Report | State |
| --- | --- |
| The deferred HUD's triad is glued to the datum instead of travelling with the object | **fixed** — the emitted handle items carry `display_offset`; `a_deferred_drag_reports_previews_and_never_commits` now asserts the drawn items, and `a_deferred_hud_drag_moves_the_triad_with_the_object` drives a whole shell drag |
| Ending a Move with the button leaves the triad on screen | **fixed** — accepting the HUD ends the session (`transform_verb = false`) and sets `triad_dismissed` |
| Still glued during a real app drag | **open** — both harness flows are green, so the next step is instrumentation against a real Part document: log `stage.item(target).anchor()` before and after `after_document_edit`, and say whether the drag went through the HUD or committed straight through |

## Colour and the software path

- The parametric `DEFAULT_COLOR` was `(0.66, 0.72, 0.80)` — in a **linear** shader that
  encodes to ~0.87 sRGB, i.e. near-white. It is `(0.30, 0.34, 0.40)` now, which is the same
  mid-tone the toy and figure blues were already using.
- The light rig is three-point with fixed directions (`0.72 key + 0.20 fill + 0.08 rim`,
  ambient `0.18`), so a planar face no longer reads as a flat wash.
- When the dma-buf ring cannot start, the presenter reports why and the shell raises a
  "Software rendering" notice with copyable details. The numbers behind it, measured in a
  debug build: **2336×1388 physical, 4× MSAA**, ~0.5–0.75 s per frame, ~95 % of one core,
  because each frame is a render + ~13 MB readback + RGBA→BGRA swizzle + upload.

## The 3-D text deliverable

`Draft.make_shape_string` (over SC2's `Part.makeWireString`) and
[`../../user/how-to/text-to-3d.md`](../../user/how-to/text-to-3d.md) with
`docs/general/images/text-3d.png`, captured from the real model by
`python3 tools/generate_general_figures.py --capture --only text-3d`. The figure was
re-captured after the lighting landed, because the first one showed the near-white grey.

## Measurements at close

| Suite | Result |
| --- | --- |
| `ferrocad_stage` | 216 passed |
| `ferrocad_viewport` | 199 passed |
| `ferrocad_wgpu` | 63 passed |
| `ferrocad_gpui` | 242 passed |
| Docs gate | `userdocs --check`, `api_docs --check` and `build_book --check` clean (36 chapters); `test_docs_tooling.py` 47 passed |

The Part/Python and upstream conformance rows were **not** re-run this round; the Part
surface only gained `Placement` properties, so they were carried.
