# Task — viewport event routing: occlusion and the settings card

Status: task (2026-10-22). P0.8 in [`../TODO-22-10-2026.md`](../TODO-22-10-2026.md).
Companions: [`toolbar-chrome.md`](toolbar-chrome.md) (popovers and the settings card),
[`settings-model.md`](settings-model.md) (the `Settings/Grid` rows) and
[`../interaction/visual-language.md`](../interaction/visual-language.md) (glass surfaces).

## 1. Drags/clicks on a pane over the viewport reach the viewport

**Symptom.** Pressing and dragging inside a pane that floats over the 3D canvas also
orbits/picks the scene underneath. Reproduced with the Grid settings card and the tool
pane (the action dock / transient tool surface).

**Reproduction.**
1. Open `Grid ▾` → `Grid settings` so the modeless card is over the canvas.
2. Press inside the card and drag (do not start on the header): the camera orbits as
   if the press started on the canvas.
3. Repeat on the bottom action dock between its buttons.

**What the code does today.**
- The modal `dialog_overlay` backdrop is explicitly `.occlude()`d
  (`crates/ferrocad_gpui/src/shell.rs`, `dialog_backdrop`, ~L1737), so the modal path
  is already guarded.
- The modeless card root (`shell.rs`, `settings_card_view`, ~L2013) has no
  `.occlude()`. Its child rows are hitboxes, but the card's background and padding
  are not, so a press on the card falls through to the viewport.
- The action dock panel (`crates/ferrocad_gpui/src/chrome.rs`, `action_dock`, ~L470)
  deliberately has no hitbox on its wrapper so clicks *around* it reach the canvas.
  The panel itself has no `.occlude()`, so a press on its background/dividers
  propagates.
- The viewport's pointer listeners (`crates/ferrocad_viewport/src/inventor_view.rs`,
  `on_mouse_move` ~L3191 and `on_any_mouse_down` ~L3198) therefore receive the event.
  The viewport cannot know a pane is above it; the pane has to own the hitbox.

**Proposed fix.**
1. Add `.occlude()` to the **card root**, the **dock panel** and any floating tool
   pane. Keep it off the wrapping layers that should be transparent to the canvas, so
   a click beside the card/dock still orbits/picks as before.
2. Do not copy the popover's no-occlude design to these surfaces: popovers are
   intentionally non-modal, but a drag that starts on a pane must not also orbit.
3. Add a headless regression test: open the card, drag inside it, assert the camera
   did not move; click beside it, assert the canvas still receives the event.
4. If `.occlude()` does not stop the drag in the live `gpui_interop` path, capture a
   minimal two-layer repro and file it against `bite-gpui/gpui_interop` (this is the
   upstream half of the report). Keep the shell-side hitboxes either way.

## 2. Clicking an editable field in the Grid settings card closes it

**Symptom.** Clicking the `minor`/`enabled`/`opacity` field in the Grid settings card
focuses it for a moment, then the card disappears.

**Likely root cause.** This is probably the same missing occlusion as §1:
- The click falls through to the viewport.
- The viewport reports an empty selection; `Shell::replace_selection` runs
  (`shell.rs`, ~L1290) and sets `settings_card = None` when the primary is not a
  `Settings/...` node.
- `Shell::refresh` (~L1320) also drops the card when the selected node is no longer
  a `Settings/...` node.
- Nothing in the card's own click path closes it; `✕` and `Esc` are the only
  intentional closers.

**Proposed fix.**
1. Fix §1 first (occlude the card) and re-test; the field click should no longer reach
   the viewport.
2. Keep the card open across a click/focus inside it. Do not close it from
   `end_settings_drag`, `on_mouse_up`, or a selection refresh that still names a
   `Settings/...` node.
3. Add a regression test: open `Settings/Grid`, click/focus
   `Settings/Grid.minor`, assert the card is still open, `model.selected` is still
   `Settings/Grid`, and the camera did not move. Then close with `✕` and `Esc`.

## Acceptance

- A press/drag that starts inside the Grid settings card, the action dock or a
  floating tool pane never reaches the viewport (camera unchanged, no pick).
- A press beside those surfaces still reaches the viewport (orbit/pick works).
- Clicking/focusing an editable Grid settings row keeps the card open; only `✕` or
  `Esc` closes it.
- A headless gpui test covers both the card and the dock; a manual X11/Wayland pass
  confirms a real pointer drag.

## Non-goals

- Making popovers modal or adding a backdrop scrim.
- Changing the viewport's own pick/orbit semantics.
- Docking or tabbing the card/dock.

## As built (2026-10-22)

- `.occlude()` now sits on the settings-card root, the action-dock panel and the tool
  strip. The dock's outer wrapper stays transparent, so clicks beside the pill still
  reach the canvas; the modal backdrop was already occluded.
- Because the card's occlusion also blocks the shell root's pointer moves, the card
  follows the header drag itself (`on_mouse_move` / `on_mouse_up` on the card root).
  The shell-root handlers remain for drags that leave the card.
- `properties_panel` exposes a `prop-<object>.<name>` debug selector for each editable
  row. `clicking_a_settings_field_keeps_the_card_open` clicks
  `prop-Settings/Grid.minor` and asserts the card, selection and rows survive.
- The card drag test still passes with occlusion.

**Live interop caveat.** Headless tests prove the card/dock hitboxes and the field
regression, not the real `gpui_interop` drag path. If the live build still orbits from
a pane press, capture the minimal two-layer repro for `bite-gpui/gpui_interop`; the
shell-side hitboxes are already in place.

## Risks

- **Interop.** If `occlude()` is not honoured through the gpui interop layer, the
  fix may need an upstream change. Record the minimal repro separately rather than
  papering over it with shell-side hacks.
- **Over-occluding.** Occluding the shell wrapper would make the whole canvas inert.
  Only the pane/card/dock panels should own hitboxes.
