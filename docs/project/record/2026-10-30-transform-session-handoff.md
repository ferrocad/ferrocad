# Handoff — 2026-10-30: the transform session, and the rest of the day

Status: **handoff note (2026-10-30)**. Not a plan and not a changelog: this is the state a
fresh session needs to continue without re-deriving it. The plan it refers to is
[`../tasks/transform-session.md`](../tasks/transform-session.md) (with
[`../tasks/tool-loadout.md`](../tasks/tool-loadout.md) and
[`../tasks/mod-events.md`](../tasks/mod-events.md)); the backlog is
[`../TODO-31-10-2026.md`](../TODO-31-10-2026.md).

Everything below is true at `ef33987`, working tree clean, `main` level with `origin/main`.

---

## 1. Where the tree is

| | |
| --- | --- |
| Branch / HEAD | `main` @ `ef33987`, pushed, clean |
| Tests (lib) | `ferrocad_core` 89, `ferrocad_stage` **225**, `ferrocad_viewport` **201**, `ferrocad_gpui` **244**, `ferrocad_wgpu` 63 |
| Docs gate | `test_docs_tooling.py` 47, `build_book.py --check` 36 chapters, `userdocs --check`, `api_docs --check` — all clean |
| External mod repo | `/home/john/projects/ferrocad-grid-overlay`, clean at `f529c86`, nothing to push |

## 2. What landed today (newest first)

| Commit | What |
| --- | --- |
| `ef33987` | **A1c**: the shell owns one `TransformSession`, opened at the press from `last_stage`, fed every tick, preferred over the Δ fields; cancel drops it with the ghost. Preview goes through `preview_transform`. Commit route unchanged (see §7.2) |
| `af7b218` | **A1a + A1b**: `ferrocad_stage::session::TransformSession`; `InventorViewport::preview_transform(entities, Option<Matrix4>)` as the single owner of item-preview writes |
| `cea7519` | Docs: why there is no pending-rotation state (only a translation HUD defers; `Rotate` opens no dialog) |
| `21d87cb` | Refactor: `TransactionManager::pending` → `booked`; the HUD's pending value consolidated into `Shell::pending_move` (`PendingMove`) |
| `dd2b398` | **D0 decided** (previews in Rust, commit through the mod layer); `tasks/mod-events.md` (E1) written |
| `9a7c40d` | Recon: [`../research/transform-session-compatibility.md`](../research/transform-session-compatibility.md) |
| `d5bc31e` | Plan split into transform-session + tool-loadout |
| `23aab47` | The transform plan written; the four lifecycle specs marked superseded-in-part |
| `1559fa0` | Fix: keep the drag and the triad when a Move session ends (the `{:.2}` round-trip + the dismissed triad) |
| `edb9b1d` | Closed TODO-30, opened TODO-31 |
| `0c5c6ef` | Fix: a configured grid pitch dissolves instead of re-spacing |
| `3aa56fa`, `e819fc2` | 3-D text figure recapture; triad-drag coverage |
| `18247c6` | Fix: the pick pass is renderable (`Rgba16Float`), readback buffer reused |

## 3. The plan, and exactly how far it is

| Stage | State |
| --- | --- |
| **A0** — `PendingEdit` + `EditView` (core) | **deferred by D0**; design only, in `transform-session.md` §2.2–§2.3. Trigger to build: the first non-display consumer (a script reading a pending value, save-with-live-session, or a preview the document must hold) |
| **A1a** — `TransformSession` | **done** (`ferrocad_stage/src/session.rs`) |
| **A1b** — `preview_transform`, single preview owner | **done** (`ferrocad_viewport/src/inventor_view.rs`) |
| **A1c** — shell wiring | **done** (`shell.rs`, `shell/dialogs.rs`) |
| **A2** — delete the alternative preview paths | **open**, next |
| **A3** — stem/scrubber preview via the mod `Preview` channel | open |
| **U1** — one drag mode: release keeps, `Enter` checkpoint, `Esc` rollback, auto-commit, HUD gone | **partial** — the mode, `Enter`, `Esc` and the selection-change commit landed (same day, after this note was first written); the HUD removal and the commit-route move are left |
| **U2** — two-stage pill | open |
| **U3** — 3-D measuring tape + `SESSION_HOLD` | open |
| **U4** — loadout slots + `Select ▾` filter (needs U1) | open |
| **U5** — hover-to-arm (needs U4) | open |
| **E1** — mod event surface | open, independent |
| **U6** — spec sweep, dead code, user docs | open |

## 4. Decisions

