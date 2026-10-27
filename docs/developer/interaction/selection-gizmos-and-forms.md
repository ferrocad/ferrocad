# Selection, gizmos, the floating form and the wheel — one interaction model

Status: **design decision (2026-10-25).** This note is the authority for how a pointer
gesture becomes a selection, a context menu, a manipulator drag, or an editable
transform. It records the state machines as built, including the fixes for two reported
defects: right-click changing the selection merely by opening the wheel, and a pending
Move form surviving the loss of its selection.

Companions: [`gizmo-manipulators.md`](gizmo-manipulators.md) (manipulator math and
topology), [`topology-selection.md`](topology-selection.md) (the pick/selection seam),
[`visual-language.md`](visual-language.md) (tokens and layer inks), and
[`../workbenches/actions-and-tools.md`](../workbenches/actions-and-tools.md) §6–§7 (the
command/dialog protocol).

## 0. Principles

1. **Geometry is the interface.** Chrome recedes; a menu, a HUD or a triad never covers
   the thing it edits and never dims the canvas as its primary state.
2. **Opening an affordance is not a mutation.** Resolving a context target, hovering a
   wedge or showing a form changes no document state and no selection. Only a chosen
   verb or a completed drag commits.
3. **One owner per state.** The viewport owns picking and gesture math; the widget owns
   the wheel's armed/latched phases and overlay; the shell owns the selection, the
   command registry, the form and precedence; Python owns validation and the
   transaction.
4. **Every pending state has an explicit cancel.** A form, a preview, a wheel and a
   drag can all be abandoned without a partial edit, and losing their context
   (selection, document, competing interaction) cancels them.
5. **State is ink; background is only ever an additive hover.** (See the visual
   language.) The interaction model does not depend on a filled "active" surface.

## 1. Vocabulary

| Term | Meaning | Owner |
| --- | --- | --- |
| **Selection** | The persistent set of document objects (and optional sub-elements) that commands act on. The last item is primary. Pushed to `FreeCADGui.Selection`. | `Shell` |
| **Context target** | The object under the pointer when a wheel opens. Used only to populate/act on that wheel; not a selection. | `Shell` (`WheelTarget`) |
| **Origin** | The world point a gizmo hugs: the display provider's declared placement origin (`StageMesh.origin`), or the body's bounding-box base corner when it reports none. | `ferrocad_stage` (`ShapeItem::anchor`) → `TransformGizmo` |
| **Gizmo** | The generic transform triad (`TransformGizmo`) with translate/rotate/universal handle sets. | `ferrocad_stage` |
| **Manipulator** | A workbench handle contributed by a factory (the toy height grip, the Part fillet arc). | workbench crate |
| **Wheel** | The radial marking menu (`WheelMenu`): armed/latched phases and its own anchored overlay. | `ferrocad_widgets` |
| **Form** | A command dialog spec hosted by the shell: `modal` (centred, dimmed backdrop) or `float` (dielectric HUD, no backdrop). | `Shell` |
| **Preview** | A temporary, non-document transform composed on the stage items. Never enters the undo stack. | `InventorViewport` |
| **Deferred drag** | A gizmo drag that reports deltas to the host instead of committing on release; used while a translation form is open. | viewport + shell |

## 2. Selection and the context target

### 2.1 Selection

- A **viewport click** produces `SelectionEvent::Picked { reference, anchor }`. The
  anchor is the world point and normal of the picked pixel (G-buffer first, then the
  fallback pick). Empty space produces `Cleared`.
- A **tree click** produces a name-only selection (`replace_selection`); there is no
  anchor.
- Clicking a viewport object that is already selected does not replace a
  multi-selection; it remains part of it.
- The selection is the *only* thing a command reads through
  `FreeCADGui.Selection`. It is pushed before command enablement is listed.

### 2.2 Context target (the wheel)

A context menu must answer "what did I point at?" without silently rewriting the
selection. The rule is therefore:

