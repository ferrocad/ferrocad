# Transform interaction: modes, pivots, re-pivoting and push/pull

Status: **design note (2026-10-25).** The mode set and the wheel's `Rotate` verb
landed 2026-10-26; re-pivot, push/pull and body/face selection remain proposed.
"Today" means at `d3c5853`; "unhosted" means the code exists but no host selects it;
everything else is proposed.
Companions: [`gizmo-manipulators.md`](gizmo-manipulators.md) (triad math and hit test),
[`selection-gizmos-and-forms.md`](selection-gizmos-and-forms.md) (wheel, form, deferred
drag), [`topology-selection.md`](topology-selection.md) (sub-element names),
[`axis-origin-grid.md`](axis-origin-grid.md) (the face frame).

## 0. What exists

| Piece | State | Where |
| --- | --- | --- |
| Triad handles | Translate is the default; **Rotate is hosted** through the shell's session mode; Universal is defined but not surfaced | `gizmo.rs:109-119`, `1232-1240`; `contributions.rs:33-40`; `shell.rs` `gizmo_mode`/`set_gizmo_mode` |
| Click anchor / `SelectionDatum` | today, translation-only | `shell.rs:1506-1516`, `337-377`, `1621-1663`; `SelectionAnchor` at `inventor_view.rs:52-63` |
| Face/edge tags, `Move` + Δ form, feature snaps | today | `inventor_view.rs:1556-1564`; `ferrocad_shell/__init__.py:837-903`; `snap.rs:20-46` |
| Face-normal push/pull | proposed | kernel pieces exist (§4) |

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


**Multi-selection is not a triad case yet.** `arm`/`GizmoFactory::create` see one
`SelectionRef` (`contributions.rs:84-87`) and the commit names one target, so a triad
would move only the primary. Until the commit fans out, suppress the triad; the wheel's
`Move` still moves the whole selection via `_move_selection` (`ferrocad_shell/__init__.py:883-899`).

## 3. Origin drag (re-pivot)

**Recommendation: a centre handle that moves the working origin, for both translate and
rotate; ship it once rotation exists.** Dragging it is the only way to rotate about a
point that is not the object's origin, and the same gesture gives translation a datum
that is not the origin. It is deferred because, while the triad only translates, the
pivot cannot change the result — the object origin (§2) is already an honest indicator.

- **Hit.** A separate `Origin` target, not a transform handle. Project a 10 px disc
  (`snap::FEATURE_PIXELS`); priority **origin > axis > plane > ring**, appended after
  `GizmoHandle::ALL` so pick ids stay stable (`gizmo.rs:155-212`). It sits inside the
  stems' excluded 18 % (`gizmo.rs:228-231`), stealing nothing.
- **Snap.** Reuse the viewport's snap candidates (`snap.rs:20-46`): a `Vertex`, `EdgeMid`,
  `Centre` or `Origin` within 10 px wins, else the ray's point on the hovered body, else
  the workplane grid; the readout names the snap. Do not invent a second snap set.
- **Frame and release.** Move the **real** frame live: `set_display_offset` exists so a
  transform drag's math cannot follow its drawing (`gizmo.rs:581-593`), but a pivot drag
  has no solver. On release a new `ViewportEvent::GizmoOrigin { point, kind }` carries the
  world point; the shell stores it as a session **user origin** (`Option<Vector3>`, not
  undoable, not a document property), cleared by a new selection or a wheel promotion and
  applied instead of the object origin while it is set. `Esc` restores the pre-drag
  frame and clears it.
- **With `Move`'s deferred form.** Disable the origin handle while a
  `preview: "translation"` form is open: the pivot is harmless to translation arithmetic,
  but the HUD is anchored to the visible origin (`shell.rs:2219-2224`) and must not slide
  under the user's typing. Closing the form re-enables it.

## 4. Face-normal alignment and push/pull

**Recommendation: push/pull is a Part manipulator, not a mode of the triad.** The triad's
commit is a 16-value placement transform and drops the element (`gizmo.rs:461-473`), so it
cannot express "extrude `Face6`". The manipulator seam already carries the element
(`interaction.rs:49-50`) and the shell passes it to the app script (`python.rs:529-552`),
which keeps the triad kernel-free.

