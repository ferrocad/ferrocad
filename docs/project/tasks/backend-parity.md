# Backend parity: one Gui app, one document half

Status: **design and plan (2026-11-08); B0–B6 built (2026-11-08); B7 recorded, B7a–B7d
built (2026-11-08).** The surface suite
(born as the parity ledger, now
`crates/ferrocad_parametric/tests/composition_surface.rs`) and the shell suite are the
gates. Two properties are the goal:

1. **Behaviour parity** — the composition (`RustAppBackend + PythonScriptHost`) answers
   everything `PythonAppBackend` answers, so the legacy type can be deleted.
2. **Gui parity** — every command the shell shows is a real `FreeCADGui` command:
   registered once, visible in `listCommands()`, runnable through `runCommand`, with
   `IsActive` / `IsChecked` honoured.

## 0. Why this exists

`PythonAppBackend` is the pre-merge engine: one `AppBackend` that answers every method
through the interpreter (`crates/ferrocad_shell_python/src/backend.rs:28-43`). The desktop
no longer installs it — `backend::install` composes `RustAppBackend` with
`PythonScriptHost` (`backend.rs:250-261`) — but **the shell's own test suite still installs
it** (`crates/ferrocad_shell_core/src/shell/tests.rs:22-30`), because those tests import
`python/ferrocad.abi3.so` and therefore have a second `ferrocad_core` (and a second
document registry).

That split hides real divergence. The Python half is the FreeCAD-shaped reference: it
implements the Gui app (`FreeCADGui` + `FerroCADGui`, driven by the `ferrocad_shell`
bridge). The Rust half re-implements parts of the same Gui layer inside the App type — a
layer upstream never puts in `App`. The cost is the divergences in §2, one of which
(a documented command that is a no-op) is visible to a user today.

The conformance suite (`docs/developer/workbenches/conformance.md`) measures `FreeCAD` and
`FreeCADGui`, not the shell, so it cannot reach `RustAppBackend`. The gate for the Rust half
is therefore the shell suite, run over the composition — which needs one core in the test
binary (`crates/ferrocad_parametric/tests/composition.rs:46-49` already does this).

## 1. The model this plan enforces

| layer | seam | owns |
| --- | --- | --- |
| App | `AppBackend` | documents, objects, property rows, meshes, edits, undo history, visibility, selection mirror |
| Gui | `ScriptHost` | command registry and execution, `Settings/*`, providers and their claims, dialogs, tools, scene/events, view state |
| shell chrome | `ferrocad_shell_core` | window, tree, panes, viewport, popover generation |
| fallback | Rust arms | Gui verbs **only** when there is no `ScriptHost` (`?runtime=plain`) |

Metadata is already single-sourced: `catalog.rs` → `catalog.json` →
`_BUILTIN_COMMANDS`, with a drift test (`tests/test_commands.py`). The plan makes
**execution** single-sourced too.

## 2. The divergence ledger

Every row is an observable difference between `PythonAppBackend` and the composition today.
The parity suite (§4, B0) starts with this list and shrinks it stage by stage.

| # | divergence | today | closed in |
| --- | --- | --- | --- |
| 1 | `App_ViewWorkplane` | **closed in B2**: the id is `Executor::Script`, so the composition runs the Gui command and returns `{"view": {"workplane": "view"}}`. The Rust stub is the `runtime=plain` fallback | B2 ✅ |
| 2 | grid / axis / gizmo toggles | **closed in B2**: both halves run `_GridCommand`/`_AxisCrossCommand`/`_GizmoCommand`, so both flip `FerroCAD.Grid/Axis/Gizmo.enabled` and derive the provider claim | B2 ✅ |
| 3 | `enabled` on a catalogue row | **closed in B2**: `commands()` takes the script row's `enabled`/`checked` (`IsActive`/`IsChecked`) and keeps the catalogue's metadata, so `App_Move`/`App_Rotate` grey out with an empty selection | B2 ✅ |
| 4 | document list | **closed in B3**: `model_tree` iterates the registry (`Application::documents`), marks the active node, and `activate_document` switches through the script half so the Gui app's listeners fire | B3 ✅ |
| 5 | `transform` / `apply` commit | **closed in B4**: the script's providers answer first (a mod's placement edit and its undo label are the ones an edition sees), and the Rust `Transform` arm is the fallback the plain, JS and wasm-Python hosts keep running | B4 ✅ |
| 6 | five verbs outside the Gui registry | **closed in B1** for the four edits: `App_Undo`, `App_Redo`, `App_Fit`, `App_ToggleVisibility` are catalogue entries with Python `Activated`s. **B2 closed the fifth**: Draft's registered `Draft_Snap_Lock` supplies the row and the executor, and the Rust switch is the `runtime=plain` fallback | B1 ✅ B2 ✅ |
| 7 | the harness itself | **closed in B5**: `ferrocad_shell_core`'s tests install the composition over a linked core (`register_test_core`), so the shell suite gates the composition; `PythonAppBackend` is gone | B5 ✅ |
| 8 | `properties()` rows | **closed in B2**: the bridge synthesizes the `Label` row (and `set_property` renames through it), so both halves show the editor's rows | B2 ✅ |

## 3. Execution ownership

Rust keeps the implementation for the verbs it already owns; the script owns the verbs
that are Gui policy. The table lives in `catalog.rs` as private policy, **not** in
`CommandInfo`/`catalog.json` (the JSON and the drift test stay untouched):

| executor | ids | rule |
| --- | --- | --- |
| `Executor::Script` | `App_ViewWorkplane`, `App_ToggleGrid`, `App_ToggleAxisCross`, `App_ToggleGizmo`, `Dev_ToggleTools` | the script answers when present; the Rust arm is the `runtime=plain` fallback (none for `Dev_ToggleTools`, which the shell intercepts in the window) |
| `Executor::Shell` | `App_Move`, `App_Rotate`, `App_Undo`, `App_Redo`, `App_Fit`, `App_ToggleVisibility` | Rust answers; **optional delegation** through `ScriptHost::claims(id)`, which defaults to `None` |

