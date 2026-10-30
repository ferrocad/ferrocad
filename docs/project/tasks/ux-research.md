# UX research: the interaction questions we keep deciding ad hoc

Status: task spec (research, 2026-10-30). The roadmap names the interaction surface as one
of the two bottlenecks (`../ROADMAP.md`), and the 2026-10-29 round found four real defects
only because they were reported after shipping. This spec turns the open UX questions into
research with evidence, so the next interaction work starts from a decision instead of a
debate.

Related: [`polyline-vertex-editing-and-active-tool-hud.md`](polyline-vertex-editing-and-active-tool-hud.md),
[`workplanes.md`](workplanes.md), [`creation-tools-ownership.md`](creation-tools-ownership.md),
[`stroke-units.md`](stroke-units.md), [`../developer/interaction/visual-language.md`](../../developer/interaction/visual-language.md).

## Method and its limits

No user studies are available here. The evidence we can actually gather:

1. **The written contract** — `workplanes.md` §7–§12, the polyline/HUD spec, the visual
   language. What was *designed*.
2. **The code** — what is *rendered and reachable* today. A design that is not reachable is a
   gap, not a feature.
3. **Upstream comparison** — FreeCAD Draft/Sketcher at `33adf61` and SimpleCAD 0.38.0, for
   the interaction each one exposes.
4. **Headless walkthroughs** — `tools/screenshot.py` (viewport/chrome/desktop scopes) and
   `ferrocad_snapshot` render the real shell without a GPU; a scripted session steps through
   a task and records where it is stuck.
5. **Heuristic review** — Norman/Nielsen-style walkthrough of the rendered frames, labelled
   as opinion, not measurement.

Every finding must say which of the five it came from. “It feels crowded” is not a finding.

## Questions

Each question is one slice; a slice is done when there is a written recommendation with
evidence, or an explicit deferral with a reason.

### U1 — The draw path, end to end

From “pick a face” to “a wire exists”, what does the user see and do? Covers closing the
loop (click the first vertex vs `Enter`), Escape, vertex editing, numeric entry, and how many
undo steps the action costs.

- Evidence: SK4's three slices (`TODO-30` §P4), the polyline spec §6 tail, upstream Draft
  `Draft_Line`/`Draft_Wire` and Sketcher's draw modes.
- Deliverable: a numbered interaction sequence with a frame for each state, and a decision on
  closing/numeric entry.
- Acceptance: the sequence matches what the code does, or names exactly where it diverges.

### U2 — The active-action surface

The HUD replaced the strip; is one surface still carrying two jobs (create vs edit), and does
the destructive action read as secondary?

- Evidence: the spec §2 and what `render.rs`/`action_dock` actually draws now.
- Deliverable: a state table (idle → drawing → editing → committing) with the buttons, their
  hierarchy and their placement.
- Acceptance: no state has two primary actions, and the table is testable against the code.

### U3 — Snap feedback

The design specifies an aurora, projection rays and a ΔU/ΔV pill. Which of those exist, and
does feedback distinguish a feature snap from a grid snap on a bound plane?

- Evidence: `axis-origin-grid.md` §3.4, `ferrocad_stage::snap`, the viewport's snap drawing.
- Deliverable: implemented/designed/none per element, plus a recommendation on what the
  minimum honest feedback is (a user must be able to tell *where* the point will land).
- Acceptance: the table cites code for every “implemented”.

### U4 — Workplane affordances

The rule is “invisible at the world origin, obvious when not”. Is the `WP` pill, the offset
stem and a saved datum plane discoverable without knowing the vocabulary?

- Evidence: `workplanes.md` §7–§12, `tasks/workplanes.md` §8 gaps, the four grid defects as
  field reports (each was a user noticing the model disagreed with the screen).
- Deliverable: a “first five minutes” walkthrough — start a bare edition, find the plane,
  offset it, save it, reuse it — with the sticking points named.
- Acceptance: every sticking point is either a bug filed in the backlog or a deferral.

### U5 — Creation verbs and the wheel

Which creation actions belong on the object-verb wheel, and which are commands? `Move` became
a wheel verb; is `Extrude`/`Cut`/`Draw` the same class?

- Evidence: `transform-ux.md`, `selection-gizmos-and-forms.md` §2.2, the command registry.
- Deliverable: a verb inventory with a rule (“verbs act on the selection; commands create
  objects”) and the consequences for P2's boolean commands.