- **Handle.** A single amber stem from the face centroid along `+N` (the normal is already
  in `SelectionAnchor.normal`; the stored `SelectionDatum.normal` is currently unused, so
  this is its first consumer). Reuse the axis solver, screen fallback and `Linear` readout
  (`gizmo.rs:60-95`, `364-383`); amber is the push-pull token (`gizmo.rs:49`).
- **Commit.** `InteractionCommit::new("push_pull", entity, "extrude" | "pocket")` with
  `.with_element("Face6")`, `.with_values([distance])` and the readout — the neutral route
  already used by fillet/chamfer and the toy height grip.
- **Kernel reality.** `Part::Extrusion` exists and resolves a single sub-shape `Base`
  (`ferrocad_part_py/src/solids.rs:525`, `596-612`); `Part::Fuse`/`Cut` exist
  (`ferrocad_part/src/containers.rs:16-19`); the prism is bound
  (`ferrocad_occt/src/backend.rs:1391-1410`). The edition's `interaction` hook
  (`ferrocad_parametric/__init__.py:106-163`) can extrude and combine in one transaction,
  with no new `ferrocad_occt` method. The real work is a planar-face test (hide the handle
  otherwise), mapping the normal/distance into the body's **shape-local** frame (the display
  hook bakes `Placement`, `ferrocad_parametric/__init__.py:81-104`), and an honest preview.
- **Preview and scope.** First slice: the stem and pill are live, the solid appears on
  release. A ghost prism (extruded face triangles) is a cheap follow-up; a boolean ghost
  needs a per-tick kernel recompute and waits. Non-planar faces, a target-aware triad and
  stable naming (`topology-selection.md:343-353`) are out.

## 5. Select mode: body vs face

There is no whole-body mode: a Part click is tagged, so it always resolves to a face
(`contributions.rs:22-28`); a body is reachable only from the tree.

| Option | Discoverability | Cost in the current pick path | Verdict |
| --- | --- | --- | --- |
| `Ctrl`/`Cmd`-click | familiar from trees | thread modifiers into selection (they only reach drags, `inventor_view.rs:1663-1666`); clashes with tree toggle and rotation-detent bypass | reject |
| Header `Select: Body/Face` | explicit, persistent | one flag gating `part_name` in `selection_ref` (`inventor_view.rs:1560-1563`); a second header mode | fallback |
| Wheel "Select body" | only after right-click | a command plus promotion; heavy for a frequent switch | reject |
| `Alt`-click | common in CAD | window managers grab it on Linux | reject |
| **Double-click the face** | spatial, no mode to remember | click timing in the viewport; first click selects the face, second upgrades | **recommend** |
| Select-through filter | invisible | a filter model with no clear default | reject |

**Recommendation: double-click a face or edge to upgrade to
`SelectionRef::whole(entity)`; a single click returns to sub-element scope.** The viewport
keeps the last selection press `(Instant, point, PickId)`; a second press within ~400 ms
and ~6 px on the same entity reports the body. Nothing below the viewport changes:
`selection_ref` still resolves the part; only an upgrade branch in `selection_event`
(`inventor_view.rs:2056-2084`) is new. Gate it after slice 1: its purpose is a sane body
pivot for Rotate.

## Contradictions to reconcile

