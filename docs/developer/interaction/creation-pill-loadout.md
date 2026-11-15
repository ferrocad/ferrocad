# The creation pill and the transform loadout: two hands, one context

Status: design (2026-11-13). Companion to [`creation-loadout.md`](creation-loadout.md)
(the Add hand's own specification: the split pill, the five categories, the tool strip,
the topology host and the C1–C4 slices), [`transform-ux.md`](transform-ux.md) (§3.1
docked fields and the finish pill, §7 order), [`gizmo-manipulators.md`](gizmo-manipulators.md)
(§7 lifecycle and A4, §9.1/§9.2 hosting, §14 edge dress-up),
[`visual-language.md`](visual-language.md) (§0.3 proximity, §1.5 chrome, §5 the state
model, §6 pass-through, §10 non-goals) and
[`selection-gizmos-and-forms.md`](selection-gizmos-and-forms.md) (§5.2 the context gate).

The creation pill and the transform loadout are **one loadout system with two hands**:
the Modify hand (bottom-left) hosts manipulators for existing geometry, and the Add hand
(bottom-centre) hosts authoring tools for new geometry. Both replace implicit inference
with an explicit, user-chosen slot, and both bind to the same topological targets
(Scene / Body / Face / Edge).

This note owns the **seam** between them: the shared vocabulary, the one context lock, the
naming rules and the edits the companion specs need. The Add hand's chrome, category
inventory, tool strip, tokens and slice plan are in
[`creation-loadout.md`](creation-loadout.md).

---

## 0. Principles

1. **One loadout, two hands.** A hand is chosen, not inferred. Neither hand may arm from
   a guess about what the pointer is over; hover only *previews* what a click would do.
2. **One context lock.** Only one transient session owns the canvas. The lock is the
   existing `InteractionContext` gate (`shell/bridge/context.rs`), not a second flag.
3. **One numeric bar.** At most one dimension/coordinate strip is live and screen-anchored;
   the canvas never gains a second row of inputs.
4. **One topological vocabulary.** `Select ▾`, `SelectionRef`, `ElementKind` and
   `ManipulatorFrame` are shared. The Add hand must not invent a parallel notion of what a
   face or an edge is.
5. **Screen-anchored readouts.** Numbers never jump (`visual-language.md` §0.4): the
   creation dimensions sit in the action-bar slot, not under the pointer.

---

## 1. The two hands

| | Add hand (creation pill) | Modify hand (transform loadout) |
| --- | --- | --- |
| Chrome anchor | centre action bar (bottom-centre) | loadout bar (bottom-left, `left: 16 px`, `bottom: 12 px`) |
| Slots | 5 categories (Sketch / Solid / Surface / Text / Mesh) | 4 slots (`1` Move/Rotate, `2` Push/Pull, `3` Fillet, `0` Inspect) |
| Governs | what you make, and which surface/edge it anchors to | what a pick targets, and which manipulator is hosted |
| Active state | IDLE → HOVER → EXPANDED → ARMED (the action bar swaps to the tool strip) | Equipped → hover-to-arm (30 %) → Pending (the context gate refuses a switch) |
| Topology binding | WP: place on the grid · Body: feature from a profile · Face: place on the face `+N` · Edge: place along the edge | Body: `TransformGizmo` · Face: `PushPullStem` · Edge: `DressUpScrubber` · Inspect: highlight only |
| Commit | new shape added, selected, one undo step | transform/manipulator session committed (`✓`/`Enter`), one undo step |

The placement, idle:

```
Bottom-left (Modify)                                  Bottom-centre (Add, idle)
[ 1 Move │ 2 Push/Pull │ 3 Fillet │ 0 Inspect ]      [ Undo  Redo  Fit │ ⊕ Box ▾ │ Search (Ctrl+K) ]
```

The placement, ARMED (the two never stack, and the canvas rectangle does not move):

```
Bottom-left (Modify, parked)                          Bottom-centre (Add, armed)
[ 1 Move │ 2 Push/Pull │ 3 Fillet │ 0 Inspect ]      [ Box on WP:XY │ W 20.00  L 20.00  H 20.00 mm │ ✗ │ ✓ ]
```

**Mutual exclusion.** The two hands are exclusive only through the existing context:

- **Pending Modify locks Add.** While a move, rotation or held manipulator is undecided
  (`Δ ≠ 0`, the `✗`/`✓` pair on screen), the gate already refuses `StartTool`,
  `SwitchHand`, `SwitchScope`, `Select` and editing commands; only the completion keys,
  field editing and `CmdType: ForView` commands pass. A creation tool therefore cannot
  arm. The loadout bar dims to 45 % and drops clicks (`chrome::loadout_bar(blocked)`);
  the action dock does not dim yet (§4).
