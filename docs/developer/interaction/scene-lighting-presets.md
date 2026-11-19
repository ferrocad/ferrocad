# Scene and lighting presets: the `Scene ▾` bundle, the light rig and the readout

Status: design (2026-11-13). Companion to [`visual-language.md`](visual-language.md)
(canvas §1.1, surface glass §1.3, chrome §1.5, lighting §4, non-goals §10) and
[`axis-origin-grid.md`](axis-origin-grid.md) (the Z-up frame the light and the grid
share).

This note specifies the compile-time **Scene & Lighting presets** and the `Scene ▾`
viewport popover. It preserves the core constraints of `visual-language.md`: **no
user-defined colour pickers, no second lighting path, no post-processing (SSAO/bloom),
and no runtime transparency pass** (V2c remains deferred).

---

## 0. Principles

1. **Compile-time bundles, not colour pickers.** Because `GLASS_TOP` and
   `GLASS_SHADE` are opaque composites over the canvas void (`visual-language.md`
   §1.3), changing the background in isolation breaks the dielectric illusion. A theme
   is a locked tuple of
   `(canvas_void, canvas_far, glass_top, glass_shade, edge_front, edge_occluded)`.
2. **One light, one shader.** Lighting controls manipulate the parameters of the
   existing directional rig and the V2b normal threshold (`visual-language.md` §4). No
   point lights, MatCaps or secondary shaders are introduced.
3. **Zero-modality segmented chrome.** The `Scene ▾` popover uses the same
   `ui::WHEEL_GLASS` (`rgba(15, 18, 24, 0.75)`), `ui::DOCK_BORDER` (`#FFFFFF1F`) and
   1 px `ui::CHROME_LINE` (`#FFFFFF14`) segmented strips as the `PERSP ┆ ORTHO` bar
   (`visual-language.md` §1.5).

The whole slice is one small flow: a preset id selects a token table, a rig id selects
the key vector, and both are pushed down to the renderer.

```
Scene ▾ ─┬─▶ ScenePresetId ──▶ ScenePresetTokens ─┬─▶ background.wgsl   (void, far)
         │                                         ├─▶ surface ink      (top, shade)
         │                                         ├─▶ edge ink         (front, ghost)
         │                                         └─▶ grid alpha scale
         └─▶ LightRig ────────▶ Globals           (key vector, threshold, ghost flag)
```

---

## 1. Scene preset tokens (`ferrocad_theme`)

Each `ScenePreset` defines the sRGB source tokens and exposes linear `[f32; 4]` at the
`ferrocad_theme::draw` boundary, exactly as the existing constants do. A packed
`0xRRGGBBAA` view belongs in `ferrocad_theme::ui` if the chrome ever needs to place a
sample of the token in the popover.

| Token role | `VoidGradient` (default) | `FlatVoid` | `Inspection` (high glare) |
| --- | --- | --- | --- |
| `canvas_void` (top) | `#0F1218` | `#0F1218` | `#1A202C` |
| `canvas_far` (bottom) | `#161B22` | `#0F1218` | `#2D3748` |
| `glass_top` (opaque composite) | `#555C67` | `#555C67` | `#647082` |
| `glass_shade` (opaque composite) | `#151A24` | `#151A24` | `#232B3A` |
| `edge_front` | `#D6E0F0` @ 0.62 | `#D6E0F0` @ 0.62 | `#38BDF8` @ 0.90 |
| `edge_occluded` | `#D6E0F0` @ 0.10 | `#D6E0F0` @ 0.10 | `#38BDF8` @ 0.22 |
| `grid_major_alpha_scale` | `1.0` | `1.0` | `1.35` |

The first two presets reproduce today's tokens verbatim, so `VoidGradient` is the
default and the current readbacks keep their expected values. `FlatVoid` differs only
in `canvas_far`: it removes the vertical falloff, which is what makes a flat backdrop
read as paper rather than as a room.

### Why `Inspection` replaces a traditional "light theme"

In high-glare environments (shop floors, tablets under overhead bay lighting), the
default `#0F1218` void suffers from specular screen reflections, while a white paper
background washes out `#38BDF8` selection highlights and turns opaque glass composites
into muddy dark blocks. `Inspection` raises the canvas luminance floor to slate
(`#1A202C → #2D3748`), re-composites the dielectric facets over that brighter floor
(`#647082` / `#232B3A`), and shifts `edge_front` to high-contrast cyan
(`#38BDF8` @ 0.90) so silhouette edges remain razor-sharp through glare.

