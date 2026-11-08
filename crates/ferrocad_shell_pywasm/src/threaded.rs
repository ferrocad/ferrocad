//! RustPython on a worker thread: the `ScriptHost` a desktop can install.
//!
//! Two facts force this shape. The VM is not `Send`, and its compiler recurses, so a host
//! has to raise the stack *before* the interpreter starts (`ferrocad_shell_web`'s test and
//! the browser's link argument both use 16 MiB). A desktop shell cannot raise its own:
//! GPUI owns the main thread, and the platform gives that thread 8 MiB on Linux and macOS
//! and 1 MiB on Windows.
//!
//! So the interpreter lives on a worker with a 16 MiB stack, and this type is the
//! `ScriptHost` in front of it. Only
//! [`evaluate`](ferrocad_shell_api::app::ScriptHost::evaluate) crosses the channel: the
//! rest of this spike's answers are constants — no tools, no catalogue, no scene, no event
//! callbacks — so they are answered where the caller is rather than marshalled.

// wasm-audit: native-only — declared behind `#[cfg(not(target_family = "wasm"))]` in
// `lib.rs`; the browser boots the interpreter on its own thread and raises that stack as
// a link argument.

use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use ferrocad_inventor::{EventListener, Scene};
use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_api::commands::{CommandInfo, CommandOutcome};
use ferrocad_shell_api::types::{EvalResult, WorkbenchReport};

use crate::{HELLO, NoopScriptListener, PywasmScriptHost, no_tool};

/// The stack the interpreter's compiler needs (see the module docs).
pub const STACK: usize = 16 << 20;

/// RustPython behind a channel, for a host whose own thread is not the interpreter's.
pub struct ThreadedScriptHost {
    inner: Mutex<Worker>,
}

/// The channel to the interpreter, which is shut down when this is dropped.
struct Worker {
    /// `None` until the host is dropped: closing it is what ends the worker's `recv`.
    jobs: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

/// One call into the interpreter.
enum Job {
    /// A console line, and where its answer goes.
    Evaluate(String, Sender<Result<EvalResult, String>>),
}

impl ThreadedScriptHost {
    /// Boot RustPython on its own thread, over the process-wide document registry.
    ///
    /// The interpreter has to be bound to the same registry the document half answers
    /// from (`RustAppBackend::from_shared_registry`), which is what makes
    /// `FreeCAD.ActiveDocument` and the model tree one document.
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

/// The worker thread: boot once, then answer console lines until the host is dropped.
fn run(inbox: Receiver<Job>, ready: Sender<Result<(), String>>) {
    let host = match PywasmScriptHost::live() {
        Ok(host) => host,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let _ = ready.send(Ok(()));

    while let Ok(job) = inbox.recv() {
        match job {
            Job::Evaluate(code, reply) => {
                let _ = reply.send(host.evaluate(&code));
            }
        }
    }
}

impl ScriptHost for ThreadedScriptHost {
    /// Read-only: the bridge behind it exposes one function, and it reads.
    fn document_grant(&self) -> ferrocad_shell_api::DocumentGrant {
        ferrocad_shell_api::DocumentGrant::Read
    }

    fn evaluate(&self, code: &str) -> Result<EvalResult, String> {
        let worker = self
            .inner
            .lock()
            .map_err(|_| "the RustPython thread is poisoned".to_string())?;
        let (reply, answer) = mpsc::channel();
        worker
            .jobs
            .as_ref()
            .ok_or_else(|| "the RustPython thread is gone".to_string())?
            .send(Job::Evaluate(code.to_string(), reply))
            .map_err(|_| "the RustPython thread is gone".to_string())?;
        answer
            .recv()
            .map_err(|_| "the RustPython thread is gone".to_string())?
    }

    fn load_workbenches(&self) -> Result<WorkbenchReport, String> {
        Ok(WorkbenchReport::default())
    }

    fn tool(&self, name: &str) -> Result<Box<dyn ferrocad_stage::Tool>, String> {
        Err(no_tool(name))
    }

    fn tool_command(&self, _name: &str, _verb: &str) -> bool {
        false
    }

    fn inventor_scene(&self) -> Result<Scene, String> {
        Ok(Scene::default())
    }

    fn event_listener(&self) -> Box<dyn EventListener> {
        Box::new(NoopScriptListener)
    }

    fn commands(&self) -> Vec<CommandInfo> {
        Vec::new()
    }

    fn run_command(&self, _id: &str) -> Option<CommandOutcome> {
        None
    }

    fn hello(&self) -> Result<Option<String>, String> {
        Ok(Some(HELLO.to_string()))
    }
}
