# Grid Overlay (Coin) — the A1 fork

The **A1** form of the grid-overlay fork (`docs/workbenches/grid-overlay-plan.md` §3.2, G5): a
faithful Coin/shader port of the upstream add-on, running under FerroCAD's
pull-based overlay bridge. It is a proving slice, not the shipping form — the
shipping form is `mods/GridOverlay/` (A2, a client of `FerroCAD.Grid`).

## Compatibility

This mod imports `FreeCAD`/`FreeCADGui` and `FerroCAD`/`FerroCADGui`, so it is
**FerroCAD-only**: it is not usable as-is on upstream FreeCAD. The FerroCAD
imports are guarded, and the command reports a clear status instead of crashing
when that API is absent. See
[`docs/architecture/api-namespaces.md`](../../docs/architecture/api-namespaces.md).

## What is preserved from upstream

- The **tree**: `SoSwitch` → (`SoPickStyle` `UNPICKABLE`, `SoLightModel`
  `BASE_COLOR`) → `SoSeparator` (`renderCulling = OFF`) → `SoDepthBuffer`
  (test on, write off) → transparent-material pass flag (when
  `SoMaterial.transparency` exists; FerroCAD's `SoMaterial` does not expose it
  yet, so the flag is omitted and the program pipeline blends regardless) →
  `SoShaderProgram` → the four-corner `SoCoordinate3` + `SoFaceSet` placeholder.
  There is no per-frame Python work: the grid is a fragment shader over one quad.
- The **six knobs** and defaults: `enabled=True`, `opacity=0.65`,
  `color="#808080"`, `base_spacing=1.0`, `shadow_enabled=True`,
  `shadow_opacity=0.30`.
- The **command intent**: one discoverable command, no workbench activation.
  The id is `GridOverlayCoin_Settings` and the menu text is
  `Grid Overlay (Coin)`.
- The **parameter names/types** of the fixed ABI: `grid_color` (vec3),
  `base_spacing`, `minor_alpha`, `major_alpha`, `shadow_opacity`,
  `shadow_count`, and the optional `shadow_data` float array. The compiler maps
  them by name; an unknown name fails extraction loudly.
- **Per-document intent**: one grid per open document, attached on
  create/activate and dropped on delete. Only the active document's grid is
  registered with the single viewport, so two documents do not draw two
  overlapping grids.

## What changes, and why

1. **WGSL instead of GLSL.** The upstream `grid.vert.glsl`/`grid.frag.glsl`
   pair is replaced by the ported `Resources/grid_overlay.wgsl`. The file is
   **byte-identical** to `crates/ferrocad_wgpu/shaders/grid_overlay.wgsl`,
   which is the reference copy; this mod's copy is a duplicate because a mod
   ships loose files and cannot reach into the crate at runtime. Keep the two
   in sync (a test asserts byte equality).
2. **A single-module shader on one stage.** WGSL has no per-stage modules, so
   the vertex stage carries the whole file and the fragment stage stays empty;
   the compiler concatenates the non-empty stage sources. `ProgramParameters`
   enforce the fixed ABI (plan §3.4) — Coin's dynamic parameter names are not
   reproduced.
3. **The pull bridge replaces `getSceneGraph()`.** The tree is registered with
   `FerroCADGui.addOverlay(root)`, which extracts it immediately; the shell
   re-extracts it on refresh. There is no live graph and no arena.
4. **`_rebind()` replaces aliasing.** `addChild` copies, so a uniform change is
   followed by re-running `fragment.parameter.setValues` /
   `program.shaderObject.set1Value`, rebuilding the separator, and removing and
   re-adding the same root. The command/dialog callback then returns
   `{"view": {"grid_overlay": ...}}` to make the shell pull it in.
5. **No Qt.** The dialog is the shell's declarative six-field spec, not a
   `QDialog`; `QSettings` becomes `FreeCAD.ParamGet` (in-memory today), and
   `getMainWindow`/toolbars are gone. `FreeCAD.addDocumentObserver` replaces the
   absent `FreeCADGui.addDocumentObserver`.
6. **Colour space.** The add-on's `#808080` is authored against GL's default
   framebuffer; the render target here is sRGB, so the mod decodes the token to
   linear light before it reaches `grid_color`.
7. **Ground shadows are deferred.** The `shadow_count`/`shadow_opacity`
   parameters and the 80-slot block are present and the shader's shadow branch
   is compiled, but `shadow_count` is always `0` because no document-bounds
   producer is wired yet (plan §3.5). The two dialog knobs persist; nothing
   draws.
8. **A1 look, not the native look.** This is a fixture and deliberately keeps
   the add-on's `MIN_PX = 8.0` line shader. Do not treat it as the product grid.

## Running it

The mod is picked up by the shell's workbench loader like any other `mods/`
entry. In the app, `GridOverlayCoin_Settings` opens the six-field dialog; Apply
rebuilds and re-registers the overlay and asks the shell for
`grid_overlay`. Everything headless is covered by
`tests/test_grid_overlay_coin.py` with a stubbed `pivy.coin`; drawing itself
needs a GPU and a manual run (`cargo run -p ferrocad_gpui`) — no GPU readback is
part of G5.

## Icon

`Resources/grid.svg` is the same MIT-licensed icon as the upstream add-on and
the A2 fork (Copyright (c) 2026 makzumi).