The bundle is deliberately **not** a user-switchable background: a paper canvas with
the default glass composites is the failure mode this design avoids.

---

## 2. Lighting and diagnostic state (`ferrocad_graphics` / `ferrocad_stage`)

The scene shader (`visual-language.md` §4) consumes one `SceneLightingUniform` per
view, updated from the viewport's view state.

### 2.1 Light direction rig

The base key vector in the Z-up frame is `normalize(vec3(0.4, 0.6, 0.8))`; the light is
two-sided (`abs(dot(normal, light))`) and there is no back-face culling.

| `LightRig` | Key vector (Z-up) | Readout |
| --- | --- | --- |
| `TopLeft` (default) | `normalize(0.4, 0.6, 0.8)` | `AZI 315° ┆ ELV 35°` |
| `TopRight` | a quarter turn about Z: `normalize(0.6, -0.4, 0.8)` | `AZI 045° ┆ ELV 35°` |
| `Headlamp` | the base offset transformed by the inverse view rotation, so the key stays locked to the camera frustum while orbiting | the view's azimuth |
| `Custom { azimuth_deg, elevation_deg }` | spherical → cartesian about Z-up, azimuth stepped in `15°` increments | `AZI {:03}° ┆ ELV {:02}°` |

The readout is a fixed tabular monospace pill at `11 px` so the digits never jump
(`visual-language.md` §0.4, §2). It is **derived** from the key vector, never a
literal: the samples above are the design's labels, and the convention that produces
them is one of the open decisions (§5.2).

### 2.2 Surface and occlusion toggles

**Facet split (`bi_tone: bool`).**

- `true` — **Bi-Tone V2b**, the default. The fragment shader selects `glass_top` when
  `dot(normal, vec3(0.0, 0.0, 1.0)) > threshold`, else `glass_shade`, with
  `threshold = 0.5`.
- `false` — **Uniform V2a**. The same shader runs with `threshold = 1.1`, which no
  normal can exceed, so every facet takes `glass_shade` and only
  `abs(dot(normal, light))` produces intra-facet gradation.

Passing a threshold rather than branching keeps one fragment stage and one pipeline
for both modes.

**Occluded edges (`ghost_edges: bool`).**

- `true` — the default. Emit the `GHOST_DEPTH` (`DepthCompare::Greater`, write-off)
  wire packet for document `ShapeItem`s, inked with `edge_occluded` (`10 %` in the void
  presets, `22 %` in `Inspection`).
- `false` — suppress the `Greater` wire pass on document shapes, leaving one clean
  exterior silhouette. Overlay and gizmo packets are unaffected: they never opted into
  the ghost pass in the first place.

---

## 3. Chrome layout (`ferrocad_shell_core::ui::chrome`)

1. **Top-left trigger.** Append `Scene ▾` to the ambient toolbar immediately after
   `Snap`, inside `chrome::viewport_header`. It is a **selector**, not a capability:
   like `Select ▾` the label carries a value instead of running a command, and both
   the label and the caret open the popover. Body: `ui::DOCK_BG` (`#16181D` @ 0.90),
   `ui::DOCK_BORDER` (`#FFFFFF1F`), 11 px sans (`CHROME_FONT`).
2. **Popover container.** Anchored below `Scene ▾`, `280 px` fixed width,
   `ui::WHEEL_GLASS` body, 1 px `ui::DOCK_BORDER`, no drop shadow and no backdrop
   (`visual-language.md` §1.5), so the canvas beneath still receives pointer events.
3. **Sections**, separated by 1 px `ui::CHROME_LINE`:
   - `THEME` (11 px `ui::MUTED` header): a three-segment strip
     `[ Void Gradient ┆ Flat Void ┆ Inspection ]`. The active segment carries
     `ui::CHROME_ACTIVE_BG` (`#1B2B38` @ 0.33) fill, `ui::CHROME_ACTIVE` (`#67D7FF`)
     ink and a 1 px `ui::ACTIVE_ACCENT` (`#38BDF8`) border.
   - `LIGHT DIRECTION`: a four-segment strip
     `[ Headlamp ┆ Top-Left ┆ Top-Right ┆ Custom ]`, with the right-aligned 11 px
     tabular monospace pill `AZI 315° ┆ ELV 35°` beneath it.
   - `OCCLUSION / EDGES`: two toggle rows, `Occluded Edges` and
     `Facet Split: Bi-Tone (V2b)`, using the cyan pill switch
     (`ferrocad_widgets::toggle`, `ui::ACTIVE_ACCENT` when checked). The occluded-edge
     label follows the active preset: `Ghost (10%)` under the void presets,
     `High Contrast` under `Inspection`.
