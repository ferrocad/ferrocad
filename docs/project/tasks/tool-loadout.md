# The tool loadout: slots, the `Select ▾` filter, and hover-to-arm

Status: **U4, U5 and U6 built (2026-10-31).** Slot 1 hosts the triad, Slots 2 and 3 host
the selection's manipulator, Slot 0 is Inspect (selection highlight only), the
`Select ▾` filter rewrites the reported selection, `0`–`3` and the bottom-left bar equip
a slot, a slot or filter change commits a dirty
session first, and the Move hand materialises an idle triad over an unselected body so a
handle press selects and drags it in one gesture. The sweep and the user page (U6) are
in: the four interaction specs agree with the tree, the retired vocabulary is deleted,
and `docs/user/how-to/tool-loadout.md` documents the bar, the filter and hover-to-arm.
Read against the tree in
[`../research/transform-session-compatibility.md`](../research/transform-session-compatibility.md),
which found the loadout **smaller** than this note first assumed: only one compiled manipulator
is hosted per selection already, and both arm seams take the element — so narrowing is a
filtered `SelectionRef`, not a new manipulator kind.

This is the **tool half** of the 2026-10-30 transform synthesis. The other half — the explicit
uncommitted edit, the one cumulative session, the pill and the 3-D tape — is
[`transform-session.md`](transform-session.md). The two share the stage numbering and are
meant to be read together; this note is the authority for *which control may arm at all* and
for the pointer's relationship with it.

## 0. The problem: one pointer, two manipulators

The triad is not the only control that wants the same click. The third strand of the
split-brain (§0 of the session note has the other two):

- **Manipulator collision.** OCCT tags every click on a solid with a sub-element (`FaceN`), and
  `translate_gizmo` claimed any selection, so a single click could arm **both** the face's
  `PushPullStem` (extrude 0.00 mm) and the body's corner `TransformGizmo`. Which one answered
  the pointer came down to registry hit priority — not to anything the user chose.
- **Invisible inference.** The arming rule was "walk the triangle pick, let every contributor
  claim what it recognises". There is no user-facing statement of *what I am about to do*, so
  the same click can mean three different things depending on tessellation.
- **A mode selector doing two jobs.** The header's `Gizmo ▾` chooses translate vs rotate, but
  a user who wants to push/pull a face has nowhere to say so except the wheel, and the wheel's
  choice then competes with the inferred claim.

### 0.1 What we will not do

- **No per-click inference of the tool.** A click may pick a *thing*; it must not decide which
  *instrument* is in the user's hand.
- **No simultaneous manipulators.** At most one manipulator class is hosted and drawn.
- **No new hidden mode.** The slot is visible in two places (header and bar) and switchable by
  a single key; nothing about it is implicit.

---

## 1. The loadout

The metaphor is the FPS tool loadout: the user carries an explicit set of active slots, and the
radial wheel is the "buy menu" that equips verbs into the active hand.

### 1.1 The slots

The active slot governs both what a viewport hover/click targets (the **topology filter**) and
which single manipulator class is hosted (`GizmoSet::set_session`):

| Slot | Hotkey | Default `Select ▾` filter | Active manipulator (`set_session`) | Numeric surface |
| --- | --- | --- | --- | --- |
| Slot 1 | `1` | Body (element: `None`) | `TransformGizmo` (Translate / Rotate) | the docked coordinate row + the in-context `✗`/`✓` pill, and the session's 3-D tape ([`transform-session.md`](transform-session.md) §3). Pressing `1` again toggles the hand Move ⇄ Rotate |
| Slot 2 | `2` | Face (`FaceN`) | `PushPullStem` (extrude / pocket) | in-situ readout pill (`Tab` to type) |
| Slot 3 | `3` | Edge (`EdgeN` chain) | `DressUpScrubber` (Fillet / Chamfer) | in-situ readout pill (`Tab` to type) |
| Slot 0 (Inspect) | `0` | Body (`None`) | none — selection only | the in-context Body/Face/Edge switcher above the bar; the stage's selection highlight shows the hovered or latched scope |
| Wheel | Right-click / Space | context-dependent | "Buy menu": equips specialised verbs (Rotate, Chamfer, Draft) into the active hand | the same session contract |

Deliberate swapping over implicit collision:

