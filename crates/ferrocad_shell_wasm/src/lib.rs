//! The wasm-addon scripting surface: a workbench bundle, hosted and sandboxed.
//!
//! **wasm is a delivery format here, not an engine mirror.** The desktop does not compile
//! `ferrocad_core`/`ferrocad_part` to wasm: `RustAppBackend` already holds the native
//! document, and putting a wasm boundary between a script half and the core would be a
//! cost with no behaviour behind it. What wasm buys is a *package* that is isolated from
//! the host: the bundle cannot read the filesystem, cannot reach the network, cannot
//! allocate outside its linear memory, and cannot hold a document — its only capability is
//! the doorway the host installs.
//!
//! The bundle contract is the browser's (`www/addons/loader.js`, `ferrocad_addon_toy`):
//!
//! ```text
//! import  ferrocad.call(ptr, len) -> i64     the document doorway
//! export  memory, ferrocad_alloc/ferrocad_dealloc,
//!         ferrocad_commands() -> i64, ferrocad_run(ptr, len) -> i32,
//!         ferrocad_stage_meshes(ptr, len) -> i64
//! ```
//!
//! Only text crosses, in either direction, and every buffer is `(ptr, len)` in the
//! bundle's own memory, packed as `len << 32 | ptr` on the way out.
//!
//! **The sandbox.** Three properties are enforced rather than promised: the import list
//! is exactly the doorway ([`Linker`] gets nothing else), the interpreter is `wasmi`
//! (no JIT, no executable memory from third-party code), and every entry call is given a
//! fuel budget — a bundle that loops forever traps with `OutOfFuel` instead of freezing
//! the window.
//!
//! [`Addons`] is how a host installs one: the primary script half (CPython, RustPython,
//! the JS page, or none) plus every bundle, with the three addon-facing answers merged.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ferrocad_inventor::{Directives, Event, EventListener, Hit, Scene};
use ferrocad_shell_api::DocumentApi;
use ferrocad_shell_api::app::{NoopListener, ScriptHost};
use ferrocad_shell_api::commands::{CommandInfo, CommandOutcome};
use ferrocad_shell_api::display::StageMesh;
use ferrocad_shell_api::events::DocumentEvent;
use ferrocad_shell_api::selection::SelectionItem;
use ferrocad_shell_api::types::{
    ApplyResult, AxisSummary, Bootstrap, DocumentNode, EvalResult, GpuPreferenceOutcome,
    GridSummary, OpenedDocument, PropRow, ProviderFeature, SelectionRequest, SetResult,
    WorkbenchReport,
};
use ferrocad_shell_api::view::ViewState;
use ferrocad_stage::InteractionCommit;
use ferrocad_types::Vector3;
use serde::Deserialize;
use wasmi::{Caller, Config, Engine, Linker, Memory, Module, Store, TypedFunc};

/// The entry file of a wasm workbench: the mirror of `InitGui.py` and `InitGui.js`.
pub const ENTRY: &str = "addon.wasm";

/// The fuel one entry call may burn before the bundle traps.
///
/// The bundles are command handlers, not simulations: a few million instructions is
/// generous for `commands`/`run`/`stage_meshes` on a real document (the toy bundle
/// spends thousands), and small enough that a runaway loop is felt as an error rather
/// than a frozen window.
pub const FUEL: u64 = 50_000_000;

/// What a bundle may do to the document.
///
/// The default is [`Capability::Read`]: a bundle has to *ask* to write, in an
/// `addon.json` beside it. A sandbox whose default was write would not be a sandbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    /// Read the document; every mutating call is refused at the doorway.
    Read,
    /// Read and write it.
    Write,
}

impl Capability {
    /// The name the manifest and the greeting use.
    pub fn id(self) -> &'static str {
        match self {
            Capability::Read => "read-only",
            Capability::Write => "read-write",
        }
    }

    /// `"read"` / `"read-write"` (and `"write"` as the short form).
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "read" | "read-only" => Ok(Capability::Read),
            "read-write" | "write" => Ok(Capability::Write),
            other => Err(format!(
                "unknown `document` capability {other:?}; expected read or read-write"
            )),
        }
    }
}

/// A bundle's manifest: `<stem>.json` beside the `.wasm` (`addon.json` for
/// `addon.wasm`, `toy.json` for `toy.wasm`, which is how the page's copy is named).
///
/// Absent, or absent field, means read-only: the safe answer is the one that has to be
/// given explicitly.
#[derive(Deserialize, Default)]
struct Manifest {
    #[serde(default)]
    document: Option<String>,
}