4. **Status bar feedback.** A preset change calls `Shell::set_status` with a one-line
   message, e.g. `"High Contrast Viewport applied"`, matching the existing
   command-status toast. The trigger's own label stays `Scene`; the state lives in the
   popover and the status toast, never as a colour swatch.

The segmented strip is the one new shared control: the `PERSP ┆ ORTHO` strip is a
private `projection_button` in `ferrocad_viewport::inventor_view`, so lift an
equivalent into `ferrocad_widgets` (or generalise the `decision` strip) rather than
copying it twice. The toggle rows already exist as `ferrocad_widgets::toggle`.

---

## 4. What the code does today

| Piece | Today | This slice |
| --- | --- | --- |
| Theme tokens | `ferrocad_theme::draw` holds single constants (`CANVAS_VOID`, `CANVAS_FAR`, `GLASS_TOP`, `GLASS_SHADE`, `EDGE_FRONT`, `EDGE_OCCLUDED`); `ferrocad_theme::ui` holds the chrome palette | add `ScenePresetId` and `ScenePresetTokens`; `VoidGradient` reuses the existing constants, so the default look does not move |
| Background | `background.wgsl` hard-codes both linear colours and the pipeline binds no group | add a small uniform so the gradient follows the preset; keep the full-screen triangle |
| Lighting | `scene.wgsl` is a three-point rig: key `0.72`, fill `0.20`, rim `0.08`, ambient `0.18`, all authored in the Z-up frame. The uniform (`UniformData`) is per draw, so the light has no per-view home yet. `visual-language.md` §4 specifies **one** light at ambient `0.25` | `LightRig` selects the key; give the light and threshold a per-view uniform and reconcile the rig and the ambient (§5) |
| Facet split | `Globals.two_tone` is `u32::from(shade.a > 0.0)` and `two_tone == 0` means *single colour*; `shade.a == 0` also means single colour | replace with the threshold uniform (`0.5` / `1.1`); note the polarity flip — `bi_tone == false` now means "all facets shade", not "use the surface colour" |
| Ghost edges | `ShapeItem::with_ghost_edges()` opts a packet in and `emit` always draws the `Greater` pass once set; the display provider opts document solids in | gate emission on the viewport's `ghost_edges` flag at scene compile (a `recompile()` like `set_grid`), leaving the packet flag as the document opt-in |
| Toolbar | Grid, Axes, WP, Gizmo, Select and Snap are built in `ferrocad_shell_core::ui::chrome::viewport_header`, not in `ferrocad_viewport` | `Scene ▾` joins that row; the popover is a shell control, shared by desktop and web |
| Status line | `Shell::set_status` | preset messages go there |

The viewport already owns this kind of state: `InventorView::set_grid`,
`set_grid_alpha` and `set_axis_cross` are session settings that rebuild the scene
(`recompile()` directly, or `sync_grid()` for the axis cross). The preset and rig
belong beside them.

---

## 5. Decisions to settle before implementation

1. **One light or the shipped three-point rig.** §2.1 and `visual-language.md` §4
   describe a single directional light; `scene.wgsl` ships a key/fill/rim rig with
   ambient `0.18`. Recommend rotating the **whole rig** by the `LightRig` (the fill and
   rim keep the pale facets from going flat), and treating `AZI`/`ELV` as describing
   the key only; collapsing to one light would change the glass look and both
   facet readbacks. Either way, update §4 and this note together.
2. **The readout convention, and its sample values.** `normalize(0.4, 0.6, 0.8)` is
   elevation `≈ 48°` (`asin 0.743`) and azimuth `≈ 56°` from +X toward +Y, so the
   `AZI 315° ┆ ELV 35°` sample in §2.1 does not derive from that vector; likewise
   `(0.6, -0.4, 0.8)` is a **−90°** turn about Z, not +90°. Pin one spherical ↔
   cartesian function (`x = cos e · cos a`, `y = cos e · sin a`, `z = sin e`, with the
   handedness stated) with a round-trip test, derive the vector *and* the pill from it,
   and let the sample read whatever that function says. A decorative literal will
   disagree with `Custom` the first time someone drags the numbers.