- **Armed Add suspends Modify.** `Shell::start_tool` already closes the Move form, clears
  `set_manipulators([])` and `set_gizmo(None)`, and sets the tool; `hover_arm_allowed`
  requires `active_tool.is_none()`; the right-press radial is not opened while a tool is
  active. The next canvas click belongs to the tool.

---

## 2. Shared topology

### 2.1 `Select ▾` is the one scope

The viewport header's `Select: Body ▾` (between `Gizmo ▾` and `Snap ▾`) writes
`SelectFilter { Auto, Body, Face, Edge }`:

- `Auto` is the raw pick.
- `Body` strips the sub-element, so `Face2` of body 7 selects `whole(7)`.
- `Face` / `Edge` keep the first element; `Edge` also resolves the nearest projected edge
  inside the viewport (`PickScope::Edge`).

Both hands read the filtered `SelectionRef { object, elements: Vec<String> }`: the Modify
hand when it arms its manipulator, the Add hand when it decides enablement and reads the
hovered target. The Add hand reads it; it never writes it. A creation tool must not
quietly narrow the user's scope to make its own enablement work.

### 2.2 Enablement is a property of the target, not the hand

The creation brief's proposed `SelectionPredicate` does not exist in the tree, and it
should not: a predicate owned by the Add hand duplicates the scope the Modify hand already
reads. Enablement is a `TopologyGate` over `(kind, arity)` derived from the selection:

- Profile-consuming tools (`Prism`, `Revolution`, `Loft`, `Sweep`, `Face from Wire`) are
  enabled iff the selection resolves to a closed profile (`Wire`) with enough references
  (2 for `Loft`/`Sweep`).
- A body tool is enabled for a body; a spatial tool places on a face, an edge, or the
  workplane fallback.
- **The slot is irrelevant.** A tool does not fail because the Modify hand is parked in
  Slot 2 (Face) or Slot 3 (Edge); it fails because the *selection* is the wrong kind.
- The brief's `TargetKind` is `ElementKind` here; the shell derives it from the element
  prefix (`Face6` → `Face`), and the edition resolves a sketch to its `Wire`.

The `CreationTool` and `TopologyGate` types, and the disabled-tool feedback, are specified
in [`creation-loadout.md`](creation-loadout.md) §3.2.

### 2.3 The frame is shared; the hover preview is new

Both hands use `ManipulatorFrame { origin, basis }`. The Modify hand's manipulators derive
it from the tessellation (the `PushPullStem`'s planar face normal, the `DressUpScrubber`'s
edge polyline); the Add hand reuses the same math for its placement frame.

The code is press-time, not hover-time: the viewport binds a creation tool's **first left
press** on a planar face as the session's epitaxial workplane (sticky, never overridden by
a later click; `Tool::picks_workplane` opts a plane-picking tool out). The slice adds the
*hover* frame and the face-normal basis
([`creation-loadout.md`](creation-loadout.md) §3.1); it does not replace the sticky press
binding.

---

## 3. One lock, one set of names

### 3.1 `SESSION_HOLD` is not `PILL_HOLD`

In `visual-language.md` §5, `SESSION_HOLD` strictly means an **uncommitted** preview
(`Δ ≠ 0`) holding the context lock with the `✗`/`✓` pair live. A creation commit writes the
shape immediately, so the post-commit linger is **`PILL_HOLD`**: it holds no uncommitted
state and must gate no action. The Add hand's machine (including when `PILL_HOLD` ends) is
in [`creation-loadout.md`](creation-loadout.md) §2.

### 3.2 The context gate is the lock

`InteractionContext::{PendingMove, PendingRotation, PendingInteraction}` allow only
view-only commands and the completion keys. `StartTool`, `SwitchHand`, `SwitchScope`,
`Select`, `Arm` and editing commands are refused with a status line that names the finish
pair. Creation arms through the same gate; there is no second "creation lock" flag.

The `Tool` context allows everything at the gate because tool input is routed elsewhere,
so the two directions are enforced where they belong:

- **Add → Modify:** `start_tool` clears the gizmo set, the manipulators and the Move form;
  `hover_arm_allowed` requires no tool; the radial is closed.
- **Modify → Add:** the pending contexts refuse `StartTool`; the loadout bar is dimmed.

### 3.3 `ARMED` already suspends the Modify hand

