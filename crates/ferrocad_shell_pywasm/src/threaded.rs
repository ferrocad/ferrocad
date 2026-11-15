//! RustPython on a worker thread: the `ScriptHost` a desktop can install.
//!
//! Two facts force this shape. The VM is not `Send`, and its compiler recurses, so a host
//! has to raise the stack *before* the interpreter starts (`ferrocad_shell_web`'s test and
//! the browser's link argument both use 16 MiB). A desktop shell cannot raise its own:
//! GPUI owns the main thread, and the platform gives that thread 8 MiB on Linux and macOS
//! and 1 MiB on Windows.
//!
//! So the interpreter lives on a worker with a 16 MiB stack, and [`ThreadedRuntime`] is the
//! `PythonRuntime` in front of it: the one call primitive crosses the channel, and the
//! answers that do not need the interpreter (the greeting, the grant, the tool refusal)
//! stay local. The shared host above it is the same [`PythonHost`] the browser installs.

// wasm-audit: native-only — declared behind `#[cfg(not(target_family = "wasm"))]` in
// `lib.rs`; the browser boots the interpreter on its own thread and raises that stack as
// a link argument.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use ferrocad_inventor::Scene;
use ferrocad_python_host::{Contract, PythonHost, PythonRuntime};
use ferrocad_shell_api::{DocumentGrant, ScriptHost};
use ferrocad_stage as fs;

use crate::{HELLO_READ_WRITE, no_tool};

/// The stack the interpreter's compiler needs (see the module docs).
pub const STACK: usize = 16 << 20;

/// The pywasm backend's script half on a worker thread.
pub type ThreadedScriptHost = PythonHost<ThreadedRuntime>;

/// Boot RustPython on its own thread, over the process-wide document registry.
///
/// The interpreter has to be bound to the same registry the document half answers from
/// (`DocumentBackend::from_shared_registry`), which is what makes `FreeCAD.ActiveDocument`
/// and the model tree one document.
pub fn live() -> Result<ThreadedScriptHost, String> {
    ThreadedRuntime::live().map(PythonHost::new)
}

/// The channel side of the runtime: every call is a job for the interpreter thread.
pub struct ThreadedRuntime {
    inner: Mutex<Worker>,
}

/// The channel to the interpreter, which is shut down when this is dropped.
struct Worker {
    /// `None` until the host is dropped: closing it is what ends the worker's `recv`.
    jobs: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

/// One job for the interpreter thread.
enum Job {
    /// Call `function` in the shell module.
    Call {
        function: String,
        args: Vec<String>,
        reply: Sender<Result<String, String>>,
    },
    /// Whether the entry module defines `function` (the runtime sees the module table,
    /// the channel's caller does not).
    HasFunction {
        function: String,
        reply: Sender<bool>,
    },
    /// Open a Python-backed tool and keep it on the interpreter thread.
    ToolOpen {
        name: String,
        reply: Sender<Result<ToolReply, String>>,
    },
    /// Forward a pointer event to the live tool.
    ToolEvent {
        name: String,
        event: fs::PointerEvent,
        snap: Option<fs::Snap>,
        reply: Sender<Result<ToolReply, String>>,
    },
    /// Forward a session verb (cancel/finish) to the Python bridge.
    ToolCommand {
        name: String,
        verb: String,
        reply: Sender<bool>,
    },
}

/// What a tool answered after one call: its transient visuals and its status.
struct ToolReply {
    items: Vec<fs::ShapeItem>,
    status: fs::ToolStatus,
}

impl ToolReply {
    fn of(tool: &dyn fs::Tool) -> Self {
        Self {
            items: tool.items().to_vec(),
            status: tool.status(),
        }
    }
}

/// A Python-backed tool that lives on the interpreter thread.
///
/// A tool is not `Send` (it owns Python objects, which the VM is not), so the caller
/// holds this proxy instead: every event crosses the channel and the worker keeps the
/// live adapter. The items and status are cached here, refreshed after each call, so
/// the viewport's per-frame `items()` is a slice and never a round trip.
pub struct RemoteTool {
    name: String,
    jobs: Sender<Job>,
    items: Vec<fs::ShapeItem>,
    status: fs::ToolStatus,
}

impl RemoteTool {
    fn accept(&mut self, reply: ToolReply) {
        self.items = reply.items;
        self.status = reply.status;
    }

