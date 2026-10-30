# Transform interaction: modes, pivots, re-pivoting and push/pull

Status: **design note (2026-10-25).** The mode set (slice 1) and the wheel's `Rotate`
verb landed 2026-10-26; re-pivot (slice 3) and push/pull (slice 5) also landed
2026-10-26. Select mode (slice 4) landed 2026-10-26. "Today" means at `d3c5853` for
the unbuilt parts; the landed slices are marked where they appear.
Companions: [`gizmo-manipulators.md`](gizmo-manipulators.md) (triad math and hit test),
[`selection-gizmos-and-forms.md`](selection-gizmos-and-forms.md) (wheel, form, deferred
drag), [`topology-selection.md`](topology-selection.md) (sub-element names),
[`axis-origin-grid.md`](axis-origin-grid.md) (the face frame).

> **Superseded in part (2026-10-30).** The session lifecycle in §3 and the pending-mode
> discussion are replaced by one cumulative transform session with explicit commit,
> checkpoint and rollback — no deferred mode, no undo-as-cancel, and a first-class
> uncommitted edit in the model. See
> [`../../project/tasks/transform-session.md`](../../project/tasks/transform-session.md); the
> mode set, pivots, re-pivoting and push/pull behaviour here stay authoritative.

## 0. What exists

| Piece | State | Where |
| --- | --- | --- |
| Triad handles | Translate is the default; **Rotate is hosted** through the shell's session mode; Universal is defined but not surfaced | `gizmo.rs`; `contributions.rs`; `shell.rs` `gizmo_mode`/`set_gizmo_mode` |
| Click anchor | the triad hugs the object origin (`ShapeItem::anchor`); a session `user_origin` from re-pivot overrides it while set | `contributions.rs` `translate_gizmo`; `shell.rs` `user_origin` |
| Origin/pivot handle | **built (slice 3)**: `GizmoTarget::Origin`, a 10 px disc, priority origin > axis > plane > ring | `gizmo.rs` `GizmoTarget`; `inventor_view.rs` `gizmo_origin_snap` |
| Face/edge tags, `Move` + Δ form, feature snaps | today | `inventor_view.rs`; `ferrocad_shell/__init__.py`; `snap.rs` |
| Face-normal push/pull | **built (slice 5)**: `PushPullStem` + the parametric `push_pull` hook | `ferrocad_part_gui/src/push_pull.rs`; `python/ferrocad_parametric/__init__.py` |

The triad targets the **whole item** even when the selection names a face
(`contributions.rs:22-28`; `gizmo-manipulators.md:779-782`). A Part click always resolves
to an element, so a face click is the normal case; an untagged toy mesh selects the body.

## 1. Mode set and switching

**Recommendation: two semantic modes, `Translate` and `Rotate`; `Universal` is a
presentation of both, not a third tool.** The shell owns `gizmo_mode` and applies it
after the factory, because every factory returns a fresh `Translate` gizmo
(`contributions.rs:35-39`).

| Surface | Sets | Why |
| --- | --- | --- |
| Wheel `Move` | `Translate` (+ opens the Δ form) | the existing verb means "move"; it must force Translate so the deferred drag and fields agree (`selection-gizmos-and-forms.md:230-235`) |
| New wheel `Rotate` | `Rotate` | the object-verb vocabulary is where users look for "what to do" |
| Header `Gizmo` control (in flight) | the same state; may expose **Both** | the mode is which handles are drawn, so it is presentation, the header's existing meaning (`gizmo-manipulators.md:767-771`, `796-797`) |
| Keyboard | `Ctrl+Shift+R` (registered on `App_Rotate`) | `shortcuts.rs:64-76` binds chorded keys; a bare `R` stays unbound |

