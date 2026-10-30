# Creation tools: core default, or mod-owned?

Status: task spec / decision (2026-10-30). Raised while cutting the SK4 draw path: the toy
edition has a working polyline prototype, and it is not obvious whether that belongs in the
core as a default that mods can override, or stays an edition/mod choice. Relates to
[`simplecad-bringup.md`](simplecad-bringup.md) (SC7 uses the same primitives) and
[`../TODO-30-10-2026.md`](../TODO-30-10-2026.md) §P4 SK4.

**This is a decision task, not a build task.** The output is a written decision plus the
smallest test that pins it.

## 0. What the tree already does (verified 2026-10-30)

- **Tools are registered per edition, with last-wins precedence.**
  `Contributions { tools: Vec<(String, Arc<dyn ToolFactory>)> }`; `register_tool` documents
  “A later registration with the same id wins”, and `tool_for` scans `.rev()`
  (`crates/ferrocad_gpui/src/contributions.rs:155-229`).
- **The base registers exactly one tool: `workplane_points`**
  (`crates/ferrocad/src/lib.rs:80-86`). The parametric edition adds manipulators only
  (`crates/ferrocad_parametric/src/main.rs:91-95`).
- **The polyline itself lives in the core crate** (`ferrocad_stage::PolylineTool`) and the
  toy edition supplies the *creation* half in Python: a command returning
  `{"tool": "polyline"}` plus an `_interaction` that creates one `App::FeaturePython`
  carrying a float list (`python/ferrocad_toy/__init__.py`, `crates/ferrocad_toy/tests/round_trip.rs`).
- **Python tools cannot shadow a Rust id.** The shell checks the `python:` prefix first, and
  a Python tool is addressed as `python:<name>` (`crates/ferrocad_gpui/src/shell/dispatch.rs:15,31,700`).
  So today precedence is **edition-only**: a later Rust registration wins, but a mod can
  only add a differently-named tool.
- **The creation policy is already split from the tool.** `InteractionCommit` says *what
  happened* (a flattened point list and `creating`); the edition's `_interaction` decides
  *what object* that becomes. The toy chooses an `App::FeaturePython`; the parametric
  edition has no handler at all.

## 1. The question

Should a polyline (and, by extension, the other creation tools) be:

- **A — a core default.** The base registers `polyline` and a core creating provider;
  editions and mods may override either by registering the same id.
- **B — core types, edition opt-in (status quo).** `PolylineTool` and the commit types are
  core; every edition registers the tool and supplies its own provider.
- **C — mod-owned.** The tool and the provider live in a mod; core knows only the `Tool`
  trait and `InteractionCommit`.

## 2. What each option costs

| | A — core default | B — edition opt-in | C — mod-owned |
| --- | --- | --- | --- |
| Bare edition can draw | yes | no | only with the mod |
| Who owns “a polyline becomes X” | core decides a default | each edition decides | the mod decides |
| Override reachable by a mod | **not today** (needs a registry change) | n/a | n/a |
| Undo/transaction ownership | core | each edition | the mod (easy to get wrong — the Draft `make_wire` path already does) |
| Matches the existing pattern | like `translate_gizmo` | like `push_pull` | like a workbench |

The load-bearing fact: **the tree already has the split that makes A cheap** (tool vs
creation policy), but **the override half of A is not reachable from a mod** because Python
tool ids are prefixed, not shadowing.

## 3. Decision criteria

1. **Does a mod need to replace the default, or only add to it?** If replacement, a
   precedence rule for Python tools must exist, and it must be explicit (`python:` ids are
   namespaced by construction; shadowing would break the “which layer won?” question).
2. **Is “a polyline becomes a wire” a core policy?** The toy creates a feature with a float
   list; the parametric edition wants a `Part::Feature` wire (and later a sketch). Those are
   workbench decisions, which argues for B even if A is convenient.
3. **What does a bare edition mean?** If the base binary is a kernel-free toy, a core default
   can only create non-kernel objects, which is exactly the toy's float-list prototype.
4. **Does the override need a test?** Yes, whichever option is chosen — the current
   last-wins rule has no test that pins *precedence* (the existing one only asserts
   `is_some()` after a duplicate registration, `contributions.rs:376-387`).

## 4. Slices

- [ ] **X1 — write the decision.** Pick A/B/C, state the override rule for Rust and for
      Python, and record it in this file. Cite the criteria above.
- [ ] **X2 — pin precedence.** A test that registers two tools with the same id and asserts
      the *later* one is created (and, if Python shadowing is allowed, the Python one), added
      to `contributions.rs`'s tests.
- [ ] **X3 — implement only what the decision needs.** Under A: register the tool and a
      core provider in `base_contributions`, and give the provider a pluggable object policy.
      Under B: leave the base alone and move the toy's provider pattern into a documented
      helper. Under C: move the toy's tool into the toy edition and delete the core type.
- [ ] **X4 — one end-to-end test.** Whichever option, a headless test that starts the tool by
      id, commits, and asserts the object (and that one undo removes it). This is also SK4c's
      acceptance, so do not build a second harness for it.

## 5. Recommendation (for the decision to accept or reject)

**B, with X2's precedence test and a documented helper.** Reasons:

- The creation policy is genuinely per-workbench (toy: feature + float list; parametric:
  `Part::Feature` wire; later: sketch), so a core default would have to be either the toy's
  shape or a plugin point anyway.
- A mod cannot shadow a Rust tool id today; claiming “mods can override” under A would be
  false without a registry change, and that change has its own naming consequences.
- B is the status quo, so the work is honest: document the helper, pin precedence, and stop
  duplicating the creation loop in each edition.
- If a bare-edition drawing tool is wanted later, A can be layered on top of B without
  breaking either: the base registers a default *and* the edition still wins.

## 6. Non-goals

- A general plugin ABI for tools (the `Tool` trait is the seam; keep it).
- Making every command overridable.
- Deciding SK4's object type here — that is `simplecad-bringup`/`TODO-30` SK4's job; this
  spec only decides *who* decides it.