    fn send_event(&mut self, event: fs::PointerEvent, snap: Option<fs::Snap>) {
        let (reply, answer) = mpsc::channel();
        if self
            .jobs
            .send(Job::ToolEvent {
                name: self.name.clone(),
                event,
                snap,
                reply,
            })
            .is_err()
        {
            return;
        }
        if let Ok(Ok(step)) = answer.recv() {
            self.accept(step);
        }
    }
}

impl fs::Tool for RemoteTool {
    fn on_event(&mut self, event: &fs::PointerEvent, snap: Option<fs::Snap>) {
        self.send_event(*event, snap);
    }

    fn items(&self) -> &[fs::ShapeItem] {
        &self.items
    }

    fn status(&self) -> fs::ToolStatus {
        self.status.clone()
    }

    fn cancel(&mut self) -> bool {
        let (reply, answer) = mpsc::channel();
        if self
            .jobs
            .send(Job::ToolCommand {
                name: self.name.clone(),
                verb: "cancel".to_string(),
                reply,
            })
            .is_err()
        {
            return false;
        }
        answer.recv().unwrap_or(false)
    }
}

impl ThreadedRuntime {
    /// Start the worker and wait for it to boot.
    ///
    /// A boot failure is reported here, not on the first call: a host that cannot start
    /// must fail where the composition can fall back to the document half.
    pub fn live() -> Result<Self, String> {
        let (jobs, inbox) = mpsc::channel();
        let (ready, booted) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("ferrocad-rustpython".to_string())
            .stack_size(STACK)
            .spawn(move || run(inbox, ready))
            .map_err(|error| format!("the RustPython thread did not start: {error}"))?;

        booted
            .recv()
            .map_err(|_| "the RustPython thread stopped before it booted".to_string())??;

        Ok(Self {
            inner: Mutex::new(Worker {
                jobs: Some(jobs),
                thread: Some(thread),
            }),
        })
    }

    /// Ask the worker whether its entry module defines `function`.
    fn has_blocking(&self, function: &str) -> bool {
        let Ok(worker) = self.inner.lock() else {
            return false;
        };
        let Some(jobs) = worker.jobs.as_ref() else {
            return false;
        };
        let (reply, answer) = mpsc::channel();
        if jobs
            .send(Job::HasFunction {
                function: function.to_string(),
                reply,
            })
            .is_err()
        {
            return false;
        }
        answer.recv().unwrap_or(false)
    }