`Draft_Snap_Lock` is not a catalogue entry: Draft registers the command, so the
composition shows Draft's row and runs Draft's `Activated`; the Rust snap switch is
the fallback for a build with no scripting surface.

`claims` is separate from `run_command` on purpose: being registered in `FreeCADGui` is not
a request to execute. Nothing implements it in B2, so D1 holds; a mod that genuinely
needs to run a catalogue id differently opts in with one method.

## 4. Stages

### B0 — Make the divergence visible (built 2026-11-08)

Landed as `crates/ferrocad_parametric/tests/backend_parity.rs` (renamed
`composition_surface.rs` when B5 retired the second side): nine tests, one process,
one core. Each scenario boots the composition and `PythonAppBackend` in turn against the
same app script (every document is closed and the script's `bootstrap` re-run between
them) and compares one observable:

| observable | test | ledger |
| --- | --- | --- |
| the core is shared | `the_test_image_has_one_core` | — (asserts `ferrocad._kernel_id`) |
| `commands()` ids and flags | `the_command_list_matches` | 7 entries (4 × B1, 3 × B2) |
| `model_tree()`, one document | `the_tree_of_one_document_matches` | — (equal) |
| `model_tree()`, two documents | `the_tree_of_two_documents_matches` | 1 (B3) |
| `run_command(id)` for the routed catalogue | `the_routed_command_outcomes_match` | 3 (1 × B2, 2 × B1) |
| a toggle's effect on the script's store | `a_toggle_writes_the_same_script_state` | 1 (B2) |
| `properties()` rows | `the_property_rows_match` | 1 (B2) |
| `apply_interaction(transform/apply)` | `a_transform_commit_matches` | 2 (B4) |
| `set_property` + `App_Undo` | `an_edit_and_its_undo_match` | 2 (B1) |

The ledger is checked in both directions: an unlisted difference fails, and a listed
difference that no longer differs fails — an entry cannot outlive the stage that closes
it. Outcome **status** text is not compared (it is UI copy); the applied view state, the
document and the script's stores are.

Two findings the plan did not predict:

1. **A kernel-free test image cannot have one core.** `python/FreeCAD/__init__.py:52`
   imports `ferrocad_combined` whenever `Part` is not a builtin, and that module
   publishes its own `ferrocad` over the builtin — a second `ferrocad_core`, and a
   second registry. So the suite registers the edition's four modules
   (`ferrocad_combined_py::register_builtins`), which is why it lives in
   `ferrocad_parametric` (the crate whose CI job has the kernel) rather than in
   `ferrocad_shell_python` (whose job is deliberately kernel-free). This is also the
   real reason the shell tests never used the composition: they cannot, kernel-free,
   without faking `Part`.
2. **`properties()` is missing the `Label` row.** The Rust half synthesizes it the way
   FreeCAD's editor shows it; the app script iterates `PropertiesList`, which has no
   `Label`. Recorded under B2: the bridge adds the row (the document half is right).

**Exit: met.** The suite runs in the `ferrocad_parametric` job and names every
divergence. The ownership table it enforces is in
[`../../developer/architecture/host-backends.md`](../../developer/architecture/host-backends.md).

### B1 — One command registry (built 2026-11-08)

- `App_Undo`, `App_Redo`, `App_Fit` and `App_ToggleVisibility` are catalogue entries now
  (`crates/ferrocad_shell_api/src/catalog.rs`), `python/ferrocad_shell/catalog.json` is
  regenerated, and each has a `_BUILTIN_COMMANDS` class in `ferrocad_shell` with
  `Activated` / `IsActive`. `Draft_Snap_Lock` stays out (Draft registers it) and the
  catalogue owns only its id, `catalog::SNAP_LOCK`.
- `RustAppBackend::commands()` reads all four from the catalogue and fills in live state
  (`enabled` from the undo/redo history and the selection); the manual rows are gone.
- **The status seam gained `ok`/`error`.** `FreeCADGui._payload` used to force
  `ok: true`, so a command could only refuse through `status` text. It now forwards
  `ok`/`error` when a command sets them; `App_Undo` depends on it (the parity suite is
  what caught the asymmetry).
- **`set_visibility` moved into the app script.** `ferrocad_shell.set_visibility` is the
  one implementation; `python::set_visibility` calls it through `call_json`, and the new
  command reuses the same helpers inside one transaction. The eye icon and the verb can
  no longer drift.
- **The visibility label is static** (`Toggle visibility`) instead of the dynamic
  `Hide`/`Show`: the catalogue is the metadata source and the Python drift test compares
  `MenuText` with it. Upstream's `Std_ToggleVisibility` is static too; the check state
  still follows the selection's visibility.

**The ledger shrank by ten entries**: the four `only:composed:*` command ids, `App_Fit`'s
`ok`/`view`, and both undo entries. What remains is B2 (three `flags:` rows and
`App_ViewWorkplane`'s view, the toggle store, the missing `Label` row), B3 (the
two-document tree) and B4 (the transform commit).

**One shell-side bug it exposed.** The real command list has three dock verbs
(`App_Undo`, `App_Redo`, `App_Fit`); the test harness had never seen them because it ran
`PythonAppBackend`. The action dock's panel stretched to the wrapper's full width, so
adding them wrapped it into a second row whose hitbox covered the bottom-left hand
column — `the_inspect_switcher_reaches_the_viewport_picker` clicked the dock instead of
the `Edge` segment. Fixed in `ui/chrome.rs`: the panel is `flex_none()` (a strip is as
wide as its content) and the wrapper is an explicit row. This ships a real fix: the
composition always had those dock verbs.

**Exit: met.** `listCommands()` ⊇ the catalogue, `runCommand` answers every row, the
Python drift test is green, and the parity ledger has no B1 entries left.

### B2 — Script-first for the Gui-authority verbs (built 2026-11-08)

- `catalog::executor(id)` (`crates/ferrocad_shell_api/src/catalog.rs`) splits the
  catalogue: `Script` for the four presentation verbs, `Shell` for the edits and their
  session verbs. It is Gui-app policy and stays out of `CommandInfo`/`catalog.json`, so
  the Python registry does not have to mirror it.