| Pointer at open | Context target | Wheel shows | Selection at open | Selection on commit |
| --- | --- | --- | --- | --- |
| object already in the selection | that object | selection verbs (registry enablement) | unchanged | unchanged |
| object outside the selection | that object | object verbs, offered (the target will be promoted) | unchanged | the target replaces the selection |
| empty space, selection exists | none | selection verbs | unchanged | unchanged |
| empty space, no selection | none | creation verbs from the dock | unchanged | unchanged |

- **Open is read-only.** `Shell::open_radial` / `space_radial` resolve the target with a
  fresh viewport pick (`handle_pointer_at`), store it as `WheelTarget`, and build slots.
  They never call `replace_selection`.
- **Commit promotes.** `WheelEvent::Selected(id)` goes through `commit_wheel`, which
  promotes a target outside the selection with its anchor (so the gizmo lands at the
  contact point), then runs the command. A target already in the selection is not
  re-selected; the command acts on the whole selection.
- **Cancel discards.** `WheelEvent::Closed` clears `WheelTarget`; `Esc`, a selection
  change and a document prune also close the wheel.
- Object verbs for an unselected target cannot rely on the registry's selection-based
  `IsActive` at open time. They are offered, and the command validates after promotion.

## 3. The wheel state machine

```text
                 right-press / Space
        Closed ───────────────────────► Armed / Latched
          ▲                                │  hover: active = wedge | None
          │                                │
          │  Esc, selection change,       │  release (Armed):
          │  document prune, canvas        │    wedge → Selected(id) → Closed (commit)
          │  dialog opens                 │    else tap (<200 ms, <12 px) → Latched
          │                                │    else → Closed (cancel)
          └────────────────────────────────┤  left click (Latched):
                                           │    wedge → Selected(id) → Closed
                                           │    deadzone/outside → Closed
                                           └─ Space (wheel open): commit hovered or close
```

| Phase | Pointer | Keyboard |
| --- | --- | --- |
| `Armed` | right button held: move highlights, release commits/latches/cancels | — |
| `Latched` | no button: move highlights, primary click commits/cancels | `Space` commits the hovered wedge or closes; `Esc` closes |

- **Tap vs flick:** a release within **200 ms** and **12 px** of the press latches (the
  trackpad case). A release over a wedge commits regardless of time. A long hold
  released in the deadzone cancels.
- **Overlay:** the sidecar's overlay `occlude`s only its own square, so the canvas is
  still live around it. It carries its own `on_mouse_move` because occlusion hides the
  canvas hover.
- **Target lifetime** is exactly the open wheel. The wheel has no target of its own;
  the shell holds it and discards it on `Closed`.

## 4. The gizmo state machine

### 4.1 Anchor and modes

- The triad hugs the object's **own origin** (the display provider's placement base
  point, else the bounding-box base corner), which also makes it an origin indicator.
  A click does not move it; a user-chosen origin is an explicit, snapped gesture
  ([`transform-ux.md`](transform-ux.md) §3).
- Modes: `Translate` (axis stems + plane handles), `Rotate` (rings), `Universal` (both).
  The generic factory currently offers `Translate` for whole objects; sub-element
  transforms await a target-aware interaction commit.
- A manipulator may carry a world anchor (`Manipulator::anchor`/`set_anchor`); the
  height grip does, so it can follow a previewed body.

### 4.2 States and transitions

```text
Absent ──selection with a target──► Idle ◄──document edit: re-arm at new geometry
                                     │  move: Hover(handle)
                                     │  press on a handle
                                     ▼
                                  Dragging ──deferred?──► reports Begin/Update/End
                                     │                     (host previews + commits)
                                     │  release (normal)
                                     ▼
                              Commit emitted → host applies → re-arm
                                     │  Esc / cancel
                                     └────► restore frame and preview, Idle
```