    /// Send one call to the interpreter thread and wait for its answer.
    fn call_blocking(&self, function: &str, args: &[&str]) -> Result<String, String> {
        let worker = self
            .inner
            .lock()
            .map_err(|_| "the RustPython thread is poisoned".to_string())?;
        let (reply, answer) = mpsc::channel();
        worker
            .jobs
            .as_ref()
            .ok_or_else(|| "the RustPython thread is gone".to_string())?
            .send(Job::Call {
                function: function.to_string(),
                args: args.iter().map(|arg| (*arg).to_string()).collect(),
                reply,
            })
            .map_err(|_| "the RustPython thread is gone".to_string())?;
        answer
            .recv()
            .map_err(|_| "the RustPython thread is gone".to_string())?
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Close the channel *before* joining: the worker leaves `recv` only when the last
        // sender is gone, and this struct is that sender.
        drop(self.jobs.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl PythonRuntime for ThreadedRuntime {
    fn call(&self, function: &str, args: &[&str]) -> Result<String, String> {
        self.call_blocking(function, args)
    }

    /// Asked on the worker: `add_datum_plane` belongs to the edition's app script, and
    /// only the interpreter can see whether it is there.
    fn has_function(&self, function: &str) -> bool {
        self.has_blocking(function)
    }

    /// The whole facade. The worker is [`crate::live_persistent`], so its parameter
    /// store is the app's file (B18b) and `view` answers the appearance the user
    /// actually saved, which is what made `NONE` necessary before.
    fn contract(&self) -> Contract {
        Contract::FULL
    }

    /// Read-write: the worker boots [`crate::live_persistent`], so its document is the
    /// process-wide registry the doorway writes (B18b item 5). The thread-local source
    /// belongs to the worker, so this answers from what that boot *installs*, not from
    /// the caller's thread.
    fn document_grant(&self) -> DocumentGrant {
        DocumentGrant::ReadWrite
    }

    /// The console's greeting: this is an interpreter, unlike the plain Rust half.
    ///
    /// A constant, because only `evaluate` crosses the channel; the worker boots
    /// [`crate::live_persistent`], whose grant is `ReadWrite`.
    fn hello(&self) -> Result<Option<String>, String> {
        Ok(Some(HELLO_READ_WRITE.to_string()))
    }

    fn tool(&self, name: &str) -> Result<Box<dyn fs::Tool>, String> {
        let worker = self
            .inner
            .lock()
            .map_err(|_| "the RustPython thread is poisoned".to_string())?;
        let jobs = worker
            .jobs
            .as_ref()
            .ok_or_else(|| "the RustPython thread is gone".to_string())?
            .clone();
        let (reply, answer) = mpsc::channel();
        jobs.send(Job::ToolOpen {
            name: name.to_string(),
            reply,
        })
        .map_err(|_| "the RustPython thread is gone".to_string())?;
        // Release the runtime lock while the worker builds the tool.
        drop(worker);
        let step = answer
            .recv()
            .map_err(|_| "the RustPython thread is gone".to_string())??;
        Ok(Box::new(RemoteTool {
            name: name.to_string(),
            jobs,
            items: step.items,
            status: step.status,
        }))
    }

    fn tool_command(&self, name: &str, verb: &str) -> bool {
        let Ok(worker) = self.inner.lock() else {
            return false;
        };
        let Some(jobs) = worker.jobs.as_ref() else {
            return false;
        };
        let (reply, answer) = mpsc::channel();
        if jobs
            .send(Job::ToolCommand {
                name: name.to_string(),
                verb: verb.to_string(),
                reply,
            })
            .is_err()
        {
            return false;
        }
        answer.recv().unwrap_or(false)
    }

    fn inventor_scene(&self) -> Result<Scene, String> {
        // `ferrocad_shell` builds its fallback scene through `pivy.coin`, which the facade
        // does not supply; the document's own meshes are the path here.
        Ok(Scene::default())
    }
}

/// The worker thread: boot once, then answer calls until the host is dropped.
fn run(inbox: Receiver<Job>, ready: Sender<Result<(), String>>) {
    let host = match crate::live_persistent() {
        Ok(host) => host,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    // The live tools stay here, where their Python objects may live.
    let mut tools: HashMap<String, Box<dyn fs::Tool>> = HashMap::new();

    while let Ok(job) = inbox.recv() {
        match job {
            Job::Call {
                function,
                args,
                reply,
            } => {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                let _ = reply.send(host.call(&function, &args));
            }
            Job::HasFunction { function, reply } => {
                let _ = reply.send(host.runtime().has_function(&function));
            }
            Job::ToolOpen { name, reply } => {
                let answer = match host.tool(&name) {
                    Ok(tool) => {
                        let step = ToolReply::of(tool.as_ref());
                        tools.insert(name, tool);
                        Ok(step)
                    }
                    Err(error) => Err(error),
                };
                let _ = reply.send(answer);
            }
            Job::ToolEvent {
                name,
                event,
                snap,
                reply,
            } => {
                let answer = match tools.get_mut(&name) {
                    Some(tool) => {
                        tool.on_event(&event, snap);
                        Ok(ToolReply::of(tool.as_ref()))
                    }
                    None => Err(no_tool(&name)),
                };
                let _ = reply.send(answer);
            }
            Job::ToolCommand { name, verb, reply } => {
                let mut handled = host.tool_command(&name, &verb);
                // `cancel` also has to clear the tool's own transient visuals; the
                // Python bridge action is idempotent, so both paths are safe. Either
                // one reporting a cancelled session is the honest answer.
                if verb == "cancel" {
                    if let Some(tool) = tools.get_mut(&name) {
                        handled |= tool.cancel();
                    }
                }
                let _ = reply.send(handled);
            }
        }
    }
}