- `ScriptHost::claims(id) -> Option<CommandOutcome>` is the optional delegation hook for a
  `Shell` id; its default is `None`, so nothing is delegated in B2.
- `PythonScriptHost::run_command` filters by **executor**, not by catalogue membership
  (replacing `scripted_commands()`); a non-catalogue id the registry lists is the
  script's.
- `RustAppBackend::run_command` asks the script first for a `Script` id (the Rust arm
  stays as the `runtime=plain` fallback), then `claims`, then its own arms.
- `RustAppBackend::commands()` fetches the registry rows once, keeps the catalogue's
  metadata and takes the script's `enabled`/`checked`.

Three changes were not mechanical:

1. **`Draft_Snap_Lock` went back to Draft** (D2 revised). Draft's row carries the label,
   the `snap` surface and `IsChecked` from `Snapper.isEnabled()`. The Rust row shadowed a
   real command, and the Rust executor would have moved a check mark no widget reads, so
   the composition shows Draft's row and runs Draft's `Activated`; the local switch is
   only the `runtime=plain` fallback.
2. **The provider-claim composition moved into the Gui app.**
   `FerroCADGui.gridViewRequest()` / `axisViewRequest()` are the one implementation of
   "`FerroCAD.Grid/Axis.apply()` plus the active provider's overlay claim";
   `App_ToggleGrid`, `App_ToggleAxisCross`, the two `App_Reload*Settings` verbs and the
   `ferrocad_shell` bridge all call them, so a command and the shell cannot derive
   different claims.
3. **`properties()` gained the `Label` row, and `set_property` can set it.** `Label` is a
   document-object field, not a `PropertiesList` entry, so the bridge synthesizes the row
   the editor already showed; a rename is its own `Rename` transaction and does not
   recompute.

**One test change, not a product change.** The semantic switches live in the process-wide
parameter store, so `the_routed_command_outcomes_match` now resets
`FerroCAD.Grid/Axis/Gizmo.enabled` before each backend's run. Without it the second
backend starts from the first one's toggles and the two requests cannot be compared.

**The ledger is empty except B3 and B4:** `flags:App_Move`, `flags:App_Rotate`,
`flags:Draft_Snap_Lock`, `App_ViewWorkplane:view`, `grid-store` and `rows` all closed.

**Exit: met.** The composition's row set, outcome views, toggle stores and property rows
are now equal to `PythonAppBackend`'s.

### B3 — Documents in the tree (built 2026-11-08)

- `AppBackend::activate_document(name)` (default: a refusal) plus the `ScriptHost` hook of
the same name. `RustAppBackend` asks the script first and otherwise moves the registry's
active marker (`Application::set_active`); the script half calls
`FreeCAD.setActiveDocument`.
- `RustAppBackend::model_tree` iterates `Application::documents()` — the registry **is**
the source (D4) — and marks the active node (`DocumentNode::active`). The Python bridge
emits the same flag from `FreeCAD.activeDocument()`.
- `Shell::activate_document` shares the open path's teardown (`yield_document`): a modal
or a form refuses, a pending move is rolled back, the selection is cleared, and then
`refresh` + `sync_stage` re-read the new document. The camera is deliberately **not**
re-fitted: it belongs to the window, not to a document.
- `refresh` takes the title and the save name from the tree's active node, so a switch
made at the console (`FreeCAD.setActiveDocument`) is reflected without a second query.
- Tree UI: a document header switches on a double click, or on `Enter` after one click
(`tree_document` is the row the last click landed on). Object rows of a non-active
document are muted and inert — a selection names objects of the *active* document, so a
click there could only fill the property pane with nothing.

Two findings the plan did not predict:

1. **Activation is a Gui event, not a registry write.** `Application::set_active` fires
   only the Rust `DocumentObserver`s; the `FreeCADGui` document listeners
   (`onDocumentDeactivated`/`onDocumentActivated`) run from `FreeCAD.setActiveDocument`.
   A Rust-only switch would silently skip a provider's detach/attach, so the
   composition routes activation through the script half — the same shape as
   `open_path`/`save_path`.
2. **The document label is not in the core.** `DocumentNode.label` is the document's name
   for the Rust half and `doc.Label` for the Python half, because a label is a field of
   the Python wrapper (`PyDocument::label`). Recorded, not compared: the trigger to close
   it is the core growing a document label.

