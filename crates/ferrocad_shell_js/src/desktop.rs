//! The desktop page: the same `__ferrocadJs` contract, over an embedded QuickJS.
//!
//! The browser's page is an ES module the *page* loads (`www/js-demo.js`, `?js=1`).
//! The desktop has no page, so it supplies one: a module per workbench, found the way
//! the Python side finds `InitGui.py` — `mods/<Name>/InitGui.js` under the same
//! directories ([`discover`], `FERROCAD_MODS_PATH`) — and each module's `install()`
//! is called in name order. A module that only defines `__ferrocadJs` and never edits
//! the document needs nothing else; one that edits it calls `ferrocad.call(request)`,
//! which is the host's one document, exactly as in the browser.
//!
//! **One thread owns the engine.** QuickJS values are tied to their runtime and are
//! not `Send`, so the runtime lives on a worker thread and every [`Page`] call is a
//! job on a channel. Two calls are *queued* rather than answered — viewport events and
//! document events. A document event can be emitted from inside a doorway call (an
//! addon's own `create`), i.e. while the engine is running, and answering it there
//! would re-enter the runtime; queueing it delivers the event after the current job
//! instead.
//!
//! **No imports yet.** A module is evaluated on its own, so it cannot `import` another
//! file. The browser page's module has no imports either; a resolver is the next step
//! if a workbench wants to split.

// wasm-audit: native-only — this module is behind `ferrocad_shell_js`'s `desktop`
// feature, which no browser build enables; the worker thread is the engine's own.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use rquickjs::function::IntoArgs;
use rquickjs::{Context, FromJs, Function, Module, Object, Runtime};

use super::Page;

/// The page's globals, before any module runs.
///
/// The browser's modules are written against `window`, and they register themselves by
/// defining `window.__ferrocadJs`; both names are the global object here.
const BOOTSTRAP: &str = "globalThis.window = globalThis;";

/// The entry file of a JS workbench, the mirror of the Python side's `InitGui.py`.
pub const ENTRY: &str = "InitGui.js";

/// The JS workbenches under `dirs`: `<dir>/<Name>/InitGui.js`, in name order.
///
/// A workbench without the entry file is not a JS workbench and is skipped — the same
/// way `InitGui.py` decides it on the Python side.
pub fn js_modules(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut modules = Vec::new();
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
        modules.extend(found);
    }
    modules
}

/// Where to look for workbenches: `FERROCAD_MODS_PATH`, else a `mods/` directory
/// beside the executable or up from the working directory.
///
/// This mirrors `ferrocad_shell_python`'s rule for its `mods/` directory without
/// depending on it: the host that composes an [`EnginePage`] can also pass its own
/// directories to [`js_modules`] and skip this entirely.
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
                if !js_modules(std::slice::from_ref(&candidate)).is_empty() {
                    return vec![candidate];
                }
            }
        }
    }
    if let Ok(mut dir) = std::env::current_dir() {
        loop {
            if !js_modules(std::slice::from_ref(&dir.join("mods"))).is_empty() {
                return vec![dir.join("mods")];
            }
            if !dir.pop() {
                break;
            }
        }
    }
    Vec::new()
}

/// The desktop's page: a QuickJS runtime on its own thread.
pub struct EnginePage {
    inner: Mutex<Engine>,
    /// The workbenches this page loaded, in load order.
    modules: Vec<PathBuf>,
}

/// The channel to the engine thread, which is joined when the page is dropped.
struct Engine {
    /// `Some` until the page is dropped: closing it is what ends the worker's `recv`.
    jobs: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

/// One call into the engine. The answer goes back on the job's own channel, so a
/// [`Page`] method blocks only for its own call.
enum Job {
    Active(Sender<Result<bool, String>>),
    Commands(Sender<Result<String, String>>),
    Evaluate(String, Sender<Result<String, String>>),
    RunCommand(String, Sender<Result<bool, String>>),
    StageMeshes(String, Sender<Result<String, String>>),
    /// Hand the page the doorway. No reply: the function is set up and the call
    /// returns, which is all the caller needs to know.
    InstallKernel(Arc<dyn Fn(String) -> String + Send + Sync>),
    /// A viewport event. Queued: the shell is drawing, and JS is not allowed to
    /// stall the frame.
    Event {
        kind: String,
        x: i32,
        y: i32,
        entity: u32,
    },
    /// A document event. Queued, because it can arrive from inside a doorway call.
    DocumentEvent { id: String, payload: String },
}

impl EnginePage {
    /// A page over `modules`, in the order given.
    ///
    /// The modules are loaded and their `install()` called before this returns, so a
    /// broken workbench is an error here rather than four frames later.
    pub fn new(modules: Vec<PathBuf>) -> Result<Self, String> {
        let (jobs, inbox) = mpsc::channel();
        let (ready, booted) = mpsc::channel();
        let loading = modules.clone();
        let thread = std::thread::Builder::new()
            .name("ferrocad-js".to_string())
            .spawn(move || run(loading, inbox, ready))
            .map_err(|error| format!("the JS engine thread did not start: {error}"))?;

        booted
            .recv()
            .map_err(|_| "the JS engine stopped before it booted".to_string())??;

        Ok(Self {
            inner: Mutex::new(Engine {
                jobs: Some(jobs),
                thread: Some(thread),
            }),
            modules,
        })
    }

