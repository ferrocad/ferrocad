# The transform session: explicit uncommitted state, loadout slots and one move lifecycle

Status: **design synthesis and plan (2026-10-30).** *Plan only — nothing below A0 is built
yet.* Reconciles the split-brain lifecycle in
[`selection-gizmos-and-forms.md`](../../developer/interaction/selection-gizmos-and-forms.md),
[`gizmo-manipulators.md`](../../developer/interaction/gizmo-manipulators.md),
[`transform-ux.md`](../../developer/interaction/transform-ux.md) and
[`visual-language.md`](../../developer/interaction/visual-language.md). The **model** this
document introduces (§5) is the base the rest depends on: an uncommitted edit is a
first-class object, not a display hack and not something undo is asked to reverse.

Staging is in §7. Every stage names the spec sections it rewrites in §6; those sections stay
authoritative until their stage lands, and this page records the edit when it does.

---

## 0. The problem

### 0.1 The split-brain, as it stands

The transform triad (`TransformGizmo`), the workbench manipulators (`PushPullStem`,
`DressUpScrubber`) and the floating Move HUD (ΔX / ΔY / ΔZ) are three partially overlapping
state machines over the same gesture:

- **Two invisible modes for one triad.** Selecting an object arms the triad in *normal* mode
  (a release commits immediately, with no form for exact entry). Choosing `Move` arms the same
  triad in *deferred* mode (`InventorViewport::set_gizmo_preview`; a release only emits
  `TransformPreview` and waits for the HUD's accept). The mode is a boolean on the viewport,
  mirrored by `Shell::gizmo_deferred`, so neither side can be sure who owns the drag.
- **Silent data loss on context exit.** In deferred mode, selecting another object, clearing
  the selection, or touching a face stem silently cancels the preview
  (`replace_selection` → `clear_dialog_preview`).
- **One-shot dismissal.** Accepting the Move HUD closes it, so typing a second precise step
  means re-opening the wheel.
- **Manipulator collision.** OCCT tags every click with a sub-element (`FaceN`), and
  `translate_gizmo` claimed any selection, so one click could arm both the face's push/pull
  stem and the body's triad; which one took the pointer came down to registry priority.

### 0.2 The defect that motivated this document

On 2026-10-30 a user reported, in three separate rounds, that dragging the triad "resets to
the original location" and that ending a Move makes the triad disappear. The causes were
symptomatic of the split brain, not of the maths:

1. The drag delta was **round-tripped through the HUD's text fields** (`{:.2}`) and re-read
   from them, so a release previewed the rounded delta — zero for any drag under 0.005 mm —
   and the object and triad snapped back.
2. Accepting the HUD set `triad_dismissed`, deliberately removing the handles from the
   object the user had just placed.
3. The first harness "proved" the triad followed the drag by **injecting hand-made
   `TransformPreview` events**, so no test could see either defect.

Each is now fixed locally (commit `1559fa0`), but the structure that produced them is intact:
the pending move lives in **three** places — the dialog's field text, `OpenDialog::translation`,
and the viewport's `preview_home` overlay snapshot — and the only durable record of a
completed gesture is an undo entry.

### 0.3 What we will not do

- **No undo-as-cancel.** A preview must never open a transaction and revert it. Undo is the
  user's history; a gesture the user has not confirmed must not appear in it.
- **No document mutation for previews.** Today the document is already untouched during a
  drag, but only because the preview lives in the *display* stage
  (`ShapeItem::temporary_transform`). That is a render-side accident, not a model: nothing
  stops the next feature from writing through and undoing.
- **No second state machine.** One session object, owned once, read by every view.

---

## 1. Core metaphor: the FPS loadout

Rather than inferring which manipulator to show from an unfiltered triangle pick — or hiding
precision behind a modal Move command — the user carries an explicit loadout of active
manipulator slots, backed by the radial wheel as the "buy menu".

### 1.1 The loadout bar and the topology filter

The active slot governs both what a viewport hover/click targets (the topology filter) and
which single manipulator class is hosted (`GizmoSet::set_session`):

| Slot | Hotkey | Default `Select ▾` filter | Active manipulator (`set_session`) | Numeric surface |
| --- | --- | --- | --- | --- |
| Slot 1 | `1` | Body (element: `None`) | `TransformGizmo` (Translate / Rotate) | 3-D measuring tape + expandable ΔX/ΔY/ΔZ pill |
| Slot 2 | `2` | Face (`FaceN`) | `PushPullStem` (extrude / pocket) | in-situ readout pill (`Tab` to type) |
| Slot 3 | `3` | Edge (`EdgeN` chain) | `DressUpScrubber` (Fillet / Chamfer) | in-situ readout pill (`Tab` to type) |
| Wheel | Right-click / Space | context-dependent | "Buy menu": equips specialised verbs (Rotate, Chamfer, Draft) into the active hand | same unified session contract |

Deliberate swapping over implicit collision: only one manipulator session is active at a
time. With Slot 1 equipped, `PushPullStem` and `DressUpScrubber` are **unoffered** even if the
pointer passes over `Face1`; with Slot 2 equipped, the corner triad is hidden.

- **Top-left `Select ▾` pill** sits in the viewport header between `Gizmo ▾` and `Snap ▾`
  (`Grid ▾ | Axes ▾ | WP: XY ▾ | Gizmo ▾ | Select: Body ▾ | Snap ▾`). Switching slots updates
  it; the user can still override it (e.g. `Face` while in Move, once sub-element transforms
  land).
- **Bottom-left loadout pill group** is anchored at `left: 16 px`, on the same baseline as the
  centred creation dock (`Create Box | Search (Ctrl+K)`), using `ui::DOCK_BG` (`#16181D` @
  0.90) and `ui::DOCK_BORDER` (@ 0.12), with the active slot in `ui::CHROME_ACTIVE` (#67D7FF)
  over `ui::CHROME_ACTIVE_BG` (@ 0.33).

## 2. Hover-to-arm and zero-click drag

Continuing [`visual-language.md`](../../developer/interaction/visual-language.md) §0.3 ("a
control materialises from pointer proximity or a drag and dissolves when idle"):

```text
Slot 1 (Move) equipped
  │
  ├─► pointer hovers a movable body (`HoverReport::target = Some(body)`)
  │     └─► the triad materialises at the body's origin in IDLE (30 % opacity)
  │           ├─► pointer within 12 px of a handle ──► HOVER bloom (#48E5FF, 1.5× width)
  │           │     └─► press & drag ──► promote the body to the selection and begin the
  │           │                          session in one motion
  │           └─► click the body interior ──► select; the triad latches in IDLE/HOVER
  │
  └─► pointer leaves (nothing selected) ──► the triad dissolves
```

- **Zero-click initiation:** with Slot 1 equipped and no selection, hovering a body previews
  the triad at its anchor; grabbing a handle selects the body and starts the drag without a
  preliminary click.
- **Selected persistence:** clicking (or selecting from the tree/keyboard) latches the
  selection, so the triad stays when the pointer leaves and keyboard-only entry (`Tab`) works.

## 3. The unified transform session

There is no "normal drag" versus "deferred drag". Every translation or rotation gesture runs
inside one cumulative **Transform Session** that collapses consecutive handle adjustments and
typed overrides into a single undo transaction.

### 3.1 Lifecycle

```mermaid
stateDiagram-v2
    [*] --> ArmedIdle: body hovered or selected (slot 1)
    ArmedIdle --> SessionActive: begin handle drag, or Tab / digit

    state SessionActive {
        [*] --> Accumulating
        Accumulating --> Accumulating: drag an axis (adds to Δ; tape + preview update)
        Accumulating --> Accumulating: type in the pill (updates Δ; tape + preview update)
    }

    SessionActive --> ArmedIdle: Enter (checkpoint: commit one step, re-arm at the new origin, Δ = 0)
    SessionActive --> NextContext: another object / slot change / wheel verb (auto-commit if Δ ≠ 0, then switch)
    SessionActive --> ArmedIdle: Esc (rollback: restore the session-start pose, clear the preview, Δ = 0)
```

### 3.2 The four session invariants

1. **Cumulative deltas.** When a session begins, the shell pins the selection's starting
   placement as the *session datum*. Drag X +15 mm, release, drag Y +20 mm, release, then type
   `16.5` into ΔX: all four adjust the same live preview relative to that datum and collapse
   into one transaction.
2. **Implicit auto-commit on context exit.** If the accumulated delta is non-zero, switching
   slot, choosing a wheel verb, selecting another object or clicking empty space commits the
   preview as one transaction *before* changing context. The user cannot lose a drag by
   forgetting to press Move.
3. **Checkpoint on Enter.** Enter commits the current delta, moves the session datum to the new
   placement, resets Δ to 0 and keeps the object selected — ready for the next step.
4. **Explicit rollback on Esc.** Esc is the only abort: mid-drag it cancels the stroke; with a
   non-zero delta it discards the preview and returns the geometry to the session datum; with
   Δ = 0 it clears the selection. **Rollback never touches the undo stack.**

## 4. In-place measuring tape, not a floating form

To honour `visual-language.md` §0.1 ("geometry is the interface; chrome recedes") the Move HUD's
button bar (`Cancel` / `Move`) is replaced by a 3-D measuring tape in the stage and a two-stage
pill in the gpui overlay.

### 4.1 Stage: the 3-D measuring tape

While a session has a non-zero offset from its datum `O_start` to the previewed origin
`O_current`:

- **Ghost origin disc** — a 10 px ring at `O_start` in `gizmo::INACTIVE_COLOR` (#94A3B8 @ 0.50),
  marking where the body started.
- **Orthogonal axis legs (stair-step tape)** — dashed 1 px lines (the `axis-origin-grid.md`
  dashed-ray technique) tracing the non-zero components along the frame's basis:
  `O_start ──(ΔX, #F87171)──► P_x ──(ΔY, #4ADE80)──► P_xy ──(ΔZ, #38BDF8)──► O_current`,
  with perpendicular 6 px end-ticks at each corner. Depth-off, never picked, rebuilt from the
  session each frame (the same contract as `workplane_stem`).

### 4.2 gpui: the expandable pill

One pill anchored to the active measurement (`readout_anchor` during a drag, clamped near
`O_current` when expanded) replaces both the separate Move dialog spec and the separate
`GizmoValue` readout.

- **Compact** (during a drag, and lingering after release) shows only the constrained value in
  12 px semibold tabular monospace on `ui::WHEEL_GLASS` with a `ui::CHROME_LINE` border:
  `[ ΔX +24.50 mm ]`, or `[ ΔX +24.50  ΔY +10.00 mm ]` for a planar accumulation.
  Clicking the pill or pressing `Tab` expands it.
- **Expanded** (via `Tab` or a digit `0–9`, `-`, `.`) unfolds in place:
  `[ ΔX ( 24.50 )  ΔY ( 10.00 )  ΔZ ( 0.00 ) ]`. There are no `Move`/`Cancel` buttons:
  `Enter` commits (checkpoint), `Tab`/`Shift+Tab` cycle the fields, `Esc` rolls back.
- **Position freeze** reuses the existing `hud_lock` / freeze-on-focus invariant
  (`selection-gizmos-and-forms.md` §5.1), so typing `500` never slides the input out from
  under the pointer.

### 4.3 Keyboard-only workflow

Select an object (click, tree or shortcut) with Slot 1 active → press `Tab` (or start typing
`25`) → the pill expands to `[ ΔX ( 25 )  ΔY ( 0.0 )  ΔZ ( 0.0 ) ]` at the object's origin and
the tape previews the +25 mm X offset live → `Tab` to ΔY, type `10`, `Enter` to commit both in
one undo step.

---

## 5. The model: uncommitted state is a first-class object

This is the base work, and it is deliberately not a UX change. Everything above is a *view* of
one thing: a pending edit.

### 5.1 Why the current shape cannot hold a ghost

The preview today is `ShapeItem::temporary_transform` — a matrix on the **display** item,
composed by the viewport, forgotten at commit, and restored by an overlay snapshot
(`preview_home`) that exists only for transforms. Three consequences:

- There is no *base* value to draw against, so a "ghost" of the uncommitted state cannot be
  expressed except by re-deriving it from the same matrix that draws the live one.
- The pending value lives in as many places as there are ways to edit it (field text, session
  delta, overlay snapshot), which is exactly how the 2026-10-30 defect happened.
- Only transforms can preview. A fillet radius, an extrusion length or a sketch point has no
  preview channel at all — `DressUpScrubber` and `PushPullStem` commit to see anything.

### 5.2 `PendingEdit` (new, `ferrocad_core::document::pending`)

```rust
pub struct PendingEdit {
    id: PendingId,
    label: String,                            // becomes the committed transaction's name
    base: BTreeMap<(ObjectId, String), Option<Property>>,   // committed values, captured once
    staged: BTreeMap<(ObjectId, String), Property>,         // what the user is changing
}

impl Document {
    pub fn begin_pending(&mut self, label: &str) -> PendingId;
    pub fn stage_property(&mut self, id: PendingId, object: ObjectId, name: &str, value: Property) -> Result<(), String>;
    pub fn unstage(&mut self, id: PendingId, object: ObjectId, name: &str);
    pub fn pending(&self, id: PendingId) -> Option<&PendingEdit>;
    pub fn commit_pending(&mut self, id: PendingId) -> bool;  // one named transaction + one recompute
    pub fn abort_pending(&mut self, id: PendingId) -> bool;   // drop it; no history entry, no recompute
}
```

Properties: **`stage_property` never writes the document**, so there is nothing to undo;
`abort_pending` is a `drop`, not a revert; `commit_pending` applies the staged values through
the ordinary `set_property` path inside one named transaction, so the undo entry is created
exactly once, with the session's label, at the moment the user confirms. `base` exists both
for rollback and for the ghost (it is the value the tape measures *from*).

One pending edit is active per document. A second `begin_pending` while one is open commits
the first if dirty (invariant 2) — the same rule the UI applies, enforced in the model so no
caller can invent a third state.

### 5.3 `EditView` — one read path for "what the user sees"

Reads must not have to remember whether a preview is live. Add a borrowed view:

```rust
pub struct EditView<'a> { /* doc + optional pending */ }
impl<'a> EditView<'a> {
    pub fn property(&self, object: ObjectId, name: &str) -> Option<Cow<'a, Property>>; // staged wins
    pub fn committed(&self, object: ObjectId, name: &str) -> Option<&'a Property>;     // the ghost's base
}
impl Document { pub fn view(&self) -> EditView<'_>; pub fn view_with(&self, id: PendingId) -> EditView<'_>; }
```

The display/stage pass reads `view()`; snapping and hit testing read `view()` (so you snap to
what you see); exporters and save read the document. This is what stops the next preview from
re-introducing a second source of truth.

### 5.4 `TransformSession` (new, `ferrocad_stage::session`)

The transform case is the first consumer, and it must live below gpui so the toy and
parametric editions and the headless tests all share it:

```rust
pub struct TransformSession {
    target: u32,              // stage entity
    base: Placement,          // the committed placement at begin (the ghost datum)
    delta: Vector3,           // accumulated translation
    mode: GizmoMode,          // translate | rotate (rotation lands in a later slice)
    dirty: bool,
}
impl TransformSession {
    pub fn begin(stage: &Stage, target: u32) -> Option<Self>;
    pub fn set_translation(&mut self, delta: Vector3);
    pub fn translate_axis(&mut self, axis: Axis, value: f64);   // absolute along the axis
    pub fn delta(&self) -> Vector3;
    pub fn base(&self) -> Placement;
    pub fn effective(&self) -> Placement;                       // base ∘ delta
    pub fn checkpoint(&mut self, placement: Placement);         // Enter: new datum, Δ = 0
    pub fn is_dirty(&self) -> bool;
}
```

One owner: the shell holds it; the viewport **renders** it (`stage.item(target)` display
transform + the tape), it does not own it. That deletes the three-way split:
`preview_translation`, `apply_gizmo_preview` and the dialog's mirrored delta all become
*reads* of the session, and `set_gizmo_preview(deferred)` disappears because there is only one
mode.

The session is also the `PendingEdit`'s writer: a translate stages `Placement` on the target
(`pending.stage_property(target, "Placement", base ∘ delta)`). So the same structure feeds the
display and, at commit, the transaction.

### 5.5 Commit, checkpoint, rollback — mapped to the model

| User action | Session | PendingEdit | History |
| --- | --- | --- | --- |
| drag / type | `set_translation` | `stage_property` (display only) | **no entry** |
| `Esc` | dropped | `abort_pending` (a `drop`) | **no entry** |
| `Enter` | `checkpoint` (new datum, Δ = 0) | commit → new pending at the new base | **exactly one** named entry |
| selection / slot / tool change while dirty | committed first | commit | one entry, then the change |
| `Ctrl+Z` after any of the above | — | — | reverts the *committed* step; a live preview is untouched because it was never in history |

### 5.6 What the renderer needs

- `ShapeItem` keeps `temporary_transform` as the *rendering* of a staged transform (no public
  API changes), but it is now written in exactly one place, from `session.effective()`.
- A new overlay, `ferrocad_stage::measure_tape`, builds the ghost disc and the stair-step legs
  from `(session.base(), session.effective())`. It is a `RenderQueue` like
  `workplane_stem`: depth off, never picked, rebuilt when the session changes.
- The tape needs no new renderer feature. Every element is a 1 px line or a screen-facing
  ribbon, both of which exist.

### 5.7 Compatibility

- **Python.** `App_Move` keeps its argument form (`Activated(dx, dy, dz)`); only its
  no-argument form stops returning a dialog (§7, U1). `FreeCADGui.Preview` keeps its channel.
  `Document` gains `begin_pending`/`commit_pending`/`abort_pending` only if a script needs
  them — not in A0.
- **Upstream scripts.** Unchanged: they call `openTransaction`/`commitTransaction` themselves,
  which is the *committed* path and is untouched.
- **`.ferrocad` / `.FCStd`.** A pending edit is never serialized; save with a live session
  commits it first (or aborts it — see the open question D4).
- **Editions.** A mod that supplies a manipulator keeps working: manipulators commit through
  the same session, so they get preview and rollback for free once (A3) lands.

---

## 6. Concrete changes required in the existing specifications

These are the edits the synthesis calls for. Section numbers are from the synthesis and are
re-checked against each file when its stage lands (§8); the status column is updated then.

### 6.1 [`selection-gizmos-and-forms.md`](../../developer/interaction/selection-gizmos-and-forms.md)

| Section | Change | Stage |
| --- | --- | --- |
| §2.1 Selection | add topology-filter gating (`SelectFilter::{Body, Face, Edge, Auto}`): when the active slot sets `Body`, `selection_event` strips sub-element names so a first click selects whole bodies | U4 |
| §4.2 States and transitions | unify the **normal** and **deferred** drag columns. Every `TransformGizmo` drag runs the cumulative session (`Begin → Update → End`), the preview survives the release, and no document transaction fires on mouse-up | U1 |
| §5.1 / §5.2 Form state machine | replace the floating Move command dialog (`Cancel`/`Move`) with the two-stage pill; change context loss from *silently cancelled* to *auto-commit if dirty*, reserving `Esc` for rollback | U1 (semantics), U2 (pill) |
| §7 Invariant 3 | "every pending transform session has an explicit rollback (`Esc`), an explicit checkpoint (`Enter`), and auto-commits non-zero deltas when the user deliberately switches selection, slot or tool" | U1 |

### 6.2 [`gizmo-manipulators.md`](../../developer/interaction/gizmo-manipulators.md)

| Section | Change | Stage |
| --- | --- | --- |
| §5 / §7 lifecycle, snapping, readout | promote `Tab`/digit entry from "deferred escalation" to the primary mechanism of the readout pill; merge `GizmoValue::readout` with the Δ pill so they are two states of one overlay | U2 |
| §9.2 / §14 kinds, hosting, priority | bring `PushPullStem` and `DressUpScrubber` under the active loadout session filter: a face click in Slot 1 (Body) arms only the triad; in Slot 2 (Face) only the stem | U4 |
| §19 where the control lives | document the top-left `Select ▾` header pill and the bottom-left `[1 Move] [2 Push/Pull] [3 Fillet]` group | U4 |

### 6.3 [`visual-language.md`](../../developer/interaction/visual-language.md)

| Section | Change | Stage |
| --- | --- | --- |
| §1.5 Text and glass chrome | remove `Cancel`/`Move` button chrome from the translation HUD; specify compact↔expanded pill geometry and the 1 px dashed tape strokes (`AXIS_X`, `AXIS_Y`, `AXIS_Z`, `INACTIVE_COLOR` ghost ring) | U2, U3 |
| §5 State model | extend IDLE → HOVER → DRAG with hover-to-arm on unselected bodies (`HoverReport::target`) and a post-drag `SESSION_HOLD` where the tape and pill stay live until `Enter`, a context switch or `Esc` | U3, U5 |

### 6.4 [`transform-ux.md`](../../developer/interaction/transform-ux.md)

The mode/pivot/re-pivot note stays authoritative for *what the handles do*; its lifecycle
section is superseded by §3 here, and its pending-mode discussion is superseded by §5. Add the
pointer at U1.

---

## 7. Staged plan

Dependency order is top to bottom. A0–A3 are invisible (no user-visible change, existing tests
must stay green); U1–U6 are the UX overhaul. Each stage is independently shippable and
revertable.

| Stage | Deliverable | Why this order | Spec updated |
| --- | --- | --- | --- |
| **A0** | `PendingEdit` + `EditView` in `ferrocad_core`, with tests. No callers. | The model the rest stands on; pure addition, zero risk | — |
| **A1** | `TransformSession` in `ferrocad_stage`; the viewport renders it; the shell owns it | One owner for the pending delta before any behaviour changes | — |
| **A2** | Delete the alternative preview paths: `set_gizmo_preview`/`gizmo_deferred`, `preview_home`'s transform branch, and `OpenDialog::translation`; every preview reads the session | Removes the three-way split that caused the 2026-10-30 bug | — |
| **A3** | Route the stem and the scrubber through the session + `PendingEdit` (they preview too, not just commit) | Proves the model generalises beyond `Placement`, before the UI leans on it | §6.2 §5/§7 (partially) |
| **U1** | One drag mode: release keeps the preview; `Enter` checkpoint; `Esc` rollback; auto-commit on context exit; `App_Move`'s no-arg form stops opening a dialog | The behaviour change all three reports asked for | §6.1 §4.2, §5.1–5.2, §7 |
| **U2** | The two-stage pill (`GizmoValue` + Δ fields merged); `Tab`/digit escalation; button bar gone | Needs U1's session to read from | §6.1 §5, §6.2 §5/§7, §6.3 §1.5 |
| **U3** | The 3-D measuring tape: ghost disc + stair-step legs + end-ticks; `SESSION_HOLD` | Needs the session's base/effective pair | §6.3 §1.5, §5 |
| **U4** | Loadout slots, `Select ▾` filter, `1`/`2`/`3` keys, `GizmoSet`/manipulator narrowing | Last, because it re-shapes what may arm; it needs one session to narrow to | §6.1 §2.1, §6.2 §9.2/§14/§19 |
| **U5** | Hover-to-arm and zero-click drag on `HoverReport::target` | Needs U4's filter (what "movable body" means is the slot's contract) | §6.3 §5 |
| **U6** | Spec sweep, dead-code removal, `.ferrocad` and Python docs, user docs page for the new Move | The convergence step | all of §6 |

### Stage acceptance

- **A0** — a pending edit stages, aborts (document byte-identical, `undo_names()` unchanged)
  and commits (one entry named by the session label, one recompute). `EditView` returns the
  staged value for reads and the committed value for ghosts. Round-trip through `.ferrocad`
  never persists a pending edit.
- **A1** — `TransformSession` arithmetic (cumulative translation, checkpoint, dirty) and one
  viewport test that renders a session and asserts the emitted item transforms.
- **A2** — grep-level: `gizmo_deferred` and `preview_home` are gone; every preview entry point
  takes a session snapshot. The 2026-10-30 regression tests
  (`a_move_hud_drag_survives_its_release`,
  `a_deferred_drag_reports_previews_and_never_commits`'s successor) pass unchanged in
  behaviour.
- **A3** — a fillet-radius session previews a ghost shape and rolls back without a history
  entry; the stem does the same.
- **U1** — drag, release, drag again, `Enter`: one undo entry; `Esc` after any drag: zero undo
  entries and the original pose; selecting another object mid-session keeps the move.
- **U2** — `Tab` on an armed object expands the pill, typing moves the preview, `Enter`
  commits; the old dialog buttons are absent; the headless harness can drive it without a
  window (the pill is shell state, not gpui-only).
- **U3** — the tape's legs are proportional to the non-zero delta components, the ghost disc
  sits at the datum, and nothing is pickable.
- **U4** — `1` arms only the triad even over a face; `2` arms only the stem; `Select ▾`
  overrides; the wheel still equips verbs into the active slot.
- **U5** — hovering an unselected body shows an IDLE triad; pressing a handle selects and
  starts the session in one gesture; leaving with nothing selected dissolves it.
- **U6** — the four specs agree, `docs/project/TODO-*.md` cites the stages, and the user docs
  have a Move page with the tape figure.

### Risks and mitigations

| Risk | Mitigation |
| --- | --- |
| A0 drifts from what the display actually needs and becomes unused scaffolding | Land A1 immediately after; A0's tests exercise `Placement` staging, which is A1's exact use |
| The `EditView` migration touches every read site | Migrate only the display/stage/snap paths in A0–A3; keep `Document`'s own reads; the view is additive |
| Recomputed ghosts (a fillet's *shape*, not just its placement) need a recompute preview, which the DAG cannot do without mutating | Out of scope for A3: A3 previews shape *properties already shown* (radius drives a rebuilt shape, but the rebuild is deferred to commit). Decide in U6 whether a real ghost recompute is worth a second DAG pass |
| Auto-commit on context exit surprises a user who expected to lose the drag | Exactly the inverse of today's data loss; the tape and pill make the pending state explicit, and one undo removes it |
| Dropping the dialog breaks scripts or tests that accept it | `App_Move` keeps the argument form; only the no-arg dialog goes, and its callers are in-tree |
| Loadout slots colliding with existing `Gizmo ▾` mode UI | U4 is last and owns the header layout; `Gizmo ▾` becomes the slot's *mode* (Translate vs Rotate) rather than a second selector |

### What is explicitly deferred

- Rotation sessions (`GizmoMode::Rotate`) — the session carries the mode from A1 but only
  translation is wired until after U3.
- Sub-element (face/edge) transforms — they need U4's filter and their own reference model.
- Multi-selection sessions — the session takes one target in A1; a set is a later widening
  (the `PendingEdit` already handles many properties).
- A real ghost *recompute* for parametric shapes (see the risk row above).
- OndselSolver / assembly constraints — unrelated (P4).

---

## 8. Open questions (decide before the stage that needs them)

| # | Question | Needed by | Leaning |
| --- | --- | --- | --- |
| D1 | Does a commit go through the Python command callback (status, naming, mod extensibility) or straight through `PendingEdit::commit`? | A1 | The command callback, with the pending edit supplying the exact values: extensions can still veto/transform, and the transaction name stays the command's |
| D2 | Where does the loadout slot live — view state (per window) or document state (per document)? | U4 | View state, beside `Gizmo ▾`, because it is a tool, not a model property |
| D3 | One session per shell or per viewport? | A1 | Per shell; the viewport is a view, and a second window must not start a second session on the same object |
| D4 | Save with a live session: commit or abort? | U1 | Commit, and say so in the status line: losing a drag to `Ctrl+S` is the same defect class as losing it to a selection change |
| D5 | Does `FreeCADGui.Preview` keep pushing items directly, or become `PendingEdit`-backed? | A3 | Keep the channel; back it with a pending edit so a script's preview and the UI's preview cannot disagree |
| D6 | Does the tape appear for a typed-only session (no drag)? | U3 | Yes, once a delta is non-zero — typing is a gesture too |
