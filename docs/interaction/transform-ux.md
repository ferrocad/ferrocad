# Transform interaction: modes, pivots, re-pivoting and push/pull

Status: **design note (2026-10-25), nothing implemented.** "Today" means at `d3c5853`;
"unhosted" means the code exists but no host selects it; everything else is proposed.
Companions: [`gizmo-manipulators.md`](gizmo-manipulators.md) (triad math and hit test),
[`selection-gizmos-and-forms.md`](selection-gizmos-and-forms.md) (wheel, form, deferred
drag), [`topology-selection.md`](topology-selection.md) (sub-element names),
[`axis-origin-grid.md`](axis-origin-grid.md) (the face frame).

## 0. What exists

| Piece | State | Where |
| --- | --- | --- |
| Triad handles | Translate today; Rotate and Universal **defined, unhosted** | `gizmo.rs:109-119`, `1232-1240`; only `translate_gizmo` (`contributions.rs:33-40`) is hosted |
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
| Keyboard | none in this slice | `shortcuts.rs:64-76` refuses printable keys without a chord; a later `Ctrl+Shift+R` beside `Ctrl+Shift+M` is safe, a bare `R` is not |

- The mode is **sticky** across re-arm and selection; only a document open (or `Move`)
  resets it. A rotate commit stays in rotate mode instead of snapping back to arrows.
- **`Universal` is an overview, not a default.** Its handles are real, but priority is
  axis > plane > ring (`gizmo-manipulators.md:191`), so rings are hard to grab; it must
  never be the only route to rotate.
- A face selection still transforms the **body**; the commit's status must say
  "Body (Face6)", not "Face6".

## 2. Pivot policy

The anchor is `frame.origin`, and that same point is the rotation centre. Split the jobs:
the **anchor** is where the triad is drawn and the translation is grabbed (pivot-free,
so only an attention anchor); the **pivot** is what rotation turns about.

**Recommendation: the visible origin is the pivot for the active mode; switching modes
re-places it at the mode's default unless a user origin is set (§3).** The move is
visible; never keep the pivot hidden behind a fixed anchor.

| Mode | Whole body | Face | Edge / vertex | Multi sub-element | Tree/command |
| --- | --- | --- | --- | --- | --- |
| Translate | click contact (today) | click contact (today) | click contact (today) | **no triad** until the commit fans out | base corner (today) |
| Rotate | **bbox centre** | face centroid | edge midpoint / vertex | area-weighted centroid (after fan-out) | bbox centre |
| Push-pull (§4) | — | face centroid, `+N` | — | — | — |

Centroids need no kernel call: the stage item carries `parts`, positions and triangles
(`shape.rs:257-307`), and the face workplane already derives a centroid and averaged
normal (`axis-origin-grid.md:99-102`). The bbox centre is a one-pass companion to
`body_corner` (`contributions.rs:47-60`). A face selection rotates the **body** about the
face centroid; to rotate about the body centre, select the body (§5).

**The `SelectionDatum` rule.** Keep offset-from-base-corner, but make it survive rotation:
on a successful `transform/apply` commit, transform the stored world anchor by the commit
matrix and re-express it against the **new** corner (`M · (corner + offset) − corner'`).
One rule covers translation and rotation, and makes the comment at `shell.rs:337-346`
true. A resize (the toy height grip) is not a matrix; it keeps the corner offset, the
known approximation. A tree/command selection is the base corner in Translate and the
bbox centre in Rotate. A **user origin** (§3) follows a translation and stays fixed under
a rotation about itself; a new selection or a wheel promotion clears it.

**Multi-selection is not a triad case yet.** `arm`/`GizmoFactory::create` see one
`SelectionRef` (`contributions.rs:84-87`) and the commit names one target, so a triad
would move only the primary. Until the commit fans out, suppress the triad; the wheel's
`Move` still moves the whole selection via `_move_selection` (`ferrocad_shell/__init__.py:883-899`).

