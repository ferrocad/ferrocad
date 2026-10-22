# Task — viewport chrome: a toolbar system and one home for grid settings

Status: task (2026-10-22). P0.6 in [`../TODO-22-10-2026.md`](../TODO-22-10-2026.md).
Companion to [`../interaction/visual-language.md`](../interaction/visual-language.md)
(tokens and states), [`../workbenches/actions-and-tools.md`](../workbenches/actions-and-tools.md) §7
(the view/tool requests a command returns),
[`../interaction/axis-origin-grid.md`](../interaction/axis-origin-grid.md) (what the grid
toggles) and [`../direction/app-shell-vision.md`](../direction/app-shell-vision.md) S2.

**Scope note.** T0–T7 landed on 2026-10-22: the chrome (header controls, popovers,
the action dock, the title-bar app menu) and the grid-settings half. T4/T5 are wired
to [`settings-model.md`](settings-model.md) S2/S4. The remaining acceptance items are
visual-only (popover/navicube overlap, drag fade).

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
     opens a popover with **Grid provider** (the active `(grid, backend)`), **Spacing**
     (`10.0 mm`, tabular), then `Grid Settings…` (selects `Settings/Grid`) and
     `↻ Reload Grid Settings`. No separate overlay toggle (the label is the one
     switch) and no mode row (a local grid follows the workplane).
   - `Axes` toggle button (`App_ToggleAxisCross`).
   - `WP` split button: the label runs `App_ViewWorkplane`; `▾` offers
     `Activate workplane` and `Clear workplane`.
   - `Mods ▾`: the provider/mod checklist (`Mods_Settings`), also offered inline as
     **Provider / Mod** in the Grid popover.
2. **Action dock**, bottom-centre over the canvas: `Create Slab` · `Polyline` ·
   `Workplane` · `Search (Ctrl+K)` when idle; filtered by selection signature when
   something is selected (`Delete`, `Move`, then workbench-specific entries). It recedes
   to 35 % during orbit/gizmo drags and returns on proximity.
3. **Grid settings are modeless**: `Grid Settings…` selects the `Settings/Grid` node in
   the model tree and the existing Properties pane renders its rows
   ([`settings-model.md`](settings-model.md)). There is no separate settings screen.
4. **App menu**, at the leading edge of the **window title bar** (moved there from the
   viewport header): `☰` opens app-level commands that are not viewport environment
   controls. Dev Tools is the first entry (checked while the window is open, showing
   `Ctrl+Shift+D`); About and Preferences follow. It exists because Dev Tools starts
   closed and is hidden from every other surface, so the shortcut was the only way in.

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
| **T0** | Surface metadata: parse `FerroCAD_Surface`/`FerroCAD_Parent` from `GetResources` (default derived from `Group`), a `Surface` enum, and a `shell_model.surfaces()` grouping | unit tests in `ferrocad_gpui::commands` ✅ |
| **T1** | `split_button` + `popover` primitives (open state, anchor, dismissal, right-click) | state-machine tests; rendering by inspection ✅ |
| **T2** | Header controls: `Grid`, `Axes`, `WP`, `Mods` wired to the existing command ids, right-aligned in the viewport header | model tests: clicking the label runs the command; `▾` opens the group without running it ✅ |
| **T3** | Action dock replaces the strip; selection filter; drag/proximity opacity | model test: the dock's visible ids follow the selection signature ✅ (drag opacity deferred) |
| **T4** | Grid quick popover: **provider** readout, spacing readout, settings/reload rows (no mode) | model test: the rows follow the active provider ✅ |
| **T5** | `Grid Settings…` selects the `Settings/Grid` node and opens the modeless 380 px card (draggable, no scrim) | gpui test: the directive selects the node, the card opens/drags/closes, and its rows edit ✅ |
| **T6** | Remove `actions_bar` and its layout slot; update the docs; keep every command reachable | test that every registered command appears on exactly one surface |
| **T7** | App menu (`☰`) in the window title bar: app-level commands, Dev Tools first, checked while open | model test: the menu opens, lists the hidden command, and running it opens the window |

