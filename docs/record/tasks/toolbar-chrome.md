# Task — viewport chrome: a toolbar system and one home for grid settings

Status: closed task spec (moved to `record/tasks/` on 2026-10-27). Original status: task (2026-10-22). P0.6 in [`TODO-22-10-2026.md`](../TODO-22-10-2026.md).
Companion to [`../interaction/visual-language.md`](../../interaction/visual-language.md)
(tokens and states), [`../workbenches/actions-and-tools.md`](../../workbenches/actions-and-tools.md) §7
(the view/tool requests a command returns),
[`../interaction/axis-origin-grid.md`](../../interaction/axis-origin-grid.md) (what the grid
toggles) and [`../direction/app-shell-vision.md`](../../direction/app-shell-vision.md) S2.

**Scope note.** T0–T7 landed on 2026-10-22: the chrome (header controls, popovers,
the action dock, the title-bar app menu) and the grid-settings half. T4/T5 are wired
to [`settings-model.md`](settings-model.md) S2/S4. The remaining acceptance items are
visual or manual (popover/navicube overlap, the dock's drag fade, and the viewport
receiving pointer events through an open popover).

## 1. Why

The current shell puts **every** registered command in one full-width strip above all
panes ([`crates/ferrocad_gpui/src/shell.rs`](../../../crates/ferrocad_gpui/src/shell.rs),
`actions_bar`, rendered at `Shell::render`). With two grid mods loaded that is fourteen
text buttons (`Delete`, `Move`, `Grid`, `Axis cross`, `Grid settings`, `Mods`,
`Reload grid settings`, `Activate workplane`, `Clear workplane`, `View workplane`,
`Create Slab`, `Polyline`, `Grid Overlay`, `Grid Overlay (Compact)`), mixing four
different kinds of thing:

| Kind | Examples | Frequency | Belongs in |
| --- | --- | --- | --- |
| environment toggle | `Grid`, `Axes`, `WP` | constant | the floating viewport pill |
| environment sub-state | overlay on/off, overlay mode, reload | occasional | a popover on that control |
| provider / workbench | `Mods`, provider switch | occasional | a popover on that control |
| creation / modification | `Create Slab`, `Polyline`, `Move`, `Delete` | per action | a floating dock over the canvas |

The strip also cannot scale to a real workbench (`Draft` registers 86 commands), and it
consumes vertical space from the canvas. The target is three surfaces that do not
compete: **header controls**, **anchored popovers**, and a **selection-reactive dock**.

## 2. State before this task (verified 2026-10-22)

The line numbers and the flat strip below describe the tree **before** T0; the strip
no longer exists (§10).

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

1. **Viewport controls, floating pill** (top-left over the canvas, never over the
   navicube):
   - There is no `3D viewport` pane header row. The four controls float as a compact
     translucent pill over the canvas, so the scene reaches the title bar instead of
     losing a full-width strip to chrome.
   - `Grid` split button: the label toggles visibility in one click; `▾` (or right-click)
     opens a popover with **Grid provider** (the active `(grid, backend)`), **Spacing**
     (`10.0 mm`, tabular), then `Grid Settings…` (selects `Settings/Grid`) and
     `↻ Reload Grid Settings`. No separate overlay on/off toggle (the label is the one
     switch) and no `Compact | Full` segmented control (a local grid follows the
     workplane); the checked provider/backend entry in the popover is the mode row.
   - `Axes` toggle button (`App_ToggleAxisCross`).
   - `WP` split button: the label runs `App_ViewWorkplane`; `▾` offers
     `Activate workplane` and `Clear workplane`.
   - `Mods ▾`: the provider/mod checklist (`Mods_Settings`), also offered inline as
     **Provider / Mod** in the Grid popover.
2. **Action dock**, bottom-centre over the canvas: `Create Slab` · `Polyline` ·
   `Workplane` · `Search (Ctrl+K)` when idle; the selection signature enables or greys
   the modification rows (`Delete`, `Move`, then workbench-specific entries) rather
   than removing them. It recedes to 35 % during orbit/gizmo drags and returns on
   proximity (the fade is not built; see T3).
3. **Grid settings are modeless**: `Grid Settings…` selects the `Settings/Grid` node in
   the model tree and the existing Properties pane renders its rows
   ([`settings-model.md`](settings-model.md)). There is no separate settings screen.
4. **App menu**, at the leading edge of the **window title bar** (moved there from the
   viewport header): `☰` opens app-level commands that are not viewport environment
   controls. Dev Tools is the first entry (checked while the window is open, showing
   `Ctrl+Shift+D`); About and Preferences follow. It exists because Dev Tools starts
   closed and is hidden from every other surface, so the shortcut was the only way in.
   The app menu remains title-bar chrome; it is not part of the floating control pill.

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
  [`actions-and-tools.md`](../../workbenches/actions-and-tools.md) §7 and §16, which
  currently document it as the A1/A4 surface.