3. **`Custom` has no editor.** The strip offers a `Custom` seat but no control changes
   its two numbers. Ship a stepper pair beside the readout (`AZI ‹ 315° ›`, 15° steps)
   or a drag on the pill; do not ship a seat that cannot be changed.
4. **`grid_major_alpha_scale`.** `InventorView::grid_alpha` already multiplies every
   grid alpha (world and workplane, minor and major). The preset asks for a
   **major-only** × 1.35. Recommend a separate major-level scale (`GridStyle` already
   carries `major_width`/`minor_width`); if the readback shows the minor dots are
   invisible under glare anyway, folding it into `grid_alpha` is acceptable and
   smaller.
5. **Where the state lives.** Session state on the viewport, pushed down each frame —
   not a document property. No undo entry, no `FerroCADGui.ViewState` mirror with
   document semantics, no `.FCStd` field. `ShapeItem` stays preset-free: it carries
   colours, not a preset id, so a preset switch repaints without rebuilding the
   document's display packets.

---

## 6. Coding agent prompt (copy-paste ready)

```markdown
Implement the Viewport Scene & Lighting Presets slice across `ferrocad_theme`,
`ferrocad_graphics`, `ferrocad_stage`, `ferrocad_viewport` and
`ferrocad_shell_core`, strictly adhering to `visual-language.md`.

### Constraints (from `visual-language.md`)
- NO colour pickers, NO user palettes, NO post-processing (SSAO/bloom), NO second
  shader, NO second light rig.
- All tokens are compile-time sRGB values in `ferrocad_theme`, converted with
  `linear_from_srgb_hex` for wgpu (`draw`) and packed `0xRRGGBBAA` for gpui (`ui`).
- Numbers in the UI (`AZI` / `ELV`) use `"monospace"` at `11.0` px, so advance widths
  never jump.

### Step 1 — `ferrocad_theme` (`ScenePreset` tokens)
1. Define `pub enum ScenePresetId { VoidGradient, FlatVoid, Inspection }` with
   `Default = VoidGradient`.
2. Define `pub struct ScenePresetTokens` holding `canvas_void`, `canvas_far`,
   `glass_top`, `glass_shade`, `edge_front`, `edge_occluded` and
   `grid_major_alpha_scale`, plus `ScenePresetId::tokens()`.
3. Populate the three tables:
   - `VoidGradient`: void `#0F1218`, far `#161B22`, top `#555C67`, shade `#151A24`,
     front `#D6E0F0` @ 0.62, ghost `#D6E0F0` @ 0.10, grid 1.0.
   - `FlatVoid`: identical except `canvas_far = #0F1218`.
   - `Inspection`: void `#1A202C`, far `#2D3748`, top `#647082`, shade `#232B3A`,
     front `#38BDF8` @ 0.90, ghost `#38BDF8` @ 0.22, grid 1.35.
4. Re-export the `VoidGradient` values as the existing constants so every current
   call site reads the default preset.
5. Unit tests: each preset round-trips linear ↔ sRGB, and each edge token keeps its
   RGB while alpha differs.

### Step 2 — `ferrocad_graphics` / `ferrocad_stage` / `ferrocad_wgpu`
1. Add `LightRig { Headlamp, TopLeft, TopRight, Custom { azimuth_deg: u16,
   elevation_deg: u16 } }` and a `SceneStyle { preset, light, bi_tone, ghost_edges }`
   to the viewport view state. Publish a derived `key_vector()` and an
   `azimuth_elevation()` pair from one spherical ↔ cartesian helper, with a
   round-trip test.
2. Add the key vector, the V2b threshold and the ghost flag to the renderer's
   **per-view** uniform. `UniformData` in `uniforms.rs` is per draw (dynamic offset)
   and its layout is shared with `pick.wgsl`, so either grow that block and its stride
   in both shaders, or add a small per-view block so a preset switch is one write
   rather than every packet. `scene.wgsl` keeps one fragment stage: it selects
   `glass_top` when `dot(normal, up) > threshold`, else `glass_shade`, and keeps the
   two-sided `abs(dot(normal, light))` gradation. Pass `0.5` for `bi_tone` and `1.1`
   for uniform (V2a).
3. Give `background.wgsl` a small uniform with `void` and `far` from the active
   preset (the pipeline currently binds no group).
4. Gate the `GHOST_DEPTH` (`DepthCompare::Greater`, write-off) wire emission on
   `SceneStyle::ghost_edges` at scene compile, and take `edge_front` /
   `edge_occluded` from the active preset. `ShapeItem::with_ghost_edges()` stays the
   document opt-in; overlays and gizmos still never opt in.
