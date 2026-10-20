# The visual language: tokens, type, strokes and states

Status: design (2026-10-20). Companion to [`axis-origin-grid.md`](axis-origin-grid.md)
(spatial tokens), [`gizmo-manipulators.md`](gizmo-manipulators.md) §8 (manipulator
tokens), [`navicube-spec.md`](navicube-spec.md) §5 (theme roles) and
[`app-shell-vision.md`](app-shell-vision.md) (chrome).

This note is the **single source of truth for the look**: colour tokens, type scale,
stroke rules, and the idle → hover → drag state machine. It also records what the code
already does, because the tokens currently live in four places with three different
conventions.

---

## 0. Principles

1. **Geometry is the interface.** Chrome recedes; it never masks the model.
2. **Dielectric, not opaque.** UI layers are frosted glass (`rgba(void, α)`) and thin
   vector strokes, not opaque rectangles.
3. **Zero modality.** A control materialises from pointer proximity or a drag and
   dissolves when idle; nothing is hidden behind a mode switch.
4. **Numbers never jump.** Every geometric readout is tabular monospace in a fixed
   pill.

---

## 1. Tokens

Colours are given as **sRGB hex + alpha**, the form a designer or a screenshot picker
produces. The renderer consumes **linear** RGBA (`Rgba8UnormSrgb` target), and gpui
consumes packed `0xRRGGBBAA`. A token is authoritative in one place and converted at
the boundary — see §7.

### 1.1 Canvas

| Token | Value | Today |
| --- | --- | --- |
| Canvas void | `#0F1218` | shell `BG = 0x101216`; renderer clear `(0.1, 0.1, 0.12)` **linear** |
| Canvas void (far) | `#161B22` | shell `PANEL = 0x16181d` (a panel, not the gradient end) |
| Panel | `#16181D` | `PANEL` |
| Panel alt | `#1D2026` | `PANEL_ALT` |

The renderer's clear is `0.1` linear ≈ `#595961` sRGB — noticeably lighter than the
canvas token. Aligning it is V2; a vertical gradient (`#0F1218 → #161B22`) is a
full-screen quad in the same slice.

### 1.2 Accents and actions

| Token | sRGB | alpha | Linear (as in code) | Symbol today |
| --- | --- | --- | --- | --- |
| Active accent | `#38BDF8` | 1.0 | `[0.040, 0.509, 0.939]` | `gizmo::ACTIVE_COLOR`, `ShapeItem::HIGHLIGHT_COLOR` |
| Hover rim | `#48E5FF` | 1.0 | `[0.065, 0.784, 1.0]` | proposed (state bloom) |
| Snap aurora | `#38BDF8` | 0.85 | accent × 0.85 | `axis-origin-grid.md` |
| Inactive structural | `#94A3B8` | 1.0 | `[0.296, 0.366, 0.479]` | `gizmo::INACTIVE_COLOR` |
| Additive (push-pull) | `#F59E0B` | 1.0 | `[0.913, 0.342, 0.003]` | `gizmo::PUSH_PULL_COLOR` |
| Subtractive (pocket) | `#EF4444` | 1.0 | `[0.863, 0.058, 0.058]` | `gizmo::SUBTRACT_COLOR` |

The four linear constants are the **reference implementation** of these tokens (see the
conversion note in §1.7).

### 1.3 Surface glass

| Token | Value | Opaque composite over the void | Meaning |
| --- | --- | --- | --- |
| Glass top | `rgba(216, 230, 250, 0.35)` | `#555C67` | upward-facing facets |
| Glass shade | `rgba(30, 38, 52, 0.42)` | `#151A24` | side / downward facets |

**Constraint.** The opaque scene pipeline is `BlendState::REPLACE` and writes
depth; only overlay packets blend (`ALPHA_BLENDING`). So these alphas currently do
nothing on a surface: an entity renders opaque. The plan is:

1. **V2a** — ship the **opaque composites** (last column) as the surface tokens; the
   look is identical over the void and needs no transparency pass.
2. **V2b** — a two-tone material: `dot(normal, up) > 0.5` picks glass-top,
   otherwise glass-shade, while the existing directional light keeps intra-facet
   gradation. Deferred until a material model exists.