**Exit: met.** Ledger row 4 closed: `activating_a_document_matches` compares the switch
in both halves (the tree's active flag and the rows that follow it), and
`a_document_row_switches_the_shell` drives the tree's own double click and `Enter`.

### B4 — Interaction ownership (built 2026-11-08)

- `ScriptHost::apply_interaction` returns `None` for a commit no provider owns — the
  trait's documented rule. `PythonScriptHost` wrapped every answer in `Some`, so a
  composition could not tell "the script refused" from "the script has no provider". The
  bridge now answers `null` for the second, and the Python backend (which has no Rust
  arm of its own) turns it into its `no module handles …` refusal.
- `RustAppBackend::apply_interaction` asks the script **first for every commit**, then
  runs its own arm: the generic `transform`/`apply` (a delta composed on the left of the
  current `Placement`, one `Transform` transaction) for a build with no script or one
  that disclaims it, and a refusal for any other id.
- **A commit is per gesture, not per frame.** An `InteractionCommit` is emitted from
  `capture_any_mouse_up` and from a finished tool; the drag itself previews inside the
  viewport (`drag_move_with_modifiers` / `flush_transform_preview`). The interpreter
  round-trip therefore costs once, on release — which is why delegating it never put
  Python on the drag path.
- **Token rule, stated and tested.** The half that minted a dialog token spends it:
  `App_Move` mints `rust-move-N`, `dialog_accept` answers those itself and forwards
  every other token to the script, and a script token is never read as a move.
  `the_half_that_minted_a_token_spends_it` records both directions.

**Ledger row 5 closed.** `a_transform_commit_matches` registers a transform provider
(the kind an edition ships) and compares three things: `ok`, the `Placement` row and the
**undo label** — the label is what names the executor, so the test fails if the document
arm runs instead of the provider. With the provider both halves record `Move Params`;
without it the composition would record `Transform` while Python refuses. The
composition suite's `a_manipulator_commit_routes_by_who_owns_it` flipped its assertion
for the same reason (`Move Box` from the Part provider).

**A second parity gain, recorded but not compared:** a script-applied commit runs through
`ferrocad_py`'s wrappers, so `slotChangedObject`/`slotChangedDocument` fire for a mod's
listeners; the Rust arm writes the core directly and fires no Python signal. Delegation
makes the observer stream match what `PythonAppBackend` did.

**Exit: met.** Ledger row 5 closed; the token rule is stated and tested; the Rust arm
remains the fallback for `?runtime=plain` and for the JS and wasm-Python hosts, none of
which register an interaction provider.

### B5 — Retire the legacy type (built 2026-11-08)

- **The shell suite runs the composition.** `ferrocad_shell_core`'s tests install
  `ferrocad_shell_python::engine()`; `boot_shell`, `boot_again_without_app` and the
  chrome test share one `register_test_core()` (`ferrocad_py` + `ferrocad_shell_python`
  dev-dependencies, one `append_to_inittab` `Once`) so the interpreter's `ferrocad` is
  **this binary's** `ferrocad_core`. `crates/ferrocad_toy/tests/open_document.rs` boots
  the same engine.
- **`PythonAppBackend` is gone.** `ferrocad_shell_python::backend` keeps `install`,
  `engine` and the Python-backed tools; its 200-line "every method through Python" type
  is deleted, and `python::interaction` (the `Option` form) is what the script half
  answers with.
- **The parity suite became a surface suite.** `tests/backend_parity.rs` →
  `tests/composition_surface.rs`: with one backend left there is nothing to compare, so
  each expectation the ledger pinned is now asserted directly — the command list (all
  catalogue metadata, the script's live flags, its own verbs, Draft's snap row), the
  tree and its switch, the routed command outcomes, the semantic stores, the property
  rows, an edit and its undo, and a transform commit through the edition's provider.

Three findings:

1. **A linked core must not import the combined extension.** `FreeCAD/Base.py` imports
   `Part.OCCError`, so `python/Part.py` loaded `ferrocad_combined`, which publishes
   `ferrocad` over the built-in — a second `ferrocad_core`, visible as pyo3's
   `'DocumentObject' object cannot be converted to 'DocumentObject'`. The shims now
   refuse when the interpreter already has a **built-in** `ferrocad`
   (`python/_shim.py`), and `ferrocad_combined_py::install` refuses the same image from
   the Rust side.
2. **`FreeCAD/__init__.py` needed the same rule** (`_builtin("ferrocad")`), because it
   imports the combined extension only when `Part` is not built in.
3. **`Dev_ToggleTools` is the script's.** It is chrome the shell intercepts in the
   window; a non-window path (`workbench::commands::run`) reads the script's
   `Activated`, which is the headless status. Its executor is `Script` now, with no
   Rust arm of its own.

**CI.** The extension step stays in both jobs, for a different reason: the Python
facade/workflow tests (`headless`) and `ferrocad_shell_python`'s own host-import test
(`windows`) still import `ferrocad` from disk. The **shell** suite no longer does.

**Exit: met.** The full shell suite (**271**) is green on the composition; the legacy
type is deleted; the surface suite (**9**) is green.

### B6 — the two script halves (built 2026-11-08)

B0 compared two *backends* (the composition against `PythonAppBackend`, retired in B5);
B6 compares the two **script halves**, which the crate's whole claim rests on: that
`python/ferrocad_shell` is one source, running unchanged in CPython and RustPython.

| composition | document half | script half |
| --- | --- | --- |
| `RustAppBackend + PythonScriptHost` | Rust | CPython |
| `RustAppBackend + PywasmScriptHost` | Rust | RustPython |

Both share the document half, so a difference between them is the VM, not the document.

`the_two_script_halves_agree` boots CPython first (its app script creates `Shell`), then
composes `PywasmScriptHost::live()` over the same registry on a 16 MiB stack thread —
RustPython compiles recursively and its `bootstrap()` default is `None`, so it adopts
that document instead of building the Rust sample.

The ledger is a **capability matrix**, not equality: the wasm facade deliberately has
no tools, dialogs, providers, filesystem, and no command registry.

| observable | CPython | RustPython | verdict |
| --- | --- | --- | --- |
| `properties("Params")`: name, type, value, status, kind, editable, choices | the editor's rows | identical | **Same** |
| `evaluate("FreeCAD.ActiveDocument.Name")` | `'Shell'` | `'Shell'` | **Same** |
| `hello()` | names CPython | names RustPython | **Intentional** — the console should say which VM is behind it |
| `commands()` | the catalogue plus the script's verbs | empty | **Gap** |
| `run_command("App_Delete")` | the script's status | `None` | **Gap** |

**The Gap has a named trigger.** `crates/ferrocad_shell_pywasm/src/bootstrap.py` stubs
`FreeCADGui` without `_command_list`/`_run`, so `ferrocad_shell.commands()` cannot run
there and the script's registered verbs never reach the browser palette. Closing it is
a `bootstrap.py` change, not a VM limitation.

**A cross-half fix the matrix forced.** The wasm facade answered `getTypeOfProperty`
with the property *type's* last segment, so the script-visible `status` row read
`"PropertyLength"` where CPython reads the flag names. `ferrocad_doc_api::payload` now
carries `status` (`ferrocad_core::status_names`) and the shim forwards it, so the rows
agree field for field; the payload is the addons' doorway too, so they gain the field.

**A pre-existing B2 regression it surfaced.** `ferrocad_shell_pywasm/tests/properties.rs`
still expected the rows without the bridge's `Label` row. Nothing had run that crate's
tests since the bridge changed; the shared source is the reason both halves had already
moved on. Updated.

**Exit: met.** The matrix is green; the two Gaps carry their trigger; the payload and
shim agree on the one row they disagreed about.

**Not a desktop runtime.** `PywasmScriptHost` is native; only its *facade* is wasm-shaped
(`bootstrap.py`). Running it as the desktop's script half would boot the Rust sample
rather than the app script's document and would drop mods, tools, settings and dialogs.
The most this should become is a feature-gated `--script-host=rustpython` debug flag that
runs the browser's script environment in a real window.

### B7 — packaging: which host ships which half (recorded 2026-11-08; not built)

The seam carries four scripting halves; each host serves three, and the intersection is
what makes the portability claim testable.

| script half | native desktop | browser wasm | why |
| --- | --- | --- | --- |
| none (`plain`) | ✓ | ✓ | no engine; the Rust arms answer |
| `PythonScriptHost` (CPython) | ✓ default | ✗ | PyO3 needs a native CPython C-ABI; the wasm answer would be Pyodide, rejected in the design (`host-backends.md` §4) |
| `PywasmScriptHost` (RustPython) | ✓ **builds and runs natively** | ✓ `?runtime=pywasm` | `ferrocad_shell_pywasm` is not `cfg(wasm)`; `ferrocad_shell_web`'s `the_pywasm_configuration_boots_one_document` composes it on a native 16 MiB thread |
| `JsScriptHost` | ✓ page built (B7b); no host installs it yet | ✓ `?runtime=js` | the page contract is `Page`; `WebPage` and `EnginePage` are the same eight calls, so `backend(Runtime::Js)` is `Err` natively only until the switch installs the desktop page |

`plain + pywasm` is the intersection, which is why B6's capability matrix is the right
gate for both hosts at once.

**Three decisions the hosts currently fold into one.** A bundle decides what it *links*,
the host decides what it *installs* at boot, and each host has a *default*:

| | linked | installed | default |
| --- | --- | --- | --- |
| desktop | CPython (`ferrocad_combined_py`, `pyo3`) | one composition, no switch (`backend::install` → `engine()`) | `PythonScriptHost` |
| browser | RustPython + JS + the core | `?runtime=plain\|pywasm\|js` | plain |

Symmetrising them is one `Runtime` enum with the target-gated variants (`Cpython` off
wasm, `Js` on wasm), a host-supplied way to ask (`?runtime=` there, `--script-host=` /
`FERROCAD_SCRIPT_HOST` here) and a fallback to `plain` per configuration — which the
browser already implements in `ferrocad_shell_web::backend`.

**Two engines were missing, and they are not the same dependency.** One is now built (B7b).

1. **A JS engine** — only for the JS half, and nothing about it is wasm. The page
   contract was eight `#[wasm_bindgen(js_namespace = ["window", "__ferrocadJs"])]` calls
   bound at the call site inside a `#![cfg(target_family = "wasm")]` crate; B7a hoisted
   them into `ferrocad_shell_js::Page` (`active`, `commands`, `evaluate`, `runCommand`,
   `onEvent`, `onDocumentEvent`, `stageMeshes`, `install_kernel`), with `WebPage` as the
   browser's implementation. What remains is the engine behind that trait: `rquickjs`
   (decided below), `boa` as the pure-Rust fallback, `deno_core` rejected for size.
2. **A wasm engine** — only for wasm addons (`toy.wasm`). The ABI is already shaped for
   one: `ferrocad_addon_toy/src/abi.rs` imports a module (`ferrocad.call`) and exports
   `ferrocad_alloc`/`ferrocad_dealloc`/`ferrocad_commands`/`ferrocad_run`/…, so a wasm
   engine hosts it by mapping that import onto `ferrocad_doc_api`'s doorway. The addon's JS
   glue (`loader.js`, `toy.js`) would be Rust on the desktop — or Gap 1 as well.

**Gap 2's engine: `wasmi` (settled 2026-11-08).** An interpreter, not a JIT: pure Rust, no
Cranelift, no executable memory, so a third-party addon's module never becomes code the CPU
runs. It is also the smaller dependency, fuel metering is built in (the right knob for a mod
boundary whether or not it is switched on now), and it compiles to wasm itself, so the same
host logic stays portable. The ABI asks nothing wasmi does not expose: one import,
`(ptr, len)` integers, and a `Memory` slice on either side.

The cost, stated so it is not discovered later: interpretation is several times slower than
a JIT. That is paid per command, not per frame — `commands`/`run`/`stage_meshes` are called
when the user acts — so wasmtime's codegen buys nothing a command can notice today. The
trigger to revisit is a wasm addon that tessellates or scans on a hot path.

Two things to verify when it is built rather than assume:

- the browser-built `toy.wasm` (250 KB, `wasm32-unknown-unknown`, no wasm-bindgen) is
  accepted by wasmi's feature set as-is: the desktop must run the bundle the browser ships,
  not a second build of it;
- wasmi's MSRV against the workspace's `rust-version = "1.87"`. `resolver = "3"` prefers an
  MSRV-compatible release; if the current one needs newer, that is a deliberate bump and
  not a silent one.

**Gap 1's engine: `rquickjs` (settled 2026-11-08).** QuickJS behind Rust bindings: a
complete-enough ES2020 in about a megabyte, with no V8 build system to ship. The surface it
has to run is a small ES module (`www/js-demo.js`), not a framework, so ES2020 is not a
constraint worth paying V8's size for. Two costs stay visible: the crate compiles C, so the
desktop gains a C dependency where it currently has none; and ES2020 is a *ceiling* — a page
module written against a newer V8 feature would pass in the browser and fail on the desktop.
`boa` is the pure-Rust escape hatch if the C build ever matters more than completeness;
`deno_core` is rejected for size. The `Page` trait is the prerequisite, and B7a built it;
what is left is the engine behind it.

**Costs, in increasing order of work.**

- **Size — unmeasured.** `ferrocad_shell_pywasm` is a *dev-dependency* of
  `ferrocad_parametric`, so the 44.6 MiB release binary carries none of it; linking it (or
  an engine) needs a build-and-diff, not an estimate.
- **Stack.** RustPython's compiler recurses and needs 16 MiB; a desktop main thread is
  8 MiB, so the desktop host composes on a big-stack thread, as the tests and
  `ferrocad_web_gpui/build.rs` already do.
- **`host_env`.** The crate pins `default-features = false` without `host_env` because
  `wasm32-unknown-unknown` needs that — which also means no real `os`/`_io`, and
  `bootstrap.py` shims `contextlib`/`io`/`pathlib`/`traceback` to compensate. A native build
  should turn `host_env` on and make the shims conditional. Until it does, desktop pywasm
  is a wasm environment running in a native window.
- **The facade is read-only.** `setattr` raises, there is no recompute and no `Shape` API,
  because the half does not share the `*_py` bindings (B6). So a desktop pywasm half is a
  *preview of the browser environment*, never a substitute for CPython, and must not
  become the default.

**Decision: record, then build one gap at a time.**

- **Gap 1 is built** (B7b: `rquickjs` behind `--features desktop`), so what remains there
  is wiring it as a host's script half (`--script-host=js`), which is B7's `Runtime`
  enum. **Gap 2** (wasm addons on the desktop) is still a plan with its engine settled
  (`wasmi`) and no slice.
- **The default never changes:** a second script half is a debug flag, as B6 says of
  `PywasmScriptHost`, and so is the JS page.

**The CPython-free host has to host something (open).** `ferrocad_shell_python` is
currently the desktop host *and* the CPython backend, so splitting it is only worth doing
for a second product. Two candidates, and they pull in opposite directions:

- **An FCStd viewer.** Python-free is feasible; *kernel*-free is not. Core parses the
  archive and `Document.xml` and hands the `.brp`/`.bms` members to the type registry as
  opaque bytes (`ferrocad_core/src/extension.rs`, `FcstdKind`), so drawing a document needs
  only a backend that reads `.brp` — OCCT's native payload. Python enters solely for
  Python-defined types (`Part::FeaturePython` and friends): their *class* comes from a mod,
  so without it the object cannot be instantiated even though its bytes are in the archive.
  So a viewer is "no interpreter, yes OCCT", and lossy exactly on scripted documents.
- **A different-kernel app.** The seam already exists: `ferrocad_geom::Shape` is a
  type-erased handle (`Box<dyn ShapeData>`), `GeometryBackend` declares 32 operations that
  each default to `Err(unsupported(..))`, and `ferrocad_part::register(backend)` is
  documented as "the application calls this once at startup with the kernel it wants". A
  second kernel is therefore a crate, not a fork. What it does not inherit: the Part
  workbench above the trait assumes OCCT semantics (edge resolution for fillets,
  `same_shape`, `moved`), the `.brp` FCStd payload is OCCT's, and the web host needs the
  kernel built for wasm — OCCT has the prebuilt one in `.occt-wasm`.

The deciding constraint is that **the file format follows the kernel.** FCStd keeps OCCT; a
different kernel means a different document format, with FCStd as an import/export step,
not a viewer.

**`SolidCore` is not evaluated.** Nothing in this repository names it and it is not in the
tree, so nothing here should be read as a judgement of it. The criteria to check it against
are the ones the seam exposes: solid modelling with booleans and dress-up (fillet/chamfer,
`crates/ferrocad_part/src/dress_up.rs`), import and export, tessellation with a deflection
parameter, a C ABI with a wasm32 build, a licence compatible with this repository's
LGPL-2.1, and active maintenance.

**Exit when built:** `--script-host=cpython|pywasm|plain` selects among what is linked,
pywasm sits behind a cargo feature, and each host has a test asserting its own row of the
matrix above.

### B7a — the page is a trait (built 2026-11-08)

The engine-agnostic half of Gap 1, and the prerequisite for any of the three engines.

`ferrocad_shell_js` was `#![cfg(target_family = "wasm")]` with eight
`#[wasm_bindgen(js_namespace = ["window", "__ferrocadJs"])]` calls bound at the call site,
so nothing in it could be tested by `cargo test` and there was no place to put an engine.
It now declares the contract instead:

| | before | after |
| --- | --- | --- |
| the page | eight global imports | `Page`: one trait, eight methods, defaults for the optional hooks |
| the browser | the whole crate | `WebPage`, the only `cfg(target_family = "wasm")` piece |
| the doorway | a `Closure` the host kept alive | `Page::install_kernel(Arc<dyn Fn(String) -> String>)`, handed to the page |
| tests | none without a browser | `tests/page.rs`, ten of them, over a fake page |
| `wasm-bindgen` | a dependency everywhere | a `cfg(wasm)` dependency |

`JsScriptHost` is now `JsScriptHost::new(page)` / `with_kernel(page, api)`, and
`PageEvents::new(page)` follows the same page, so the doorway's own `create`/`set` is
announced to the same place as an edit the shell made.

**A defect the tests found.** The display hook's "has it ever answered" flag was a
`OnceLock<bool>` initialised to `false` on the first failure — so once a hook had missed,
a later success could not set it, and a hook that worked and then broke was reported as
"no hook" forever (which reads as *the document's rows stand*). It is a `OnceLock<()>` now:
presence is the answer. `the_display_hook_is_silent_until_it_works` is the regression test.

One deliberate change in the browser page: `install_kernel` hands the `Closure` to JS with
`into_js_value()` instead of keeping a Rust handle. The page owns `ferrocad.call` for the
session, and dropping the handle would have freed the function under it.

**Exit: met.** `cargo test -p ferrocad_shell_js` 10/10 on the host,
`cargo check -p ferrocad_shell_js --target wasm32-unknown-unknown` clean, and
`ferrocad_shell_web` 11/11 + 2/2 unchanged. What remains for Gap 1 is an engine and a
desktop `Page`: no shell, no host and no doorway change.

### B7b — the desktop page (built 2026-11-08)

Gap 1's engine, behind a feature so no browser build and no default build pays for it:
`ferrocad_shell_js::desktop::EnginePage`, QuickJS through `rquickjs` 0.14
(`--features desktop`).

**Discovery mirrors the Python side.** A JS workbench is `mods/<Name>/InitGui.js` — the
mirror of `InitGui.py` — found by `desktop::js_modules(dirs)` in name order, with
`desktop::discover()` applying the same rule the Python side applies:
`FERROCAD_MODS_PATH`, then `mods/` beside the executable, then up from the working
directory. A host can pass its own directories instead; the finding rule lives with the
half, the *decision* stays the host's. The web's modules ship in `www/` and its page
imports them (`?js=1`) — the same contract with a different loader.

**The same module runs on both.** A module exports `install()`, which defines
`window.__ferrocadJs` and may call `window.ferrocad.call(request)` — exactly
`www/js-demo.js`. The engine aliases `window` to the global object and installs
`ferrocad.call` over the host's `DocumentApi`, so neither the module nor the shell knows
which host it is on.

**One thread owns the engine.** QuickJS values are not `Send`, so the runtime lives on a
worker thread and each call is a channel job. Viewport and document events are *queued*
rather than answered: a document event can be emitted from inside a doorway call (an
addon's own `create`), and answering it there would re-enter the runtime mid-call. The
event is delivered after the current job, which is also what makes the ordering testable.

Two defects found by writing the tests, both worth recording:

- **Drop order deadlocked every page.** `Drop for Engine` joined the worker while still
  holding the channel sender, so `recv` never returned and the suite hung. The sender is
  closed *before* the join now.
- **`Module::evaluate` returns a promise.** rquickjs 0.14 evaluates an ES module
  asynchronously, so `install` was read before the job queue ran and came back
  `undefined`; every boot failed. The module is declared, evaluated, and the queue driven
  to settlement first.

**Costs.** `rquickjs-sys` compiles vendored QuickJS as C, so the feature's first build is
slow (then `kache` serves it) and the desktop gains a C dependency; ES2020 is the ceiling;
and a module cannot `import` another file yet (the browser's page has no imports either).

**Exit: met.** 8 engine tests and 10 page tests green, in under a second once built; the
default `cargo test -p ferrocad_shell_js` still compiles no C. Installing the page as a
host's script half (`--script-host=js`) is the remaining wiring, and belongs with B7's
`Runtime` enum rather than with this page.

### B7c — the switch on the desktop (built 2026-11-08)

`Runtime` moved to the seam (`ferrocad_shell_api::runtime`) and the desktop gained the
browser's switch: `--script-host=plain|cpython|pywasm|js`, or `FERROCAD_SCRIPT_HOST`, with
CPython as the default. One table of names, aliases and error text, two switches — so
`?runtime=` and `--script-host=` cannot drift. `ferrocad_shell_web` re-exports the type and
keeps only its own composition.

| name | desktop today | notes |
| --- | --- | --- |
| `plain` | ✓ | the document half alone: the script-less app the browser already had |
| `cpython` | ✓ default | the edition's engine |
| `js` | ✓ with `--features js` | `EnginePage` over `mods/`; without the feature the error says how to build it |
| `pywasm` | ✓ with `--features pywasm` (B7d) | ✓ | RustPython on the 16 MiB worker its compiler needs |

A name this build cannot serve is reported and replaced by `plain` — the browser's rule, so
a scripting surface that cannot start never costs the window. `backend::install_with` does
that; `backend::install` is `install_with(cx, Cpython)`, so no existing call site changed.

The composition is one function, `ferrocad_shell_python::compose(runtime)`, tested per name
in `crates/ferrocad_shell_python/tests/runtime.rs`; the naming is tested in
`crates/ferrocad_shell_api/tests/runtime.rs`. The binaries forward the flag through
`ferrocad::run_edition_with_script`, and `js` forwards as a cargo feature through
`ferrocad` → `ferrocad_shell_python` → `ferrocad_shell_js/desktop`. `ferrocad_shell_python`
names `ferrocad_shell_js` only on native and only behind that feature; the reverse edge
must never exist.

**What is left of the switch is nothing; what is left of parity is `host_env`** (B7d) and
the wasm addon engine (Gap 2). `--script-host=pywasm` composes RustPython on the worker its
compiler needs, but with the frozen stdlib the browser has, so a mod that opens a file does
not get the desktop's filesystem.

**Exit: met.** 4 composition tests, 5 naming tests, `ferrocad_shell_web` 13 + 2,
`ferrocad_shell_core` 271.

### B7d — desktop RustPython (built 2026-11-08)

`--script-host=pywasm` composes `ThreadedScriptHost`, and the name is the design: the VM
is not `Send` and its compiler recurses, so the interpreter needs a 16 MiB stack that a
desktop shell cannot give its own thread — GPUI owns the main thread, and the platform's
stack is 8 MiB on Linux and macOS and 1 MiB on Windows. The VM therefore lives on a worker
with that stack, and the type in front of it implements `ScriptHost`.

The wrapper is small because the half is small: **only `evaluate` reaches the
interpreter**. The spike's other answers — no tools, no catalogue, no scene, no event
callbacks — are constants, so they are answered where the caller is instead of being
marshalled. `crates/ferrocad_shell_pywasm/src/threaded.rs` shares `HELLO`, `no_tool` and
`NoopScriptListener` with the in-thread host, so the two cannot drift.

The first version of the suite proved the stack requirement the hard way: a test booted
the unthreaded host on the *test* thread, and the process aborted with `has overflowed its
stack` at 2 MiB. The test asserts the constants directly now, and the stack is a
`pub const STACK` with the reason written down beside it.

**`host_env` is still off**, so this is the browser's environment in a real window: the
frozen stdlib has no `os`/`_io`, and `bootstrap.py` supplies `contextlib`, `io`, `pathlib`
and `traceback` itself. A mod that calls `open()` or walks a directory does not get the
desktop's filesystem. Turning `host_env` on natively — target-conditional features, plus
the bootstrap shims made conditional — is the next step *if* a mod needs it; the shell
script does not, and matching the browser is the point of the flag.

**Exit: met.** 4 `ThreadedScriptHost` tests, 4 composition tests in both feature
configurations, `cargo check -p ferrocad_parametric --features pywasm` clean, and the
edition still builds with neither feature.

## 5. Decisions (recorded 2026-11-08)

| # | decision | outcome |
| --- | --- | --- |
| D1 | who executes `App_Move` | **Rust**, with optional delegation through `ScriptHost::claims` (B2/B4). The typed form and the gizmo commit stay on the measured path |
| D2 | `Draft_Snap_Lock` | **revised in B2**: Draft owns it when its command is registered (registering *is* the opt-in), because its check state is `Snapper.isEnabled()`. The Rust switch stays as the `runtime=plain` fallback, so a build with no script keeps a working snap toggle |
| D3 | multi-document tree | **In scope**: the tree lists every document and activating one (`activate_document`) switches the shell |
| D4 | `model_tree` source | **`ferrocad_core`**: the registry already holds every document; Rust builds the whole tree, no script call needed |
| D5 | ids for the four new verbs | Keep the existing `App_*` ids; upstream aliases (`Std_Undo`, …) only if a mod needs them. Deferred with that trigger |
| D6 | a desktop runtime switch | **Built in B7c**: one `Runtime` in the seam, `--script-host=cpython\|pywasm\|js\|plain` on the desktop, `?runtime=` in the browser. The default stays `PythonScriptHost` |
| D7 | a JS or a wasm engine first | **Neither built.** Recorded as two separate gaps: a wasm engine for addons comes before a JS engine, which waits on JS mods shipping |
| D8 | the wasm engine for addons | **`wasmi`** (interpreter, pure Rust, no JIT): the smaller dependency, fuel metering, and no executable memory from third-party code. Revisit if a wasm display hook puts interpretation on a hot path |
| D9 | the JS engine for a desktop JS half | **`rquickjs`** (QuickJS, C): ES2020 in ~1 MB with no V8 build. `boa` is the pure-Rust escape hatch if a C dependency is unacceptable; `deno_core` rejected for size. The `Page` trait is the prerequisite for any of them |

## 6. Acceptance

- The surface suite is green, including the script-half capability matrix; there is no
  ledger left.
- `cargo test -p ferrocad_shell_core` is green with `engine()` installed.
- `FreeCADGui.listCommands()` ⊇ `catalog::builtin()`; `runCommand` covers every palette row;
  the conformance `App`/`Draft` groups stay at or above baseline.
- `?runtime=plain` has an explicit, tested feature matrix: no interpreter, no registered
  commands, no `Settings`, no workplane view.
- `PythonAppBackend` is deleted.

## 7. Non-goals and deferrals

- **No interpreter inside `RustAppBackend`.** The upstream suite cannot drive the shell;
  the unit of conformance is the App+Gui pair, and the shell suite is its gate.
- **No runtime ownership negotiation without a consumer.** `ScriptHost::claims` exists but
  nothing implements it; the trigger to use it is a mod that needs to execute a catalogue id
  with different semantics.
- **No upstream id aliases yet** (D5). The trigger is a FreeCAD-shaped mod calling
  `FreeCADGui.runCommand("Std_…")`.
- **`PythonAppBackend` is deleted** — done in B5. Its remaining value was a test binary
  that had not linked the extension; the fix was a dev-dependency, not a type.
- **No CPython on the web.** The wasm host runs RustPython instead; Pyodide is rejected
  in the design, not deferred (`host-backends.md` §4).
- **No JS engine on the desktop, and no wasm engine in the desktop bundle.** Both are
  recorded as separate gaps in B7 with their own triggers; neither is linked today, so the
  release binary pays for neither.

## 8. Evidence

| claim | evidence |
| --- | --- |
| the desktop already composes | `ferrocad_shell_python/src/backend.rs:250-261` |
| the test harness does not | `ferrocad_shell_core/src/shell/tests.rs:22-30` |
| one core is possible | `ferrocad_parametric/tests/composition.rs:46-49`, `ferrocad_combined_py/src/lib.rs:41-49` |
| the registry is the document source | `ferrocad_core/src/application.rs:123-149` |
| catalogue → JSON → Python | `ferrocad_shell_api/examples/catalog.rs:4`, `tests/test_commands.py` |
| the token protocol | `FreeCADGui/__init__.py:1356-1370` |
| a kernel-free image splits its core | `python/FreeCAD/__init__.py:32-57` |
| one core in a test image | `ferrocad_parametric/tests/composition_surface.rs`, `ferrocad_combined_py/src/lib.rs:41-49` |
| unknown ids raise | `FreeCADGui/__init__.py:1216-1225` |
| Draft owns the snap lock | `FreeCADGui/draft.py:110` |
| activation fires the Gui listeners | `python/FreeCAD/__init__.py` (`setActiveDocument`), `crates/ferrocad_py/src/lib.rs` (`slotActivateDocument`) |
| the tree is the registry | `crates/ferrocad_shell_rust/src/backend.rs` (`model_tree` over `Application::documents`) |
| one script, two VMs | `crates/ferrocad_shell_pywasm/src/lib.rs` (`SHELL`), `composition_surface.rs` (`the_two_script_halves_agree`) |
| workplane view is documented as working | `docs/developer/workbenches/actions-and-tools.md:539` |
| pywasm also builds natively | `crates/ferrocad_shell_pywasm/Cargo.toml` (not `cfg(wasm)`), `ferrocad_shell_web/src/lib.rs` (`the_pywasm_configuration_boots_one_document`) |
| the JS half is browser-only | `crates/ferrocad_shell_js/src/lib.rs` (`Page`, `WebPage` behind `cfg(target_family = "wasm")`), `crates/ferrocad_shell_web/src/lib.rs:114-118` |
| pywasm costs the shipped binary nothing | `crates/ferrocad_parametric/Cargo.toml:42-44` (dev-dependency) |
| an addon ABI a wasm engine can host | `crates/ferrocad_addon_toy/src/abi.rs:20-25` (`ferrocad.call` import module) |