| # | Decision | Where recorded |
| --- | --- | --- |
| **D0** | **Previews stay in Rust; the commit goes through the mod layer.** A tick is 60–120/s and the commit path recomputes, so a per-tick GIL crossing is the one place the boundary costs more than it buys. One crossing per commit is exactly where the vision wants the seam | `transform-session.md` §2.8 |
| D1 | Closed by D0: the interaction provider, not the command dialog | same |
| **D3** | **One session per shell.** It is driven by ticks the shell already receives, the pending value already lives there, and a second window must not start a second session on the same object | §6, decided |
| **D4** | **Saving with a live session commits it and says so**; `Esc` is the only discard. *The save command does not exist yet* (`App_Save`/`Ctrl+S`), so this needs building before it can be tested | §6, decided |
| D2 | Loadout slot lives in view state, beside `Gizmo ▾` | `tool-loadout.md` §6 (open) |
| D5 | `FreeCADGui.Preview` stays a channel, backed by a pending edit when A0 lands | `tool-loadout.md` §6 (open) |
| D6 | The tape shows for a typed-only session too | `transform-session.md` §6 (open) |

## 5. The model in one page

- **`TransformSession`** (`ferrocad_stage::session`) is the uncommitted value: `target`, `datum`
  (a **world point** from `ShapeItem::anchor()`), an **absolute** `delta`, `mode`, plus
  `transform() -> Matrix4`, `checkpoint(datum)`, `is_dirty()`. Absolute, not incremental — a
  dropped tick self-corrects and typing replaces a drag instead of adding to it.
- **Nothing is written to the document before a commit.** `Esc` is a drop, never an undo.
- **The commit is one `InteractionCommit`** through the mod layer. `to_interaction_with("transform",
  "apply", <matrix>)` already exists and is what a non-deferred gizmo release emits
  (`gizmo.rs:671-699`). No facade method was added.
- **`PendingMove`** is now only the HUD's *view* (the Δ fields, the drag baseline) plus the
  fallback for a shell with no stage. A2 deletes the fallback.
- **The viewport renders, it does not own**: `preview_transform(entities, Option<Matrix4>)` is the
  single writer of `ShapeItem::temporary_transform`; `preview_translation` and `apply_gizmo_preview`
  route through it. `preview_home` is untouched — it anchors gizmos/manipulators, **not** items.

## 6. Facts about the tree the plan depends on

1. **Document items are world-space with an identity placement.** The provider bakes `Placement`
   into the mesh and reports its base as `origin`
   (`python/ferrocad_parametric/__init__.py:86-93`); `ShapeItem::placement()` is the identity.
   Pinned by `a_document_item_is_world_space_with_an_identity_placement` (`gpui/src/display.rs`).
   **Never read `placement` as "where the object is".**
2. **The shell has no `Document`.** It knows `entity → object name` and world-space meshes; the
   document lives behind Python (`PyDocument` wraps `Arc<Mutex<CoreDocument>>`).
3. **The display pipeline is Python → Rust.** `stage_meshes()` crosses into OCCT; a commit
   re-tessellates (`after_document_edit`). Therefore **no ghost of a recomputed shape** — previews
   are transforms only.
4. **`last_stage`** (shell) is the un-previewed document stage; `set_stage` keeps it and merges
   script preview items on top. That is the ghost's base-vs-effective pair, already in memory.
5. **Rotation is never deferred**: `Rotate` opens no dialog, so `queue_transform_preview` returns
   early and no `TransformPreview` is emitted. The viewport previews/commits rotation from the
   full matrix. There is no shell-side rotation state, by construction.
6. **The commit seam**: `python::apply_interaction` → `FerroCAD.interaction` → the edition's
   `_interaction` provider. The parametric provider handles `transform`/`apply` with a 4×4 matrix
   in one transaction named `Move <Name>` — **for one object**.
7. **The loadout's seam is `arm()`**: one compiled manipulator per selection
   (`contributions.manipulator_for(reference, stage)`) plus mod-declared handles
   (`python::manipulator_specs(object, element)`), with the body triad retired by the
   `transform_verb` heuristic. Filtering the `SelectionRef` before those calls replaces the
   heuristic; **no manipulator kind is needed**.

## 7. Next steps

### 7.1 A2 — delete the alternatives (small, no observable change)

- `Shell::gizmo_deferred`, `viewport.set_gizmo_preview`, `defer_gizmo_preview`.
- The two wrapper routes: `preview_translation`'s remaining callers (`preview_dialog` and
  `clear_dialog_preview` now call `preview_transform`; `apply_gizmo_preview` still calls
  `set_items_transform` directly — check whether it can fold into `preview_transform`).
- `PendingMove`'s mirrored delta once every path has a stage (the headless path in
  `a_gizmo_drag_updates_the_move_fields` has no stage, so either keep the fallback or give the
  test a stage).