`start_tool` already does what the Add hand needs: it gates on `Action::StartTool`, closes
the Move HUD, clears `set_manipulators([])` and `set_gizmo(None)`, installs the tool and
keeps the equipped slot. The viewport's routing then reaches the tool first; no new
priority tier is needed (see [`creation-loadout.md`](creation-loadout.md) §6.1).

---

## 4. What the code does today

| Piece | Today | This slice |
| --- | --- | --- |
| Modify hand | `[1 Move][2 Push/Pull][3 Fillet][0 Inspect]` in `chrome::loadout_bar`, bottom-left 16 px; the slot's default filter is `Move`/`Inspect` → Body, `PushPull` → Face, `Fillet` → Edge | unchanged |
| Add hand | the dock is a registry command strip; the only creation affordances are commands: `Part_Box` creates 20×20×20 immediately, `Toy_MakeSlab` opens a **modal** three-field form, `Toy_Polyline` starts a tool whose strip is label/points/hint/Cancel/Finish. **2026-11-15:** the dock is `[globals │ ⊕ <LastTool> ▾ │ Search]` and the pill's flyout lists the `creation` commands by `FerroCAD_Category` | [`creation-loadout.md`](creation-loadout.md): one split pill whose label arms a tool, one non-modal strip; the fields are still missing |
| Pending lock | `InteractionContext::{PendingMove, PendingRotation, PendingInteraction}` allow only view-only commands and the finish keys; `StartTool`, `SwitchHand`, `SwitchScope`, `Select` and edits are refused with a status line | keep; creation arms through the same gate |
| Bar dim | `loadout_bar(blocked)` dims to 0.45 and drops clicks; `action_dock` takes no `blocked` and stays lit | **done 2026-11-15:** `action_dock(blocked)` dims the same way while a transform or manipulator is pending, and the hand column dims while a creation tool is armed |
| Tool owns canvas | `start_tool` gates, closes the Move form, clears manipulators + gizmo, installs the tool, keeps the slot; the radial and hover-to-arm are already suppressed | already "Armed Add suspends Modify" |
| Hit priority | navicube → manipulators → gizmo → tool → scene | with the set empty, the tool is already directly behind the cube |
| Face binding | a creation tool's first left press on a planar face binds the session workplane (sticky; `Tool::picks_workplane` opts out) | add the hover preview frame; keep the sticky press binding |
| Slot switch while pending | **refused**: `set_loadout_slot` calls `gate(Action::SwitchHand)` before `commit_pending_session`. The documented test `switching_the_slot_commits_a_pending_drag_first` is not in the tree | reconcile the docs to "refuse" (A4) or auto-commit — decision §6.2 |
| Scope | `SelectFilter { Auto, Body, Face, Edge }`, label `Select: Body`, one store in `Loadout` | unchanged; the Add hand reads it (open question: may the scope change while armed? §6.6) |
| Creation metadata | `Contributions` holds tools by id only; no category, label, icon, fields or enablement | add a `CreationTool` descriptor, supplied by the edition (creation-loadout §1, §3.2) |
| `HoverReport` | `{ target, through: Option<&'static str> }`, `"navicube"` only, written by the viewport | `"creation_pill"` written by the shell (the viewport cannot see gpui chrome) |
| `ToolStatus` | `{ id, label, active, points, hint }`, text-only; an inactive status hides the strip | add `fields: Vec<ToolField>` and a `Tool::set_field` write-back; a creation tool reports `active` from the moment it is armed (0 points), or the strip never appears |

The tree also still calls the last bar slot **`0 Idle`** in one doc comment while the slot
is `Slot::Inspect` and the label is `Inspect`; the note keeps `Inspect`.

---

## 5. Companion spec edits

**Applied with these notes (2026-11-13):**

- `transform-ux.md` — contradiction 9 added: the code refuses a slot or scope switch while
  a value is pending, so the older "commits it first" rule is the one to change.
- `gizmo-manipulators.md` — §9.2 now states the refusal; §14 records that an armed creation
  tool clears the `GizmoSet` and the manipulators, so a click routes to the tool.
- `visual-language.md` — §1.5 states the one-numeric-bar rule and the in-place strip swap;
  §5 names `PILL_HOLD` and keeps `SESSION_HOLD` for the uncommitted transform; §6 lists
  `"creation_pill"` and notes that the shell supplies it.
- `creation-loadout.md` — the Add hand's own spec, including the inventory, the tool strip,
  the topology host and the C1–C4 acceptance.

