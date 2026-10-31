# Selection, gizmos, the floating form and the wheel — one interaction model

Status: **design decision (2026-10-25).** This note is the authority for how a pointer
gesture becomes a selection, a context menu, a manipulator drag, or an editable
transform. It records the state machines as built, including the fixes for two reported
defects: right-click changing the selection merely by opening the wheel, and a pending
Move form surviving the loss of its selection.

> **Lifecycle replaced (built 2026-10-30); selection filter still planned.** The
> pending-transform lifecycle — the normal versus deferred drag split, the floating Move
> dialog's ownership of the value, and the "silently cancelled on context loss" transition —
> has been **replaced** by one cumulative `TransformSession` with explicit
> commit/checkpoint/rollback: there is one drag mode (a release reports a preview and never
> commits), `Enter` commits one transaction, `Esc` drops the session and its ghost without
> spending an undo entry, and a selection change commits a dirty session first. §4.2 and
> §5.1–5.2 describe the built flow and §7 its invariant; the floating Move HUD remains only
> as the session's numeric surface, and the two-stage pill that replaces it is **planned**.
> The selection half — a topology filter that lets a loadout slot decide whether a click
> selects a body or a sub-element — is **still planned** and changes §2.1. Authority:
> [`../../project/tasks/transform-session.md`](../../project/tasks/transform-session.md)
> (§4.1, §5) and
> [`../../project/tasks/tool-loadout.md`](../../project/tasks/tool-loadout.md) (§2.1).
> Where this note records the retired *deferred* split (§4.2, §5.2), it is marked as
> history rather than the built model.

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
4. **Every pending state has an explicit rollback.** A form, a preview, a wheel and a
   drag can all be abandoned without a partial edit, and `Esc` is that rollback; it never
   spends an undo entry. Context loss is a different rule: a dirty transform session is
   **auto-committed** before the context changes, because losing a drag is data loss, not
   a cancel. Only a delta that cannot be committed (its object was pruned) is discarded.
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
| **Preview** | A temporary, non-document transform composed on the stage items. Never enters the undo stack. | `Shell` (`TransformSession`) → `InventorViewport` (`preview_transform`) |
| **Pending session** | The selection's uncommitted transform: its targets, the primary's world anchor as datum, and the absolute accumulated translation. The shell owns the value; the viewport renders it. `Enter` commits it, `Esc` drops it. | `Shell` (`TransformSession`) |

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
- **Topology filter (U4, built 2026-10-31).** The active loadout slot presets a
  `SelectFilter` (`Auto`/`Body`/`Face`/`Edge`; see
  [`../../project/tasks/tool-loadout.md`](../../project/tasks/tool-loadout.md)). The filter
  is applied where the shell **reports** the selection, not at the pick: `Body` rewrites
  `Face6` to its whole body, while `Auto`/`Face`/`Edge` keep the element. Hover and
  snapping still see the sub-element. The header's `Select ▾` overrides the slot's
  preset; the bottom-left `[1 Move] [2 Push/Pull] [3 Fillet]` bar and `1`/`2`/`3`
  equip the slot.

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
                                     │  press on a handle → Begin (pin the world datum)
                                     ▼
                                  Dragging ──tick──► Update (absolute delta; preview + tape)
                                     │  release → End (preview kept; nothing committed)
                                     ▼
                              Session live ──Enter───────────────► commit one step, re-arm
                                     ├── selection change, Δ ≠ 0 ─► commit, then switch
                                     └── Esc ──────────────────────► drop session + ghost, Idle