- Acceptance: the rule explains every current verb or names the exception.

### U6 — Numeric entry and precision

Where can the user type exact values today, and where must they drag? Offset has a field and
the stem is two-way; push/pull has a readout. What is the minimum set of typed inputs for a
“draw on a face, extrude by 5 mm” session?

- Evidence: the offset dialog, the push/pull stem, the polyline spec's numeric-entry tail.
- Deliverable: a list of operations with typed input today (yes/no) and the smallest set to
  add.
- Acceptance: the list is checked against the code, not the specs.

### U7 — Task walkthroughs

Five scripted tasks, each run headlessly or rendered, recording the step where a competent
user would be stuck:

1. Draw a closed path on a tilted face.
2. Offset the workplane and draw on it.
3. Save a datum plane, select it later, draw on it.
4. Push/pull a face by a typed distance.
5. Cut a hole with the boolean command.

- Deliverable: one table per task: steps, what happened, stuck-or-not, evidence (frame or
  command output).
- Acceptance: at least one finding per task, or a recorded “no gap” with the frame proving it.

## Findings (2026-10-30)

Read from this checkout. U3 and U6 only. The evidence is the contract, the code and the upstream
comparison; **no headless walkthrough** was run (another agent had the build), so a perceptual
claim is limited to what the cited render path draws.

### U3 — Snap feedback

| Element | Status | Evidence |
| --- | --- | --- |
| Snap aurora (diamond, `#38BDF8`, 7 px) | implemented | `feedback::aurora`, `crates/ferrocad_stage/src/feedback.rs:23-26,52-65`; drawn at `crates/ferrocad_viewport/src/inventor_view.rs:4679-4686` |
| 120 ms acquisition flash | implemented | `feedback.rs:29-30,47-50`; restarted only when point or kind changes, `inventor_view.rs:4636-4652` |
| Projection rays to U and V | implemented, **grid only** | `feedback.rs:71-90`; gated on `kind == SnapKind::Grid`, `inventor_view.rs:4687-4701` |
| ΔU/ΔV pill | implemented | `snap_pill`, `inventor_view.rs:5579-5598`; rendered at `:5177-5184` |
| Origin proximity bloom (5 px) | implemented | `feedback.rs:40-44,93-112`; `origin_bloomed`, `inventor_view.rs:4744-4755`; `MarkerLayer`, `:747-756` |
| **Snap kind named in the feedback** | **none** | the pill has only ΔU/ΔV; `SnapKind::is_feature` is defined and never called (`snap.rs:37-46`); `snap_name` exists but serves only the pivot readout (`inventor_view.rs:3324-3334,5567-5577`); `snap_kind()` is not read by the shell |
| Major vs minor grid intersection | none | `Snap.major` is carried (`snap.rs:63-65`) and never used for ink (`inventor_view.rs:4674-4701`) |

**Verdict.** A user can tell a feature snap from a grid snap only *negatively*: a grid snap draws
rays and round ΔU/ΔV values, a feature snap draws neither. That is not honest feedback, because a
feature can sit on the grid, a `Point` snap on a gridless plane also has no rays, and vertex, edge
midpoint, face centre and origin are drawn identically to one another. Nothing on screen says
*what* the point is. The data already exists: `FreeCADGui.Snapper.snap` keeps the whole `Snap`
(kind, local, major) in `snapInfo` (`python/FreeCADGui/draft.py:580-614`; the `Snapper` lives
in `FreeCADGui/draft.py`, not `FerroCADGui/__init__.py`), and the viewport exposes
`snap_kind()`; only the on-screen label is missing.

**Minimum honest feedback.** (1) Add one word to the pill from the existing `snap_name`
(`grid · major`, `vertex`, `edge mid`, `face centre`, `origin`, `surface`); no geometry, ink or
token change. (2) Keep the rays grid-only — with the label they become the “why”, the aurora
stays the “where”. (3) Defer per-kind glyph shapes; the label is the cheaper contract change and
does not collide with SK4's HUD work.

### U6 — Numeric entry and precision