## 5. Slices

| # | Slice | Verifiable headlessly |
| --- | --- | --- |
| **T0** | Surface metadata: parse `FerroCAD_Surface`/`FerroCAD_Parent` from `GetResources` (an absent or unknown value defaults to the **dock**, never derived from `Group`), a `Surface` enum, and a `shell_model.surfaces()` grouping | unit tests in `ferrocad_gpui::commands` ✅ |
| **T1** | `split_button` + `popover` primitives (open state, anchor, dismissal, right-click) | state-machine tests; rendering by inspection ✅ |
| **T2** | Header controls: `Grid`, `Axes`, `WP`, `Mods` wired to the existing command ids, right-aligned in the viewport header | model tests: clicking the label runs the command; `▾` opens the group without running it ✅ |
| **T3** | Action dock replaces the strip; selection-driven enablement; drag/proximity opacity | model test: the dock's rows follow the selection through `IsActive` ✅ (rows are never hidden; drag opacity deferred) |
| **T4** | Grid quick popover: **provider** readout, spacing readout, settings/reload rows (no compact/full mode) | gpui test: the rows follow the active provider ✅ |
| **T5** | `Grid Settings…` selects the `Settings/Grid` node and opens the modeless 380 px card (draggable, no scrim) | gpui test: the directive selects the node, the card opens/drags/closes, and its rows edit ✅ |
| **T6** | Remove `actions_bar` and its layout slot; update the docs; keep every command reachable | `every_registered_command_lands_on_exactly_one_surface`, plus the Dev Tools list holding every command ✅ |
| **T7** | App menu (`☰`) in the window title bar: app-level commands, Dev Tools first, checked while open | `the_app_menu_opens_dev_tools`: title-bar/leading-bar placement, the popover, `Esc`, and the row opening the window ✅ |

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
- The app menu is **title-bar** chrome (`window_frame_with_leading`), not a viewport
  pill control; the pill keeps `Grid`, `Axes`, `WP` and `Mods`. ✅ (T7 test asserts
  the menu is outside the header, inside the title bar, and that the `leading-bar`
  fallback exists; the client-decoration frame path is visual-only.)
- `Grid` toggles in one click without opening the popover; `▾` opens it without toggling. ✅
- With a popover open, viewport orbit/zoom still receives pointer events; `Esc` and an
  outside click both dismiss. ✅ for dismissal (verified by
  `the_grid_split_button_runs_the_label_and_the_caret_opens` and
  `the_app_menu_opens_dev_tools`); the panel has no backdrop and no `occlude` **by
  inspection**. That orbit/zoom still receives the events is **not verified** — it
  needs a visual/manual pass.
- Popovers never overlap the navicube at any window width ≥ 800 px. ⏳ (visual only)
- A split-button popover opens below its control, inside the window, not over the
  title bar. ✅ (headless bounds assertion)
