# Task — viewport feedback and window-bounds correctness

Status: closed task spec (moved to `record/tasks/` on 2026-10-27). Original status: task (2026-10-22). P0.8 in [`TODO-22-10-2026.md`](../TODO-22-10-2026.md).
Companions: [`toolbar-chrome.md`](toolbar-chrome.md) (Grid split button),
[`../../../developer/interaction/visual-language.md`](../../../developer/interaction/visual-language.md) §1.1 (canvas)
and [`../../direction/app-shell-vision.md`](../../direction/app-shell-vision.md) (window
frame).

## 1. The Grid control gives no on/off indication on hover

**Symptom.** Hovering `Grid ▾` does not tell you whether the grid is currently enabled
or disabled; the only state cue is the split button's background, which is easy to
miss.

**What the code does today.**
- `crates/ferrocad_gpui/src/chrome.rs`, `split_button`: the control background is
  `ACCENT` when `command.checked`, else `PANEL_ALT` (~L258). There is no hover text,
  tooltip, dot or check mark, and the label `Grid` never changes.
- `App_ToggleGrid.IsChecked` (`python/ferrocad_shell/__init__.py`, ~L723) reads
  `FerroCADGui.ViewState.grid`. The shell mirrors the state after a `{"view": …}`
  request (`Shell::apply_view`), so a native toggle is reflected. A backend that
  changes the grid outside that path (the Coin overlay's own switch) can leave the
  mirror stale until the next refresh.

**Proposed fix.**
1. Show the state on hover: a tooltip/status suffix (`Grid: on` / `Grid: off`) and a
   small state dot or check on the control. The hover cue must not require opening
   the popover.
2. Keep the checked background but make the on/off dot the primary cue; it must be
   visible against both `PANEL_ALT` and `ACCENT`.
3. Refresh `IsChecked` after `App_ReloadGridSettings` and after a provider `refresh`,
   so a Coin-side toggle updates the control.
4. Add a gpui test: toggling `App_ToggleGrid` flips the control's checked state and
   the hover label; `App_ReloadGridSettings` re-reads the same state.

**Acceptance.** Hovering the Grid control names the current state without opening the
popover; the label/dot matches the drawn grid after a toggle, a provider switch and a
reload; a headless test covers both states.

## 2. Maximized window: pointer coordinates are wrong and the cursor becomes a resize grip

**Symptom.** After maximizing the window, pointer positions inside the 3D viewport are
offset from the pointer, and the cursor changes to a window-resize shape while over
the viewport content.

**What the code does today.**
- `crates/ferrocad_widgets/src/window_chrome.rs`, `window_frame_with_leading`: it
  calls `window.set_client_inset(INSET)` unconditionally and always draws
  `resize_canvas(INSET)`. The `Decorations::Client { tiling }` value only decides
  which side is padded; it does not disable the grips.
- `resize_canvas` (`window_chrome.rs`, ~L233) inserts a full-window hitbox and
  `resize_edge` (~L210) returns an edge purely from the pointer's distance to the
  window bounds. On a maximized/tiled window there is no edge to drag, but the
  cursor still becomes `ResizeUpDown`/`ResizeLeftRight` near the border.
- The viewport converts window-relative logical points to local points by
  subtracting the element's prepaint bounds (`InventorViewport::to_local`,
  `crates/ferrocad_viewport/src/inventor_view.rs`, ~L1286) and sizes the camera from
  `last_bounds × window.scale_factor()` (`sync_size`, ~L1302). A client inset or a
  stale bounds after maximize/resize shifts the mapping, so picking lands off the
  pointer.

**Proposed fix.**
1. Do not inset or draw resize grips on a maximized/tiled window. Use the window's
   maximized/tiled state (or `tiling` per edge) to disable `resize_canvas` and
   `set_client_inset` on those edges.
2. Keep the resize cursor and hitbox only on edges that can actually resize; the
   content area must keep the normal arrow/selection cursor.
3. Ensure `last_bounds` is refreshed on a window resize/maximize before the next
   pointer mapping; if the client inset changes, invalidate the cached bounds.
4. Add tests: `resize_edge` returns `None` on a tiled edge; `to_local` uses the
   post-maximize element bounds; a manual X11/Wayland pass checks that a click on a
   known viewport point selects the expected object and that the cursor is not a
   resize grip inside the canvas.

**Acceptance.** At maximized size: no resize grip over the content; a click on a known
point picks the object under the pointer; restoring the window brings the grips back
on the resizable edges. The same holds after a resize/DPR change.

## Non-goals

- Redesigning the title bar or the window frame.
- Changing the camera projection or the picking semantics.
- Adding a new tooltip widget system; the existing status line/tooltip primitives are
  enough for the Grid cue.

## As built (2026-10-22)

- **Grid feedback.** The Grid split button now renders a persistent `grid-state-dot`
  (`CHROME_ACTIVE` when on, `MUTED` when off) and a tooltip reading `Grid: on` /
  `Grid: off`. The test asserts the dot exists; the existing toggle test asserts the
  checked state flips.
- **Maximized window.** `frame_inset(maximized)` collapses the client inset to `0`;
  the frame skips per-edge padding and the full-window resize canvas when maximized
  or fullscreen, and the frame's resize handler returns early. A unit test covers the
  inset helper. The viewport's `last_bounds` mapping therefore receives the same
  content origin as the pointer.
- **Reopen caveat.** A real maximized X11/Wayland pass is still needed to confirm the
  platform's `is_maximized`/fullscreen report and that no resize cursor remains at the
  content edge.

## Risks

- **Platform differences.** `WindowDecorations` and tiling reports differ across
  X11, Wayland and macOS. Gate the fix on the actual per-edge tiling state, not a
  platform check.
- **Stale bounds for one frame.** A resize can produce one frame with old bounds;
  do not let that frame's pick mutate the document/selection. Prefer updating the
  mapping on prepaint and ignoring pointer events until the new bounds are known.