| Event | Normal drag | Deferred drag (translation form open) |
| --- | --- | --- |
| press on handle | starts drag; frame frozen at press | starts drag; emits `Begin`; shell captures the Δ-field baseline |
| move | `drag_to`; target gets a temporary transform; **grips follow**; the triad's *drawing* is offset so it stays on the surface | same, plus emits `Update`; shell mirrors the accumulated delta into the fields and previews the whole selection |
| release | `GizmoCommitEvent` → `"transform"/"apply"` → one transaction; the overlays re-arm at the new geometry | emits `End`; **no commit**; the form keeps the value; Accept applies it once |
| Esc / cancel | frame and preview restored | frame and preview restored; form stays or closes by its own rule |

### 4.3 The display offset invariant

`drag_to` measures against `frame.origin`, which is frozen at the press. If the triad
were physically moved during the drag, the math would feed back into the gesture.
Therefore:

- `set_display_offset(Some(delta))` moves **only the drawn handles** (a temporary
  transform on the handle items).
- `origin()`, `hit()`, `drag_to()`, the commit and the interaction frame never see the
  offset.
- `origin_anchor`/`readout_anchor` include it, so the HUD and numeric pill stay with the
  visible triad.
- On release in deferred mode the viewport clears the offset and moves the real frame to
  the new pose; after a normal commit the shell re-arms the selection.
- Typed previews (no drag) move the real frame, so hit-testing agrees with the drawing.

### 4.4 Re-arming

The world anchors a gizmo/grip was built for go stale when geometry moves or resizes.
The shell re-arms the current selection against the rebuilt stage after:

- a manipulator/gizmo commit (`after_document_edit`),
- a form callback accepting (`finish_dialog`),
- a command run (including undo/redo), a property edit, or the console,
- and **not** while an active tool owns the pointer (the tool suppresses overlays) or
  while a tree `select` directive is merely being applied.

The selection is pruned first, so a delete leaves no handle for a dead object.

## 5. The form state machine

### 5.1 Presentation

| `presentation` | Layout | Pointer | Keyboard |
| --- | --- | --- | --- |
| `modal` (default) | centred 380 px glass panel over a dimmed backdrop that occludes the canvas | only the panel | panel owns keys; `Enter` accepts, `Esc` cancels |
| `float` | one-row dielectric HUD beside the gizmo, **no backdrop**; only the panel occludes | the canvas stays live (orbit, select, drag) | first field focused and selected; `Tab`/`Shift+Tab` cycle fields; `Enter` accepts; `Esc` cancels |

A floating HUD may be repositioned when the gizmo moves; its anchor is recomputed from
the visible gizmo origin (or the live pointer when no gizmo is armed) each frame it
renders. **Position stability:** the HUD is
clamped inside the viewport safe area (16 px window margins; 64 px above the bottom
dock) and its screen position is **frozen while the pointer is inside the HUD rect or a
field is focused**, so typing a large Δ (`500`) cannot move the inputs out from under the
cursor. It resumes tracking once the pointer leaves and focus is released. The pointer
is read from the window's live position, and the HUD rect from the previous paint, so
entering and leaving are judged against what the user actually saw.

### 5.2 States

```text
Closed ──command returns a dialog spec──► Open
Open ──field edit──► preview (if the spec asks) ──► Open
Open ──deferred gizmo drag──► fields updated + preview ──► Open
Open ──Enter / Accept──► validate → callback → one transaction ──► Closed
Open ──Esc / Cancel──► token cancelled, preview restored ──► Closed
Open ──selection replaced/cleared, selection pruned, competing interaction──►
     silently cancelled ──► Closed
```

- **One dialog at a time.** A second request is rejected while a dialog or a rendered
  form is open; a wizard callback replaces the spec rather than stacking.
- **Non-modal does not mean inert.** A float keeps the pointer live; clicking the
  canvas does **not** dismiss it (only `Esc`, Accept/Cancel, a selection change, a
  competing interaction or a document prune does). This is deliberate: the user may
  orbit to see the preview.