- With **Slot 1** equipped, `PushPullStem` and `DressUpScrubber` are **unoffered**, even if the
  pointer passes over `Face1`; the face is still *selectable*, it just does not get its stem.
- With **Slot 2** equipped, the corner triad is hidden, and a click on a face arms the stem.
- With the **Inspect** hand (`0`), no manipulator is hosted at all; the stage highlight
  previews the scope a click would select, and the latched selection stays visible. A
  Body/Face/Edge switcher docks above the bar and writes the same filter as
  `Select ▾`.
- The wheel's verbs equip *into* the active slot: `Rotate` keeps Slot 1 and switches the
  triad's mode; a future `Chamfer` would equip Slot 3 and preselect the operation.

### 1.2 The header pills

`Select ▾` sits in the viewport header between `Gizmo ▾` and `Snap ▾`:

`Grid ▾ | Axes ▾ | WP: XY ▾ | Gizmo ▾ | Select: Body ▾ | Snap ▾`

Switching slots updates it; the user can still override it (e.g. `Face` while in Move, once
sub-element transforms land). `Gizmo ▾` keeps its existing meaning — it is the *mode* of Slot 1
(Translate vs Rotate), not a second selector of which control is armed.

### 1.3 The loadout bar

A bottom-left pill group, anchored at `left: 16 px` on the same baseline as the centred
creation dock (`Create Box | Search (Ctrl+K)`):