/// The capability `bundle`'s manifest grants.
fn manifest_capability(bundle: &Path) -> Result<Capability, String> {
    let path = bundle.with_extension("json");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Capability::Read),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let manifest: Manifest =
        serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    match manifest.document {
        Some(value) => Capability::parse(&value).map_err(|error| format!("{}: {error}", path.display())),
        None => Ok(Capability::Read),
    }
}

/// A doorway that refuses the verbs which change the document.
///
/// The check is on the request's `call` name, so the payloads never reach the document —
/// which is also why the same door can be handed to a JS bundle later.
pub fn read_only(document: Arc<dyn DocumentApi>) -> Arc<dyn DocumentApi> {
    Arc::new(ReadOnly(document))
}

struct ReadOnly(Arc<dyn DocumentApi>);

impl DocumentApi for ReadOnly {
    fn call(&self, request: &str) -> String {
        let writes = serde_json::from_str::<serde_json::Value>(request)
            .ok()
            .and_then(|request| request.get("call")?.as_str().map(str::to_owned))
            .is_some_and(|call| ferrocad_shell_api::document_api::is_write(&call));
        if writes {
            return serde_json::json!({
                "ok": false,
                "error": "read-only: this bundle was not granted document writes \
                          (its manifest does not say {\"document\": \"read-write\"})",
            })
            .to_string();
        }
        self.0.call(request)
    }
}

/// The wasm workbenches under `dirs`: `<dir>/<Name>/addon.wasm`, in name order.
///
/// A directory without the entry file is not a wasm workbench and is skipped — the same
/// rule `InitGui.py`/`InitGui.js` follow for the other two kinds.
pub fn packs(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut packs = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut found: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path().join(ENTRY))
            .filter(|entry| entry.is_file())
            .collect();
        found.sort();
        packs.extend(found);
    }
    packs
}

/// Where to look for wasm workbenches: `FERROCAD_MODS_PATH`, then a `mods/` directory
/// beside the executable, then up from the working directory.
///
/// The same rule the Python and JS halves apply, with a different entry file.
pub fn discover() -> Vec<PathBuf> {
    if let Ok(value) = std::env::var("FERROCAD_MODS_PATH") {
        let paths: Vec<PathBuf> = std::env::split_paths(&value).collect();
        if !paths.is_empty() {
            return paths;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for candidate in [dir.join("mods"), dir.join("..").join("mods")] {
                if !packs(std::slice::from_ref(&candidate)).is_empty() {
                    return vec![candidate];
                }
            }
        }
    }
    if let Ok(mut dir) = std::env::current_dir() {
        loop {
            if !packs(std::slice::from_ref(&dir.join("mods"))).is_empty() {
                return vec![dir.join("mods")];
            }
            if !dir.pop() {
                break;
            }
        }
    }
    Vec::new()
}

/// One instantiated bundle.
pub struct WasmAddonHost {
    /// The interpreter is single-threaded state: one call at a time.
    bundle: Mutex<Bundle>,
    /// What the bundle was granted, for the greeting and for a host that logs it.
    capability: Capability,
}

/// The store's data: the doorway, and the handles the doorway needs while executing.
struct HostState {
    document: Arc<dyn DocumentApi>,
    /// Filled in after instantiation, because the doorway allocates through the bundle.
    handles: Option<Handles>,
}

/// The bundle's exports, type-checked once at construction.
#[derive(Clone, Copy)]
struct Handles {
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    dealloc: TypedFunc<(i32, i32), ()>,
    commands: TypedFunc<(), i64>,
    run: TypedFunc<(i32, i32), i32>,
    stage_meshes: TypedFunc<(i32, i32), i64>,
}

struct Bundle {
    store: Store<HostState>,
    handles: Handles,
    /// What [`Bundle::arm`] restores before each entry call.
    fuel: u64,
}

impl WasmAddonHost {
    /// Instantiate `bytes` read-only: no manifest, no writes.
    pub fn new(bytes: &[u8], document: Arc<dyn DocumentApi>) -> Result<Self, String> {
        Self::build(bytes, document, Capability::Read, FUEL)
    }

    /// [`new`](Self::new) with an explicit capability.
    pub fn with_capability(
        bytes: &[u8],
        document: Arc<dyn DocumentApi>,
        capability: Capability,
    ) -> Result<Self, String> {
        Self::build(bytes, document, capability, FUEL)
    }

