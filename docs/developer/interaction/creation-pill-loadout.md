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
| Slots | a create dock of one pill per category (`5`–`9` arm them, in strip order) over 5 categories (Sketch / Solid / Surface / Text / Mesh) | 4 slots (`1` Move/Rotate, `2` Push/Pull, `3` Fillet⇄Chamfer, `0` Inspect) |
| Governs | what you make, and which surface/edge it anchors to | what a pick targets, and which manipulator is hosted |
| Active state | IDLE → HOVER → EXPANDED → ARMED (the tool's pill is added above the create dock) | Equipped → hover-to-arm (30 %) → Pending (the context gate refuses a switch) |
| Topology binding | WP: place on the grid · Body: feature from a profile · Face: place on the face `+N` · Edge: place along the edge | Body: `TransformGizmo` · Face: `PushPullStem` · Edge: `DressUpScrubber` · Inspect: highlight only |
| Commit | new shape added, selected, one undo step | transform/manipulator session committed (`✓`/`Enter`), one undo step |

The placement, idle:

```
Bottom-left (Modify)                                  Bottom-centre (Add, idle)
[ 1 Move │ 2 Push/Pull │ 3 Fillet │ 0 Inspect ]      [ Undo  Redo  Fit │ ⊕ Create Dock ^ │ Search (Ctrl+K) ]

[ 5 ⊕ Sketch: Rectangle ▾ │ 6 ⊕ Solid: Box ▾ │ 7 ⊕ Surface: Loft ▾ ]     ← the create dock, above
```

The Add hand is its **own strip**, one pill per category, sitting above the action dock —
the **continuation of the action loadout**, not a separate tray: the dock carries a single
`⊕ Create Dock ^` toggle and never a list of tools. Each pill reads
`<key> ⊕ <Category>: <Tool> ▾` and remembers its category's tool; its left half is the
**slot**, the same gesture as its key. The keys are the **one digit space both hands
share**: `0`–`3` equip the Modify hand's slots and `5`–`9` are the Add hand's pills at
those positions (`5` Sketch, `6` Solid, `7` Surface, `8` Text, `9` Mesh). `4` stays
unbound: it is the gap between the two runs.

A press **selects** the pill and arms the tool it names — the pill lights with the accent,
as the Modify bar lights its hand — and a press on the pill that already holds the armed
tool **steps to the next tool of that category**, wrapping round and skipping a tool the
selection cannot use. That is Slot 1's `1` Move ⇄ Rotate shape applied to a category's
tools, and it is why the key never opens a menu: browsing a list is what the slot exists
to save.

The flyout (`▾`) shows one category at a time behind a segmented header (`Sketch | Solid | Surface |
…`, empty categories omitted); picking a tool binds it to its pill and runs it, and
clicking a tab only points the pill at that category's remembered tool. Holding `Shift`
**peeks** it: the equipped pill's list appears while the key is down and goes away when it
is released, which is the keyboard's hover over the slot. The peek stands down whenever
`Shift` already means something — a running tool, an armed gizmo (plane drag), a pending
transform, a focused field — and never closes a flyout the user opened with the caret.
The rows are
numbered `1`–`8` in their own right-hand column, and a number pressed while the flyout is
open picks that row — the panel owns the digits while it is up, so `5` is the fifth tool
there and the Sketch pill otherwise. The pill's caret reads `^` while its flyout is open,
`▾` otherwise. The first press from another
hand is not a toggle — the slot keeps the tool it was left on.

Slot 3 reads `3 Fillet` until you press `3` again, which toggles it to `3 Chamfer` (and
back). The other slots keep one verb each; Slot 1 (`Move`/`Rotate`) and Slot 3
(`Fillet`/`Chamfer`) are the two that name the sub-mode they currently arm, which is why
their labels, glyphs and the handles they build all come from the slot rather than the
command. The docked numeric row carries the same toggle beside its fields, so the row and
the bar can never disagree.

The placement, ARMED (the tool's pill is added above the strip; nothing is swapped and the
canvas rectangle does not move):

```
Bottom-left (Modify, parked)                          Bottom-centre (Add, armed)
[ 1 Move │ 2 Push/Pull │ 3 Fillet │ 0 Inspect ]      [ Box on WP:XY │ W 20.00  L 20.00  H 20.00 mm │ ✗ │ ✓ ]
                                                     [ 5 ⊕ Sketch: Rectangle ▾ │ 6 ⊕ Solid: Box ▾ ]
                                                     [ Undo  Redo  Fit │ ⊕ Create Dock ^ │ Search (Ctrl+K) ]
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
  (2 for `Loft`/`Sweep`). The declaration is `FerroCAD_Gate: "profile:2"`: a **profile** is
  a whole body, a face or a wire — any of the three satisfies it — and the hint says
  `select at least 2 profiles` rather than claiming two bodies.
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
| Add hand | the dock is a registry command strip; the only creation affordances are commands: `Part_Box` creates 20×20×20 immediately, `Toy_MakeSlab` opens a **modal** three-field form, `Toy_Polyline` starts a tool whose strip is label/points/hint/Cancel/Finish. **2026-11-15:** the dock is `[globals │ ⊕ <LastTool> ▾ │ Search]`, the pill's flyout lists the `creation` commands by `FerroCAD_Category` with an unmet `FerroCAD_Gate` greyed in `INACTIVE_STRUCTURAL`, and an armed Box shows its `W/L/H` strip (`Tab`, typed digits, `Enter` commits) | [`creation-loadout.md`](creation-loadout.md) |
| Pending lock | `InteractionContext::{PendingMove, PendingRotation, PendingInteraction}` allow only view-only commands and the finish keys; `StartTool`, `SwitchHand`, `SwitchScope`, `Select` and edits are refused with a status line | keep; creation arms through the same gate |
| Bar dim | `loadout_bar(blocked)` dims to 0.45 and drops clicks; `action_dock` takes no `blocked` and stays lit | **done 2026-11-15:** `action_dock(blocked)` dims the same way while a transform or manipulator is pending, and the hand column dims while a creation tool is armed |
| Tool owns canvas | `start_tool` gates, closes the Move form, clears manipulators + gizmo, installs the tool, keeps the slot; the radial and hover-to-arm are already suppressed | already "Armed Add suspends Modify" |
| Hit priority | navicube → manipulators → gizmo → tool → scene | with the set empty, the tool is already directly behind the cube |
| Face binding | a creation tool's first left press on a planar face binds the session workplane (sticky; `Tool::picks_workplane` opts out) | add the hover preview frame; keep the sticky press binding |
| Slot switch while pending | **refused**: `set_loadout_slot` calls `gate(Action::SwitchHand)` before `commit_pending_session`; `switching_the_slot_waits_for_a_pending_move` pins it (`loadout.rs`) | docs reconciled to "refuse" (§6.2) |
| Scope | `SelectFilter { Auto, Body, Face, Edge }`, label `Select: Body`, one store in `Loadout` | **2026-11-15:** `SwitchScope` is refused while the armed creation tool's gate is unmet, with the status naming the tool (§6.6) |
| Creation metadata | `Contributions` holds tools by id; the registry row carries `category`, `target`, `gate` and the tool's own `ToolStatus::fields` | **2026-11-15:** the gate travels as `FerroCAD_Gate` and is evaluated by the chrome against the selection; a descriptor struct would add only an icon and default fields, which no edition needs yet |
| `HoverReport` | `{ target, through: Option<&'static str> }`, `"navicube"` written by the viewport; **2026-11-15:** the shell merges its own `creation_pill` note into the report it shows | done |
| `ToolStatus` | **2026-11-15:** `{ id, label, active, points, hint, fields: Vec<ToolField> }`; `Tool::set_field` writes a typed value back, and the strip is tabular monospace, unfocused by default | done (C2) |

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

1. **`Part_Box`'s fate and the screenshot workflow.** **Decided and landed:** `Part_Box`
   stays registered for Search/the app menu and now returns `{"tool": "box"}`, arming the
   Box tool; `Enter` commits at the placement origin, so the zero-mouse path survives as
   "arm the pill, press `Enter`". The screenshot step and the quickstart describe that
   two-step path; no second dock row exists. (`FerroCAD` 2026-11-15.)
2. **Pending + slot switch.** **Settled: refuse** (`gate(Action::SwitchHand)`), as the
   code always did. The stale "commits first" sentences are gone from the companion notes,
   and `switching_the_slot_waits_for_a_pending_move` pins the refusal.
3. **The disabled tool's ink.** **Landed:** `ui::INACTIVE_STRUCTURAL = #94A3B8`, used by a
   disabled menu row (and so by the pill's flyout when a gate is unmet) and by the pill's
   arm; `MUTED` keeps its meaning as the quiet text tone.
4. **Who reports `creation_pill` through the hover report?** **Landed as recommended:** the
   viewport's `HoverReport` is unchanged; when the creation flyout's negative space takes a
   press, the shell notes the overlay and merges it into the report it shows, so the status
   line reads `clicked through: creation_pill`.
5. **The loadout bar while ARMED.** **Landed:** it dims to 45 % and drops clicks while a
   creation tool is armed (`loadout_bar(blocked)`), the same visual as the pending lock, so
   "one hand owns the canvas" is visible rather than implied.
6. **May the scope change while ARMED?** **Landed: refused while the armed tool declares a
   gate the selection does not meet**, with the status naming the tool and its requirement
   (`Box needs select a face — finish it before changing the scope`). A tool with no gate
   leaves the scope switch alone.
7. **The ghost's alpha.** **Landed:** `ferrocad_stage::GHOST_SURFACE_ALPHA = 0.35`, used by
   `ghost_item()` and pinned by the stage tests.

---

## 7. Verification

- **Context.** `StartTool` is refused while a move/rotation/manipulator is pending, and the
  status names the finish pair; arming a creation tool clears the gizmo set, the
  manipulators and the Move form; the radial does not open while the tool runs.
- **Chrome.** Armed, the tool's pill is **added above** the create dock and the viewport
  rect is identical before and after; the dock and the strip keep their rows; the loadout
  bar dims only while pending (and, if decided, while armed).
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