| Operation | Typed input today | Evidence |
| --- | --- | --- |
| Move the selection | **yes** — ΔX/ΔY/ΔZ quantity fields, two-way with the gizmo drag | `_MoveCommand`, `python/ferrocad_shell/__init__.py:1134-1138`; `dialog::translation_preview`, `crates/ferrocad_gpui/src/dialog.rs:283-313`; drag to field `shell/dialogs.rs:327-339` |
| Offset the workplane | **yes** — one “Offset along normal (mm)” field, two-way with the `+N` stem | `_OffsetWorkplaneCommand`, `ferrocad_shell/__init__.py:2111-2131`; readout `crates/ferrocad_stage/src/workplane_stem.rs:255-258`; field and stem `shell/viewport.rs:956-1004` |
| Extrude or pocket a face (push/pull) | **no** — drag only; readout `extrude 12.30 mm` | `crates/ferrocad_part_gui/src/push_pull.rs:86-89,214-235`; hosted as a manipulator, `crates/ferrocad_parametric/src/main.rs:93-95` |
| Fillet or chamfer an edge | **no** at the gesture; the committed feature's radius is typed in the property pane | `crates/ferrocad_part_gui/src/dress_up.rs:103-114`; `shell/properties.rs:11-58` |
| Draw a polyline vertex (X/Y) | **no** — designed, not built | spec §3.4, `polyline-vertex-editing-and-active-tool-hud.md:157-163`; “Still open … not built”, `:251-252`; the strip has no fields, `shell/render.rs:18-81` |
| Edit an object property | **yes** — free-text rows, one property per transaction | `shell/properties.rs:11-58` |
| Grid spacing | **yes** — quantity field in the settings fallback | `ferrocad_shell/__init__.py:1774-1780` |

**Verdict.** Typed entry exists wherever a pending session has a two-way field (Move, workplane
offset). Every tool and every face/edge gesture is drag-only, and the typed escape hatch arrives
only after the fact, in the property pane. For “draw on a face, extrude by 5 mm” the first half
is not typed at all, and the second cannot be typed when it happens. Upstream is ahead on both:
Draft's task panel has X/Y/Z, length, angle and radius fields
(`freecad-upstream/src/Mod/Draft/DraftGui.py:725-739`, `validatePoint` `:1436-1461`), and
SimpleCAD's push/pull dialog focuses a distance edit
(`simplecad/simplecad/gui/tools/push_pull.py:15-25,355-367`).

**Smallest additions, ordered.** (1) Numeric X/Y on the polyline tool — already specified in
the polyline spec §3.4, and the HUD field machinery exists; this is the “draw on a face” half. (2) An editable
push/pull distance: a `DialogSpec` with `preview: "push_pull"` that seeds from
`PushPullStem::distance` and commits through the existing `PushPullChange::into_commit`, the same
pattern the offset HUD uses; this is the “extrude by 5 mm” half. (3) Nothing else — a generic
command line or coordinate widget is not justified by the evidence; the property pane already
covers post-commit edits.

## Decision log (fill as the slices close)

| # | Question | Decision | Evidence | Follow-up |
| --- | --- | --- | --- | --- |
| U1 | draw path | | | |
| U2 | active action | | | |
| U3 | snap feedback | The feedback is real (aurora, flash, grid-only rays, pill, origin bloom) but never names the snap; add the kind to the existing pill and defer per-kind glyphs. | `feedback.rs:23-112`, `inventor_view.rs:4636-4701,5579-5598`; unused `is_feature`, `snap.rs:37-46` | Pill text uses `snap_name`; a viewport test asserts the kind line |
| U4 | plane affordances | | | |
| U5 | wheel verbs | | | |
| U6 | numeric entry | Two-way typed fields exist for Move and workplane offset; tools and face/edge gestures are drag-only. Add polyline X/Y, then an editable push/pull distance; no generic command line. | `dialogs.rs:106-116,327-339`, `shell/viewport.rs:956-1004`, `push_pull.rs:86-89`; polyline spec §3.4 | SK4's numeric tail; a `preview: "push_pull"` dialog |
| U7 | walkthroughs | | | |

## Sequencing

U3 and U6 are cheapest and feed SK4 directly. U1 and U7 should follow SK4's first working
commit, because a walkthrough of a path that cannot finish measures nothing. U2 and U4 are
review-heavy and can run in parallel with anything. U5 blocks P2's command work.

## Non-goals

- Recruitment, usability labs or analytics.
- A visual redesign: the visual language is a contract; findings propose changes to it, not
  a new one.
- Re-deciding the stroke unit (see [`stroke-units.md`](stroke-units.md)).