    /// [`new`](Self::new) with an explicit fuel budget.
    ///
    /// The budget is per entry call, so it bounds one bad command rather than the
    /// session. A host with a heavier workbench raises it deliberately.
    pub fn with_fuel(
        bytes: &[u8],
        document: Arc<dyn DocumentApi>,
        fuel: u64,
    ) -> Result<Self, String> {
        Self::build(bytes, document, Capability::Read, fuel)
    }

    /// What this bundle may do to the document.
    pub fn capability(&self) -> Capability {
        self.capability
    }

    fn build(
        bytes: &[u8],
        document: Arc<dyn DocumentApi>,
        capability: Capability,
        fuel: u64,
    ) -> Result<Self, String> {
        // A read-only bundle is refused *at the door*, so no payload reaches the
        // document and the bundle cannot learn anything it was not granted.
        let document = match capability {
            Capability::Read => read_only(document),
            Capability::Write => document,
        };
        // Fuel metering has to be enabled on the engine, not the store.
        let mut config = Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, bytes)
            .map_err(|error| format!("the bundle is not a wasm module: {error}"))?;

        let mut store = Store::new(
            &engine,
            HostState {
                document,
                handles: None,
            },
        );
        store
            .set_fuel(fuel)
            .map_err(|error| format!("wasm fuel: {error}"))?;

        let mut linker: Linker<HostState> = Linker::new(&engine);
        // The one import the sandbox grants.
        linker
            .func_wrap("ferrocad", "call", doorway)
            .map_err(|error| format!("the doorway: {error}"))?;

        let instance = linker
            .instantiate_and_start(&mut store, &module)
            .map_err(|error| format!("the bundle did not instantiate: {error}"))?;

        let handles = Handles {
            memory: instance
                .get_memory(&store, "memory")
                .ok_or("the bundle exports no `memory`")?,
            alloc: instance
                .get_typed_func::<i32, i32>(&store, "ferrocad_alloc")
                .map_err(|error| format!("the bundle exports no `ferrocad_alloc`: {error}"))?,
            dealloc: instance
                .get_typed_func::<(i32, i32), ()>(&store, "ferrocad_dealloc")
                .map_err(|error| format!("the bundle exports no `ferrocad_dealloc`: {error}"))?,
            commands: instance
                .get_typed_func::<(), i64>(&store, "ferrocad_commands")
                .map_err(|error| format!("the bundle exports no `ferrocad_commands`: {error}"))?,
            run: instance
                .get_typed_func::<(i32, i32), i32>(&store, "ferrocad_run")
                .map_err(|error| format!("the bundle exports no `ferrocad_run`: {error}"))?,
            stage_meshes: instance
                .get_typed_func::<(i32, i32), i64>(&store, "ferrocad_stage_meshes")
                .map_err(|error| format!("the bundle exports no `ferrocad_stage_meshes`: {error}"))?,
        };
        store.data_mut().handles = Some(handles);

        Ok(Self {
            bundle: Mutex::new(Bundle {
                store,
                handles,
                fuel,
            }),
            capability,
        })
    }

    /// Open a bundle from disk, with the capability its manifest grants.
    ///
    /// No manifest means [`Capability::Read`]: writes are refused at the doorway until
    /// the bundle asks for them.
    pub fn open(path: &Path, document: Arc<dyn DocumentApi>) -> Result<Self, String> {
        let capability = manifest_capability(path)?;
        let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        Self::build(&bytes, document, capability, FUEL)
            .map_err(|error| format!("{}: {error}", path.display()))
    }

    /// Open every pack in `paths`, skipping (and reporting) one that will not load.
    ///
    /// A broken bundle is a broken workbench, not a broken app: the desktop keeps the
    /// window the same way the browser keeps the page.
    pub fn open_all(
        paths: &[PathBuf],
        document: Arc<dyn DocumentApi>,
    ) -> (Vec<Self>, Vec<String>) {
        let mut hosts = Vec::new();
        let mut errors = Vec::new();
        for path in paths {
            match Self::open(path, Arc::clone(&document)) {
                Ok(host) => hosts.push(host),
                Err(error) => {
                    log::error!("wasm addon: {error}");
                    errors.push(error);
                }
            }
        }
        (hosts, errors)
    }

    /// The pack's command list, read once at boot.
    fn read_commands(&self) -> Vec<CommandInfo> {
        let Ok(mut bundle) = self.bundle.lock() else {
            return Vec::new();
        };
        match bundle.commands() {
            Ok(json) => match serde_json::from_str::<Vec<PackCommand>>(&json) {
                Ok(entries) => entries.into_iter().map(PackCommand::into_command).collect(),
                Err(error) => {
                    log::error!("wasm addon: bad command list: {error}");
                    Vec::new()
                }
            },
            Err(error) => {
                log::error!("wasm addon: commands: {error}");
                Vec::new()
            }
        }
    }
}