**Still to do when the slice is cut:**

- `transform-ux.md` §3.1 / §7 — carry the creation strip under the same docked-fields and
  unfocused-by-default rule, and list the pending lock as the single rule for both hands.
- `gizmo-manipulators.md` §7/A4 — no wording change is needed: "a pick, a hand switch, a
  tool, or an editing command is refused" already covers the creation arm. Keep `tool`
  there if the list is ever split.

---

## 6. Decisions to settle

The Add hand's own choices are settled in [`creation-loadout.md`](creation-loadout.md):
the label is `⊕ Box ▾` (the tool it arms) and there is no duplicate `Create Box` button;
`PILL_HOLD` ends on pointer leave, `Esc` or a timeout; enablement is a
`TopologyGate { kind: ElementKind, min_arity, max_arity }`.

1. **`Part_Box`'s fate and the screenshot workflow.** `Part_Box` creates a default box
   immediately and returns `created Box001`; the screenshot tool's `create` step and the
   quickstart depend on that status. The Add hand removes the duplicate button but keeps
   the zero-mouse path as "arm the pill, press `Enter`" (commit at the WP origin).
   Recommend keeping `Part_Box` registered for Search/menu and updating the screenshot step
   and quickstart to the two-step pill path; do not keep a second dock row.
2. **Pending + slot switch.** The code refuses (`gate(Action::SwitchHand)`); two docs and
   a named test say it commits first. Recommend refuse (A4's rule: an explicit unfinished
   decision owns the context), delete the stale sentences, and add the missing test.
3. **The disabled tool's ink.** `ui::INACTIVE_STRUCTURAL` does not exist; `MUTED`
   (`#8B919A`) is the UI's muted text and the stage's `INACTIVE_COLOR` is `#94A3B8` @ 0.9.
   Recommend adding `ui::INACTIVE_STRUCTURAL = #94A3B8` (the value the Add-hand note
   already uses) rather than overloading `MUTED`.
4. **Who reports `creation_pill` through the hover report?** The viewport cannot; either
   the shell writes `HoverReport.through` for its own chrome or the status line grows a
   separate chrome-through field. Recommend the latter: `HoverReport` stays a neutral
   viewport type.
5. **The loadout bar while ARMED.** It is parked but still clickable today. Recommend
   dimming it to 45 % during ARMED (the same visual as the pending lock) and letting a
   hand switch take effect when the tool ends, so "one hand owns the canvas" is visible.
6. **May the scope change while ARMED?** The `Tool` context allows `SwitchScope`. A
   creation tool whose `TopologyGate` depends on the selection kind can lose its
   requirement mid-session if the user changes `Select ▾`. Recommend refusing
   `SwitchScope` while a selection-gated tool is armed, with the status naming the tool;
   the alternative is to re-evaluate and cancel the tool with a message.
7. **The ghost's alpha.** The Add-hand note specifies a 35 % display-only ghost. That is a
   new `ferrocad_stage` constant (the manipulator previews carry their own alpha today);
   pin the value in the theme/stage tests rather than leaving a literal in the tool.

---

## 7. Verification

- **Context.** `StartTool` is refused while a move/rotation/manipulator is pending, and the
  status names the finish pair; arming a creation tool clears the gizmo set, the
  manipulators and the Move form; the radial does not open while the tool runs.
- **Chrome.** The action dock swaps to the strip in place and the viewport rect is
  identical before and after; the loadout bar dims only while pending (and, if decided,
  while armed).
- **Naming.** `PILL_HOLD` leaves nothing pending and gates no action; `SESSION_HOLD` keeps
  its uncommitted-transform meaning.
- **Topology.** A face hover aligns the preview's `+Z` with the face normal; an edge hover
  anchors on the edge polyline; empty space falls back to the active workplane; a Loft is
  disabled with one profile and enabled with two, independent of the Modify slot.
- **Pass-through.** A click in the popover's negative space reports
  `clicked through: creation_pill` and still reaches the scene.
- The Add hand's own acceptance (C1–C4) is in
  [`creation-loadout.md`](creation-loadout.md) §8.

---

## 8. Non-goals

- No second command surface or pointer-following creation HUD.
- No implicit inference of the hand from the selection (the loadout slot stays explicit).
- No per-tool chrome styling: every creation tool shares the strip and the tokens.
- No change to the Modify hand's manipulator math or to `SESSION_HOLD`.
- No new topological primitive: `SelectionRef`/`ElementKind` stay the vocabulary.
- No modal dimension dialog for a creation tool; the strip replaces it.
