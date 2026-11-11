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

use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use ferrocad_inventor::Scene;
use ferrocad_python_host::{PythonHost, PythonRuntime};
use ferrocad_shell_api::DocumentGrant;
use ferrocad_stage::Tool;

use crate::{HELLO, no_tool};

/// The stack the interpreter's compiler needs (see the module docs).
pub const STACK: usize = 16 << 20;

/// The pywasm backend's script half on a worker thread.
pub type ThreadedScriptHost = PythonHost<ThreadedRuntime>;

/// Boot RustPython on its own thread, over the process-wide document registry.
///
/// The interpreter has to be bound to the same registry the document half answers from
/// (`RustAppBackend::from_shared_registry`), which is what makes `FreeCAD.ActiveDocument`
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

/// One call into the interpreter.
struct Job {
    function: String,
    args: Vec<String>,
    reply: Sender<Result<String, String>>,
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
            .send(Job {
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

    /// Read-only: the bridge behind it exposes one function, and it reads.
    fn document_grant(&self) -> DocumentGrant {
        DocumentGrant::Read
    }

    /// The console's greeting: this is an interpreter, unlike the plain Rust half.
    fn hello(&self) -> Result<Option<String>, String> {
        Ok(Some(HELLO.to_string()))
    }

    fn tool(&self, name: &str) -> Result<Box<dyn Tool>, String> {
        Err(no_tool(name))
    }

    fn tool_command(&self, _name: &str, _verb: &str) -> bool {
        false
    }

    fn inventor_scene(&self) -> Result<Scene, String> {
        // `ferrocad_shell` builds its fallback scene through `pivy.coin`, which the facade
        // does not supply; the document's own meshes are the path here.
        Ok(Scene::default())
    }
}

/// The worker thread: boot once, then answer calls until the host is dropped.
fn run(inbox: Receiver<Job>, ready: Sender<Result<(), String>>) {
    let host = match crate::live() {
        Ok(host) => host,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let _ = ready.send(Ok(()));

    while let Ok(job) = inbox.recv() {
        let args: Vec<&str> = job.args.iter().map(String::as_str).collect();
        let _ = job.reply.send(host.call(&job.function, &args));
    }
}
