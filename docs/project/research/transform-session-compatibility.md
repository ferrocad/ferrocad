# Recon: is the tree compatible with the transform session and the tool loadout?

Status: **reconnaissance (2026-10-30)**, read-only. Read against `d5bc31e`, after the plan was
split into [`../tasks/transform-session.md`](../tasks/transform-session.md) and
[`../tasks/tool-loadout.md`](../tasks/tool-loadout.md). No code was changed to answer these
questions; every finding cites the line that decides it.

**Headline:** the *display* half is nearly free (the shell already keeps a base stage and an
overlay), the *model* half is feasible but has no caller from the shell today (the shell has no
`Document`), and the loadout is **smaller** than the plan assumed (it needs a filtered
`SelectionRef`, not a new manipulator kind). Five structural changes are needed; §6 lists them.

---

## 1. Verdicts

| Piece | Verdict | Why |
| --- | --- | --- |
| `PendingEdit` never reaching a file | ✅ as designed | persistence is `SavedDocument`/`SavedObject`, built explicitly (`document/native.rs:44-63`); `Document` itself has no `Serialize` |
| `stage` / `abort` / `commit` semantics | ✅ as designed | `set_property` is the only recording path, and `record_change` is a no-op outside a transaction (`document/transaction.rs:357-361`) |
| `EditView` as the read path | ⚠️ additive, not enforceable | `Document::object()`/`object_mut()` hand out the raw store (`document/mod.rs:215-221`) |
| `TransformSession` on `Placement` | ❌ wrong base value | document items carry **world-space** geometry and an identity `placement`; the datum is `ShapeItem::origin` (`python/ferrocad_parametric/__init__.py:86-93`) |
| Session owned by the shell | ⚠️ needs a new viewport input | the viewport applies the preview itself today (`inventor_view.rs:3642-3648`) |
| Ghost = base vs effective | ✅ already has a home | `last_stage` + `stage_with_previews` + `preview_generation` (`shell/viewport.rs:41-96`) |
| `PendingEdit` reachable from the shell | ❌ no path exists | the shell has entity ids and object **names**, never an `ObjectId` or a `Document` (`display.rs:110-113`) |
| Loadout narrowing | ✅ smaller than planned | `arm()` already hosts one manipulator; the element is what decides (`shell/viewport.rs:568-604`) |
| `SelectFilter` | ✅ one rewrite point | `SelectionRef { object, elements }` (`ferrocad_geom/…:583-604`) |

---

## 2. Core document (`ferrocad_core`) — A0

**Compatible, with one naming hazard.**

- **Persistence is safe by construction.** `Document` holds `objects: BTreeMap<ObjectId,
  DocumentObject>` (`document/mod.rs:81`) and neither type derives `Serialize`; a file is built
  from `SavedDocument`/`SavedObject` in `native.rs:44-63`. A runtime `pending` field on
  `Document` therefore cannot leak into `.ferrocad` or `.FCStd` — no `#[serde(skip)]` needed,
  and no change to either writer.
- **The three operations are expressible with what exists.** `commit_pending` must open a named
  transaction around its `set_property` calls, because `record_change` only records when a
  transaction is active or booked (`document/transaction.rs:357-361`) — a bare `set_property`
  mutates and is silently not undoable. `abort_pending` needs nothing: the document was never
  written.
- **`set_property` runs the type hooks** (`on_property_change` veto at
  `document/mod.rs:526-537`, `on_property_changed` after the write). Replaying through
  `set_property` at commit is therefore the only way a feature's derived properties stay in
  sync; a staging path that wrote the container directly would skip them.
- ⚠️ **Name collision.** `TransactionManager` already has a `pending: Option<Pending>`
  meaning "a transaction requested by `open`, not yet made active"
  (`document/transaction.rs:96-106, 119-148`, exposed as `has_pending()`). Two different
  "pending" concepts in one module will read badly. Rename the new type (e.g. `StagedEdit`) or
  rename the internal one (`booked`).
- ⚠️ **The read view cannot be enforced yet.** `object()` and `object_mut()` are public
  (`document/mod.rs:215-221`), so any caller can read a committed value while a preview is live,
  or write one without staging. `EditView` is the right shape, but the guarantee is "the display
  and snap paths go through it", not "nothing else can". Making it a guarantee means making
  `object_mut` crate-private — a wider change than this stage should carry.
- ℹ️ `property_versions` is bumped by `set_property` only (`document/mod.rs:551-553`). A staged
  value does not bump it, so version-keyed caches (geometry handles) keep seeing the committed
  version during a preview. Fine for `Placement`; for A3's scrubbed properties it is the reason
  the preview must stay display-side.

## 3. Stage (`ferrocad_stage`) — A1

**The item already has a base and a preview, but neither is a `Placement`.**