```

| Event | Session flow |
| --- | --- |
| press on a handle | `Begin`: the frame is frozen at the press and the shell opens a `TransformSession` from the un-previewed stage, pinning the object's world origin (`ShapeItem::anchor`) as its datum |
| move (tick) | `Update`: the shell accumulates the absolute delta and previews the whole selection; the target is drawn with `session.transform()`, **grips follow**, and the triad's *drawing* is offset so it stays on the surface |
| release | `End`: the preview is **kept**; nothing is committed and no undo entry exists |
| `Enter` | commit the accumulated delta as one transaction through `_move_selection` (the `App_Move` callback); the session and ghost are released and the triad re-arms at the new origin, so the next step starts there |
| selection change while dirty | auto-commit first, then change context |
| `Esc` / cancel | rollback: drop the session and its ghost; no undo entry is spent |

**Changed 2026-10-30 (U1), unified 2026-10-31 (A2).** There is one mode: a **translation**
drag always reports `Begin`/`Update`/`End` and never commits on release. The viewport has no
deferral flag — `set_gizmo_preview`/`gizmo_deferred` are gone — and the drag kind decides:
translation is preview-only (the shell's session owns the value), while rotation, which has no
session yet, keeps the viewport's direct preview and commit. The session already carries the
whole selection; the commit route is still the `App_Move` callback (`_move_selection` is the
writer), and moving it to the interaction provider is **planned**.

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
- On release the preview is **kept**: the viewport clears the draw-only offset and moves
  the real frame to the previewed pose, so hit-testing agrees with the drawing while the
  session stays pending. The frame move is **settled** (`preview_transform_settled`) and
  rebuilds the handle geometry even if the viewport still reports the drag for a tick; a
  draw-offset-only release is what made the triad appear to snap back. The shell re-arms
  the selection after a commit, in `after_document_edit`.
- A preview moves **every hosted kind**, not only the currently offered one. The set keeps
  the other kinds hosted for a later verb, and a stale entry surfaced as a second triad at
  the old origin after a re-narrow (2026-10-31).
- Typed previews (no drag) move the real frame, so hit-testing agrees with the drawing
  **and the handle geometry is rebuilt in the same call** (`TransformGizmo::set_origin`
  only marks the frame dirty; `update` rebuilds it). A frame move without that rebuild
  left the typed object and the HUD anchor at the new pose while the drawn triad stayed
  behind — the 2026-10-31 screenshot bug.

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

**The translation HUD is a view, not an owner (built 2026-10-30).** The pending value for a
move lives in the shell's `TransformSession`, not in the form: the Δ fields mirror
`session.delta()` and an edit writes an absolute value back to the session. The session
exists independently of the form, so a drag with no HUD open previews the same ghost. The
two-stage pill (compact ↔ expanded, no `Cancel`/`Move` buttons) replaced this HUD on
2026-10-31 (U2): a drag shows the compact readout, and `Tab`, a digit or a click unfolds
the three Δ fields.

A floating HUD may be repositioned when the gizmo moves; its anchor is recomputed from
the visible gizmo origin (or the **viewport centre** when no gizmo is armed; the live
pointer is the last resort for a shell with no viewport) each frame it renders. The
fixed offset is **+24 px right and 36 px up to the panel's bottom-left**
(`render::place_hud`), so a low body keeps the panel beside its handles instead of in
the bottom-right clamp and the pill does not cover the arrow it describes. **Position stability:** the HUD is
clamped inside the viewport safe area (16 px window margins; 64 px above the bottom
dock) and its screen position is **frozen while the pointer is inside the HUD rect or a
field is focused**, so typing a large Δ (`500`) cannot move the inputs out from under the
cursor. It resumes tracking once the pointer leaves and focus is released. The pointer
is read from the window's live position, and the HUD rect from the previous paint, so
entering and leaving are judged against what the user actually saw.

### 5.2 States

```text
Closed ──command returns a dialog spec──► Open (expanded)
Open ──field edit──► session Δ updated + preview ──► Open
Open ──gizmo drag──► compact pill; fields mirror the session Δ + preview ──► Open
Open ──Tab / digit / click──► expanded fields ──► Open
Open ──Enter / Accept──► validate → callback → one transaction ──► Closed
Open ──Esc / Cancel──► rollback: token cancelled, session dropped, preview restored ──► Closed
Open ──selection replaced/cleared, Δ ≠ 0──► auto-commit ──► Closed
Open ──selection pruned, competing interaction──► rollback (stale delta) ──► Closed
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
- **Preview ownership is the session's (built 2026-10-30).** `preview: "translation"`
  reads the shell's `TransformSession` rather than a value the form owns; a field edit
  sets the session's absolute delta and the viewport renders `session.transform()`. The
  form no longer toggles a deferred gizmo mode: the viewport's preview mode is enabled
  once at startup. Closing the form restores the overlays and clears the ghost only
  after the session has committed or been dropped.