- `ui::DOCK_BG` (#16181D @ 0.90) with `ui::DOCK_BORDER` (@ 0.12);
- the active slot highlighted in `ui::CHROME_ACTIVE` (#67D7FF) over `ui::CHROME_ACTIVE_BG`
  (@ 0.33);
- one pill per slot, labelled with the hotkey and the tool (`[1 Move] [2 Push/Pull]
  [3 Fillet] [0 Inspect]`), so the hotkey is discoverable without the wheel. Inspect
  comes last: it is the neutral hand, not a fourth tool.

### 1.4 How the narrowing is implemented

At the **arm seam**, not in the manipulator trait. One selection already hosts at most one
compiled manipulator plus whatever a mod declares, and both calls take the element:

| Call | Where | What the slot does to it |
| --- | --- | --- |
| `contributions.manipulator_for(reference, stage)` | `shell/viewport.rs:534-536` | only called for the slots that want a manipulator (2 and 3) |
| `python::manipulator_specs(object, element)` | `shell/viewport.rs:568` | receives the slot-filtered element, so a mod's face handle cannot arm under Slot 1 |
| `arm_gizmos(reference, stage)` | `shell/viewport.rs:542` | only called for Slot 1 |

`GizmoSet::set_session` is **not** the narrowing mechanism: it selects a gizmo's *kind*
(Translate/Rotate/Universal), which is Slot 1's mode, not which class is in the hand. The
`transform_verb` heuristic that used to retire the body triad is **gone (U4,
2026-10-31)**: the slot's two calls decide, and `Arm` passes a slot-filtered
`SelectionRef` to both. The old "a claimed sub-element hides the triad, otherwise the
triad stays" fallback is deliberately replaced: Slot 2 on an unclaimed element arms
nothing, and the fixed bar says why (`[1 Move]` is one key away).

### 1.5 What a slot change does to a live session

A slot change is a **context change**: while the transform session is dirty the pending move
is unfinished, so the change is **refused** by the context gate
(`shell/context.rs`, `transform-session.md` §1.2 invariant 2) — the value is neither
committed nor dropped, and the status line says why. Finish the move with `✓`/`Enter` or
`✗`/`Esc` and equip the hand again. The same holds for `Select ▾`, the handle set
(Rotate), starting a tool and edit commands (`Create Box`), while view commands stay live. The one exception is the
"equip a verb into the current slot" case (Move on the active hand), which re-arms rather than
changing the slot.

### 1.6 As built (2026-10-31)

- `Shell::loadout` (`shell/loadout.rs`) is the one store: a `Slot` and a
  `SelectFilter`. `Loadout::set_slot` also resets the filter to the slot's preset;
  `Loadout::set_filter` is the header override. `Shell::loadout` sits beside
  `gizmo_mode` because both are tool state, not document state (D2).
- The bottom-left bar (`chrome::loadout_bar`, `[1 Move] [2 Push/Pull] [3 Fillet]
  [0 Inspect]`) and the header selector (`chrome::select_button`, `Select: Body ▾`
  between `Gizmo` and `Snap`) both call the same setters; `0`–`3` do too. The
  selector's row ids are synthetic (`select:<filter>`) and `run_command` intercepts
  them before the registry.
- `Slot::Inspect` is a real slot, not a paused Slot 1: it defaults to the `Body`
  filter, hosts no manipulator, and owns the Inspect hand's visual answer. Its own
  `chrome::inspect_bar` docks Body/Face/Edge above the loadout bar (the same store as
  `Select ▾`; `Auto` stays header-only). `Shell::sync_inspect_highlight` paints the
  **filtered** hover target (or the latched selection when the pointer is away)
  through `InventorViewport::set_inspect_highlight`, which uses the stage's existing
  per-part selection tint/wire. That is what replaces the triad as the "what will a
  click select?" cue.
- `Shell::arm` narrows the `SelectionRef` once and then calls **either**
  `arm_gizmos` (Slot 1) **or** `manipulator_for` + `manipulator_specs` (Slots 2/3).
  Slot 0 calls neither. `replace_selection` applies the same filter to every item, so
  the tree, the facade and the arm seam agree (`Body` strips the element).
- `transform_verb` is deleted. `Move`/`Rotate` from the wheel and `App_Move` equip
  Slot 1; the old "claimed element suppresses the triad" rule and its fallback are
  both gone. A tool session does not change the hand.
- A slot or filter change commits a dirty session first (`commit_session`), then
  rearms. The slot keys are reserved above the resting form's typing path: `0` on an
  armed triad equips Inspect, `2` equips the face hand, and a value that starts
  with a slot digit begins with `Tab` or a click into a field.
- Slot 1's key toggles its two hands (Move ⇄ Rotate); it no longer cycles into idle.
  Entering Slot 1 from another slot always starts at the arrows (Rotate is a sub-mode
  of the active Move pill).
- Covered by `ferrocad_gpui`'s `a_face_click_selects_the_body_in_the_move_slot`,
  `the_slot_sets_the_filter_and_select_overrides_it`,
  `the_wheel_verbs_equip_the_move_slot`,
  `the_loadout_bar_renders_and_equips_a_slot`,
  `the_move_and_inspect_slots_are_separate_hands`, `inspect_offers_a_scope_switcher`,
  `switching_the_slot_commits_a_pending_drag_first`, and the GPU-only
  `a_face_click_in_the_move_slot_arms_only_the_triad`,
  `a_claimed_sub_element_suppresses_the_shared_body_triad` /
  `an_unclaimed_sub_element_leaves_the_face_slot_empty`; the Inspect highlight has
  `ferrocad_viewport`'s `the_inspect_highlight_paints_the_filtered_scope`.

---

## 2. The topology filter contract

### 2.1 `SelectFilter`

```rust
pub enum SelectFilter { Auto, Body, Face, Edge }
```

- `Auto` is the pre-loadout behaviour: whatever the pick reports.
- `Body` **strips the sub-element** from the selection event: a click on `Face2` of body 7
  selects `whole(7)`. This is what makes Slot 1's "grab the body and move it" work on the first
  click, with no Shift-click or tree interaction.
- `Face` / `Edge` are the first-element variants the manipulator slots need.

The filter lives in view state (D2), beside `Gizmo ▾`. The shell applies it where it
reports a selection, where it arms a hand, and where the Inspect hand paints its preview;
`InventorViewport` only sees the filtered `SelectionRef`.

### 2.2 Where the stripping happens

At the **selection event**, not at the pick: the pick still reports the sub-element (hover
needs it for the bloom, snapping needs it for the element map), and the shell rewrites the
reference it reports to the tool, the tree and the manipulator arming.

### 2.3 What the filter does *not* decide

- It does not change what is **hoverable** or what the raw pick reports. The Inspect
  hand's highlight is the exception that proves the rule: it applies the filter, so its
  preview always matches what a click would select.
- It does not change the wheel's target resolution (right-click still resolves the element,
  because a wheel's verbs are element verbs).
- It does not apply to a translation session once it has begun: the session's target is fixed
  at `begin`.

---

## 3. Hover-to-arm and zero-click drag

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

- **Zero-click initiation.** With Slot 1 equipped and no selection, hovering a body previews
  the triad at its anchor; grabbing a handle selects the body and starts the drag without a
  preliminary click.
- **Selected persistence.** Clicking (or selecting from the tree/keyboard) latches the
  selection, so the triad stays when the pointer leaves and keyboard-only entry (`Tab`) works.
- **The armed triad is the session's `ArmedIdle`** (`transform-session.md` §1.1): hover-to-arm
  only changes *whether* it is shown and at what origin, never the commit semantics.

### 3.1 As built (2026-10-31)

- `Shell::hover_arm` holds the previewed body, and `Shell::sync_hover_arm` runs from the
  viewport's notification observer: the viewport already notifies on every hover change
  (the report is not an event of its own). It is allowed only in the Move hand with an
  empty selection, no session, no tool and no modal/wheel.
- The preview hosts the same `arm` seam as a selection, with the template's `Body` filter,
  at **`HOVER_ARM_OPACITY` 0.30**: `InventorViewport::set_gizmo_idle_scale` multiplies the
  workspace style's opacity for the hosted set. The scale is presentation only; hit-testing
  and the commit are unchanged. It is cleared by every explicit re-arm.
- While the pointer travels from the body onto the hovered triad's handles, the hover
  report is empty (the gizmo consumes the move). `InventorViewport::gizmo_hovered` keeps
  the preview alive there, so it does not dissolve just before the press.
- `PreviewPhase::Begin` promotes the previewed body before the session opens
  (`Shell::promote_hover_target`): the commit reads the facade selection, which would
  otherwise be empty. Promotion deliberately does **not** replace the hosted gizmo (the
  drag already owns it); it only removes the idle dimming. A latched selection, not a
  hover, is what keeps the triad after the pointer leaves.
- The **Inspect hand** (`0`) has no handles to preview. `Shell::sync_inspect_highlight`
  runs from the same observer and paints the stage's selection tint/wire on the filtered
  hover target (`Body` = every part, a named element = that part), or on the latched
  selection when the pointer is away. `InventorViewport::set_inspect_highlight` reports
  whether anything changed, so a stationary pointer does not repaint. This is the
  pointer's substitute for the triad's implicit "what will a click select?" cue.
- Covered by `ferrocad_gpui`'s `a_hover_preview_promotes_its_body_when_a_drag_begins`,
  `hover_to_arm_needs_the_move_slot_and_an_empty_selection`,
  `hover_arming_hosts_and_dissolves_the_preview` and (with a GPU)
  `the_idle_scale_dims_the_hosted_triad`; `ferrocad_stage`'s `a_scaled_opacity_clamps`,
  and `ferrocad_viewport`'s `the_inspect_highlight_paints_the_filtered_scope`.

---

## 4. Spec changes this note owns

Section numbers are re-checked against each file when its stage lands. The session-owned rows
are in [`transform-session.md`](transform-session.md) §4.

### 4.1 [`selection-gizmos-and-forms.md`](../../developer/interaction/selection-gizmos-and-forms.md)

| Section | Change | Stage |
| --- | --- | --- |
| §2.1 Selection | add topology-filter gating (`SelectFilter::{Body, Face, Edge, Auto}`): when the active slot sets `Body`, `selection_event` strips sub-element names so a first click selects whole bodies | U4 |

### 4.2 [`gizmo-manipulators.md`](../../developer/interaction/gizmo-manipulators.md)

| Section | Change | Stage |
| --- | --- | --- |
| §9.2 / §14 kinds, hosting, priority | bring `PushPullStem` and `DressUpScrubber` under the active loadout session filter: a face click in Slot 1 (Body) arms only the triad; in Slot 2 (Face) only the stem | U4 |
| §19 where the control lives | document the top-left `Select ▾` header pill and the bottom-left `[1 Move] [2 Push/Pull] [3 Fillet]` group | U4 |

### 4.3 [`visual-language.md`](../../developer/interaction/visual-language.md)

| Section | Change | Stage |
| --- | --- | --- |
| §5 State model | extend IDLE → HOVER → DRAG with hover-to-arm on unselected bodies (`HoverReport::target`); the post-drag `SESSION_HOLD` row is the session note's | U5 |

### 4.4 [`transform-ux.md`](../../developer/interaction/transform-ux.md)

Its "what the handles do" sections stay authoritative; add a pointer to this note where it
discusses which manipulator a selection arms (the intent is unchanged — context-driven — the
*inference* is what goes).

---

## 5. Stages

This note owns **U4** and **U5**, and its share of the shared sweep **U6**. A0–A3 and U1–U3 are
in [`transform-session.md`](transform-session.md) §5.

Prerequisites: **U4 needs U1** — one session must exist before "which control may arm" can be
narrowed to it. U5 needs U4 (the slot's filter defines what a "movable body" is). U4 may land
before U2/U3, i.e. the loadout can arrive before the pill and the tape.

| Stage | Deliverable | Why this order | Spec updated |
| --- | --- | --- | --- |
| **U4** | loadout slots, `Select ▾` filter, `0`–`3` keys, the bottom-left bar, and the two arm calls gated by the slot (no trait change) | Needs one session to narrow to; no `ManipulatorKind` is required | §4.1, §4.2 — **built 2026-10-31**; `transform_verb` retired |
| **U5** | hover-to-arm and zero-click drag on `HoverReport::target` | Needs U4's filter: "movable body" is the slot's contract | §4.3 — **built 2026-10-31** |
| **U6** | this note's share of the sweep: spec text final, the user-docs page for the loadout, and any claim the tree no longer makes | Converges with the session half | all of §4 — **done 2026-10-31**: the four interaction specs state the built loadout/hover behavior, the retired vocabulary is gone from the code (`transform_verb` was retired by U4), and `docs/user/how-to/tool-loadout.md` is authored and indexed. `transform_verb` was already deleted by U4, so the sweep no longer owns it |

### Stage acceptance

- **U4** — `1` arms only the triad even over a face; `2` arms only the stem; a face click in
  Slot 1 selects `whole(body)` and never `FaceN`; `Select ▾` overrides the slot's filter; the
  wheel still equips verbs into the active slot; switching slot with a dirty session commits it
  first (a test that drives a drag, presses `2`, and finds one undo entry).
- **U5** — hovering an unselected body shows an IDLE triad; pressing a handle selects the body
  and starts the session in one gesture; leaving with nothing selected dissolves it; a latched
  selection keeps the triad after the pointer leaves.
- **U6** — the four specs agree with both notes, `docs/project/TODO-*.md` cites the stages, and
  the user docs have a page for the loadout bar and the `Select ▾` pill.

### Risks and mitigations

| Risk | Mitigation |
| --- | --- |
| Stripping sub-elements in `Body` breaks a face workflow the user expects | The filter is visible and overridable in the header, and `Face` is one click away — the same distance as today's wheel |
| Three slots is the wrong number | The bar is data-driven; a slot is a `(filter, session kind, key)` triple, so adding or reordering is configuration |
| Narrowing only the compiled manipulator leaves a mod's declared face handle armed | The slot filters the `SelectionRef` **before** both arm calls, so the declared-handle path sees no element under Slot 1 |
| Slot state and the file's `Select ▾` can disagree | One store, two views: the bar writes the same `SelectFilter` the header shows, never a copy |
| Hover-to-arm picks the wrong body in a dense view | The armed triad is a *preview* only: pressing a handle is what promotes the selection, so the worst case is one stray press, and `Esc`/`[1]` gives it back |
| The bar collides with the creation dock at small widths | Same row as the dock, anchored left; the dock is centred, so they only meet on very narrow windows — clamp the bar's width and let the dock shift |

### Explicitly deferred

- Per-slot settings (a slot remembering a chamfer size) — the bar is a selector, not a preset
  store.
- Tool slots for creation (polyline, box) — those are transient tools, not loadout slots;
  [`creation-tools-ownership.md`](creation-tools-ownership.md) decides where they live.
- Slot persistence across sessions — after U6, if the store already exists for `Gizmo ▾`.

---

## 6. Open decisions

| # | Question | Needed by | Leaning |
| --- | --- | --- | --- |
| D2 | Where does the loadout slot live — view state (per window) or document state (per document)? | U4 | **Resolved by U4 (2026-10-31): view state**, `Shell::loadout` beside `gizmo_mode`; it is a tool, not a model property |
| D5 | Does `FreeCADGui.Preview` keep pushing items directly, or become pending-backed? | U4 | **Unchanged by U4:** the channel stays. `Manipulator::preview_items` (A3) is the display-only native half; a pending-backed script preview is still the leaning, not built |

The session note owns D1, D3, D4 and D6.
