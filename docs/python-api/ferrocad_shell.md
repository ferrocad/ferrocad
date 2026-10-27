# `ferrocad_shell`

The host bridge the shell calls. Not upstream API: it is how FerroCAD's window (and `tools/userdocs.py`) drive the facade.

> **Generated page.** `tools/api_docs.py` reads this surface from the
> module itself. Do not edit it by hand.

App-shell helpers used by the FerroCAD shell library.

_Every entry point returns a JSON string._

## Functions

| Function | Summary |
| --- | --- |
| `add_namespace(name: str, value) -> None` | Expose ``value`` to the console under ``name``. |
| `appearance_view() -> str` | The appearance policy the shell should apply at boot, as JSON. |
| `axis_summary() -> str` | The axis's provider and stem size for the shell's popover. |
| `bootstrap(doc_name: str = 'Shell') -> str` | Create (or recreate) the sample document and report host status. |
| `close_document(name: str) -> str` | Close a document by name. |
| `commands() -> str` | The registered workbench commands, with resources and live state, as JSON. |
| `dialog_accept(payload_json: str) -> str` | Accept an open dialog: ``{"token": ..., "values": {...}}``. |
| `dialog_cancel(token: str) -> str` | Cancel an open dialog and drop its token. |
| `evaluate(code: str) -> str` | Evaluate one console line, capturing stdout/stderr and any traceback. |
| `gpu_adapter() -> str` | The persisted viewport GPU adapter setting (``auto`` by default). |
| `grid_summary() -> str` | The grid's provider and spacing for the shell's popover. |
| `hello() -> str` | A first console line, proving the interpreter is live. |
| `interaction(object_name: str, manipulator: str, action: str, element: str, values_json: str) -> str` | Apply a manipulator commit through the module that supports it. |
| `load_workbenches(dirs_json: str = '[]') -> str` | Best-effort workbench loading. |
| `manipulator_specs(payload_json: str) -> str` | The handles the mods declare for the shell's current selection, as JSON. |
| `model_tree() -> str` | Documents and their objects, for the inspector pane. |
| `open_document(path: str) -> str` | Open a saved document and make it active (the ``--open`` path). |
| `properties(object_name: str) -> str` | Property rows for one object, for the property editor pane. |
| `provider_features() -> str` | Every registered provider feature, with its backends and active flag. |
| `provider_skin(payload_json: str) -> str` | One feature's provider-supplied WGSL source, for the shell to build. |
| `run_command(name: str) -> str` | Invoke a workbench command by id; returns ``{ok, status?, dialog?, token?}``. |
| `set_gpu_adapter(payload_json: str) -> str` | Persist the viewport's GPU adapter choice from the Dev Tools panel. |
| `set_property(object_name: str, prop: str, text: str) -> str` | Set one property from its editor string, in a named transaction. |
| `set_provider(payload_json: str) -> str` | Switch one feature's active provider from the shell's popover. |
| `set_selection(pairs_json: str) -> str` | Replace ``FreeCADGui.Selection`` with the shell's selection. |
| `set_view_state(state_json: str) -> str` | Mirror the shell's presentation state to ``FerroCADGui.ViewState``. |
| `stage_meshes() -> str` | The tessellated shapes the viewport draws, as JSON. |
| `take_selection_requests() -> str` | Drain the writes a script queued on ``FreeCADGui.Selection``. |
| `tool_bridge(name: str)` | The ``ferrocad_stage.Tool`` that forwards viewport input to ``name``. |
| `tool_command(name: str, verb: str) -> bool` | Send a session verb (``cancel``/``finish``) to a Python-backed tool. |

## Values

| Name | Summary / value |
| --- | --- |
| `DRAFT_TOOL` | 'draft' |
| `PYTHON_TOOL_PREFIX` | 'python:' |