impl Bundle {
    /// Every entry call starts from the same budget, so one bundle cannot starve the next.
    fn arm(&mut self) -> Result<(), String> {
        self.store
            .set_fuel(self.fuel)
            .map_err(|error| format!("wasm fuel: {error}"))
    }

    fn commands(&mut self) -> Result<String, String> {
        self.arm()?;
        let packed = self
            .handles
            .commands
            .call(&mut self.store, ())
            .map_err(|error| format!("ferrocad_commands: {error}"))?;
        self.take(packed)
    }

    fn run(&mut self, id: &str) -> Result<bool, String> {
        self.arm()?;
        let (ptr, len) = self.give(id.as_bytes())?;
        let ran = self.handles.run.call(&mut self.store, (ptr, len as i32));
        let freed = self.handles.dealloc.call(&mut self.store, (ptr, len as i32));
        if let Err(error) = freed {
            log::warn!("wasm addon: ferrocad_dealloc: {error}");
        }
        ran.map(|value| value == 1)
            .map_err(|error| format!("ferrocad_run: {error}"))
    }

    fn stage_meshes(&mut self, have: &str) -> Result<String, String> {
        self.arm()?;
        let (ptr, len) = self.give(have.as_bytes())?;
        let packed = self.handles.stage_meshes.call(&mut self.store, (ptr, len as i32));
        let freed = self.handles.dealloc.call(&mut self.store, (ptr, len as i32));
        if let Err(error) = freed {
            log::warn!("wasm addon: ferrocad_dealloc: {error}");
        }
        self.take(packed.map_err(|error| format!("ferrocad_stage_meshes: {error}"))?)
    }

    /// Copy `bytes` into the bundle's memory with its own allocator.
    fn give(&mut self, bytes: &[u8]) -> Result<(i32, i32), String> {
        let ptr = self
            .handles
            .alloc
            .call(&mut self.store, bytes.len() as i32)
            .map_err(|error| format!("ferrocad_alloc: {error}"))?;
        if !bytes.is_empty() {
            let start = ptr as usize;
            let end = start
                .checked_add(bytes.len())
                .ok_or("ferrocad_alloc: the buffer overflows the address space")?;
            self.handles
                .memory
                .data_mut(&mut self.store)
                .get_mut(start..end)
                .ok_or("ferrocad_alloc: the buffer is outside the bundle's memory")?
                .copy_from_slice(bytes);
        }
        Ok((ptr, bytes.len() as i32))
    }

    /// Copy a packed `(len << 32 | ptr)` answer out, then free it.
    fn take(&mut self, packed: i64) -> Result<String, String> {
        let (ptr, len) = unpack(packed);
        if len <= 0 {
            return Ok(String::new());
        }
        let start = ptr as usize;
        let end = start
            .checked_add(len as usize)
            .ok_or("the answer overflows the address space")?;
        let text = String::from_utf8_lossy(
            self.handles
                .memory
                .data(&self.store)
                .get(start..end)
                .ok_or("the answer is outside the bundle's memory")?,
        )
        .into_owned();
        if let Err(error) = self
            .handles
            .dealloc
            .call(&mut self.store, (ptr, len as i32))
        {
            log::warn!("wasm addon: ferrocad_dealloc: {error}");
        }
        Ok(text)
    }
}

/// The `ferrocad.call` import: the sandbox's only capability.
fn doorway(mut caller: Caller<'_, HostState>, ptr: i32, len: i32) -> i64 {
    let (document, handles) = {
        let state = caller.data();
        match state.handles {
            Some(handles) => (Arc::clone(&state.document), handles),
            None => return 0,
        }
    };

    let request = match read(&caller, &handles, ptr, len) {
        Ok(request) => request,
        Err(error) => {
            log::error!("wasm addon: the doorway cannot read the request: {error}");
            return 0;
        }
    };
    let answer = document.call(&request);

    match write(&mut caller, &handles, answer.as_bytes()) {
        Ok(packed) => packed,
        Err(error) => {
            log::error!("wasm addon: the doorway cannot write the answer: {error}");
            0
        }
    }
}