## 3. Origin drag (re-pivot)

**Recommendation: a small centre handle in `Rotate` and `Universal` only; pass-through in
`Translate`.** Dragging the origin is worthless while only translation exists, and it is
the only way to choose a rotation centre that is not the mode default.

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
  world point; the shell stores `SelectionDatum { source: User, point }`. No transaction,
  no undo, no recompute. `Esc` restores the pre-drag frame and datum.
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
| 4 | `SelectionDatum` says "a rotation would rotate the offset too" (`shell.rs:337-346`), but `resolve` only adds the offset to the new corner (`shell.rs:367-376`) | apply the commit matrix to the stored anchor on re-arm; §2 |
| 5 | "Clicking an already-selected object does not replace a multi-selection" (`selection-gizmos-and-forms.md:55-56`), but every viewport pick makes a one-item selection (`shell.rs:1233-1251`) | correct the doc; viewport multi-select is out of scope, and the triad is suppressed for a multi-selection; §2 |
| 6 | Rust `GizmoMode::default()` is `Translate` (`gizmo.rs:110-114`); the Python `TransformGizmo` binding defaults to `Universal` (`ferrocad_stage_py/src/gizmo.rs:104`) | Translate is the app default; align or document the binding default; §1 |
| 7 | "The triad's centre remains pass-through" (`selection-gizmos-and-forms.md:260-261`; `gizmo-manipulators.md:192-198`) vs the origin drag | pass-through in Translate only; a 10 px origin handle in Rotate/Universal; §3 |
| 8 | §19 rejects a `Gizmo` header toggle because "the header means how the viewport is drawn" (`gizmo-manipulators.md:767-771`), while a `Gizmo` control is in flight | consistent if it selects a mode/presentation (Translate/Rotate/Both), not an on/off enable; the wheel keeps the verbs; §1 |

## 6. Minimum implementation order

| # | Slice | Acceptance | Touches |
| --- | --- | --- | --- |
| 1 | **Rotate mode** — `Shell::gizmo_mode` applied after the factory; `App_Rotate` radial verb via a `{"view": …}` mode request; header mirror | A body shows rings only; a 90° ring drag commits one `transform/apply`; the mode survives re-arm; Translate shows no rings | `ferrocad_gpui/src/shell.rs`, `view.rs`; `python/ferrocad_shell/__init__.py`; `chrome.rs` as the in-flight control lands |
| 2 | **Pivot policy** — bbox centre, face/edge/vertex centroids, mode-dependent pivot, datum transformed by the commit matrix; no triad for multi-selection | Rotating a body keeps its bbox centre fixed; a face selection rotates the body about the face centroid and says so; a translate after a rotate keeps the anchor on the same surface point | `ferrocad_gpui/src/contributions.rs`, `shell.rs` |
| 3 | **Re-pivot** — centre handle, snap reuse, `GizmoOrigin` event, user-origin persistence, disabled during the Move HUD | The centre snaps to a vertex/edge mid/face centre; a ring drag rotates about it; a new selection clears it; the Move HUD disables it | `ferrocad_stage/src/gizmo.rs`, `ferrocad_viewport/src/inventor_view.rs`, `shell.rs` |
| 4 | **Select mode** — double-click a face/edge upgrades to the whole body | Click selects `Face6`; double-click selects the body and re-pivots at its centre; a click on another face returns to that face | `inventor_view.rs`; `shell.rs` (status, datum clear) |
| 5 | **Push/pull stem** — Part manipulator + edition hook; stem/pill live, solid on release, one transaction | A face shows an amber normal stem; dragging commits an extrusion/pocket that recomputes; undo restores; non-planar faces show no stem | `ferrocad_part_gui/src/…`; `python/ferrocad_parametric/__init__.py`; no `ferrocad_occt` change |

Slices 1–2 are the transform slice proper; 3 depends on 1; 4 is gated on 1; 5 does not
depend on 4 (a face is already the default) but follows 2 so the centroid helper is shared.