- `ShapeItem { placement, temporary_transform, origin, … }`, composed as
  `world = placement * temporary` (`shape.rs:121-127, 543-547`). That is exactly a base/effective
  pair — for **rendering**.
- ❌ **Document items do not use `placement`.** `DisplaySync::build` calls
  `ShapeItem::from_tessellation(entity, …)`, whose placement is the constructed default
  (`shape.rs:352-354`), and the mesh it is given is already world-space: the parametric provider
  runs `_apply_object_placement` and then reports the placement's base as `origin`
  (`python/ferrocad_parametric/__init__.py:86-93`).
- **Consequence for the plan:** `TransformSession::begin` must take a world-space datum
  (`ShapeItem::origin`, falling back to `anchor()` — `shape.rs:650-660`), and its delta/effective
  must be a `Matrix4`/`Vector3`, not an `Option<Placement>`. Rotation is already expressible
  (`temporary_transform` is a full matrix), even though no `Placement` is involved.
- ✅ `Stage::item(id)`, `items_mut()`, `anchor()` and `entity_id()` are the accessors the session
  and the tape need, and `RenderQueue` overlays already exist (`workplane_stem` is the pattern to
  copy for the tape).
- Next to nothing in this crate has to move: the session is a new module, and `ShapeItem` keeps
  its API.

## 4. Viewport (`ferrocad_viewport`) — A1, A2

**The viewport owns the preview; the session wants to.**

- The preview is applied inside the viewport on every pointer tick
  (`handle_pointer_with_modifiers` → `apply_gizmo_preview(Some(…))` → `apply_preview_overlays`
  → `set_temporary_transform`), and *also* re-applied when a host sends a snapshot
  (`preview_translation`). Two writers already exist; the shell's dialog is a third.
- `preview_home` is broader than transforms: it snapshots **gizmo origins and manipulator
  anchors** as well as item transforms (`inventor_view.rs:3490-3535`). So A2 cannot delete it
  wholesale — only its item-transform branch. Gizmo/manipulator anchoring keeps working through
  the existing overlay path, which is what the stem and the grips use.
- ⚠️ **Ownership and latency.** The pointer is handled in the viewport, so a shell-owned session
  must be fed a tick and pushed back. The round trip already exists and is proven
  (`ViewportEvent::TransformPreview` is emitted from the release path at
  `inventor_view.rs:2991-3009`, and the shell re-previews on it), but today the viewport also
  updates locally so there is no visible lag. If the viewport stops writing the preview
  (A2's goal), either the shell must push the snapshot back inside the same update (gpui emits
  are queued until the update returns, so this is possible) or the drag visually lags one event
  hop. **Structural change: add an explicit `set_session(Option<SessionSnapshot>)` input and
  make the renderer read only from it.**
- The session type itself should live in `ferrocad_stage` (headless, shared by toy/parametric
  tests), not in the viewport — the viewport is generic over `InventorView` and must not learn
  what a document is.

## 5. Shell and Python (`ferrocad_gpui`, `python/`) — A0, A2, U1

**The display half already matches the plan; the model half has no bridge.**

- ✅ **Base stage + overlay is already the pattern.** `set_stage` keeps `last_stage` and merges
  `ferrocad_stage_py::preview_items()` over it, keyed by a generation counter
  (`shell/viewport.rs:41-96`; the cell is `ferrocad_stage_py/src/view.rs:87-118`). A ghost is
  literally `last_stage` (committed) vs the merged stage (effective) — the data for the tape and
  for any faded "uncommitted" rendering is already in memory.
- ❌ **The shell cannot stage a property.** It knows `entity → object name`
  (`display.rs:110-113`) and world-space geometry. It has no `Document`, no `ObjectId`, and no
  `Placement`; the commit path is entirely Python (`_move_selection` reads `obj.Placement` and
  opens its own transaction, `ferrocad_shell/__init__.py:1193-1209`).
  **Consequence:** a `PendingEdit` created in Rust would have no caller. It has to be reachable
  through the facade — `PyDocument` already wraps `Arc<Mutex<CoreDocument>>`
  (`ferrocad_py/src/lib.rs:1973-1983`), so the cheapest shape is a pending **handle** (like a
  dialog token) that the shell holds opaquely:
  `transform_begin(object_name) -> id`, `transform_stage(id, dx, dy, dz)`,
  `transform_commit(id)`, `transform_abort(id)`.
- ⚠️ **Commit re-tessellates.** `after_document_edit` calls `stage_meshes()` (a kernel crossing)
  and rebuilds the stage, then re-arms (`shell/viewport.rs:790-818`). That is fine for a
  checkpoint (`Enter`) but confirms the plan's scope: a ghost of a *recomputed* shape would need
  a tessellation per tick, so only transforms preview.
