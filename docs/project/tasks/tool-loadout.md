# The tool loadout: slots, the `Select ▾` filter, and hover-to-arm

Status: **design and plan (2026-10-30).** *Plan only — nothing here is built.*

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
| Slot 1 | `1` | Body (element: `None`) | `TransformGizmo` (Translate / Rotate) | the session's 3-D tape + expandable ΔX/ΔY/ΔZ pill ([`transform-session.md`](transform-session.md) §3) |
| Slot 2 | `2` | Face (`FaceN`) | `PushPullStem` (extrude / pocket) | in-situ readout pill (`Tab` to type) |
| Slot 3 | `3` | Edge (`EdgeN` chain) | `DressUpScrubber` (Fillet / Chamfer) | in-situ readout pill (`Tab` to type) |
| Wheel | Right-click / Space | context-dependent | "Buy menu": equips specialised verbs (Rotate, Chamfer, Draft) into the active hand | the same session contract |

Deliberate swapping over implicit collision:

- With **Slot 1** equipped, `PushPullStem` and `DressUpScrubber` are **unoffered**, even if the
  pointer passes over `Face1`; the face is still *selectable*, it just does not get its stem.
- With **Slot 2** equipped, the corner triad is hidden, and a click on a face arms the stem.
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
  [3 Fillet]`), so the hotkey is discoverable without the wheel.

### 1.4 What a slot change does to a live session

A slot change is a **context exit**: if the transform session is dirty it commits first
(`transform-session.md` §1.2, invariant 2), then the new slot narrows the hosted set and the
topology filter. The one exception is the "equip a verb into the current slot" case (Rotate),
which keeps the session and changes its mode.

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

The filter lives in view state (D2), beside `Gizmo ▾`, and is what the shell passes to
`InventorViewport::set_select_filter`.

### 2.2 Where the stripping happens

At the **selection event**, not at the pick: the pick still reports the sub-element (hover
needs it for the bloom, snapping needs it for the element map), and the shell rewrites the
reference it reports to the tool, the tree and the manipulator arming.

### 2.3 What the filter does *not* decide

- It does not change what is **hoverable** or what the hover bloom shows.
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
| **U4** | loadout slots, `Select ▾` filter, `1`/`2`/`3` keys, the bottom-left bar, and `GizmoSet`/manipulator narrowing | Needs one session to narrow to | §4.1, §4.2 |
| **U5** | hover-to-arm and zero-click drag on `HoverReport::target` | Needs U4's filter: "movable body" is the slot's contract | §4.3 |
| **U6** | this note's share of the sweep: spec text final, dead code (`transform_verb` and the claim rule it replaced) gone, user-docs page for the loadout | Converges with the session half | all of §4 |

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
| D2 | Where does the loadout slot live — view state (per window) or document state (per document)? | U4 | View state, beside `Gizmo ▾`, because it is a tool, not a model property |
| D5 | Does `FreeCADGui.Preview` keep pushing items directly, or become pending-backed? | U4 | Keep the channel; back it with a pending edit so a script's preview and the UI's preview cannot disagree |

The session note owns D1, D3, D4 and D6.