`T0` was deliberately first: it is pure data, it made `T6`'s "every command reachable"
assertion possible, and it let T2/T3 land without touching the Python side.

## 6. Decisions (2026-10-22)

The five questions below were settled in review. Three of them moved work out of
this task and into [`settings-model.md`](settings-model.md), which is why T4/T5
depend on that file rather than on more debate here.

1. **There is no `Compact | Full` mode; a compact grid is a *local* grid.**
   `ferrocad_stage` already renders a bounded workplane grid
   (`GridSpec::workplane()`, 1 mm dots / 10 mm lines, `WorkplaneKind::{World, Datum,
   Face}`) and dims the world grid while it is active. Upstream FreeCAD likewise
   changes the *workplane*; it does not add a second grid. So: drop the segmented
   control from the popover. The world grid is what a provider implements (native or
   the Coin/WGSL overlay); the local grid follows the active workplane and is not a
   provider mode. Reusing the overlay for a local grid is not planned: it is an
   infinite full-screen ground shader, while a face grid is bounded and
   depth-tested against the face.
2. **The selectable thing is a feature provider, not a mod.** A mod is a package; a
   provider is one `(feature, backend)` implementation of a capability. The row is
   therefore **`Grid provider`** (capability-scoped), with the backend ids the
   registry already uses (`ferrocad`, `coin`, and the external mod's own id). A mod
   that provides two features appears twice, once per feature.
3. **Settings are advertised, not hard-coded.** A provider declares its knobs as a
   schema (name, label, kind, default, range/choices) and the shell renders one
   surface for whichever provider is active. That is the answer to "two different
   dialogs": switching the provider changes the rows, not the window. The schema and
   its storage are [`settings-model.md`](settings-model.md) S0.
4. **Settings belong in the tree, not in a synthetic pane source.** A `Settings` root
   (not a document) holds one node per capability, for example `Settings/Grid`; its
   rows are the active provider's schema. Document/project settings, when they exist,
   are real document objects under the document instead. `/` is not legal in a
   FreeCAD object name, so the two namespaces cannot collide. This also avoids
   teaching `properties_panel` a second source: it renders whatever node is selected.
5. **The provider choice is a user setting and must persist.** `ParamGet` is in-memory
   today, so persistence is a prerequisite rather than a polish item. The choice is
   written as a user setting; a document value, when added, overrides it
   (document > user > provider default). [`settings-model.md`](settings-model.md) S3.

Nothing in the chrome needs to change for 3–5; T4/T5 become wiring once S0–S3 land.

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
- A split-button popover opens below its control, inside the window, not over the
  title bar. ✅ (headless bounds assertion)
- The dock's visible actions follow the selection signature, and it fades during a drag.
  ✅ for enablement; the drag fade is deferred with T3's visual half.
- `cargo test -p ferrocad_gpui` and `cargo test -p ferrocad_toy` stay green; the Xvfb
  real-window smoke test still boots.
- The docs (`actions-and-tools.md` §7/§16) describe the new surfaces, not the strip. ✅

## 10. As built (2026-10-22)

- `commands.rs` owns `Surface` (`grid`, `axis`, `workplane`, `mods`, `dock`, `hidden`),
  `CommandInfo::surface()/parent()`, `group_by_surface()` and `popover_commands()`.
  An unannotated **or unknown** surface is the dock.
- The `FerroCAD_Surface`/`FerroCAD_Parent` keys are written in each command's `GetResources()`; the shell
  bridge (`ferrocad_shell.commands()`) copies them into the JSON list. `FreeCADGui`'s
  own `_command_list()` is untouched, so the facade keeps its upstream shape and a mod
  that does not know the keys is unaffected.
- `chrome.rs` holds the primitives: `split_button`, `popover`, `action_dock` and the Dev
  Tools `command_list_panel`. The popover is a `deferred(anchored(..))` child of the
  control, with `on_mouse_down_out` for outside-click dismissal. The **caret opens**
  rather than toggles: the outside-click handler fires before the caret's click, so a
  toggle would immediately reopen the menu. `Esc` closes it from the shell key path.
- A popover lists the commands on that surface whose `FerroCAD_Parent` names the
  control's primary. When a surface has no such children (`Mods`), it falls back to
  the surface's own commands so the caret is never dead.
- T4 adds two read-only rows at the top of the **Grid** popover: `Provider: <label>`
  and `Spacing: <n mm>` (`grid-row-provider`, `grid-row-spacing`). They come from
  `ferrocad_shell.grid_summary()`, fetched when the popover opens (not per frame),
  and refreshed after a `Settings/…` edit. With no provider the panel says
  `No grid provider`; there is no compact/full mode row.
- The Grid popover groups the two backend-settings rows above the config actions
  with a `1 px` `#FFFFFF14` hairline, and the active backend's row is checked in
  `#67D7FF` (`CHROME_ACTIVE`). The dock has the same hairline between the
  modification and creation groups, and before `Search`.
- **Anchoring.** A split-button popover is placed from the caret's actual pointer
  event (`popover_anchor`) in window coordinates, 18 px below the press. The flex
  header gives an `Anchored` child a negative layout origin, which previously put
  the panel over the title bar; the pointer is the reliable origin. The app menu
  keeps its title-bar `Anchored` placement.
- T5 is a `select` directive: `View_GridSettings` returns `{"select":
  "Settings/Grid"}`, `CommandOutcome.select` carries it through the facade's
  `_payload`, and `Shell::run_command` routes it through the normal selection path.
  The tree spelling (`Settings/grid`) and the display form (`Settings/Grid`) are
  the same node. Selecting a `Settings/…` node opens the **modeless settings card**:
  a draggable 380 px panel with no backdrop, the property rows, a `✕`, and `Esc`
  dismissal. It reuses `properties_panel` and `set_property`; the Dev Tools pane
  remains a second view of the same rows.
- The dock is `model.surfaces().dock`, so the existing `IsActive` results drive it.
  Disabled rows stay visible. `Search (Ctrl+K)` is a status-reporting stub; there is no
  palette.
- T6 removed `actions_bar`; the Dev Tools window lists **every** command vertically,
  with its surface tag, as the reachability safety net.
- T7's app menu is **title-bar chrome**, not header chrome: `Shell::render` builds it
  with `chrome::app_menu` and passes it to
  `ferrocad_widgets::window_chrome::window_frame_with_leading`, which puts it at the
  leading edge of the title bar. The viewport header keeps only `Grid`, `Axes`, `WP`
  and `Mods`. The menu button (and its popover) stop mouse-down propagation so a
  click does not also start a window move; the Dev Tools window calls plain
  `window_frame`, so its title bar has no menu. When the platform draws the
  decorations (macOS, some Wayland compositors), the frame draws a slim
  `leading-bar` fallback below the platform title bar instead of dropping the menu;
  the headless test platform reports server-side decorations, so that branch is what
  the T7 test exercises. The client-decoration `title_bar` is rendered in a harness
  window for the same clicks, popover, `Esc` and row execution; the real
  client-decoration path remains visual-only.

## 9. Risks

- **Anchoring in GPUI.** A popover is an absolutely positioned child of the pane, not a
  window; the anchor must survive layout changes and window resize. T1 should be
  proven with a trivial popover before T2 wires the real controls.
- **No display in the dev sandbox.** State transitions can be unit-tested, but the
  visual checks in §8 need the Xvfb job or a human. Do not claim a visual pass from a
  headless run.
- **Command surface drift.** A mod that registers a command with no `FerroCAD_Surface` must still
  appear. The default must be the dock (or `hidden` with a warning), never "nowhere".
