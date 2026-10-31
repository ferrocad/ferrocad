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
> as the session's numeric surface — since 2026-10-31 a single shape (three Δ fields plus
> `✓`/`✗`), with the drag's numbers drawn on the 3-D tape's legs. The selection half — a
> topology filter that lets a loadout slot decide whether a click selects a body or a
> sub-element — is **built** too (U4). Authority:
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
   spends an undo entry. A pending **move** is a decision the user has not made yet: while
   its `✗`/`✓` pair is on screen, the context is **locked** — selecting another object,
   switching the hand or the handle set, changing `Select ▾`, or starting a tool is
   refused, so the value is neither committed nor dropped by a stray click. Finish it
   with `✗`/`✓` (or `Esc`/`Enter`). Only a delta that cannot survive (its object was
   pruned by a document edit) is discarded.
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
                                     ├── object / hand / scope / tool ─► refused while Δ ≠ 0
                                     └── Esc ──────────────────────► drop session + ghost, Idle
```

| Event | Session flow |
| --- | --- |
| press on a handle | `Begin`: the frame is frozen at the press and the shell opens a `TransformSession` from the un-previewed stage, pinning the object's world origin (`ShapeItem::anchor`) as its datum (a ring also pins `orientation_0` from the document) |
| move (tick) | `Update`: the shell accumulates the absolute delta (or replaces the ring's one pending `(axis, angle)` step) and previews the whole selection; the target is drawn with `session.transform()`, **grips follow**, and a translation's triad *drawing* is offset so it stays on the surface |
| release | `End`: the preview is **kept**; nothing is committed and no undo entry exists |
| `Enter` | commit as one transaction: a translation through `_move_selection` (the `App_Move` callback), a rotation through the `transform`/`apply` interaction hook; the session and ghost are released and the triad re-arms |
| selection change while dirty | refused: finish with `✓`/`✗` first (`block_pending_move`) |
| `Esc` / cancel | rollback: drop the session and its ghost; no undo entry is spent |

**Changed 2026-10-30 (U1), unified 2026-10-31 (A2), single-dirty-ring lock 2026-10-31.**
There is one mode: both a **translation** and a **rotation** drag report
`Begin`/`Update`/`End` and never commit on release. The viewport has no deferral flag —
`set_gizmo_preview`/`gizmo_deferred` are gone. A ring session stores **one
`(axis, angle)` step**, not a composed quaternion or three Euler fields: `SO(3)` is not a
vector space, so three independent angles would have to multiply in an order and would
decompose into fractional cross-talk. While a step is pending its ring is the only one
that answers: the siblings and the re-pivot disc are muted, their presses are consumed
(the viewport queues `LockedRing` instead of starting a drag or orbiting), and the bar
shows them as `—`. The only exits are `✓`/`Enter` and `✗`/`Esc`, so `dirty_ring_count <=
1` and no ring switch spends a hidden transaction. The ring session owns exactly one body
(the interaction hook applies a transform to one object) and commits through
`transform`/`apply`; translation keeps the multi-body selection and the `App_Move` route.
The rotate hand docks `ΔθX/ΔθY/ΔθZ` above the loadout: only the pending ring's cell holds
an input, and the frame switch (`WLD` world grid, `LOC` body, `WP` workplane) orients
both the arrows and the rings. `Reset 0°` commits the rotation that cancels the stored
orientation. The 3-D protractor
(swept glass sector, dashed press spoke, live spoke, detent ticks) is built from the same
single step, so it is always one arc and one chip with the degree tag on the arc bisector.
Moving the translation commit to the interaction provider is **planned**.

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

The rotate form's **ring lock** is enforced at the viewport's press gate, not by a re-arm:
a pending step keeps its ring's input and mutes the siblings, so a press on one is consumed
and reported (`ViewportEvent::LockedRing`) without a drag, orbit or deselection. The
ordinary `✓`/`Enter` and `✗`/`Esc` paths still clear and re-arm.

## 5. The form state machine

### 5.1 Presentation

| `presentation` | Layout | Pointer | Keyboard |
| --- | --- | --- | --- |
| `modal` (default) | centred 380 px glass panel over a dimmed backdrop that occludes the canvas | only the panel | panel owns keys; `Enter` accepts, `Esc` cancels |
| `float` | one-row dielectric HUD beside the gizmo, **no backdrop**; only the panel occludes | the canvas stays live (orbit, select, drag) | **unfocused by default** (the viewport keeps the keys); `Tab`/`Shift+Tab` cycle fields, a digit or `-`/`.` opens one, a click in a field focuses it; `Enter` accepts; `Esc` cancels |

**The translation HUD is a view, not an owner (built 2026-10-30).** The pending value for a
move lives in the shell's `TransformSession`, not in the form: the Δ fields mirror
`session.delta()` and an edit writes an absolute value back to the session. The session
exists independently of the form, so a drag with no HUD open previews the same ghost.

The Move surface has **two pieces** since 2026-10-31: a coordinate row docked at the
bottom-left with the loadout (`[Δ|XYZ]  ΔX [ ]  ΔY [ ]  ΔZ [ ] mm`, `shell::move_fields_bar`)
and a small in-context `✗`/`✓` pill that floats beside the gizmo
(`shell::move_finish_overlay`). Both appear whenever Slot 1 has a Move target. They are
**shell-owned surfaces**, not a command dialog (`shell/move_hud.rs`): they survive a
commit, never hold a command token, and can coexist with a command modal. The finish pair
is the shared `ferrocad_widgets::decision_pill`, so it is the same flat `✗ │ ✓` strip a
modal dialog's footer renders — one widget, not two lookalikes. The old
two-stage compact ↔ expanded pill, and the second (single-axis) readout beside it, are
gone; the numbers a drag produces are drawn on the session's 3-D measuring tape as
**ruler tags** (`gizmo-manipulators.md` §5), so the viewport no longer floats a duplicate
pill at the arrow tip. Docking the fields keeps the canvas around the gizmo clear for
the finish pair.

**At rest the row is an inspector; the finish pair is the pending move's control.** With
`Δ = (0, 0, 0)` the fields show the live coordinate readout and the `✗`/`✓` pill is not
rendered at all: there is nothing to spend, and a button that closes nothing reads as
broken. As soon as a drag or a typed value makes the session dirty, the 3-D dashed ruler
appears and the pair materialises beside the gizmo. `✓`/`Enter` commits one undo step and
`✗`/`Esc` returns to the ghost origin; either dissolves the ruler and the pill, leaving
the row at rest.

**Always visible while Move has a target.** The row appears with the selection (or a hover
promotion) and disappears when the selection clears, another hand or tool takes over, or
the Move hand is **idle** (pressing `1` twice while Slot 1 is active cycles Move →
Rotate → idle — the selection stays for inspection — and `1` again, a new selection or
a `Move` verb re-arms Move). A modal or
the object wheel hides both pieces for its lifetime; they return with the canvas. A
right-press on the finish pill opens the object wheel, so it never swallows the verb
gesture.

**Two coordinate modes.** The leading badge (or `=`) flips the fields between `ΔX/ΔY/ΔZ`
(offsets) and `X/Y/Z` (world coordinates): the display is `origin_0 + delta`, where
`origin_0` is the gizmo origin frozen while the session is dirty, and a typed coordinate
is parsed back as `value - origin_0`. Both modes preview, tape and commit the same delta.
A per-field `=`/`@` prefix (`=15`) asks for an absolute value without leaving offset mode.

**Unfocused is the default.** Opening the row or releasing a drag leaves every field
unfocused, so `1`/`2`/`3` still equip loadout slots and the viewport keeps its own keys.
`Tab` opens the field of the last gesture (drag-axis memory) and selects its value, then
cycles; a digit other than `1`–`3`, or `-`/`.`, focuses that field and starts the number;
a click in a field or on a ruler tag does the same. `Enter`/`✓` commits and `Esc`/`✗` rolls
back; both reset the fields and hand the keyboard back to the canvas, and the row stays.
Only the **focused** field paints a selection highlight: a blurred `TextInput` clears its
selection range, so a `Tab` from `ΔY` to `ΔZ` never leaves a purple range behind.

**The finish pill sits in the empty screen octant around the triad.** It is the one
floating piece left. A fixed offset runs into whatever the camera puts there (the Z
arrowhead and the YZ plane quad at a common angle), so `render::place_finish` sums the
**screen directions the triad occupies** — its three projected stems
(`InventorViewport::gizmo_stem_directions`) and the incoming tape leg from the ghost
origin (`world_anchor` of the session datum, weighted 0.75) — negates the sum and puts
the pill's **centre 36 px** out along it. With every stem up/right and the tape from
up-left the pair lands down-left, in open canvas. `clamp_hud` keeps it reachable at an
edge. While the pointer owns a stroke the pill is **not rendered at all**, so a drag can
never end on top of it; it comes back on the first paint after the release, and its
position is frozen while the pointer is inside it.

The pill is the **same flat segmented strip** as `PERSP ┆ ORTHO` and `Create Box ┆
Search` (`render::hud_buttons`): one `WHEEL_GLASS` body, a `DOCK_BORDER` hairline, a
`CHROME_LINE` divider, `overflow_hidden`, and no nested button boxes. `✓` rests on
`CHROME_ACTIVE_BG` with `CHROME_ACTIVE` ink — the accent the projection segment and the
active loadout slot use — and hovers to `HIGHLIGHT_TINT` / `HOVER_RIM`; `✗` is `MUTED`
until hover, when it takes `SUBTRACT_COLOR` ink on a 14 % subtract wash. State is ink,
backgrounds are additive only (`visual-language.md` principles 2 and 5). It is clamped inside the viewport safe area (16 px margins; 64 px
above the bottom dock) and its position is **frozen while the pointer is inside its
rect**, so a click cannot slide the buttons out from under the cursor. The docked
coordinate row does not move at all: it is laid out above the loadout, so typing a large
Δ (`500`) can never move the inputs. The pointer is read from the window's live position
and the pill rect from the previous paint, so entering and leaving are judged against
what the user actually saw.

### 5.2 States

```text
Closed ──Move hand + a target──► At rest (fields, no ✗/✓, no ruler)
At rest ──drag / typed value (Δ ≠ 0)──► Pending (ruler + the in-context ✗/✓ pill)
Pending ──Enter / ✓──► one transaction, fields reset ──► At rest (canvas focused)
Pending ──Esc / ✗──► rollback: session dropped, preview restored ──► At rest
At rest ──gizmo drag──► fields mirror the session Δ + preview; no focus taken
At rest ──Tab / digit / tag click──► one field focused (drag-axis memory)
Any ──badge / `=`──► Δ fields ⇄ world coordinates (same pending Δ)
Any ──selection replaced──► refused while pending; a clean form re-targets
Pending ──selection / hand / scope / tool──► refused (status: "move/rotation pending") ──► Pending
At rest ──Esc──► selection cleared ──► Closed
At rest ──selection cleared / another hand / a tool / `1` (idle)──► Closed
Pending ──competing document edit (selection pruned)──► rollback ──► the new context
```

- **One dialog at a time — for dialogs.** A second *command* dialog is rejected while a
  dialog or a rendered form is open; the Move form is not a dialog, so a command can open
  its own modal over it (the form hides for the modal's lifetime). A wizard callback
  replaces the spec rather than stacking.
- **Non-modal does not mean inert.** A float keeps the pointer live; clicking the
  canvas does **not** dismiss it. This is deliberate: the user may orbit to see the
  preview.
- **Precedence for `Esc`** (root handler, after the focused element had its chance):
  palette → wheel → command dialog → popover → tool → settings card → a **pending** Move
  session (rollback, the form stays) → the session pivot → **at rest, clear the
  selection** (the form, triad and ruler go with it). A command dialog consumes its own
  `Esc`: the panel stops propagation, so cancelling a modal never also clears a selection
  behind it.
- **Preview ownership is the session's (built 2026-10-30).** A field edit sets the
  session's absolute delta and the viewport renders `session.transform()`. The form no
  longer toggles a deferred gizmo mode: the viewport's preview mode is enabled once at
  startup. Rollback restores the overlays and clears the ghost; the form itself stays.
- **Two-way binding.** Typing → session Δ → preview (only the fields that parse; an
  unparsable field keeps the last preview and is an error only on commit). A drag →
  session Δ → fields, using the values at `Begin` as a baseline so successive drags
  accumulate. Drag ticks are programmatic `set_text`, which does not re-emit a change
  event, so the two directions cannot loop.
- **Commit is single.** A drag never commits; `Enter`/Accept runs the command callback
  (`App_Move` → `_move_selection`, one transaction). A typed preview never commits until
  `Enter`.
- **One context gate (built 2026-10-31).** The shell models what it is doing explicitly
  (`shell/context.rs`): `InteractionContext` is `Modal → Tool → (PendingMove |
  PendingRotation) { target } → Armed → Selection → Idle`, and every entry point asks
  `Shell::gate(action)` before it acts. The pending variants carry the **dirty object**
  (the session's primary stage entity) and split on the session's mode, so the refusal
  names the right verb; `Action::Select { target }` carries the body a pick would select,
  so the rule can be stated against the subject rather than "some selection". `Action` also
  names `SwitchHand`, `SwitchScope`, `SwitchHandleSet`, `StartTool`,
  `RunCommand { id, edit }` and `Arm`, so a new gesture cannot forget to check the state
  of the one it competes with. Completion (`✓`/`✗`, `Enter`/`Esc`) and field editing are
  not actions: they are how a context is left.
- **A pick is refused before it re-targets.** `Shell::on_selection` gates first, so the
  viewport's pick cannot re-arm the triad on the clicked body (the defect behind "the
  gizmo jumped to the other box while the move was pending"). Re-applying the **same**
  selection is a no-op early return in `replace_selection`, so a re-pick cannot reach the
  session reset and silently drop the pending value either.
- **What a pending transform refuses.** Selecting or clearing another object, pressing a
  loadout key (pausing Move included), changing `Select ▾`, switching the handle set,
  starting a tool, dragging another body, and **edit commands** (`CmdType: ForEdit`,
  e.g. `Create Box`). Each refusal writes the reason to the status line
  (`move pending — …` or `rotation pending — …`), the loadout bar dims and drops
  its click handlers, and nothing is committed or dropped. **View commands**
  (`CmdType: ForView`: grid, projection, …) stay live, so navigation and presentation
  still work under the lock. A command the shell has not seen in its registry snapshot is
  treated as an edit — without the declaration it cannot be proved harmless.
- **Nothing is silently hijacked.** A transform preview whose target is not the session's
  subject is refused and the viewport cancels the stroke
  (`InventorViewport::cancel_gizmo_drag`), so dragging another box cannot move the
  pending selection under a triad that sits elsewhere. A **document** event is still
  different: a pruned selection or a competing edit cannot keep a stale delta, so it is
  rolled back. `Esc` is the only user-initiated rollback and spends no undo entry.
- **Fast escalation (built 2026-10-31, §10).** Pressing `Tab` or a numeric key (`0–9`, `-`, `.`)
  while a `TransformGizmo` is armed (idle or mid-drag) is the primary entry into the
  numeric surface: the HUD seeds its fields from the current delta (or zero when idle)
  and focuses the field the gesture names. **`1`/`2`/`3` are loadout slots** (U4) and are
  reserved: they equip Move / Push-Pull / Fillet even while the HUD is on screen, so a
  value that starts with one of them begins with `Tab` or a click into a field. Pressing
  `1` while Move is already active **pauses** it (selection kept, handles and form gone);
  pressing it again arms the hand. Once a field is focused its digits belong to it. A
  drag on the resting HUD updates the fields and the tape but never takes the keyboard.

### 5.3 Selection scope

A form is scoped to the selection it was opened for, and a pending session is scoped the
same way. Replacing the selection (tree or viewport) or clearing it **commits a dirty
session first** — while the old selection is still the selection, so the move lands on
the objects the user dragged — and only then re-targets the form: a replacement keeps it
open with a fresh session and origin, a clear closes it. A translation preview measured
on the old selection is never applied to a new one. A form still open with no pending
delta is simply re-targeted; pruning a deleted object rolls the session back. (The wheel
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
5. The Move HUD while **no field is focused**: `Enter` accepts, `Tab` opens a field on
   the remembered drag axis, and a numeric key other than `1`–`3` focuses the field and
   starts the number. A focused field consumes its own digits, `Enter` and `Tab` first.
6. Loadout `1`/`2`/`3` — they are reserved even while the resting HUD is on screen, so
   they equip a hand instead of typing (U4).
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
   checkpoint (`Enter`), and **locks** the context: selecting another object, switching
   selection, slot, handle set or tool is refused while the delta is non-zero.
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
| HUD clamp + freeze | `place_hud`, `clamp_hud`, `dialog_overlay`, `move_finish_overlay` | `a_floating_hud_is_clamped_into_the_safe_area`, `the_finish_pill_tracks_the_anchor_and_the_form_stays_docked` |
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
