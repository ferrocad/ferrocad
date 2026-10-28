# Intent, lineage and safe automation

Status: ideation (2026-10-28). A product thesis, not a commitment. Companion to
[`README.md`](README.md), [`../ROADMAP.md`](../ROADMAP.md), the
[general CAD introduction](../../general/cad-architecture.md) and the
[developer architecture](../../developer/README.md).

## The thesis

FerroCAD should not merely be "FreeCAD with a nicer Rust/GPUI shell". It should use its
Rust document model, lineage layer, semantic grid and FreeCAD-compatible Python facade
to make **design intent, dependencies and engineering context first-class**.

> **One-sentence pitch.** FerroCAD is a modern, local-first FreeCAD host that keeps
> your familiar parametric workflow but makes design intent, dependencies, failures
> and automation understandable.

That is distinct from a generic "modern CAD in Rust" pitch and from upstream
FreeCAD's compatibility-first approach. The claim is not a better kernel or a
different file format; it is a host that understands *why* a model is built the way it
is and helps you change it safely.

## What is already here

This thesis is credible because the foundations exist. It is a direction for using
them, not a proposal to start over.

| Foundation | Where it lives | Why it matters |
| --- | --- | --- |
| FreeCAD-compatible Python facade | `python/FreeCAD`, `ferrocad_py`, [`python-api/`](../../python-api/index.md) | Compatibility is the credibility; the new layer must not break it. |
| Document engine | `ferrocad_core`: properties, links, expressions, undo, serializers | Intent, provenance and dependencies can be document data, not side files. |
| `.FCStd` reading and writing | `ferrocad_core`, `ferrocad_part_py` | Interoperability is a strategic asset, not a legacy burden. |
| Lineage layer | `ferrocad_lineage`; [`lineage-and-naming.md`](../../developer/architecture/lineage-and-naming.md) | Naming is already separated from the kernel, so explanations can query real provenance. |
| Semantic grid and snapping | `FerroCAD.Grid`; [`axis-origin-grid.md`](../../developer/interaction/axis-origin-grid.md) | Alignment and spacing can become candidate constraints, not only viewport aids. |
| Selection, picking and gizmos | `ferrocad_graphics`, `ferrocad_stage`, `ferrocad_viewport`; [`selection-gizmos-and-forms.md`](../../developer/interaction/selection-gizmos-and-forms.md) | Interaction already produces the signals a proposal system needs. |
| Provider registry | `FerroCADGui`; [`mod-providers.md`](../../developer/workbenches/mod-providers.md) | Context checks and suggestions can be providers, not monolithic features. |
| Embedded CPython and the JSON bridge | `ferrocad_shell`; [`ferrocad_shell.md`](../../python-api/ferrocad_shell.md) | Automation can use the same surface the UI uses. |

## 1. Design intent as a document object

**Existing strength.** Documents, typed properties, expressions, links and undo are
already in the Rust engine.

**Proposed extension.** Add a first-class **Design Intent** object, for example
`App::DesignIntent`. It would store:

- **Requirements:** mass limit, envelope, material, load case, fastener standard.
- **Assumptions:** "3 mm sheet", "5 mm edge distance", "FDM-compatible".
- **Rules:** minimum wall thickness, minimum hole spacing, maximum deflection.
- **Provenance:** whether a value came from the user, a spreadsheet, a rule or an
  agent.

A request such as "design a 40 mm extrusion bracket, M5 holes, 3 mm aluminium, under
60 g" would then create a visible intent record plus conventional sketches,
constraints, features and parameters. It would not create opaque geometry.

**Implementation sketch.**

1. Add an `App::DesignIntent` document object with typed properties and expression
   support.
2. Let sketches, Part features and Draft objects reference intent parameters.
3. Add a "Propose from intent" command that generates a draft feature tree.
4. Require explicit user acceptance before committing generated features.

This fits the existing namespace rule: the Python facade stays FreeCAD-compatible,
while the new extensions live in `FerroCAD`/`FerroCADGui`.

## 2. Turn lineage into explanations

**Existing strength.** The lineage layer, links and document/property semantics already
record more than a flat feature list.

**Proposed extension.** Expose lineage in the UI as an explainable dependency graph.
Instead of only showing "Sketch → Pad → Fillet", the model tree could answer:

- "What uses this datum plane?"
- "What breaks if I change this hole spacing?"
- "Why did recompute fail?"
- "Which objects are stale, orphaned or externally referenced?"
- "Which properties are expression-driven?"

A useful UI pattern is a **Dependency Inspector**:

| Panel | Content |
| --- | --- |
| Upstream | Sketch geometry, datum, spreadsheet cells, linked bodies |
| Downstream | Pads, pockets, holes, assemblies and drawings that depend on this object |
| Failure cause | Missing reference, cyclic dependency, invalid geometry, changed topology |
| Safe actions | Rebind reference, suppress feature, convert to a datum-relative reference |
| Provenance | Who or what created or modified the object, and when |

This is where FerroCAD can beat a generic AI bolt-on. The assistant should not guess
dependencies from geometry; it should query the actual Rust document graph and lineage
metadata, then explain it in plain language.

## 3. Constraint proposals from interaction

**Existing strength.** FerroCAD already has a native snap API, Draft tooling, selection,
ranked picking, gizmos and interaction sessions.

**Proposed extension.** Use those interaction signals to propose — not silently
impose — constraints.

Examples:

- The user drags a line near perpendicular: propose `Perpendicular` and show the
  resulting residual error.