fn read(caller: &Caller<'_, HostState>, handles: &Handles, ptr: i32, len: i32) -> Result<String, String> {
    if len <= 0 {
        return Ok(String::new());
    }
    let start = ptr as usize;
    let end = start
        .checked_add(len as usize)
        .ok_or("the request overflows the address space")?;
    let bytes = handles
        .memory
        .data(caller)
        .get(start..end)
        .ok_or("the request is outside the bundle's memory")?;
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

fn write(caller: &mut Caller<'_, HostState>, handles: &Handles, bytes: &[u8]) -> Result<i64, String> {
    let ptr = handles
        .alloc
        .call(&mut *caller, bytes.len() as i32)
        .map_err(|error| format!("ferrocad_alloc: {error}"))?;
    let start = ptr as usize;
    let end = start
        .checked_add(bytes.len())
        .ok_or("the answer overflows the address space")?;
    handles
        .memory
        .data_mut(&mut *caller)
        .get_mut(start..end)
        .ok_or("the answer is outside the bundle's memory")?
        .copy_from_slice(bytes);
    Ok(pack(ptr, bytes.len()))
}

/// `(ptr, len)` as the ABI packs it.
fn pack(ptr: i32, len: usize) -> i64 {
    ((len as i64) << 32) | (ptr as u32 as i64)
}

/// The reverse: `(ptr, len)` from a packed answer.
fn unpack(packed: i64) -> (i32, usize) {
    let ptr = (packed as u64 & 0xffff_ffff) as u32 as i32;
    let len = ((packed as u64 >> 32) & 0xffff_ffff) as usize;
    (ptr, len)
}

/// One entry of a bundle's command list — the same JSON the page's addons answer with.
#[derive(Deserialize)]
struct PackCommand {
    id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    tooltip: String,
}

impl PackCommand {
    fn into_command(self) -> CommandInfo {
        CommandInfo {
            menu: if self.label.is_empty() {
                self.id.clone()
            } else {
                self.label
            },
            tooltip: self.tooltip,
            cmd_type: "ForEdit".to_string(),
            group: "Wasm".to_string(),
            surface: "hidden".to_string(),
            enabled: true,
            id: self.id,
            ..Default::default()
        }
    }
}

impl ScriptHost for WasmAddonHost {
    fn evaluate(&self, code: &str) -> Result<EvalResult, String> {
        // A bundle answers commands, not expressions: it has no console by design.
        Ok(EvalResult {
            ok: false,
            output: format!("wasm addon: no console (did not run {code:?})"),
        })
    }

    fn load_workbenches(&self) -> Result<WorkbenchReport, String> {
        Ok(WorkbenchReport::default())
    }

    fn tool(&self, name: &str) -> Result<Box<dyn ferrocad_stage::Tool>, String> {
        Err(format!("wasm addon: no tool {name}"))
    }

    fn tool_command(&self, _name: &str, _verb: &str) -> bool {
        false
    }

    fn inventor_scene(&self) -> Result<Scene, String> {
        Ok(Scene::default())
    }

    fn event_listener(&self) -> Box<dyn EventListener> {
        Box::new(NoopListener)
    }

    fn hello(&self) -> Result<Option<String>, String> {
        Ok(Some(format!(
            "FerroCAD shell · wasm addon ({})",
            self.capability.id()
        )))
    }

    fn stage_meshes(&self, have: &[&str]) -> Option<Result<Vec<StageMesh>, String>> {
        let have = serde_json::to_string(have).ok()?;
        let mut bundle = self.bundle.lock().ok()?;
        match bundle.stage_meshes(&have) {
            Ok(json) => Some(StageMesh::from_json_array(&json)),
            Err(error) => Some(Err(error)),
        }
    }

    fn commands(&self) -> Vec<CommandInfo> {
        self.read_commands()
    }

    fn run_command(&self, id: &str) -> Option<CommandOutcome> {
        // The bundle answers for its own ids only; the list is read once.
        if !self.read_commands().iter().any(|command| command.id == id) {
            return None;
        }
        let Ok(mut bundle) = self.bundle.lock() else {
            return Some(CommandOutcome {
                ok: false,
                error: "wasm addon: the bundle is poisoned".to_string(),
                ..Default::default()
            });
        };
        Some(match bundle.run(id) {
            Ok(true) => CommandOutcome {
                ok: true,
                status: format!("wasm addon: ran {id}"),
                ..Default::default()
            },
            Ok(false) => CommandOutcome {
                ok: false,
                // A refusal is the bundle's own answer, but when it holds *only*
                // read access the likeliest reason is the door: say so.
                error: match self.capability {
                    Capability::Read => format!(
                        "wasm addon: {id} refused (this bundle is read-only: its addon.json \
                         does not grant document writes)"
                    ),
                    Capability::Write => format!("wasm addon: {id} refused"),
                },
                ..Default::default()
            },
            Err(error) => CommandOutcome {
                ok: false,
                error: format!("wasm addon: {error}"),
                ..Default::default()
            },
        })
    }
}

/// One or more script halves plus every wasm addon, as one `ScriptHost`.
///
/// The **first** half is the primary: it owns the console, the workbenches, the tools, the
/// document lifecycle and every other hook. Everything after it is a subordinate — it may
/// answer commands, contribute display rows and hear the document's events, and cannot own
/// what the primary owns. An addon answers commands and the display hook, which is exactly
/// what a bundle can do.
///
/// The three merged answers run in order — the halves in switch order, then the addons in
/// load order:
///
/// * `commands` is each host's list, concatenated;
/// * `run_command` and `claims` take the first host that answers;
/// * `stage_meshes` is the composition's rows, with the primary's answer deciding whether
///   it has a hook at all.
///
/// On the browser the same merge happens in the page (`?addons=`); on the desktop it is
/// here, so an addon works next to CPython rather than instead of it.
pub struct Addons {
    /// The script halves, in the order the switch named them; the first is the primary.
    /// Shared rather than owned: the doorway's sink announces an addon's own edits to the
    /// same half, so both need it (`announce_to`).
    hosts: Vec<Arc<dyn ScriptHost + Send + Sync>>,
    /// The sandboxed bundles. They are hosts too, stored concretely because a
    /// `WasmAddonHost` owns the sandbox's fuel and capability.
    addons: Vec<WasmAddonHost>,
}

impl Addons {
    /// Compose `primary` (which may be `None`) with already-instantiated addons.
    pub fn new(
        primary: Option<Arc<dyn ScriptHost + Send + Sync>>,
        addons: Vec<WasmAddonHost>,
    ) -> Self {
        Self::chain(primary.into_iter().collect(), addons)
    }

    /// Compose `hosts` — the first is the primary — with already-instantiated addons.
    pub fn chain(
        hosts: Vec<Arc<dyn ScriptHost + Send + Sync>>,
        addons: Vec<WasmAddonHost>,
    ) -> Self {
        Self { hosts, addons }
    }

    /// Load every bundle in `paths` under `document`, and compose them behind `primary`.
    ///
    /// `Err` is a report, not a failure: it names the packs that could not load and the
    /// composed host is still installed, because one broken workbench must not cost the
    /// window.
    pub fn boot(
        primary: Option<Arc<dyn ScriptHost + Send + Sync>>,
        paths: &[PathBuf],
        document: Arc<dyn DocumentApi>,
    ) -> (Self, Vec<String>) {
        Self::boot_chain(primary.into_iter().collect(), paths, document)
    }

    /// [`boot`](Self::boot) for a chain: the first host is the primary.
    pub fn boot_chain(
        hosts: Vec<Arc<dyn ScriptHost + Send + Sync>>,
        paths: &[PathBuf],
        document: Arc<dyn DocumentApi>,
    ) -> (Self, Vec<String>) {
        let (addons, errors) = WasmAddonHost::open_all(paths, document);
        (Self::chain(hosts, addons), errors)
    }

    /// Whether this composition answers anything at all.
    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty() && self.addons.is_empty()
    }

    /// The primary: the first script half, if any.
    fn primary(&self) -> Option<&Arc<dyn ScriptHost + Send + Sync>> {
        self.hosts.first()
    }

    /// Every host, the bundles last: the order a merged answer or a fan-out uses.
    fn every(&self) -> impl Iterator<Item = &dyn ScriptHost> {
        self.hosts
            .iter()
            .map(|host| host.as_ref() as &dyn ScriptHost)
            .chain(self.addons.iter().map(|addon| addon as &dyn ScriptHost))
    }

    /// The addons' command rows, in load order.
    fn addon_commands(&self) -> Vec<CommandInfo> {
        self.addons
            .iter()
            .flat_map(WasmAddonHost::read_commands)
            .collect()
    }
}

