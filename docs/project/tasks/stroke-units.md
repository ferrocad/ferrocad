# Stroke widths: device pixels, and the UI-scale option we deferred

Status: decided (2026-10-30), no code change. Viewport strokes are measured in **device
pixels**. A logical-pixel (UI-scale) option was considered and rejected for now; this
note records why, what it would touch, and the test that pins the current contract.

## 0. The observation that raised it

While making the minor grid's style and the two stroke widths configurable, the Coin
grid's lines still looked **thicker** than the native grid's at the same numbers. The
plausible suspect was the device-pixel ratio: the native generator sizes its quads from
`scale_for`, the Coin shader feathers with `fwidth`, and it was not obvious that the two
agree on what "a pixel" is.

They do. Two separate things were going on, and only one was real:

- **Real:** the Coin shader had its own ink — `minor_alpha = opacity × 0.35`,
  `major_alpha = opacity` — while the native tokens are `0.15 / 0.15` (or `0.15 / 0.30`
  on a bound plane). A 1 px line at 4× the alpha reads as a thicker line. Fixed by sending
  the host's base ink in `Frame.workplane_steps.zw` (see
  [`../interaction/workplanes.md`](../../developer/interaction/workplanes.md) §9).
- **Not real:** a DPI mismatch. `scale_for` divides by `camera.viewport.device_size()[1]`,
  and the shader's `fwidth` is per device pixel, so a device-pixel-ratio change moves both
  backends the same way. `a_stroke_is_measured_in_device_pixels_at_any_scale`
  (`crates/ferrocad_viewport/src/inventor_view.rs`) pins this: the same 4-device-pixel
  request is half the world width on a 2× target, in both backends.

## 1. The decision

**Viewport measurement is device (physical) pixels.** This is required by the gpui
interop: the render target and the input hit test are in device pixels, and the viewport
has no notion of UI scale. Everything the viewport sizes in "pixels" is device pixels:

| Setting | Type | Where |
| --- | --- | --- |
| `FerroCAD.Grid.major_width` / `minor_width` | 1–8 px | `GridPolicy`, `GridStyle` |
| `AxisStyle.width` | device px | `crates/ferrocad_stage/src/axis.rs` |
| `AxisStyle.length` (`TRIAD_PIXELS`) | device px | same |
| `GizmoStyle.width`, `halo_width`, `hit_radius_px`, `size` | device px | `crates/ferrocad_stage/src/gizmo.rs` |
| `MIN_STROKE_PIXELS` / `MAX_STROKE_PIXELS`, `MINOR_PIXELS`, `MIN_PX` | device px | `crates/ferrocad_stage/src/grid.rs` |

The conversion to world units is one function, `ferrocad_stage::screen::scale_for`, and
the Coin shader converts back with the screen-space derivative of the same plane
coordinate. The two therefore agree by construction, not by a shared constant.

## 2. The consequence, stated plainly

On a 2× display a "1 px" hairline is **½ logical pixel** and a "4 px" stroke is 2 logical
pixels. So the grid, the triad and the gizmo look **half as thick** on HiDPI than on a 1×
display at the same settings. That is the price of device-pixel measurement; it is also
what makes a stroke a fixed number of physical pixels rather than a fraction of a CSS
pixel.

The 1 px hairline is the exception that hides the problem: a `LineList` is one *device*
pixel in WebGPU, so a hairline is thinner on HiDPI too, but a hairline's readability does
not depend on its exact logical width.

## 3. Why logical pixels were rejected for now

A UI-scale option would mean storing widths in **logical** pixels and dividing by
`device_pixel_ratio` at the host boundary. Reasons not to do it in this change:

1. **It is not a grid setting.** The same conversion would have to apply to the axis
   cross, the gizmo, the origin node, the hover bloom and every hit radius. Doing it for
   `Grid` alone would make the grid's "4 px" mean something different from the gizmo's
   "4 px", which is worse than the current uniform rule.
2. **The viewport has no UI-scale state.** `Viewport` carries `device_size` and
   `device_pixel_ratio`; the ratio is set by the gpui host per frame. A logical-pixel
   policy would add a second consumer of that ratio and a second place it can be wrong.
3. **The shader needs a new uniform.** The Coin overlay cannot read the ratio from
   `fwidth` — `fwidth` is per device pixel by definition. A logical width would need the
   ratio in `Frame` (a change to a 256-byte ABI that three shaders share) and a test that
   the native and Coin paths still agree after the division.
4. **The difference the user actually saw was ink, not units.** Fixing the units would
   not have fixed that; fixing the ink did, and a units change would have muddied the
   diagnosis.

## 4. If this is reopened

The narrowest viable form: keep the stored numbers in device pixels (no ABI change), and
let the **host** scale a user-facing "UI size" by `device_pixel_ratio` before it reaches
`GridPolicy` / `AxisStyle` / `GizmoStyle`. Then:

- one conversion point, in the shell, where the ratio already lives;
- no new `Frame` field;
- the existing tests still hold, because they operate below the conversion;
- add one test that a 2× host requests the same **logical** width as a 1× host, in both
  backends.

## 5. Related

- [`../interaction/workplanes.md`](../../developer/interaction/workplanes.md) §9 — the grid
  stroke/ink paragraph.
- [`../workbenches/grid-overlay-plan.md`](../../developer/workbenches/grid-overlay-plan.md)
  §3.4 — the `Frame` ABI, including `workplane_steps` and `grid_stroke`.
- [`../interaction/axis-origin-grid.md`](../../developer/interaction/axis-origin-grid.md) —
  the triad's own sizing, also in device pixels.