- **Two-way binding.** Typing → session Δ → preview (only the parsed fields; unparsable
  text is zero, never an error while typing). A drag → session Δ → fields, using the
  values at `Begin` as a baseline so successive drags accumulate. Drag ticks are
  programmatic `set_text`, which does not re-emit a change event, so the two directions
  cannot loop.
- **Commit is single.** A drag never commits; `Enter`/Accept runs the command callback
  (`App_Move` → `_move_selection`, one transaction). A typed preview never commits until
  `Enter`.
- **Context loss auto-commits when dirty (built 2026-10-30; slot/filter 2026-10-31).** Replacing or clearing the
  selection commits a non-zero pending delta as one transaction *before* the selection
  changes; it is no longer "silently cancelled", because losing a drag is the defect the
  session exists to remove. A **loadout slot or `Select ▾` filter change** does the same
  (U4, built): `Shell::set_loadout_slot`/`set_select_filter` call `commit_session` first,
  then re-arm. Starting a tool does not change the hand, so it does not commit. A pruned
  selection or a competing document edit cannot commit a stale delta,
  so it is rolled back. `Esc` is the only user-initiated rollback and spends no undo
  entry.
- **Fast escalation (built 2026-10-31, §10).** Pressing `Tab` or a numeric key (`0–9`, `-`, `.`)
  while a `TransformGizmo` is armed (idle or mid-drag) is the primary entry into the
  numeric surface: the two-stage pill seeds its fields from the current delta (or zero
  when idle) and takes focus. **`1`/`2`/`3` are loadout slots** (U4): without an open pill
  they equip Move / Push-Pull / Fillet, so typing a value that starts with one of them
  begins with `Tab` or a click into a field; with a compact pill open, a digit types into
  it. The wheel's `Move` still opens the same pill for a pointer user; `Tab` cycles its
  fields once it is expanded.

### 5.3 Selection scope

A form is scoped to the selection it was opened for, and a pending session is scoped the
same way. Replacing the selection (tree or viewport) or clearing it **commits a dirty
session first** — while the old selection is still the selection, so the move lands on
the objects the user dragged — and only then closes the form and changes the selection.
A translation preview measured on the old selection is never applied to a new one. A
form still open with no pending delta is cancelled and its token spent; pruning a
deleted object cannot commit a stale delta, so it rolls the session back. (The wheel
closes for the same reason.)

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
- The **hover-to-arm preview** (U5) is the same gizmo layer, hosted only while the Move
  hand is equipped and nothing is selected; it is dimmed (`gizmo_idle_scale` 0.30) and
  becomes the selection's own triad on a handle press. It never adds a layer.
- The canvas's right-press is gated on **no active tool and no command dialog**.
- A floating HUD's panel is the only part that occludes; the rest of the canvas behaves
  normally.
- Opening a modal (palette, form, settings) closes the wheel.

### 6.2 Keyboard precedence (root `handle_key`)

1. A rendered form dialog owns every key.
2. Palette `Esc`, then the palette shortcut.
3. Wheel `Esc`.
4. Command dialog `Esc` (needed when focus moved to the viewport).
5. The Move pill's `Enter`/`Tab`/digit escalation (below the modal checks above).
6. Loadout `1`/`2`/`3` — below the pill's typing path, above `Space` (U4).
7. `Space` — only with no dialog, palette, settings card or active tool.
8. Popover `Esc`, then the `Esc` cascade (cancel dialog flag → tool → settings).
9. Global shortcut registry.

### 6.3 Single-writer rules

