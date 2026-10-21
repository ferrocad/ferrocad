# Grid Overlay (FerroCAD)

A thin client of FerroCAD's semantic grid API: the **Grid Overlay** command
(`GridOverlay_Settings`) opens a settings dialog for the native reference grid.
It is the A2 shipping form of the fork described in `docs/workbenches/grid-overlay-plan.md`
§3.2, not a port of the upstream overlay: there is no Qt, no `pivy`, no Coin
tree, no `getSceneGraph()`/`getMainWindow()` and no per-document attach code.
The dialog is the shell's declarative spec, and its `on_accept` writes
`FreeCAD.Grid` and returns `FreeCAD.Grid.apply()` (a `{"view": …}` request the
shell applies to the same state the native grid draws).

## Functional intent that survives

The command id and menu text ("Grid Overlay") are unchanged, `IsActive()`
returns `True`, and the six knobs are preserved as `enabled`, `color`,
`opacity`, `spacing`, `shadows` and `shadow_opacity`. Values persist through
`ParamGet("User parameter:BaseApp/Preferences/View")` — the same group the
upstream addon's `QSettings` stood in for, so there is one store, not two. The
grid itself is native, so it costs no Python work per frame.

Intent changes, stated rather than hidden:

- **Global, not per document.** The native grid belongs to the viewport, not to
  a document. That is deliberate: a document is content, a grid is not.
- **Spacing is an absolute minor step in millimetres**, not the upstream
  dimensionless "base spacing scale". It writes `GridMinorStep` and
  `GridMajorStep` together; `opacity` is a multiplier over the visual
  language's token alphas (default `1.0`), not an absolute alpha.
- **The look is the native one** (dots plus major lines, native tokens), not the
  addon's line-only shader at `#808080`.
- **Shadows persist but nothing draws them yet.** `shadows` and
  `shadow_opacity` are stored and validated so the dialog does not have to lie
  about what exists; a shadow producer is future work (§3.5 of the plan).

## Feature detection

`grid_api()` is the single discriminator: `getattr(FreeCAD, "Grid", None)` (§3.3
of the plan). When it is `None` — that is, on upstream FreeCAD — `Activated()`
returns a clear status saying this build needs FerroCAD's semantic grid API. The
fork is split cleanly: it carries **no** legacy Coin/GLSL backend, and the
upstream addon remains the addon for upstream FreeCAD.

## Icon

`Resources/grid.svg` is copied verbatim from the upstream `grid-overlay` addon
(https://github.com/makzumi/freecad-grid-overlay), which is MIT licensed,
Copyright (c) 2026 makzumi.