/// The doorway's sink: an addon's own `create`/`set` is an edit of the document.
///
/// The host gives the bundles a `DocApi` (that is what `ferrocad.call` reaches); this
/// adapter is the sink that watches it, so an addon's edit is announced to the script half
/// the same way the shell's own edits are — the browser's `PageEvents` in the other host.
pub fn announce_to(
    script: Arc<dyn ScriptHost + Send + Sync>,
) -> Arc<dyn ferrocad_shell_api::DocumentEventSink> {
    Arc::new(ToScript(script))
}

struct ToScript(Arc<dyn ScriptHost + Send + Sync>);

impl ferrocad_shell_api::DocumentEventSink for ToScript {
    fn document_event(&self, event: &DocumentEvent) {
        self.0.on_document_event(event);
    }
}

/// Every installed host's listener, run in order.
struct FanOut(Vec<Box<dyn EventListener>>);

impl EventListener for FanOut {
    fn on_event(
        &mut self,
        event: &Event,
        hit: Option<&Hit>,
        scene: &mut Scene,
        directives: &Directives,
    ) -> bool {
        // All of them, not until the first `true`: the flag says "the scene changed", and
        // one host changing it must not hide another host's edit from the same event.
        let mut changed = false;
        for listener in &mut self.0 {
            changed |= listener.on_event(event, hit, scene, directives);
        }
        changed
    }
}