    /// A page whose workbenches are the ones `mods/` yields ([`discover`]).
    pub fn from_mods_dir() -> Result<Self, String> {
        Self::new(js_modules(&discover()))
    }

    /// A page with no modules: `evaluate` works, nothing is registered.
    pub fn empty() -> Result<Self, String> {
        Self::new(Vec::new())
    }

    /// The workbenches this page loaded, in load order.
    pub fn modules(&self) -> &[PathBuf] {
        &self.modules
    }

    /// One call, and its answer.
    fn round_trip<T>(
        &self,
        make: impl FnOnce(Sender<Result<T, String>>) -> Job,
    ) -> Result<T, String> {
        let engine = self
            .inner
            .lock()
            .map_err(|_| "the JS engine is poisoned".to_string())?;
        let (reply, answer) = mpsc::channel();
        engine
            .jobs
            .as_ref()
            .ok_or_else(|| "the JS engine is gone".to_string())?
            .send(make(reply))
            .map_err(|_| "the JS engine is gone".to_string())?;
        answer
            .recv()
            .map_err(|_| "the JS engine is gone".to_string())?
    }

    /// A call with no answer: queued in order behind whatever the engine is doing.
    fn fire(&self, make: impl FnOnce() -> Job) {
        if let Ok(engine) = self.inner.lock() {
            if let Some(jobs) = engine.jobs.as_ref() {
                let _ = jobs.send(make());
            }
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Close the channel *before* joining: the worker leaves `recv` only when the
        // last sender is gone, and this struct is that sender.
        drop(self.jobs.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Page for EnginePage {
    fn active(&self) -> Result<bool, String> {
        self.round_trip(Job::Active)
    }

    fn commands(&self) -> Result<String, String> {
        self.round_trip(Job::Commands)
    }

    fn evaluate(&self, code: &str) -> Result<String, String> {
        self.round_trip(|reply| Job::Evaluate(code.to_string(), reply))
    }

    fn run_command(&self, id: &str) -> Result<bool, String> {
        self.round_trip(|reply| Job::RunCommand(id.to_string(), reply))
    }

    fn on_event(&self, kind: &str, x: i32, y: i32, entity: u32) -> Result<(), String> {
        self.fire(move || Job::Event {
            kind: kind.to_string(),
            x,
            y,
            entity,
        });
        Ok(())
    }

    fn on_document_event(&self, id: &str, payload: &str) -> Result<(), String> {
        self.fire(move || Job::DocumentEvent {
            id: id.to_string(),
            payload: payload.to_string(),
        });
        Ok(())
    }

    fn stage_meshes(&self, have: &str) -> Result<String, String> {
        self.round_trip(|reply| Job::StageMeshes(have.to_string(), reply))
    }

    fn install_kernel(
        &self,
        call: Arc<dyn Fn(String) -> String + Send + Sync>,
    ) -> Result<(), String> {
        let engine = self
            .inner
            .lock()
            .map_err(|_| "the JS engine is poisoned".to_string())?;
        engine
            .jobs
            .as_ref()
            .ok_or_else(|| "the JS engine is gone".to_string())?
            .send(Job::InstallKernel(call))
            .map_err(|_| "the JS engine is gone".to_string())
    }
}

/// The engine thread: boot once, then answer jobs until the page is dropped.
fn run(modules: Vec<PathBuf>, inbox: Receiver<Job>, ready: Sender<Result<(), String>>) {
    let runtime = match Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = ready.send(Err(format!("QuickJS did not start: {error}")));
            return;
        }
    };
    let context = match Context::full(&runtime) {
        Ok(context) => context,
        Err(error) => {
            let _ = ready.send(Err(format!("QuickJS has no context: {error}")));
            return;
        }
    };

    let booted = context.with(|ctx| {
        ctx.eval::<(), _>(BOOTSTRAP)
            .map_err(|error| describe(&ctx, "the page bootstrap", error))?;
        for path in &modules {
            install(&ctx, path)?;
        }
        Ok(())
    });
    if let Err(error) = booted {
        let _ = ready.send(Err(error));
        return;
    }
    let _ = ready.send(Ok(()));

    while let Ok(job) = inbox.recv() {
        match job {
            Job::Active(reply) => {
                let answer = context.with(|ctx| match page(&ctx) {
                    // A page that never installed itself is simply not available.
                    Err(_) => Ok(false),
                    Ok(page) => {
                        let Ok(function) = page.get::<_, Function>("active") else {
                            return Ok(false);
                        };
                        function
                            .call::<(), bool>(())
                            .map_err(|error| describe(&ctx, "active", error))
                    }
                });
                let _ = reply.send(answer);
            }
            Job::Commands(reply) => {
                let _ = reply.send(context.with(|ctx| call_page(&ctx, "commands", ())));
            }
            Job::Evaluate(code, reply) => {
                let _ = reply.send(context.with(|ctx| call_page(&ctx, "evaluate", (code,))));
            }
            Job::RunCommand(id, reply) => {
                let _ = reply.send(context.with(|ctx| call_page(&ctx, "runCommand", (id,))));
            }
            Job::StageMeshes(have, reply) => {
                let _ = reply.send(context.with(|ctx| call_page(&ctx, "stageMeshes", (have,))));
            }
            Job::InstallKernel(call) => {
                let installed = context.with(|ctx| -> Result<(), String> {
                    let kernel = Object::new(ctx.clone())
                        .map_err(|error| describe(&ctx, "the doorway", error))?;
                    let doorway = Function::new(ctx.clone(), move |request: String| -> String {
                        call(request)
                    })
                    .map_err(|error| describe(&ctx, "the doorway", error))?;
                    kernel
                        .set("call", doorway)
                        .map_err(|error| describe(&ctx, "the doorway", error))?;
                    ctx.globals()
                        .set("ferrocad", kernel)
                        .map_err(|error| describe(&ctx, "the doorway", error))
                });
                if let Err(error) = installed {
                    log::error!("ferrocad js: {error}");
                }
            }
            Job::Event {
                kind,
                x,
                y,
                entity,
            } => {
                let answer = context.with(|ctx| call_optional(&ctx, "onEvent", (kind, x, y, entity)));
                if let Err(error) = answer {
                    log::debug!("ferrocad js: onEvent: {error}");
                }
            }
            Job::DocumentEvent { id, payload } => {
                let answer = context.with(|ctx| call_optional(&ctx, "onDocumentEvent", (id, payload)));
                if let Err(error) = answer {
                    log::debug!("ferrocad js: onDocumentEvent: {error}");
                }
            }
        }
    }
}

/// Evaluate one workbench module and call its `install()`.
fn install(ctx: &rquickjs::Ctx<'_>, path: &Path) -> Result<(), String> {
    let source = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let name = path.display().to_string();

    // ES module evaluation is a promise even when the module imports nothing, so the
    // module is declared first (to keep the namespace), evaluated, and the job queue
    // driven until it settles.
    let module = Module::declare(ctx.clone(), name.clone(), source)
        .map_err(|error| describe(ctx, &name, error))?;
    let (module, settled) = module
        .eval()
        .map_err(|error| describe(ctx, &name, error))?;
    settled
        .finish::<()>()
        .map_err(|error| describe(ctx, &format!("{name} did not settle"), error))?;

    let install: Function = module
        .get("install")
        .map_err(|error| describe(ctx, &format!("{name}: install()"), error))?;
    install
        .call::<(), ()>(())
        .map_err(|error| describe(ctx, &format!("{name}: install()"), error))
}

/// The page object a module installed.
fn page<'js>(ctx: &rquickjs::Ctx<'js>) -> Result<Object<'js>, String> {
    ctx.globals().get::<_, Object>("__ferrocadJs").map_err(|_| {
        "no __ferrocadJs: no workbench ran install()".to_string()
    })
}

/// Call one of the page's functions and decode its answer.
fn call_page<'js, A, R>(ctx: &rquickjs::Ctx<'js>, name: &str, args: A) -> Result<R, String>
where
    A: IntoArgs<'js>,
    R: FromJs<'js>,
{
    let page = page(ctx)?;
    let function: Function = page
        .get(name)
        .map_err(|_| format!("the page has no {name}"))?;
    function
        .call::<A, R>(args)
        .map_err(|error| describe(ctx, name, error))
}

/// Call a page function that may not be installed: a page with no hook is silent.
fn call_optional<'js, A>(ctx: &rquickjs::Ctx<'js>, name: &str, args: A) -> Result<(), String>
where
    A: IntoArgs<'js>,
{
    let page = page(ctx)?;
    let Ok(function) = page.get::<_, Function>(name) else {
        return Ok(());
    };
    function
        .call::<A, ()>(args)
        .map_err(|error| describe(ctx, name, error))
}

/// An error as the workbench's author needs to see it: the message and the stack when
/// the runtime threw.
fn describe(ctx: &rquickjs::Ctx<'_>, what: &str, error: rquickjs::Error) -> String {
    if !matches!(error, rquickjs::Error::Exception) {
        return format!("{what}: {error}");
    }
    let caught = ctx.catch();
    match caught.as_exception() {
        Some(exception) => {
            let message = exception.message().unwrap_or_default();
            match exception.stack() {
                Some(stack) => format!("{what}: {message}\n{stack}"),
                None => format!("{what}: {message}"),
            }
        }
        None => format!("{what}: {caught:?}"),
    }
}
