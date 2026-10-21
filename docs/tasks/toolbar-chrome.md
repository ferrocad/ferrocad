# Task — viewport chrome: a toolbar system and one home for grid settings

Status: task (2026-10-22). P0.6 in [`../TODO-22-10-2026.md`](../TODO-22-10-2026.md).
Companion to [`../interaction/visual-language.md`](../interaction/visual-language.md)
(tokens and states), [`../workbenches/actions-and-tools.md`](../workbenches/actions-and-tools.md) §7
(the view/tool requests a command returns),
[`../interaction/axis-origin-grid.md`](../interaction/axis-origin-grid.md) (what the grid
toggles) and [`../direction/app-shell-vision.md`](../direction/app-shell-vision.md) S2.

**Scope note.** The chrome (header controls, popovers, the action dock, the app menu)
is implemented. The **grid settings** half is not: it depends on decisions listed in
§6. T0–T3, T6 and T7 landed on 2026-10-22; T4–T5 wait for those decisions.

## 1. Why

The current shell puts **every** registered command in one full-width strip above all
panes ([`crates/ferrocad_gpui/src/shell.rs`](../../crates/ferrocad_gpui/src/shell.rs),
`actions_bar`, rendered at `Shell::render`). With two grid mods loaded that is fourteen
text buttons (`Delete`, `Move`, `Grid`, `Axis cross`, `Grid settings`, `Mods`,
`Reload grid settings`, `Activate workplane`, `Clear workplane`, `View workplane`,
`Create Slab`, `Polyline`, `Grid Overlay`, `Grid Overlay (Compact)`), mixing four
different kinds of thing:

| Kind | Examples | Frequency | Belongs in |
| --- | --- | --- | --- |
| environment toggle | `Grid`, `Axes`, `WP` | constant | the viewport header |
| environment sub-state | overlay on/off, overlay mode, reload | occasional | a popover on that control |
| provider / workbench | `Mods`, provider switch | occasional | a popover on that control |
| creation / modification | `Create Slab`, `Polyline`, `Move`, `Delete` | per action | a floating dock over the canvas |

The strip also cannot scale to a real workbench (`Draft` registers 86 commands), and it
consumes vertical space from the canvas. The target is three surfaces that do not
compete: **header controls**, **anchored popovers**, and a **selection-reactive dock**.

## 2. Current state (verified)

| Piece | Where | State |
| --- | --- | --- |
| command list | `commands.rs` (`CommandEntry`) | id, menu label, tooltip, accel, group, `CmdType`, `enabled`, `checked` |
| flat actions bar | `shell.rs:1226` (`actions_bar`), rendered at `:1800` | one button per command, above the pane row |
| pane header | `shell.rs:1206` (`panel_header`) | a text label only; no controls |
| tool strip | `shell.rs:1751` (`tool_strip`) | appears under the viewport header during a tool session |
| dialogs | `shell.rs:1299` (`dialog_overlay`) | modal over a dimmed shell, with focus and `Esc` |
| properties pane | `shell.rs:1654` | already renders editable rows for a selection |
| widgets | `crates/ferrocad_widgets` | `window_chrome`, `text_field`, `text_area`, `events` — **no popover, menu, split button or toggle** |
| command ids (today) | `App_Delete`, `App_Move`, `App_ToggleGrid`, `App_ToggleAxisCross`, `View_GridSettings`, `Mods_Settings`, `App_ReloadGridSettings`, `App_ActivateWorkplane`, `App_ClearWorkplane`, `App_ViewWorkplane`, `Toy_MakeSlab`, `Toy_Polyline`, `GridOverlay_Settings`, `GridOverlayCoin_Settings` | |

There is no grouping metadata beyond `Group` (`Edit`, `View`, `Toy`) and `CmdType`
(`ForEdit`, `ForView`): enough to *sort*, not enough to say **which surface** a command
belongs to.

## 3. Target

The design brief (2026-10-22) specifies:

1. **Viewport header, right-aligned** (in `panel_header("3D viewport")`, never over the
   navicube):
   - `Grid` split button: the label toggles visibility in one click; `▾` (or right-click)
     opens a popover with **Grid Overlay** (toggle), **Overlay Mode**
     (`Compact` | `Full`), **Spacing** (`10.0 mm`, tabular), **Provider / Mod**, then
     `Grid Settings…` and `↻ Reload Grid Settings`.
   - `Axes` toggle button (`App_ToggleAxisCross`).
   - `WP` split button: the label runs `App_ViewWorkplane`; `▾` offers
     `Activate workplane` and `Clear workplane`.
   - `Mods ▾`: the provider/mod checklist (`Mods_Settings`), also offered inline as
     **Provider / Mod** in the Grid popover.