- Acceptance: `gizmo_deferred`/`set_gizmo_preview` gone; the three 2026-10-30 regression tests
  (`a_move_hud_drag_survives_its_release`, `a_deferred_drag_reports_previews_and_never_commits`,
  `a_gizmo_drag_updates_the_move_fields`) still pass.

### 7.2 U1 — the behaviour change (do it with A2)

- Release keeps the preview; `Enter` = `session.checkpoint`, one named undo entry; `Esc` = drop the
  session (no entry); auto-commit when the user switches selection/slot/tool with a dirty session.
- The Move HUD goes away: `App_Move`'s no-arg form returns a status instead of a dialog;
  `python/ferrocad_shell/__init__.py` `_MoveCommand.Activated` (~line 1134) changes.
- **A decision U1 must take (not yet recorded):** the session holds **one target**, while
  `_move_selection` moves the **selection** and the parametric `transform` provider moves **one
  object**. Either widen the session to a set (recommended, and what D3's "one session" implies),
  or accept single-object sessions and keep `_move_selection` for multi-select. Until this is
  settled the commit route stays `App_Move` — that is the only reason A1c did not finish D0's
  unification.
- Acceptance: drag, release, drag again, `Enter` → one undo entry; `Esc` after a drag → zero
  entries and the original pose; selecting another object mid-session keeps the move.

### 7.3 Then

- **U2** pill (`GizmoValue` + Δ fields merged, `Tab`/digit), **U3** tape (now has `session.datum()`),
  **U4** loadout + `Select ▾` (filter the `SelectionRef`, delete `transform_verb`), **U5**
  hover-to-arm, **E1** mod events, **U6** sweep.

## 8. Environment and build notes

- Repo `/home/john/projects/ferrocad/ferrocad`; file-tool paths are `ferrocad/crates/...`.
- **The terminal sandbox is broken** (`bwrap: execvp … zed-editor (deleted)`): every terminal call
  needs `unsandboxed: true` with a reason.
- Builds: `CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 cargo test --offline -p <crate>`.
  Never `./build.sh`, `cargo fmt`, `cargo update`, or a workspace-wide build. `/home` is ~99 % full.
- Part/OCCT work needs `LD_LIBRARY_PATH=/home/john/projects/ferrocad/ferrocad/.occt/prefix/lib`
  and `PYTHONPATH=…/python`.
- Docs gates after any doc edit: `tools/test_docs_tooling.py`, `tools/build_book.py --check`,
  `tools/userdocs.py --check`, `tools/api_docs.py --check`. **`docs/INDEX.md` must list every
  note** or `test_docs_tooling` fails; relative links are not checked by the tool, so grep for
  moved files.
- kache prints "caching is disabled" in the sandbox; that is expected, not a failure.

## 9. Pitfalls that bit us (do not re-learn these)

1. **Text round-trips lose precision.** `{:.2}` formatting of a drag delta and reading it back made
   a release commit a value the user never saw (zero under 0.005 mm). The exact value must be the
   one the fields are a *view* of.
2. **A harness can hide a bug.** The original triad test injected `TransformPreview` events with a
   hand-made matrix, so it could not observe the field round-trip. Drive the real pointer path
   (`handle_button`/`handle_pointer`) when the bug is about state.
3. **`face_claim_shell` used to call `viewport.set_stage` directly**, leaving `Shell::last_stage`
   `None` — which silently disabled the session. It now calls `Shell::set_stage`.
4. **`apply_gizmo_preview(None)` clears every item's preview**, not just its target (the
   no-stranded-ghost rule). Intentional after A1b.
5. **The pick format must be renderable.** `Rgba32Float` killed the pick pipeline silently; the
   trace showed 3 validation errors/frame. `Rgba16Float` + `read_normals`-style decoding.
6. **A configured grid pitch must not re-space.** `effective_steps` is for extent-derived steps;
   configured pairs go through `fixed_steps` + `resolve_fade`.
7. **Closers:** `cargo test` counts are per crate and drift; re-run before quoting them.
8. **Backlogs are never deleted**, only moved to `record/` and appended to.

## 10. Open questions

| # | Question | Owner |
| --- | --- | --- |
| Q1 | Does U1 widen the session to a selection, or keep one target and `_move_selection` for multi-select? | U1, before touching the commit route |
| Q2 | Does a rotate session get the same checkpoint/rollback contract? (It has no form, so nothing to checkpoint yet.) | U1/U2 |
| Q3 | Save with a live session (D4) — the save command does not exist; build it first | P6/U1 |
| Q4 | Does `FreeCADGui.Preview` become pending-backed (D5)? | A0, when it lands |
| Q5 | Do mod events need per-document lifecycle (`document.opened`/`closed`)? | E1, on the second consumer |
| Q6 | Is the external grid mod's schema advertisement worth porting, or is the lag intended? | P6 |
