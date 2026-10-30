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

## Decision log (fill as the slices close)

| # | Question | Decision | Evidence | Follow-up |
| --- | --- | --- | --- | --- |
| U1 | draw path | | | |
| U2 | active action | | | |
| U3 | snap feedback | | | |
| U4 | plane affordances | | | |
| U5 | wheel verbs | | | |
| U6 | numeric entry | | | |
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