3. **V2c** — true translucency needs an explicit ordered/wboit pass; deferred, and not
   a prerequisite for the look.

The spec's alpha is therefore a **design intent** and the composite is the build target
until V2c.

### 1.4 Edges

| Token | Value | Today |
| --- | --- | --- |
| Front edge | `#D6E0F0` @ 0.62 | navicube `EDGE_COLOR = [0.84, 0.88, 0.94, 0.62]` |
| Occluded edge | `#D6E0F0` @ 0.10 | navicube `EDGE_AWAY = [0.55, 0.60, 0.68, 0.10]` |
| Entity edge (default) | `#D6E0F0` @ 0.62 | **`ShapeItem` default `[0.1, 0.1, 0.1, 1.0]`** (dark) |
| Entity surface (default) | glass-shade composite | **`ShapeItem` default `[0.8, 0.8, 0.8, 1.0]`** (light grey) |

The two entity defaults predate this language and read as "light grey solid with black
edges", not the dark canvas. Changing them is V1; the display provider can still
override per object (the toy does).

### 1.5 Text and glass chrome

| Token | Value | Today |
| --- | --- | --- |
| Text | `#D8DBE0` | `FG`, `CHROME_TEXT`, `TITLE_FG`, `INPUT_FG` |
| Muted | `#8B919A` | `MUTED`, `CHROME_MUTED` |
| Face label | `rgba(235, 242, 252, 0.78)` | navicube `LABEL_COLOR ≈ [0.92, 0.95, 1.0, 0.80]` |
| Glass background | `rgba(15, 18, 24, 0.75)` | pill `0x0f1218cc` (0.80) — align to 0.75 |
| Glass border | `#FFFFFF14` | pill, chrome `CHROME_BORDER` |
| Chrome accent | `#67D7FF` | `CHROME_ACTIVE` (navicube bar) |

### 1.6 The convention problem (important)

There are **three** colour conventions in the tree today:

- `ferrocad_stage::gizmo` — **linear** RGBA, documented as such (`#38BDF8` stored as
  `[0.040, 0.509, 0.939]`). Correct for the draw contract.
- `ferrocad_viewport::navicube::theme` — mostly **sRGB-normalised** values
  (`EDGE_COLOR = [0.84, 0.88, 0.94]` is `#D6E0F0 / 255`), with exactly one constant
  (`TURNTABLE_COLOR`) documented as linear. Fed straight into `StateSnapshot.color`,
  so the cube's edges render brighter than their nominal hex.
- `ferrocad_gpui` / `ferrocad_widgets` / the viewport chrome — packed `u32` sRGB
  (`0xD8DBE0`), which is what gpui wants.

V0 normalises all three onto one token table: **linear at the draw contract, packed
sRGB at the gpui boundary**, with a round-trip test.

---

## 2. Typography

| Role | Size / weight | Family | Today |
| --- | --- | --- | --- |
| Status bar, micro-pills | 11 px regular | mono | `CHROME_FONT = 11.0`, `"monospace"` |
| In-situ dimension tag | 12 px semibold | mono, tabular | pill is 11 px, `"monospace"` |
| Face labels (`T, F, R`) | 14 px bold | geometric sans or vector glyphs | navicube labels (producer-defined) |
| UI chrome | 11–13 px | Inter or system sans | gpui default + `"monospace"` where set |

- **Tabular numerals are mandatory** for every geometric number. gpui does not expose
  `font-variant-numeric`, so those runs use a **monospace** family, which is tabular by
  construction. This is why the pill is monospace even though the chrome is not.
- **Fonts are not bundled today.** `font_family("monospace")` resolves through the
  system. Bundling JetBrains Mono (OFL) and falling back to
  `JetBrains Mono, ui-monospace, monospace` is V4; until then the pill is system
  monospace at the correct size and weight.
- Sizes are **logical px**; gpui applies DPR. The renderer's text is a gpui layer, not
  GPU text, so no atlas is involved.

---

## 3. Stroke weight and occlusion

WebGPU lines and points are **1 px**. Anything that must read as 1.5 px is a
**screen-space ribbon** (triangles), which is already how the gizmo, the dress-up arc
and the navicube glow work. There is therefore no "line width" knob; weight is carried
by **opacity and pairing**, per the spec.