- **Precedence for `Esc`** (root handler, after the focused element had its chance):
  palette → wheel → command dialog → popover → tool → settings card. A floating form
  does not retain keyboard focus once the canvas is clicked, so the root fallback is
  what keeps `Esc` working everywhere.
- **Preview ownership.** `preview: "translation"` composes a temporary transform from
  `dx`/`dy`/`dz`. The form sets the viewport's **deferred** gizmo mode while open;
  closing the form disables it, restores the overlays and clears the ghost.
- **Two-way binding.** Typing → preview (only the parsed fields; unparsable text is
  zero, never an error while typing). Deferred drag → fields, using the values at
  `Begin` as a baseline so successive drags accumulate. Drag ticks are programmatic
  `set_text`, which does not re-emit a change event, so the two directions cannot loop.
- **Commit is single.** A deferred drag never commits; Accept runs the command callback
  (one transaction). A typed preview never commits until Accept.
- **Fast escalation (planned, §10).** Pressing `Tab` or a numeric key (`0–9`, `-`, `.`)
  while a `TransformGizmo` is armed (idle or mid-drag) escalates the session into the
  floating Move form: its `dx/dy/dz` seed from the current drag delta (or zero when
  idle) and the active axis field takes focus. The drag switches to deferred mode, so
the same gesture continues as exact entry instead of committing on release. Until this
lands, the wheel's `Move` is the entry point and `Tab` only cycles an already-open form.

### 5.3 Selection scope

A form is scoped to the selection it was opened for. Replacing the selection (tree or
viewport), clearing it, or pruning a deleted object **closes the form** and spends its
token. A translation preview measured on the old selection must never be applied to a
new one. (The wheel closes for the same reason.)

## 6. Precedence and ownership

### 6.1 Pointer layering (top‑most claims first)

```text
rendered form dialog (backdrop occludes everything)
  └ command dialog: modal backdrop | float panel (own rect only)
      └ palette overlay
          └ settings card
              └ wheel overlay (its square only; own hover)
                  └ workbench manipulators (most recent first)
                      └ gizmo handles
                          └ active tool
                              └ scene (selection, orbit, pick)
```

- Workbench manipulators are **more specific** than the generic triad and claim their
grab targets first (`gizmo-manipulators.md` §14). The triad's centre is the
  origin/pivot handle (`transform-ux.md` §3); while that handle is disabled (the
  translation HUD) the centre is pass-through again.
- The canvas's right-press is gated on **no active tool and no command dialog**.
- A floating HUD's panel is the only part that occludes; the rest of the canvas behaves
  normally.
- Opening a modal (palette, form, settings) closes the wheel.

### 6.2 Keyboard precedence (root `handle_key`)

1. A rendered form dialog owns every key.
2. Palette `Esc`, then the palette shortcut.
3. Wheel `Esc`.
4. Command dialog `Esc` (needed when focus moved to the viewport).
5. `Space` — only with no dialog, palette, settings card or active tool.
6. Popover `Esc`, then the `Esc` cascade (cancel dialog flag → tool → settings).
7. Global shortcut registry.

### 6.3 Single-writer rules

| State | Written by | Read by |
| --- | --- | --- |
| selection | `replace_selection` / `toggle_select` / `clear_selection` | commands, inspector, viewport arming |
| context target | `open_radial` / `space_radial` | `commit_wheel`, `radial_slots` |
| wheel phases | `WheelMenu` | shell (events) |
| deferred flag | form open/close | viewport drag release |
| preview transform | form field edits + deferred drags | stage items, overlays |
| interaction frame | gizmo drag/commit | commit math, hit test |

## 7. Invariants

1. Opening a wheel, hovering a wedge or showing a form changes no document state and no
   selection.
2. A context target outside the selection is promoted **at commit**, never at open.
3. Every pending form/preview has one owner and one cancel path; losing the selection,
   the document object, or precedence cancels it.