2. **Action dock**, bottom-centre over the canvas: `Create Slab` · `Polyline` ·
   `Workplane` · `Search (Ctrl+K)` when idle; filtered by selection signature when
   something is selected (`Delete`, `Move`, then workbench-specific entries). It recedes
   to 35 % during orbit/gizmo drags and returns on proximity.
3. **Grid settings are modeless**: `Grid Settings…` loads the grid parameters into the
   existing Properties pane (no selection = viewport/grid properties), rather than a new
   modal. There is no separate settings screen.
4. **App menu**, left-aligned in the same header: `☰` opens app-level commands that are
   not viewport environment controls. Dev Tools is the first entry (checked while the
   window is open, showing `Ctrl+Shift+D`); About and Preferences follow. It exists
   because Dev Tools starts closed and is hidden from every other surface, so the
   shortcut was the only way in.

## 4. What is missing (the actual work)

- **Surface metadata.** A command must declare (or be assigned) a surface and a parent:
  `header:grid`, `header:workplane`, `header:axis`, `header:mods`, `dock`, `hidden`.
  Deriving this from `Group` strings is fragile; `GetResources` has no such field
  upstream, so this belongs in a FerroCAD extension of the command registry
  (`Surface=`/`Parent=` keys, ignored by upstream mods).
- **Split-button and popover primitives.** State (`open` per surface), anchoring below
  the caret, outside-click and `Esc` dismissal, and right-click-to-open.
- **Dock** with selection filtering and drag/proximity opacity.
- **Provider-aware grid popover** (T4) and **Properties-pane grid settings** (T5) —
  blocked (§6).
- **Deleting the flat strip** and updating
  [`actions-and-tools.md`](../workbenches/actions-and-tools.md) §7 and §16, which
  currently document it as the A1/A4 surface.

## 5. Slices

| # | Slice | Verifiable headlessly |
| --- | --- | --- |
| **T0** | Surface metadata: parse `Surface`/`Parent` from `GetResources` (default derived from `Group`), a `Surface` enum, and a `shell_model.surfaces()` grouping | unit tests in `ferrocad_gpui::commands` ✅ |
| **T1** | `split_button` + `popover` primitives (open state, anchor, dismissal, right-click) | state-machine tests; rendering by inspection ✅ |
| **T2** | Header controls: `Grid`, `Axes`, `WP`, `Mods` wired to the existing command ids, right-aligned in the viewport header | model tests: clicking the label runs the command; `▾` opens the group without running it ✅ |
| **T3** | Action dock replaces the strip; selection filter; drag/proximity opacity | model test: the dock's visible ids follow the selection signature ✅ (drag opacity deferred) |
| **T4** | Grid quick popover content (overlay toggle, mode, spacing readout, provider) | **blocked** (§6) |
| **T5** | `Grid Settings…` loads grid rows into the Properties pane | **blocked** (§6) |
| **T6** | Remove `actions_bar` and its layout slot; update the docs; keep every command reachable | test that every registered command appears on exactly one surface |
| **T7** | App menu (`☰`) at the header's left: app-level commands, Dev Tools first, checked while open | model test: the menu opens, lists the hidden command, and running it opens the window |

`T0` was deliberately first: it is pure data, it made `T6`'s "every command reachable"
assertion possible, and it let T2/T3 land without touching the Python side.

## 6. Decisions the grid half needs

These are why T4/T5 are not ready:

1. **What "Overlay Mode: Compact / Full" means.** Today there are **two mods**
   (`mods/GridOverlay` = native/ferrocad backend, `mods/GridOverlayCoin` = Coin/WGSL)
   and two settings commands. The brief's "Grid Overlay" toggle plus a
   `Compact | Full` segmented control implies one feature with two *modes*, not two
   providers — or that "Compact" is the native grid and "Full" the shader grid. Pick
   one; the external-mod ingestion item in the backlog (P0) already asks the same
   question and should be decided with it.
2. **What `Provider / Mod` selects.** A provider backend, a loaded mod, or both? The
   registry offers `setActiveProvider(feature, backend)`; the brief calls the row
   "Provider / Mod". Name it after the registry, not after the file layout.