| Class | How | State |
| --- | --- | --- |
| Front-facing primary edges | 1 px `Lines`, front-edge token | ✓ mechanism; default token change is V1 |
| Occluded / back edges | 1 px `Lines`, ghost token | ▢ needs a **depth-`Greater`** pass (V1) |
| Machined bevels | twin parallel lines, `0.08` model units apart | ▢ geometry-level, deferred (V4) |
| Gizmo / manipulator strokes | 1.5 px ribbons | ✓ |
| Dashed rays | runs of short segments | ✓ technique, used by the grid spec |

**The occlusion gap.** Today every non-overlay pipeline uses `depth_compare: Less`
with depth writes, and overlays use `Always` with no writes. A ghost line *behind*
geometry needs `Greater` with no writes and alpha blending — a third pipeline variant.
The clean contract is a per-packet intent:

```rust
pub enum DepthMode {
    /// Normal opaque geometry.
    Less,
    /// Drawn over everything (tool overlays, pills are not packets).
    Always,
    /// Visible only where something occludes it (ghost edges).
    Greater,
}
```

`StateSnapshot` gains `depth_mode` (default `Less`); the renderer selects the pipeline.
Packets that set `Greater` also set `depth_write_enabled = false` and alpha blend.

---

## 4. Surfaces and lighting

The scene shader is one directional light (`(0.4, 0.8, 0.6)`), ambient `0.25`, a
two-sided `abs(dot(normal, light))`, and **no back-face culling**. That already gives
top facets their brightest value and sides a darker one, which is the physical basis of
the glass-top/glass-shade pair.

- **V2a** keeps the shader and feeds the composite tokens; the two-tone effect is then
  automatic for axis-aligned faces and approximate elsewhere.
- A true **material model** (albedo, roughness, top/shade thresholds) is V2b. It must
  not become a second lighting path: one light, one shader, tokens in.
- The spec's translucency is deferred (§1.3); the design must look right opaque first.

---

## 5. State model

```
IDLE      ambient: wireframe 30 %, structural tokens, no labels
  │  pointer within HOVER_RADIUS
HOVER     full luminosity, rim #48E5FF, crisp labels, no commit
  │  press on a grab target
DRAG      solid handle, amber/cyan hull, tabular pill follows
```

| Constant | Value | Notes |
| --- | --- | --- |
| `HOVER_RADIUS` | 20 px | visual bloom only |
| `GRAB_SLOP` | 12 px | today's `HIT_SLOP`; the radius that actually starts a drag |
| Origin proximity | 5 px | the world-origin node's own, smaller radius (`axis-origin-grid.md`) |
| Aurora flash | 120 ms | new snap point |
| Hover in / out | 80 / 120 ms | out is slower, so a pass-over does not strobe |

**The 20 vs 12 question.** The spec blooms at 20 px but the gizmo grabs at 12 px, so a
handle can *light up* before it can be grabbed. Recommend unifying both at **20 px**
(`HIT_SLOP` becomes 20) and accepting a slightly tighter triad interior; the alternative
is to keep the split and treat the bloom as a hint. Decision §9.2.

---

## 6. Pass-through indication

When an overlay's interior passes a click through to the model, the status bar says so
(`clicked through: recompute()`). The mechanism is a small, neutral hover report from
the viewport, not a workbench concept:

```rust
pub struct HoverReport {
    /// What the click would hit in the document.
    pub target: Option<SelectionRef>,
    /// The overlay under the pointer that will not consume the click, if any
    /// (`"gizmo"`, `"dress_up"`, `"navicube"`, …).
    pub through: Option<&'static str>,
}
```

The viewport already knows both (its pick and its manipulator/gizmo hit tests); the
shell renders it in the status line. This is also the natural feed for the actions/tool
registry's status text, so it is designed here and wired there.

---

## 7. Ownership: one token source

Tokens are currently duplicated with three conventions (§1.6). Add a leaf crate:

```
ferrocad_theme (rlib, no dependencies)
  pub struct Srgb { pub hex: u32, pub a: u8 }     // design form
  pub fn linear(token: Srgb) -> [f32; 4]          // draw contract
  pub fn packed(token: Srgb) -> u32               // gpui 0xRRGGBBAA
  pub mod tokens { /* CANVAS, ACCENT, STRUCTURAL, … */ }
```