- The dock's rows follow the selection signature through `IsActive` (disabled rows stay
  visible; nothing is filtered out). ✅ Verified by
  `the_action_dock_follows_the_selection`; the drag/proximity fade is **not built**
  (deferred with T3's visual half) and is unverified.
- `cargo test -p ferrocad_gpui` and `cargo test -p ferrocad_toy` stay green. ✅
  (2026-10-22 verification: gpui lib **115 passed**, toy **14 passed**.) The Xvfb
  real-window smoke test was **not run** in this pass — it remains unverified.
- The docs (`actions-and-tools.md` §7/§16) describe the new surfaces, not the strip. ✅

## 10. As built (2026-10-22)

- `commands.rs` owns `Surface` (`grid`, `axis`, `workplane`, `mods`, `dock`, `hidden`),
  `CommandInfo::surface()/parent()/dev_only`, `group_by_surface()` and
  `popover_commands()`. An unannotated **or unknown** surface is the dock.
- The `FerroCAD_Surface`/`FerroCAD_Parent`/`FerroCAD_DevOnly` keys are written in each command's `GetResources()`; the shell
  bridge (`ferrocad_shell.commands()`) copies them into the JSON list. `FreeCADGui._command_list()`
  also forwards `dev_only`, while its public upstream surface stays unchanged, so the facade
  keeps its upstream shape and a mod that does not know the key is unaffected.
- `chrome.rs` holds the primitives: `split_button`, `popover`, `action_dock` and the Dev
  Tools `command_list_panel`. The popover is a `deferred(anchored(..))` child of the
  control, with `on_mouse_down_out` for outside-click dismissal. The **caret opens**
  rather than toggles: the outside-click handler fires before the caret's click, so a
  toggle would immediately reopen the menu. `Esc` closes it from the shell key path.
- A popover lists the commands on that surface whose `FerroCAD_Parent` names the
  control's primary. When a surface has no such children, it falls back to the
  surface's own commands so the caret is never dead. Commands carrying
  `FerroCAD_DevOnly` are filtered from `popover_commands`: they stay in the
  registry and the Dev Tools list, but never appear in a header popover.
- T4 adds two read-only rows at the top of the **Grid** popover: `Provider: <label>`
  and `Spacing: <n mm>` (`grid-row-provider`, `grid-row-spacing`). They come from
  `ferrocad_shell.grid_summary()`, fetched when the popover opens (not per frame),
  and refreshed after a `Settings/…` edit. With no provider the panel says
  `No grid provider`; there is no compact/full mode row.
- Both the **Grid** and **Axes** popovers start with one provider row per registered
  backend (id `ferrocad-provider:<feature>:<backend>`, label from the manifest, the
  active backend carrying the `#38BDF8` status dot). Choosing a row calls
  `ferrocad_shell.set_provider` through `Shell::activate_provider`, applies the
  returned `view`, and re-reads both summaries. It is not a registry command: the
  shell intercepts the synthetic id in `run_command`. The generic settings/reload
  rows follow under a `1 px` `#FFFFFF14` hairline.
- The backend-specific settings commands (`GridOverlay_Settings`,
  `GridOverlayCoin_Settings`, `AxisOverlay_Settings`, `AxisOverlayCoin_Settings`) and
  `Mods_Settings` are **dev-only**: they remain registered and appear in the Dev
  Tools command list, but no header popover offers them. The Grid menu therefore
  no longer has a mode/config partition of backend rows; the provider group replaces
  it. The Dev Tools body also has an inline **Providers** panel (one button per
  backend, the active one accented) that replaces the Mods dialog as the user-facing
  switch.
- The **Axes** control mirrors the Grid one as a split button (`Axes ▾`): the label
  runs `App_ToggleAxisCross`, the caret opens the `axis` popover, and the state dot
  reports the cross's on/off state (`axis-state-dot`). Three read-only rows report
  `Provider: <label>`, `Length: <n px>` and `Width: <n px>` (the panel says
  `No axis provider` with none) from `ferrocad_shell.axis_summary()`, fetched when
  the popover opens and refreshed after a `Settings/…` edit. The provider rows for
  the `axis` feature lead the popover, followed by the generic
  `View_AxisSettings` and `App_ReloadAxisSettings` rows; the two
  `AxisOverlay*_Settings` commands are dev-only and do not appear.
- **Anchoring.** A split-button popover is placed from the caret's actual pointer
  event (`popover_anchor`) in window coordinates, 18 px below the press. An `Anchored`
  child of the old flex header reported a negative layout origin, which previously put
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
  Disabled rows stay visible and no row is filtered by the selection; the drag/proximity
  fade has no code yet. `Search (Ctrl+K)` is a status-reporting stub; there is no
  palette.
- T6 removed `actions_bar`; the Dev Tools window lists **every** command vertically,
  with its surface tag, as the reachability safety net.
- T7's app menu is **title-bar chrome**, not header chrome: `Shell::render` builds it
  with `chrome::app_menu` and passes it to
  `ferrocad_widgets::window_chrome::window_frame_with_leading`, which puts it at the
  leading edge of the title bar. The viewport controls float as a compact translucent
  pill (`rgba(0x151a24cc)` over a `#FFFFFF14` hairline) at the canvas' top-left, with
  `Grid`, `Axes` and `WP`; the `Mods` control left the header (its `Mods_Settings`
  command remains registered as dev-only, so it is reachable from Dev Tools). The
  pill occludes so a press on it does not start a scene orbit, while its anchored
  popovers keep their own hitboxes. There is no
  `3D viewport` sub-header: the canvas starts at the pane top and only the transient
  tool strip still consumes a row while a session runs. The menu button (and its
  popover) stop mouse-down propagation so a
  click does not also start a window move; the Dev Tools window calls plain
  `window_frame`, so its title bar has no menu. When the platform draws the
  decorations (macOS, some Wayland compositors), the frame draws a slim
  `leading-bar` fallback below the platform title bar instead of dropping the menu;
  the headless test platform reports server-side decorations, so that branch is what
  the T7 test exercises. The client-decoration `title_bar` is rendered in a harness
  window for the same clicks, popover, `Esc` and row execution; the real
  client-decoration path remains visual-only.
- **2026-10-24 — dielectric state.** Popover rows moved to the domain-free
  `ferrocad_widgets::menu` (left `#38BDF8` dot, additive `#FFFFFF14` hover, no
  checked fill). The action dock follows the same language: a **live tool** or a
  checked row is `ui::ACTIVE_ACCENT` ink, not a filled `ACCENT` chip, and hover is
  the additive `ui::ROW_HOVER`; the strip itself is `ui::DOCK_BG` over
  `ui::DOCK_BORDER`. The dock's `Move` split keeps its caret as the options cue.
  Popovers and the dock are now visually and behaviourally consistent (state rows
  keep the menu open, action rows close it).
- **2026-10-24 — object verbs are radial.** `App_Move` and `App_Delete` moved to
  `Surface::Radial` (tone `structural` / `destructive`), so the dock is creation +
  search (`Toy_MakeSlab`, `Toy_Polyline`, `Search`) and never reflows on selection.
  A right-press (or `Space`) opens the in-situ wheel at the pointer; the pure
  `ferrocad_widgets::wheel` paints it and `slot_at` hit-tests it, while the stateful
  `WheelMenu` sidecar owns the `Armed`/`Latched` phases, the hover, the tap-vs-flick
  timing and its own overlay. A flick commits on release; a quick tap
  (< 200 ms, < 12 px) **latches** the wheel for trackpad use, and a primary click then
  commits. Both the right-press and `Space` re-pick at the pointer through the
  viewport's G-buffer path, so the wheel follows the object under the cursor rather
  than the previous selection. **Opening the wheel never changes the selection**: the
  pick is a context target, promoted only when a verb is chosen; empty space falls back
  to the selection's verbs (or creation when nothing is selected), so the key is
  contextual rather than global. A long hold released in the hollow centre cancels. The
  dock's `Move` split (and its gizmo-options popover) is gone; the gizmo is armed by
  selection and its settings stay in `Settings/Gizmo`.
- **2026-10-25 — geometric dialogs float.** `App_Move` asks for
  `"presentation": "float"`, so its dialog is a compact `WHEEL_GLASS` HUD beside the
  gizmo instead of a centred 380 px panel over a dimmed backdrop. The canvas stays
  interactive, the first Δ field is focused and selected (`Tab` cycles), and
  `"preview": "translation"` composes a temporary transform on the selection while
  typing — no transaction until `Enter`. A gizmo drag while the HUD is open is
  **deferred** (`ViewportEvent::TransformPreview`): each tick mirrors the accumulated
  delta into the Δ fields and keeps previewing, and only Accept applies the move, so
  dragging and typing cannot double-apply. The gizmo itself is anchored at the picked
  world point (`SelectionAnchor`), or the body's base corner for a tree selection, and
  it follows a live preview (as do manipulators with a world anchor), and it stays glued
  to the moved surface even during its own drag (a draw-only offset; the interaction
  frame is frozen at the grab point). After a committed edit the shell re-arms the
  current selection, so a height grip stays on the moved top face and the triad does not
  vanish; any competing interaction (for example a height change) closes the pending HUD
  instead of leaving a stale delta, and so does replacing or clearing the selection (the
  form is scoped to the selection it was opened for).
  Creation dialogs (`Toy_MakeSlab`) keep the modal presentation until they grow a
  placement preview.
- **2026-10-25 — the floating HUD is clamped and freezes.** Its placement is now a
  pure rule (`place_hud`/`clamp_hud`): below-right of the gizmo (or the live pointer
  when no gizmo is armed), kept 16 px from the window edges and 64 px above the bottom
  dock. While the pointer is inside the painted HUD rect or a Δ field holds focus, the
  screen position is **frozen**, so typing `500` into ΔX cannot slide the inputs out
  from under the cursor; releasing focus and leaving the rect resumes tracking. The
  painted rect is captured with `on_children_prepainted` for hit-testing, and hover is
  read from `window.mouse_position()` because a mouse-move listener only fires on the
  hovered hitbox. Tests: `a_floating_hud_is_clamped_into_the_safe_area`,
  `a_focused_floating_hud_freezes_in_place`.

## 9. Risks

- **Anchoring in GPUI.** A popover is an absolutely positioned child of the pane, not a
  window; the anchor must survive layout changes and window resize. T1 should be
  proven with a trivial popover before T2 wires the real controls.
- **No display in the dev sandbox.** State transitions can be unit-tested, but the
  visual checks in §8 need the Xvfb job or a human. Do not claim a visual pass from a
  headless run.
- **Command surface drift.** A mod that registers a command with no `FerroCAD_Surface` must still
  appear. The default must be the dock (or `hidden` with a warning), never "nowhere".