4. A preview is never committed by a repaint, a camera move or a re-arm.
5. A command runs only against a selection that contains its target.
6. At most one pending preview exists; the last explicit interaction (typing or drag,
   for the same form) wins.
7. The gizmo's interaction frame is independent of its drawing; moving the drawing can
   never change a drag's math.
8. Overlays are rebuilt from the current geometry after any committed edit.
9. `Esc` reaches the topmost dismissible state regardless of which element last took
   focus.

## 8. The reported defects, resolved

| Symptom | Cause | Rule now |
| --- | --- | --- |
| Right-click auto-selected the object | `open_radial` called `select_under_pointer` before building slots | open resolves a **context target** only; `commit_wheel` promotes on choose |
| Unselecting did not cancel the Move form | the form was only closed by its own Accept/Cancel/Esc | `replace_selection` and `refresh`'s prune path close a translation form (and the wheel) first |

## 9. As built and tested

| Piece | Where | Tests |
| --- | --- | --- |
| pick → selection + anchor | `InventorViewport::selection_event`, `SelectionAnchor` | `a_click_selects_the_named_sub_element_under_the_pointer` |
| context target / promotion | `Shell::{wheel_target_at, radial_slots, commit_wheel}` | `the_radial_gesture_opens_hovers_and_runs`, `space_opens_a_latched_wheel_and_commits` |
| wheel phases/overlay | `ferrocad_widgets::wheel::WheelMenu` | `the_sidecar_latches_flicks_and_reports_events` |
| gizmo anchor + re-arm | `Shell::arm`, `contributions::body_corner`, `rearm_selection` | `the_default_translate_gizmo_sits_at_a_whole_objects_base_corner`, `a_window_position_repicks_through_the_element_origin` |
| display offset | `TransformGizmo::set_display_offset` | `a_display_offset_moves_the_drawing_not_the_drag_frame` |
| preview + deferred drag | `InventorViewport::{preview_translation, set_gizmo_preview}`, `Shell::on_transform_preview` | `a_typed_translation_previews_only_the_named_entities`, `a_deferred_drag_reports_previews_and_never_commits`, `a_gizmo_drag_updates_the_move_fields` |
| floating form | `Shell::{open_dialog_state, dialog_overlay}` | `the_move_dialog_floats_without_a_backdrop`, `the_command_dialog_renders_and_escape_cancels` |
| HUD clamp + freeze | `place_hud`, `clamp_hud`, `dialog_overlay` (`on_children_prepainted`) | `a_floating_hud_is_clamped_into_the_safe_area`, `a_focused_floating_hud_freezes_in_place` |
| selection-scoped form | `Shell::replace_selection`, `Shell::refresh` | `a_selection_change_closes_the_move_hud`, `a_competing_interaction_closes_the_move_hud` |
| grip follows preview | `Manipulator::{anchor, set_anchor}` | `a_typed_preview_moves_the_gizmo_and_the_grips`, `a_grip_anchor_follows_the_object` |

## 10. Open decisions

- **Sub-element gizmos.** The event carries the anchor and the element, but the generic
  gizmo targets a whole object and `GizmoCommitEvent` does not name the element. Face
  push-pull / UV translation needs a target-aware interaction commit (and a workbench
  that accepts it). Until then, a face selection gets the workbench manipulator, not
  the triad.
- **Anchor orientation.** `SelectionAnchor.normal` is carried but the triad stays
  world-aligned; aligning a face gizmo to its normal is available once the
  sub-element commit exists.
- **Rotate in the form.** `TransformGizmo` supports rotate rings and matrix commits, but
  no wheel/form command exposes Rotate yet.
- **Promotion policy for multi-selection.** Choosing a verb for an object outside a
  multi-selection replaces the selection with that object. If a later need arises to
  add instead of replace, it is a `commit_wheel` policy change, not a wheel change.
- **Composing a height edit with a pending move.** Today the competing interaction
  cancels the form (the delta is stale). A queued-edit model would be the alternative;
  it is deliberately out of scope.