- The user places four holes by eye: propose an equal-spacing pattern or a symmetry
  constraint.
- The user moves a face with a gizmo: offer to convert the movement into an offset
  dimension.
- The user selects two edges and invokes "Relate": suggest coincident, parallel,
  tangent, equal-length or concentric based on geometry and context.

The semantic grid is especially relevant. Rather than treating it only as a viewport
aid, FerroCAD could make it an **intent surface**: grid alignment, spacing and snap
behaviour become candidate constraints.

**UX rule.** Every inferred constraint appears as a reviewable suggestion with
accept, edit and reject. The system must never make a sketch harder to edit by adding
hidden constraints.

## 4. Live engineering context

**Existing strength.** The property system, expressions and GPUI shell are suitable for
live panels and overlays.

**Proposed extension.** Add a **Context Rail** — a persistent right-side panel that
changes with the selection and the design state.

For a selected bracket it might show:

- Material and process assumptions.
- Mass and volume estimates.
- Minimum wall thickness and smallest internal radius.
- Fastener compatibility.
- Interference or clearance warnings.
- STEP export readiness.
- Open assumptions that need confirmation.

For a Draft drawing it might show scale, units, dimension standards, missing views or
annotations, inconsistent tolerances, and DXF/PDF export issues.

The architectural decision is to make these **providers**, not monolithic features.
FerroCAD already has a provider concept in `FerroCADGui`; context checks can plug into
that same surface.

## 5. Local-first agent automation

**Existing strength.** FerroCAD preserves the `FreeCAD` Python API and already drives
the UI through embedded CPython, so scripted and automated workflows are natural.

**Proposed extension.** Add a **local Model Context Protocol (MCP) server** as a
controlled front door to the document.

Start with read-only tools:

- `list_documents`
- `get_object_tree`
- `get_object_properties`
- `get_dependencies`
- `get_recompute_errors`
- `export_step`
- `export_svg`
- `screenshot_viewport`

Then add narrowly scoped write tools:

- `create_sketch`
- `add_constraint`
- `create_pad`
- `set_parameter`
- `run_validators`
- `undo_last_agent_action`

Each agent action should be recorded as a normal undoable transaction, with a visible
provenance entry: "Created by Claude via FerroCAD MCP from prompt X." That preserves the
local-first ethos while enabling real automation.

MCP is emerging as a way to connect agents to modelling, manufacturing and export
workflows. FerroCAD's opportunity is to do this **without requiring the user's design
data to live in someone else's cloud**. This is an assumption to validate, not a
measured market fact.

## 6. A FerroCAD-specific roadmap

These are product phases for the thesis. They do not replace the sequenced work in the
[TODO backlog](../TODO-27-10-2026.md) or the horizons in the [roadmap](../ROADMAP.md).

### Phase 1 — Make the familiar layer excellent

- Finish dependable `.FCStd` round-tripping for common Part/Draft workflows.
- Make recompute failures actionable: identify the failing object, the reason and the
  affected downstream objects.
- Standardise selection, gizmo, snapping and property-editing behaviour.
- Keep the FreeCAD Python facade stable; compatibility is FerroCAD's credibility.

### Phase 2 — Expose the model's meaning

- Build the Dependency Inspector from the existing lineage and link model.
- Add named assumptions and design parameters.
- Add a "Why did this fail?" action.
- Add object-level provenance: user edit, macro, import or agent.

### Phase 3 — Add assistive intelligence

- Add constraint suggestions from selection and interaction.
- Add natural-language queries over the document graph.
- Add "propose feature tree from intent", initially limited to simple mechanical parts.
- Keep every proposal as normal, editable, FreeCAD-compatible objects.

### Phase 4 — Add the agent surface

- Ship a local MCP server.
- Begin with inspection and export.
- Add write operations behind explicit user approval and undoable transactions.
- Add an action log and a permission model.

## What FerroCAD should avoid

- **Do not make AI the geometry kernel.** The Rust document model and geometry
  operations must remain deterministic and inspectable.
- **Do not hide FreeCAD compatibility behind a new file format.** `.FCStd`
  interoperability is a strategic asset, not a legacy burden.
- **Do not build a chat-only CAD app.** A command palette, direct manipulation, a
  property editor and a feature tree remain faster for precise engineering work.
- **Do not confuse suggestions with truth.** Manufacturability, FEA and cost estimates
  are estimates unless produced by a validated service, and must be labelled as such.

## Open questions

1. Is `DesignIntent` a core object, a workbench object or a document extension, and how
   does it round-trip through `.FCStd`?
2. What is the provenance schema, and how does it survive recompute, undo/redo and
   save/open?
3. How do we keep inferred constraints reviewable and prevent hidden-constraint debt?
4. Which context checks are deterministic rules and which are estimates, and how is the
   difference shown in the UI?
5. Which MCP transport and permission model fit a local-first app, and how are agent
   actions attributed and undone?
6. What is the minimum useful `DesignIntent` for Phase 1 without overfitting to one
   domain?
7. How do we measure "safe change" beyond tests — dependency-graph coverage, resolved
   recompute failures, or user-reviewed proposals?
8. Where is the compatibility boundary with upstream FreeCAD's `ElementMap` and
   topological naming?

## The one-sentence pitch

> **FerroCAD is a modern, local-first FreeCAD host that keeps your familiar parametric
> workflow but makes design intent, dependencies, failures and automation
> understandable.**

That is a genuinely novel direction: not "rebuild Fusion", and not merely "rebuild
FreeCAD's UI", but **make the engineering model self-explanatory and safely
automatable**.