**As built (2026-10-26).** `Shell::gizmo_mode` is the sticky session state; `arm`
applies it after the factory (`gizmo.set_mode`), so a re-selection or a re-arm keeps
the chosen handle set. A command returns a top-level `"gizmo": "translate" |
"rotate" | "universal"` directive from `Activated`; the shell applies it before a
dialog opens. `App_Rotate` returns `rotate` (a mode-only verb: no form, no
transaction), and `App_Move` returns `translate` on both its HUD and its
`(dx,dy,dz)` path. The radial menu lists `App_Rotate` beside `App_Move`; with three
verbs the slots are N/SE/SW, so Delete is south-west rather than due south.
Switching away from translate cancels an open translation HUD, whose Δ fields would
no longer describe the visible handles.

- The mode is **sticky** across re-arm and selection; only a document open (or `Move`)
  resets it. A rotate commit stays in rotate mode instead of snapping back to arrows.
- **`Universal` is an overview, not a default.** Its handles are real, but priority is
  axis > plane > ring (`gizmo-manipulators.md:191`), so rings are hard to grab; it must
  never be the only route to rotate.
- A face selection still transforms the **body**; the commit's status must say
  "Body (Face6)", not "Face6".

## 2. Pivot policy

**Recommendation: the triad hugs the object's own origin, and that origin is the pivot of
every mode.** The display provider reports the placement's base point (`StageMesh.origin`
→ `ShapeItem::anchor`), so the triad sits on the point the object is defined at and
doubles as an **origin indicator**; a mesh that does not report one falls back to its
bounding-box base corner. A click does not move it: for translation the pivot does not
change the result, and a different centre is a deliberate, snapped gesture (§3).

| Mode | Whole body | Face / edge / vertex | Multi sub-element | Tree/command |
| --- | --- | --- | --- | --- |
| Translate | object origin | object origin (the body moves) | no triad until the commit fans out | object origin |
| Rotate | object origin | object origin (the body turns) | no triad until the commit fans out | object origin |
| Push-pull (§4) | — | face centroid, `+N` — a manipulator, not the triad | — | — |

The origin travels with the body: the display hook bakes the placement into the positions
and re-reports the base point, so a re-arm after a commit, a resize or an undo puts the
triad back where the object says its origin is. No shell-side bookkeeping, no stale-point
rule. A face click still acts on the **body**; see §1 for the frame wording.

**As built (2026-10-26).** Re-pivot (§3) adds one exception, and it is session state,
not document state: `Shell::user_origin: Option<Vector3>` replaces the factory datum
while set. A new selection (including a context promotion) clears it, `Esc` clears it,
and it is never persisted or undone. Nothing else in the pivot policy changed: the
triad still hugs `ShapeItem::anchor` and a click still does not re-anchor it.


**Multi-selection is not a triad case yet.** `arm`/`GizmoFactory::create` see one
`SelectionRef` (`contributions.rs:84-87`) and the commit names one target, so a triad
would move only the primary. Until the commit fans out, suppress the triad; the wheel's
`Move` still moves the whole selection via `_move_selection` (`ferrocad_shell/__init__.py:883-899`).

## 3. Origin drag (re-pivot)

**Built 2026-10-26.** A centre handle moves the working origin, for both translate and
rotate; it is the only way to rotate about a point that is not the object's origin, and
the same gesture gives translation a datum that is not the origin.

- **Hit.** A separate `GizmoTarget::Origin`, not a transform handle: a 10 px projected
disc (`snap::FEATURE_PIXELS`), appended after `GizmoHandle::ALL` so every existing pick
id is unchanged. Priority **origin > axis > plane > ring**. It sits inside the stems'
excluded 18 %, stealing nothing. A host can disable it (`set_origin_enabled`), and a
disabled origin is neither drawn nor picked.
- **Snap.** It reuses the viewport's own pipeline (`snap_candidates` +
`snap_to_geometry_with`): a `Vertex`, `EdgeMid`, `Centre` or `Origin` within 10 px wins,
else the ray's point on the hovered body, else the workplane grid; the readout names the
snap (`pivot · vertex`). There is no second snap set.
- **Frame and release.** The drag moves the **real** frame live (`set_origin`): a pivot
drag has no solver, so there is no `display_offset`. On release
`ViewportEvent::GizmoOrigin { point, kind }` carries the world point and the snap kind;
the shell stores it as a session `user_origin` and applies it instead of the object
origin while it is set. It is not a transform commit and never touches the document.
`Esc` restores the pre-drag frame and clears it.
- **With `Move`'s deferred form.** The origin handle is disabled while a
`preview: "translation"` form is open: the HUD is anchored to the visible origin and
must not slide under the user's typing. Closing the form re-enables it.