- `ferrocad_graphics` (and therefore `stage`, `viewport`, `wgpu`) consume `linear`.
- `ferrocad_widgets` and the shell consume `packed`.
- `navicube::theme` keeps its **role** layer (`NavicubeRole → token`), so producers do
  not change; only the constants underneath do.
- A leaf crate, not a module in `graphics`: `widgets` must not depend on the renderer,
  and the renderer must not depend on gpui.

No kernel, no document, no Python. A workbench may override a surface colour per
object (the toy does) but does not redefine the chrome tokens.

---

## 8. Slice plan

| Slice | Deliverable | Acceptance |
| --- | --- | --- |
| **V0 — tokens** | `ferrocad_theme`; migrate gizmo, navicube theme, shell/chrome, pill; normalise to linear/packed | every token round-trips (`hex → linear → packed`); existing suites green; the only visual change is the navicube's over-bright edges, which the tokens correct |
| **V1 — occlusion** | `DepthMode` + `Greater` pipeline; entity edge/surface defaults from the tokens | a wgpu readback: a front edge and a ghost edge over a solid; the ghost disappears where the solid covers it; a stage test asserts the packet's mode |
| **V2 — canvas & surfaces** | clear colour `#0F1218`; optional gradient; glass composites; two-tone threshold | the frame's corner readback is the void token; a top and a side facet composite to the two glass tokens |
| **V3 — states** | `HOVER_RADIUS`, in/out timings, rim bloom; `HoverReport` → status bar | a headless viewport test: at 21 px no bloom, at 19 px bloom; a gizmo-interior click reports `through: gizmo` and still selects the object behind |
| **V4 — type & bevels** | bundle JetBrains Mono + fallback chain; pill 12 px semibold; tabular runs; twin-line bevels | the pill's digits do not change advance width across `0–9`; a chamfer renders two parallel lines in a readback |

V0 is the only cross-cutting slice; V1–V4 are independent afterwards.

---

## 9. Decisions

1. **Alpha semantics.** Ship opaque composites now and defer translucency (§1.3).
   Recommend yes; a transparency pass is a renderer project of its own.
2. **Hover vs grab radius.** Unify at 20 px, or keep 20/12? Recommend **unify at 20**
   so a lit handle is always grabbable. Cost: `HIT_SLOP` and its tests change.
3. **Font bundling.** JetBrains Mono (OFL) is redistributable; bundling adds ~300 KB and
   makes the pill deterministic across platforms. Recommend bundle, with the system
   fallback.
4. **Canvas gradient.** The viewport is an image/zero-copy target, so the gradient is a
   full-screen quad in the scene pass (V2), not a gpui background behind a transparent
   canvas. Recommend the quad; it also covers the zero-copy path.
5. **Occluded edges.** Depth-`Greater` pass (recommended) vs a stencil/mesh-based
   approach. `Greater` is one pipeline and one packet flag.
6. **Bevel twin lines.** Geometry-level offset curves (recommended; the renderer has no
   outline shader) vs a shader effect. Deferred to V4 either way.
7. **Token crate.** `ferrocad_theme` (recommended) vs `graphics::theme` + a mirror in
   `widgets`. The leaf crate is the only option that keeps both boundaries clean.

---

## 10. Non-goals

- No opaque chrome over the canvas; no modal dialogs as the primary interaction.
- No post-processing (bloom, SSAO, outlines) in this language; weight is opacity and
  pairing.
- No theme editor or user-defined palettes yet; tokens are compile-time.
- No GPU text; labels and pills stay gpui layers.
- No per-workbench redefinition of chrome tokens.

---

## 11. Verification

- **Tokens** — a unit test per token: `linear(hex) → packed(hex)` round-trips, and the
  four existing linear constants equal the converted hex (guards §1.6).
- **Renderer** — readback tests for the clear colour, the two glass composites and the
  occluded-edge pass; the existing depth-bias test is the template.
- **Viewport** — headless tests for the hover radius, the pass-through report, and the
  pill's size/weight tokens.
- **Typography** — an advance-width test over `0123456789` in the pill's font.
- The overall composition remains a spec: there is no visual-regression harness yet,
  and this note does not invent one.