impl ScriptHost for Addons {
    fn evaluate(&self, code: &str) -> Result<EvalResult, String> {
        match self.primary() {
            Some(primary) => primary.evaluate(code),
            None => Ok(EvalResult {
                ok: false,
                output: "this build has no console: only wasm addons are installed".to_string(),
            }),
        }
    }

    fn load_workbenches(&self) -> Result<WorkbenchReport, String> {
        match self.primary() {
            Some(primary) => primary.load_workbenches(),
            None => Ok(WorkbenchReport::default()),
        }
    }

    fn tool(&self, name: &str) -> Result<Box<dyn ferrocad_stage::Tool>, String> {
        match self.primary() {
            Some(primary) => primary.tool(name),
            None => Err(format!("no tool {name}: a wasm addon supplies none")),
        }
    }

    fn tool_command(&self, name: &str, verb: &str) -> bool {
        self.primary()
            .is_some_and(|primary| primary.tool_command(name, verb))
    }

    fn inventor_scene(&self) -> Result<Scene, String> {
        match self.primary() {
            Some(primary) => primary.inventor_scene(),
            None => Ok(Scene::default()),
        }
    }

    fn event_listener(&self) -> Box<dyn EventListener> {
        // Every host hears the event; each one asked for its own listener.
        let listeners: Vec<Box<dyn EventListener>> =
            self.every().map(|host| host.event_listener()).collect();
        match listeners.len() {
            0 => Box::new(NoopListener),
            1 => listeners.into_iter().next().expect("one listener"),
            _ => Box::new(FanOut(listeners)),
        }
    }

    fn bootstrap(&self) -> Result<Option<Bootstrap>, String> {
        match self.primary() {
            Some(primary) => primary.bootstrap(),
            None => Ok(None),
        }
    }

    fn open_startup_document(
        &self,
        boot: &mut Bootstrap,
    ) -> Result<Option<OpenedDocument>, String> {
        match self.primary() {
            Some(primary) => primary.open_startup_document(boot),
            None => Ok(None),
        }
    }

    fn open_path(&self, path: &Path) -> Option<Result<OpenedDocument, String>> {
        self.primary().and_then(|primary| primary.open_path(path))
    }

    fn save_path(&self, path: &Path) -> Option<Result<(), String>> {
        self.primary().and_then(|primary| primary.save_path(path))
    }

    fn activate_document(&self, name: &str) -> Option<Result<(), String>> {
        self.primary()
            .and_then(|primary| primary.activate_document(name))
    }

    fn hello(&self) -> Result<Option<String>, String> {
        match self.primary() {
            Some(primary) => primary.hello(),
            None => Ok(Some(format!(
                "FerroCAD shell · {} wasm addon(s)",
                self.addons.len()
            ))),
        }
    }

    fn stage_meshes(&self, have: &[&str]) -> Option<Result<Vec<StageMesh>, String>> {
        // Answers in order: the script halves, then the bundles. The primary's answer is
        // what decides whether the composition has a display hook at all — a `None` there
        // leaves the backend's own rows in place until some host actually contributes one.
        let mut answers: Vec<Option<Result<Vec<StageMesh>, String>>> = Vec::new();
        if let Some(primary) = self.primary() {
            answers.push(primary.stage_meshes(have));
        }
        answers.extend(self.addons.iter().map(|addon| addon.stage_meshes(have)));

        let has_primary = self.primary().is_some();
        let mut rows = Vec::new();
        let mut failed = None;
        let mut primary_answered = false;
        for (index, answer) in answers.into_iter().enumerate() {
            match answer {
                Some(Ok(mut meshes)) => {
                    primary_answered |= has_primary && index == 0;
                    rows.append(&mut meshes);
                }
                Some(Err(error)) => {
                    failed.get_or_insert(error);
                }
                None => {}
            }
        }

        match failed {
            // One failing host speaks for the composition: no host can vouch for the row
            // set when a sibling could not produce its own.
            Some(error) => Some(Err(error)),
            None if primary_answered || !rows.is_empty() => Some(Ok(rows)),
            None => None,
        }
    }