5. `Headlamp` transforms the base offset by the inverse view rotation; `TopRight`
   applies the pinned quarter-turn about Z; `Custom` builds its vector from the
   spherical helper.
6. Apply `grid_major_alpha_scale` to the major grid level only.

### Step 3 — `ferrocad_shell_core::ui::chrome` (`Scene ▾` popover)
1. Add a `Scene ▾` selector button to `viewport_header`, immediately after `Snap`
   (`select_button` is the shape: a value label plus a caret, both opening the
   popover). Use a distinct popover key, e.g. `"scene"`.
2. Render a `280 px` popover in `ui::WHEEL_GLASS` with a 1 px `ui::DOCK_BORDER` and
   1 px `ui::CHROME_LINE` dividers, containing:
   - `THEME`: a three-segment strip (`Void Gradient`, `Flat Void`, `Inspection`) with
     `ui::CHROME_ACTIVE_BG` fill, `ui::CHROME_ACTIVE` ink and a 1 px
     `ui::ACTIVE_ACCENT` border on the active segment.
   - `LIGHT DIRECTION`: a four-segment strip (`Headlamp`, `Top-Left`, `Top-Right`,
     `Custom`) plus the right-aligned 11 px `"monospace"` pill
     `AZI {:03}° ┆ ELV {:02}°`, derived from the key vector.
   - `OCCLUSION / EDGES`: `ferrocad_widgets::toggle` rows for `Occluded Edges`
     (labelled `Ghost (10%)`, or `High Contrast` under `Inspection`) and
     `Facet Split: Bi-Tone (V2b)`.
3. Lift the segmented strip into `ferrocad_widgets` (or generalise
   `ferrocad_widgets::decision`) so the `PERSP ┆ ORTHO` bar and this popover share one
   implementation.
4. Call `Shell::set_status` on a preset change, e.g.
   `"High Contrast Viewport applied"`, and push the preset and rig down through the
   viewport's `set_*` + `recompile()` path (the model for this is `set_grid`).

### Step 4 — Verification
- Keep `the_background_is_the_canvas_gradient` and
  `a_two_tone_surface_reads_back_as_glass_top_and_shade` passing under the default
  `VoidGradient` + bi-tone state.
- Add a headless GPU readback asserting that `FlatVoid` produces `#0F1218` at both the
  top and bottom canvas pixels.
- Add a readback asserting that `bi_tone: false` renders an upward-facing facet in
  `glass_shade`.
- Add an `Inspection` readback: both facet composites over the slate floor, plus the
  cyan edge tokens.
- Add chrome tests for the 3- and 4-segment strips, the active-segment tokens, the
  toggle rows, the status message, and the advance width of `0123456789` in the
  readout font.
```

---

## 7. Verification

- **Theme** — each preset round-trips `linear ↔ sRGB`; `VoidGradient` equals the
  current `draw` constants; `FlatVoid` shares every row with `VoidGradient` except
  `canvas_far`.
- **Background** — `the_background_is_the_canvas_gradient` still reads `#0F1218` /
  `#161B22` under the default preset; a `FlatVoid` readback reads `#0F1218` at both
  ends.
- **Surfaces** — `a_two_tone_surface_reads_back_as_glass_top_and_shade` under
  `VoidGradient` + bi-tone; a uniform (V2a) readback puts an upward-facing facet in
  `glass_shade`; an `Inspection` readback reads `#647082` / `#232B3A`.
- **Edges** — with `ghost_edges` off, the `Greater` packet count is zero for document
  shapes and unchanged for overlays; the ghost ink follows the preset.
- **Lighting** — the spherical ↔ cartesian round-trip; `TopLeft` and `TopRight` differ
  by the pinned quarter-turn; `Headlamp` follows the camera through a yaw change.
- **Chrome** — the segment count and active tokens, the monospace advance width over
  `0123456789`, the two toggle rows, and the status text on each change.
- There is still no visual-regression harness; this note does not invent one.

---

## 8. Non-goals

- No user colour pickers, theme editor or per-object scene overrides.
- No post-processing (SSAO, bloom, outlines) and no second shader or light rig.
- No runtime transparency pass; V2c stays deferred (`visual-language.md` §1.3).
- No time-of-day, animated or environment light.
- No document persistence of a preset and no undo entry: it is view state, like the
  grid switches.