- ℹ️ `App_Move`'s no-arg form returns the floating HUD (`ferrocad_shell/__init__.py:1134-1157`),
  and `on_accept` is `_move_selection`. U1 keeps `Activated(dx, dy, dz)` and replaces the dialog
  with the pill, so only the no-arg branch changes.

## 6. Loadout (`U4`) — smaller than planned

**No trait change needed.**

- Only **one** compiled manipulator is hosted per selection: `arm()` asks
  `contributions.manipulator_for(reference, stage)` for a single `Box<dyn Manipulator>` and then
  appends mod-declared handles (`shell/viewport.rs:528-604`;
  `contributions.rs:232-241`).
- The triad is already retired by a heuristic: `!transform_verb && element.is_some() &&
  !manipulators.is_empty()` removes the body-datum gizmo (`shell/viewport.rs:597-603`).
  **A slot filter replaces that heuristic**, and the filter is applied *before* the calls to
  `manipulator_for(reference, stage)` and `python::manipulator_specs(object, element)` — both
  take the element, so stripping it in `on_selection` is the whole mechanism.
- `GizmoSet::set_session` narrows by **kind** (Translate/Rotate/Universal), not by class
  (`gizmo.rs:176-196, 1851-1868`). The slot must gate *whether* `arm_gizmos` and
  `manipulator_for` are called — not `set_session`.
- `SelectionRef { object, elements }` with `whole()`/`element()` (`ferrocad_geom/…:583-604`) is
  what `on_selection` receives, so the `Body` filter is a one-line rewrite there, not a pick
  change (hover keeps the sub-element; the bloom and the element map depend on it).
- `transform_verb` becomes redundant once the slot exists (it is today's way of saying "I
  explicitly want the body control"); U6 should delete it rather than keep two switches.
- ⚠️ Two manipulator sources, two code paths: a compiled one (`manipulator_for`) and declarative
  ones (`manipulator_specs`). A slot that narrows "the manipulator class" has to narrow both, or
  a mod-declared face handle would still arm under Slot 1.

## 7. Structural changes required

1. **Name the new pending type away from `TransactionManager::pending`.** One module would
   otherwise document two "pending" concepts (`S0`/A0, `ferrocad_core`).
2. **Add `pending` to `Document` and `EditView` as the read path** — additive; do not try to
   make it enforceable in A0 (that needs `object_mut` to become crate-private).
3. **Make the pending edit reachable from the shell through the facade**, as an opaque handle
   (`transform_begin/stage/commit/abort`). Without something like this, A0 has no caller: the
   shell has no document handle by design (`ferrocad_py` + `python/ferrocad_shell`).
   **Superseded the same day by D0** (see §8): the commit goes through the existing interaction
   provider, so no staging call is needed and A0 is deferred rather than extended.
4. **Give the session a world-space datum and a matrix delta**, not `Placement`: document items
   are world-space with an identity placement, and `origin` is the datum the triad already uses.
5. **Add `viewport.set_session(Option<Snapshot>)` and delete the viewport's own item-preview
   writes** (A2). Keep `preview_home` for gizmo/manipulator anchoring; only the item-transform
   branch goes. Decide the tick ordering explicitly: the shell pushes the snapshot back in the
   same gpui update, or a drag lags one hop.

Plus one deletion, not a change: **`transform_verb`** is subsumed by the loadout slot, and the
`arm()` heuristic it feeds is replaced by the slot's filter (U4/U6).

## 8. What this means for the stage plan

| Stage | Effect of the recon |
| --- | --- |
| **A0** | deferred by D0: the commit is the existing `InteractionCommit`, so nothing needs the core type yet |
| **A1** | session takes `(origin, delta: Vector3, matrix)` instead of `Placement` (item 4); no `ShapeItem` API change |
| **A2** | smaller: `set_gizmo_preview` and the item-transform branch die; `preview_home`'s gizmo/manipulator half stays |
| **A3** | unchanged in scope; the stem/scrubber already preview nothing, so this is mostly the pending edit and a ghost look |
| **U1** | unchanged; the commit can reuse `_move_selection(dx, dy, dz)` |
| **U2, U3** | unchanged; the ghost's base/effective pair is already in memory as `last_stage` vs the merged stage |
| **U4** | **smaller**: filter the `SelectionRef` and gate the two arm calls; no `ManipulatorKind`, no `GizmoSet` change |
| **U5** | unchanged; `HoverReport` already carries the target (`inventor_view.rs:98, 2462`) |
| **U6** | add `transform_verb` to the dead-code list |

The one thing the plan got materially wrong is the *shape of the pending value*: it assumed a
`Placement` because that is what `_move_selection` writes, but the display path never sees one.
The session can measure and draw in world space with no placement at all, and only the commit
needs the object — which is the facade's job already.