## 4. Face-normal alignment and push/pull

**Built 2026-10-26: push/pull is a Part manipulator, not a mode of the triad.** The
triad's commit is a 16-value placement transform and drops the element, so it cannot
express "extrude `Face6`". The manipulator seam already carries the element and the
shell passes it to the app script, which keeps the triad kernel-free.

- **Handle.** A single amber stem from the face centroid along `+N`, with a linear
  readout. The face frame is derived from the item's own tessellation: `ShapeItem`
  carries `part_names` and per-triangle `parts`, so the named face resolves to its
  triangles. Only planar faces get a stem (every triangle normal must agree); a curved
  face returns `None`. The axis solver, the near-view-aligned fallback and the `Linear`
  value are reused from `TransformGizmo` (the stem frame's local Z is the face normal).
- **Commit.** `InteractionCommit::new("push_pull", entity, "extrude" | "pocket")` with
  `.with_element("Face6")`, `.with_values([distance])` and the readout — the same
  neutral route the fillet/chamfer scrubber and the toy height grip use.
- **Edition hook.** The parametric script builds a `Part::Extrusion` from the named
  sub-shape, then one `Part::Fuse` (outward) or `Part::Cut` (inward), in a single named
  transaction. The intermediate prism is hidden, and the boolean result inherits the
  body's `Placement`. The extrusion direction must be **shape-local**, so the hook
  re-derives the face normal from `obj.Shape.mesh()` rather than undoing the display
  hook's placement bake (a rigid placement preserves the sign and the distance).
- **Preview and scope.** The stem and readout are live; the solid appears on release.
  A ghost prism is a deliberate follow-up. Non-planar faces, a target-aware triad and
  stable naming are out.

## 5. Select mode: body vs face

There is no whole-body mode: a Part click is tagged, so it always resolves to a face
(`contributions.rs:22-28`); a body is reachable only from the tree.

| Option | Discoverability | Cost in the current pick path | Verdict |
| --- | --- | --- | --- |
| `Ctrl`/`Cmd`-click | familiar from trees | thread modifiers into selection (they only reach drags, `inventor_view.rs:1663-1666`); clashes with tree toggle and rotation-detent bypass | reject |
| Header `Select: Body/Face` | explicit, persistent | one flag gating `part_name` in `selection_ref` (`inventor_view.rs:1560-1563`); a second header mode | fallback |
| Wheel "Select body" | only after right-click | a command plus promotion; heavy for a frequent switch | reject |
| `Alt`-click | common in CAD | window managers grab it on Linux | reject |
| **Double-click the face** | spatial, no mode to remember | click timing in the viewport; first click selects the face, second upgrades | **built** |
| Select-through filter | invisible | a filter model with no clear default | reject |

**Built (2026-10-26): a double-click on a face or edge upgrades to
`SelectionRef::whole(entity)`; a single click returns to sub-element scope.** The viewport
uses gpui's `MouseDownEvent::click_count` (no timing heuristic of its own), carries it on
`SelectionEvent::Picked`, and the shell's `click_target` upgrades `click_count >= 2` to
the body. A later single click on a face returns to that face; a third click stays whole
(no toggling). The triad still arms at the object's own origin for either scope.

## Contradictions to reconcile

| # | Conflict | Recommendation |
| --- | --- | --- |
| 1 | "Contact point" is where the gizmo *belongs* (`gizmo-manipulators.md:50-58`, `selection-gizmos-and-forms.md:37-38`), and code makes it `frame.origin` (`shell.rs:1506-1516`); no page defines the **rotation** pivot, yet the rings rotate about that origin | split anchor (translation) from pivot (rotation); §2 |
| 2 | "Never the interior centroid" (`selection-gizmos-and-forms.md:119-121`) vs the multi-face **area-weighted centroid** (`gizmo-manipulators.md:56`) | both hold for different jobs: never the translate anchor; the centroid is the rotate/push-pull pivot; §2 |
| 3 | The triad "targets the whole item even when the selection names a sub-element" (`contributions.rs:22-28`, `gizmo-manipulators.md:779-782`), while sub-element transforms are called the next thing (`selection-gizmos-and-forms.md:122-124`, `329-333`) and `to_interaction` drops `element` (`gizmo.rs:461-473`) | triad stays whole-body; a face gets a manipulator, never a face triad, until the commit carries the element; §4 |
| 4 | `SelectionDatum` promised rotation handling (`shell.rs:337-346`) | the type is gone: the triad hugs the object's own origin, so there is no stored click point to rotate; the session user origin (re-pivot, landed 2026-10-26) replaces it; §2 |
| 5 | "Clicking an already-selected object does not replace a multi-selection" (`selection-gizmos-and-forms.md:55-56`), but every viewport pick makes a one-item selection (`shell.rs:1233-1251`) | correct the doc; viewport multi-select is out of scope, and the triad is suppressed for a multi-selection; §2 |
| 6 | Rust `GizmoMode::default()` is `Translate` (`gizmo.rs:110-114`); the Python `TransformGizmo` binding defaults to `Universal` (`ferrocad_stage_py/src/gizmo.rs:104`) | Translate is the app default; align or document the binding default; §1 |
| 7 | "The triad's centre remains pass-through" (`selection-gizmos-and-forms.md:260-261`; `gizmo-manipulators.md:192-198`) vs the origin drag | resolved: the centre is the 10 px origin handle; while that handle is disabled (the translation HUD) the centre passes through again; §3 |
| 8 | §19 rejected a `Gizmo` header toggle (`gizmo-manipulators.md:767-771`) | the control that landed is **configuration** (show, providers, settings, reset), not a mode switch — consistent with “the header means how the viewport is drawn”; the wheel owns the verbs; §1 |

## 6. Minimum implementation order

| # | Slice | Acceptance | Touches |
| --- | --- | --- | --- |
| 0 | **Origin anchor — landed 2026-10-25** — `StageMesh.origin` → `ShapeItem::anchor`; the triad hugs it; the click no longer re-anchors | The toy slab's triad sits at its placement point, not the bbox corner; a move keeps it on the origin; a mesh without an origin falls back to the corner | `ferrocad_stage/src/shape.rs`, `ferrocad_gpui/{display,contributions,shell}.rs`, both app scripts |
| 1 | **Rotate mode** — `Shell::gizmo_mode` applied after the factory; `App_Rotate` radial verb via a `{"view": …}` mode request; header mirror | A body shows rings only; a 90° ring drag commits one `transform/apply`; the mode survives re-arm; Translate shows no rings | `ferrocad_gpui/src/shell.rs`, `view.rs`; `python/ferrocad_shell/__init__.py`; `chrome.rs` |
| 2 | **Multi-selection guard** — no triad while more than one object is selected (the commit names one target) | Selecting two bodies shows no triad and the wheel's `Move` still moves both | `ferrocad_gpui/src/shell.rs` |
| 3 | **Re-pivot — landed 2026-10-26** — `GizmoTarget::Origin`, the snap pipeline, `GizmoOrigin`, session user origin, disabled during the Move HUD | The centre snaps to a vertex/edge mid/face centre/grid; a ring drag rotates about it; a new selection clears it; the Move HUD disables it | `ferrocad_stage/src/gizmo.rs`, `ferrocad_viewport/src/inventor_view.rs`, `shell.rs` |
| 4 | **Select mode — landed 2026-10-26** — double-click a face/edge upgrades to the whole body; `MouseDownEvent::click_count` rides on `SelectionEvent::Picked` | Click selects `Face6`; double-click selects the body; a click on another face returns to that face; the triad arms at the object origin either way | `ferrocad_viewport/src/inventor_view.rs`; `shell/{viewport,picking}.rs` |
| 5 | **Push/pull stem — landed 2026-10-26** — Part manipulator + edition hook; stem/pill live, solid on release, one transaction | A face shows an amber normal stem; dragging commits an extrusion/pocket that recomputes; undo restores; non-planar faces show no stem | `ferrocad_part_gui/src/push_pull.rs`; `python/ferrocad_parametric/__init__.py` |

Slices 1–2 are the transform slice proper; 3 depends on 1; 4 is gated on 1; 5 does not
depend on 4 (a face is already the default) but follows 2 so the centroid helper is shared.

## 7. Structure for several gizmos

The current container is one `TransformGizmo` with a `GizmoMode` (`Translate` /
`Rotate` / `Universal`), hosted through `Contributions::gizmo_for` →
`Option<TransformGizmo>` → `viewport.set_gizmo`. That is enough for the triad and not
enough for what comes next (bevel, push/pull, axis handles): they differ in hit rules,
constraints, previews and commits, so they should not grow inside `TransformGizmo`.

**Recommendation: keep one *type* per gizmo kind, host a set, compose by mode.**
Push/pull already landed *outside* this container, as the manipulator route (§4) — the
rehearsal for the kinds split is still the toy height grip below.

- **Kinds are independent.** `translate`, `rotate` and `universal` become separate
  constructors (or types) over the same frame math, each with its own handle set, hit
  test and drag solver — so each is unit-tested alone, and `universal` is a composition
  rather than a third implementation.
- **The registry holds a set.** `Contributions::gizmo_for` becomes `gizmos_for → Vec`, and
  the viewport hosts a list with a stable priority (most recently registered first, the
  same rule manipulators use). A workbench gizmo and the triad then coexist without
  one shadowing the other.
- **The edition composes, the wheel preselects.** An edition registers the kinds it
  offers; a wheel verb (`Move`, `Rotate`) narrows the set to that kind for the session,
  and a new selection keeps the mode until another verb changes it. No verb ⇒ the
  edition's default set.
- **The height grip is the rehearsal.** It is a `Manipulator` today (`height_grip`),
  coupled to the toy's `Height` property and drawn with `GizmoStyle`. It should become an
  **axis-handle gizmo** once the kinds/trait split exists: same origin policy, same
  readout, same commit route, but owned by the gizmo registry instead of the manipulator
  registry — which is what makes the Part push/pull stem cheap.

**Built (2026-10-27).** `GizmoKind` (`Translate`/`Rotate`/`Universal`) is the kind
vocabulary, ordered specific → composed; `TransformGizmo::{translate,rotate,universal}`
build each kind over the same frame math, and `GizmoSet` hosts them (`gizmos()`,
`narrow(session)`, ordered `hit`). `GizmoFactory::kinds()` declares what a provider
offers; `Contributions::gizmos_for(selection, stage) -> Vec<TransformGizmo>` orders the
most recently registered factory first and the factory's kinds specific before
`universal`; `with_gizmo_kinds` composes a subset. The shell's `armed_gizmos` hosts the
set and `armed_gizmo` narrows it with the sticky mode.

The toy height grip has a `HeightGizmo` adapter (`kind == Translate`, `AxisZ`) as the
proof. The viewport now hosts the whole `GizmoSet`: drawing, hit/hover/press/drag and the
re-pivot record the grabbed entry's index, so a lower-priority gizmo cannot steal a
gesture, and `set_gizmo_mode` rewrites the **session** kind of every hosted entry rather
than replacing the one gizmo (`GizmoSet::set_session`). An entry that cannot serve the
session stays hosted but unoffered, so no registered kind is lost; a verb the edition
did not register offers nothing rather than substituting another kind. The grip now
builds the same `GizmoCommitEvent` a translate triad does (`GizmoCommitEvent::
to_interaction_with` -> `toy_height`/`set`), so move and height share one commit path.

The remaining step is the trait-object host: `GizmoFactory`/`GizmoSet` still accept only
`TransformGizmo`, so a workbench gizmo with a different interface is registered through
the manipulator registry even though its commit already rides the shared route.