| State | Written by | Read by |
| --- | --- | --- |
| selection | `replace_selection` / `toggle_select` / `clear_selection` (the loadout filter is applied here) | commands, inspector, viewport arming, hover-to-arm |
| context target | `open_radial` / `space_radial` | `commit_wheel`, `radial_slots` |
| wheel phases | `WheelMenu` | shell (events) |
| loadout slot/filter | `Shell::set_loadout_slot` / `set_select_filter` (the bar, `1`/`2`/`3`, `Select ▾`) | `Shell::arm`, `replace_selection`, the bar/header |
| hover preview | `Shell::sync_hover_arm` (from the viewport's hover report) | `Shell::arm`, the viewport's idle scale |
| pending transform | `Shell` (`TransformSession`; `commit_session` / `drop_session`) | `preview_transform`, the Δ fields, the commit |
| preview transform | `InventorViewport::preview_transform`, fed by the session | stage items, overlays |
| interaction frame | gizmo drag/commit | commit math, hit test |

## 7. Invariants

1. Opening a wheel, hovering a wedge or showing a form changes no document state and no
   selection.
2. A context target outside the selection is promoted **at commit**, never at open.
3. Every pending transform session has an explicit rollback (`Esc`), an explicit
   checkpoint (`Enter`), and auto-commits a non-zero delta when the user deliberately
   switches selection, slot or tool.
4. Hover-to-arm writes no selection and no session: only a handle press or a body click
   promotes the hovered body, and the preview is dimmed until then.
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
| Unselecting did not cancel the Move form **(history, 2026-10-25)** | the form was only closed by its own Accept/Cancel/Esc | **Superseded 2026-10-30:** a dirty session is *committed* before the selection changes, so the move survives. A form left open with no pending delta is cancelled, and a pruned object rolls the session back. The wheel closes for the same reason. |

## 9. As built and tested

| Piece | Where | Tests |
| --- | --- | --- |
| pick → selection + anchor | `InventorViewport::selection_event`, `SelectionAnchor` | `a_click_selects_the_named_sub_element_under_the_pointer` |
| context target / promotion | `Shell::{wheel_target_at, radial_slots, commit_wheel}` | `the_radial_gesture_opens_hovers_and_runs`, `space_opens_a_latched_wheel_and_commits` |
| wheel phases/overlay | `ferrocad_widgets::wheel::WheelMenu` | `the_sidecar_latches_flicks_and_reports_events` |
| gizmo anchor + re-arm | `Shell::arm`, `contributions::body_corner`, `rearm_selection` | `the_default_translate_gizmo_sits_at_a_whole_objects_base_corner`, `a_window_position_repicks_through_the_element_origin` |
| display offset | `TransformGizmo::set_display_offset` | `a_display_offset_moves_the_drawing_not_the_drag_frame` |
| preview + pending session | `InventorViewport::preview_transform`, `Shell::on_transform_preview` (opens the shell's session) | `a_typed_translation_previews_only_the_named_entities`, `a_move_hud_drag_survives_its_release`, `a_plain_drag_is_pending_until_enter_or_escape`, `a_gizmo_drag_updates_the_move_fields` |
| floating form | `Shell::{open_dialog_state, dialog_overlay}` | `the_move_dialog_floats_without_a_backdrop`, `the_command_dialog_renders_and_escape_cancels` |
| HUD clamp + freeze | `place_hud`, `clamp_hud`, `dialog_overlay` (`on_children_prepainted`) | `a_floating_hud_is_clamped_into_the_safe_area`, `a_focused_floating_hud_freezes_in_place` |
| selection-scoped form | `Shell::replace_selection`, `Shell::refresh` | `a_selection_change_closes_the_move_hud`, `a_competing_interaction_closes_the_move_hud` |
| grip follows preview | `Manipulator::{anchor, set_anchor}` | `a_typed_preview_moves_the_gizmo_and_the_grips`, `a_grip_anchor_follows_the_object` |
| topology filter (U4, 2026-10-31) | `shell/loadout.rs`, `Shell::replace_selection` | `a_face_click_selects_the_body_in_the_move_slot`, `the_slot_sets_the_filter_and_select_overrides_it`, `a_scripted_selection_reaches_the_shell` (with `Select: Auto`) |
| loadout host + chrome (U4, 2026-10-31) | `Shell::arm`, `chrome::{loadout_bar, select_button}` | `the_loadout_bar_renders_and_equips_a_slot`, `the_wheel_verbs_equip_the_move_slot`, `switching_the_slot_commits_a_pending_drag_first`, `a_face_click_in_the_move_slot_arms_only_the_triad` |

## 10. Open decisions

- **Sub-element gizmos.** The event carries the anchor and the element, but the generic
  gizmo targets a whole object and `GizmoCommitEvent` does not name the element. Face
  push-pull / UV translation needs a target-aware interaction commit (and a workbench
  that accepts it). Until then, the **loadout** decides (U4): Slot 1's `Body` filter
  reports the whole body and arms the triad; Slot 2 keeps the face and arms the
  workbench manipulator. A face-scoped *transform* (the triad acting on `Face6`) is
  still not built.
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