    fn apply_interaction(&self, commit: &InteractionCommit, object: &str) -> Option<ApplyResult> {
        self.primary()
            .and_then(|primary| primary.apply_interaction(commit, object))
    }

    fn add_datum_plane(
        &self,
        origin: Vector3,
        u: Vector3,
        v: Vector3,
    ) -> Option<Result<String, String>> {
        self.primary()
            .and_then(|primary| primary.add_datum_plane(origin, u, v))
    }

    fn extra_tree_nodes(&self) -> Vec<DocumentNode> {
        match self.primary() {
            Some(primary) => primary.extra_tree_nodes(),
            None => Vec::new(),
        }
    }

    fn extra_properties(&self, object: &str) -> Option<Vec<PropRow>> {
        self.primary()
            .and_then(|primary| primary.extra_properties(object))
    }

    fn set_extra_property(&self, object: &str, property: &str, value: &str) -> Option<SetResult> {
        self.primary()
            .and_then(|primary| primary.set_extra_property(object, property, value))
    }

    fn on_selection_changed(&self, items: &[SelectionItem]) {
        for host in self.every() {
            host.on_selection_changed(items);
        }
    }

    fn take_selection_requests(&self) -> Vec<SelectionRequest> {
        match self.primary() {
            Some(primary) => primary.take_selection_requests(),
            None => Vec::new(),
        }
    }

    fn on_view_state_changed(&self, state: &ViewState) {
        for host in self.every() {
            host.on_view_state_changed(state);
        }
    }

    fn on_document_event(&self, event: &DocumentEvent) {
        for host in self.every() {
            host.on_document_event(event);
        }
    }

    fn appearance_view(&self) -> Option<CommandOutcome> {
        self.primary().and_then(|primary| primary.appearance_view())
    }

    fn grid_summary(&self) -> Option<GridSummary> {
        self.primary().and_then(|primary| primary.grid_summary())
    }

    fn axis_summary(&self) -> Option<AxisSummary> {
        self.primary().and_then(|primary| primary.axis_summary())
    }

    fn provider_features(&self) -> Vec<ProviderFeature> {
        match self.primary() {
            Some(primary) => primary.provider_features(),
            None => Vec::new(),
        }
    }

    fn set_provider(&self, feature: &str, backend: &str) -> Option<CommandOutcome> {
        self.primary()
            .and_then(|primary| primary.set_provider(feature, backend))
    }

    fn refresh_providers(&self, feature: &str) -> Option<CommandOutcome> {
        self.primary()
            .and_then(|primary| primary.refresh_providers(feature))
    }

    fn provider_skin(&self, feature: &str) -> Option<String> {
        self.primary()
            .and_then(|primary| primary.provider_skin(feature))
    }

    fn manipulator_specs(
        &self,
        object: &str,
        element: Option<&str>,
    ) -> Option<Vec<serde_json::Value>> {
        self.primary()
            .and_then(|primary| primary.manipulator_specs(object, element))
    }

    fn dialog_accept(&self, token: &str, values: &serde_json::Value) -> Option<CommandOutcome> {
        self.primary()
            .and_then(|primary| primary.dialog_accept(token, values))
    }

    fn dialog_cancel(&self, token: &str) -> Option<CommandOutcome> {
        self.primary()
            .and_then(|primary| primary.dialog_cancel(token))
    }

    fn gpu_adapter(&self) -> Option<String> {
        self.primary().and_then(|primary| primary.gpu_adapter())
    }

    fn set_gpu_adapter(&self, value: &str) -> Option<GpuPreferenceOutcome> {
        self.primary()
            .and_then(|primary| primary.set_gpu_adapter(value))
    }

    fn commands(&self) -> Vec<CommandInfo> {
        let mut rows: Vec<CommandInfo> =
            self.hosts.iter().flat_map(|host| host.commands()).collect();
        rows.extend(self.addon_commands());
        rows
    }

    fn run_command(&self, id: &str) -> Option<CommandOutcome> {
        // First host to answer wins, in load order: the primary, then the subordinates,
        // then the bundles.
        self.every().find_map(|host| host.run_command(id))
    }

    fn claims(&self, id: &str) -> Option<CommandOutcome> {
        self.every().find_map(|host| host.claims(id))
    }
}