3. **Which parameters `Grid Settings…` edits.** `FreeCAD.Grid` exposes enabled, colour,
   opacity, minor/major, radius, shadows, shadow opacity. The brief mentions 15
   parameters, subdivisions, snap radius — some of which do not exist yet
   ([`../interaction/axis-origin-grid.md`](../interaction/axis-origin-grid.md)). Decide
   the set, and whether the Properties pane renders them as typed rows (it already
   renders property rows for a selection) or the popover grows fields.
4. **Reusing the Properties pane.** `properties_panel` takes a document selection. Grid
   settings are not a document object. Either give it a second source (a synthetic
   "Viewport" record) or keep the card from the brief. The brief prefers the pane; that
   needs the pane's model to accept non-document rows.
5. **Persisted provider choice.** Still in-memory (`ParamGet`), so a restart loses the
   selection; the popover must not imply otherwise.

## 7. Non-goals

- Redesigning the navicube, the visual tokens, or the type scale (separate work).
- A full-screen or modal settings page: the brief rejects it, and so does this task.
- Command palette / `Ctrl+K` beyond the dock slot that opens it (a later slice).
- Reimplementing upstream `QToolBar`/`QAction` semantics in Python.

## 8. Acceptance

- One button per legacy action, reachable from exactly one surface; no command silently
  disappears when the strip is removed (T6 test). Dev Tools stays reachable through the
  app menu even though it is hidden from the header and the dock (T7 test). ✅
- `Grid` toggles in one click without opening the popover; `▾` opens it without toggling. ✅
- With a popover open, viewport orbit/zoom still receives pointer events; `Esc` and an
  outside click both dismiss. ✅ (structurally: no backdrop, no `occlude`; headless
  state tests only — a visual pass still needs Xvfb or a human)
- Popovers never overlap the navicube at any window width ≥ 800 px. ⏳ (visual only)
- The dock's visible actions follow the selection signature, and it fades during a drag.
  ✅ for enablement; the drag fade is deferred with T3's visual half.
- `cargo test -p ferrocad_gpui` and `cargo test -p ferrocad_toy` stay green; the Xvfb
  real-window smoke test still boots.
- The docs (`actions-and-tools.md` §7/§16) describe the new surfaces, not the strip. ✅

## 10. As built (2026-10-22)

- `commands.rs` owns `Surface` (`grid`, `axis`, `workplane`, `mods`, `dock`, `hidden`),
  `CommandInfo::surface()/parent()`, `group_by_surface()` and `popover_commands()`.
  An unannotated **or unknown** surface is the dock.
- The `Surface`/`Parent` keys are written in each command's `GetResources()`; the shell
  bridge (`ferrocad_shell.commands()`) copies them into the JSON list. `FreeCADGui`'s
  own `_command_list()` is untouched, so the facade keeps its upstream shape and a mod
  that does not know the keys is unaffected.
- `chrome.rs` holds the primitives: `split_button`, `popover`, `action_dock` and the Dev
  Tools `command_list_panel`. The popover is a `deferred(anchored(..))` child of the
  control, with `on_mouse_down_out` for outside-click dismissal. The **caret opens**
  rather than toggles: the outside-click handler fires before the caret's click, so a
  toggle would immediately reopen the menu. `Esc` closes it from the shell key path.
- Because T4 is blocked, a popover is a plain list of the commands on that surface
  whose `Parent` names the control's primary. When a surface has no such children
  (`Mods`), it falls back to the surface's own commands so the caret is never dead.
- The dock is `model.surfaces().dock`, so the existing `IsActive` results drive it.
  Disabled rows stay visible. `Search (Ctrl+K)` is a status-reporting stub; there is no
  palette.
- T6 removed `actions_bar`; the Dev Tools window lists **every** command vertically,
  with its surface tag, as the reachability safety net.

## 9. Risks

- **Anchoring in GPUI.** A popover is an absolutely positioned child of the pane, not a
  window; the anchor must survive layout changes and window resize. T1 should be
  proven with a trivial popover before T2 wires the real controls.
- **No display in the dev sandbox.** State transitions can be unit-tested, but the
  visual checks in §8 need the Xvfb job or a human. Do not claim a visual pass from a
  headless run.
- **Command surface drift.** A mod that registers a command with no `Surface` must still
  appear. The default must be the dock (or `hidden` with a warning), never "nowhere".