| # | Conflict | Recommendation |
| --- | --- | --- |
| 1 | "Contact point" is where the gizmo *belongs* (`gizmo-manipulators.md:50-58`, `selection-gizmos-and-forms.md:37-38`), and code makes it `frame.origin` (`shell.rs:1506-1516`); no page defines the **rotation** pivot, yet the rings rotate about that origin | split anchor (translation) from pivot (rotation); §2 |
| 2 | "Never the interior centroid" (`selection-gizmos-and-forms.md:119-121`) vs the multi-face **area-weighted centroid** (`gizmo-manipulators.md:56`) | both hold for different jobs: never the translate anchor; the centroid is the rotate/push-pull pivot; §2 |
| 3 | The triad "targets the whole item even when the selection names a sub-element" (`contributions.rs:22-28`, `gizmo-manipulators.md:779-782`), while sub-element transforms are called the next thing (`selection-gizmos-and-forms.md:122-124`, `329-333`) and `to_interaction` drops `element` (`gizmo.rs:461-473`) | triad stays whole-body; a face gets a manipulator, never a face triad, until the commit carries the element; §4 |
| 4 | `SelectionDatum` promised rotation handling (`shell.rs:337-346`) | the type is gone: the triad hugs the object's own origin, so there is no stored click point to rotate; a user origin replaces it when re-pivot lands; §2 |
| 5 | "Clicking an already-selected object does not replace a multi-selection" (`selection-gizmos-and-forms.md:55-56`), but every viewport pick makes a one-item selection (`shell.rs:1233-1251`) | correct the doc; viewport multi-select is out of scope, and the triad is suppressed for a multi-selection; §2 |
| 6 | Rust `GizmoMode::default()` is `Translate` (`gizmo.rs:110-114`); the Python `TransformGizmo` binding defaults to `Universal` (`ferrocad_stage_py/src/gizmo.rs:104`) | Translate is the app default; align or document the binding default; §1 |
| 7 | "The triad's centre remains pass-through" (`selection-gizmos-and-forms.md:260-261`; `gizmo-manipulators.md:192-198`) vs the origin drag | pass-through until re-pivot ships; then a 10 px origin handle in every mode; §3 |
| 8 | §19 rejected a `Gizmo` header toggle (`gizmo-manipulators.md:767-771`) | the control that landed is **configuration** (show, providers, settings, reset), not a mode switch — consistent with “the header means how the viewport is drawn”; the wheel owns the verbs; §1 |

## 6. Minimum implementation order

| # | Slice | Acceptance | Touches |
| --- | --- | --- | --- |
| 0 | **Origin anchor — landed 2026-10-25** — `StageMesh.origin` → `ShapeItem::anchor`; the triad hugs it; the click no longer re-anchors | The toy slab's triad sits at its placement point, not the bbox corner; a move keeps it on the origin; a mesh without an origin falls back to the corner | `ferrocad_stage/src/shape.rs`, `ferrocad_gpui/{display,contributions,shell}.rs`, both app scripts |
| 1 | **Rotate mode** — `Shell::gizmo_mode` applied after the factory; `App_Rotate` radial verb via a `{"view": …}` mode request; header mirror | A body shows rings only; a 90° ring drag commits one `transform/apply`; the mode survives re-arm; Translate shows no rings | `ferrocad_gpui/src/shell.rs`, `view.rs`; `python/ferrocad_shell/__init__.py`; `chrome.rs` |
| 2 | **Multi-selection guard** — no triad while more than one object is selected (the commit names one target) | Selecting two bodies shows no triad and the wheel's `Move` still moves both | `ferrocad_gpui/src/shell.rs` |
| 3 | **Re-pivot** — centre handle in every mode, snap reuse, `GizmoOrigin` event, session user origin, disabled during the Move HUD | The centre snaps to a vertex/edge mid/face centre; a ring drag rotates about it; a new selection clears it; the Move HUD disables it | `ferrocad_stage/src/gizmo.rs`, `ferrocad_viewport/src/inventor_view.rs`, `shell.rs` |
| 4 | **Select mode** — double-click a face/edge upgrades to the whole body | Click selects `Face6`; double-click selects the body; a click on another face returns to that face | `inventor_view.rs`; `shell.rs` (status) |
| 5 | **Push/pull stem** — Part manipulator + edition hook; stem/pill live, solid on release, one transaction | A face shows an amber normal stem; dragging commits an extrusion/pocket that recomputes; undo restores; non-planar faces show no stem | `ferrocad_part_gui/src/…`; `python/ferrocad_parametric/__init__.py`; no `ferrocad_occt` change |

Slices 1–2 are the transform slice proper; 3 depends on 1; 4 is gated on 1; 5 does not
depend on 4 (a face is already the default) but follows 2 so the centroid helper is shared.

## 7. Structure for several gizmos

The current container is one `TransformGizmo` with a `GizmoMode` (`Translate` /
`Rotate` / `Universal`), hosted through `Contributions::gizmo_for` →
`Option<TransformGizmo>` → `viewport.set_gizmo`. That is enough for the triad and not
enough for what comes next (bevel, push/pull, axis handles): they differ in hit rules,
constraints, previews and commits, so they should not grow inside `TransformGizmo`.

**Recommendation: keep one *type* per gizmo kind, host a set, compose by mode.**

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
