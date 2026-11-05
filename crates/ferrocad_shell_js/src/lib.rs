//! The JS scripting surface: [`ScriptHost`] over a page's ES module (S7 **spike**).
//!
//! The JS backend is a *composition*, not a fork: the document half is
//! [`ferrocad_shell_rust`](https://crates.io/crates/ferrocad_shell_rust)'s
//! `RustAppBackend`, and this crate supplies the script half the shell asks for
//! through `AppBackend::script_host()`:
//!
//! ```text
//! RustAppBackend (document)  +  JsScriptHost (this crate)  =  the JS backend
//! ```
//!
//! The page owns the module. `www/js-demo.js` installs `window.__ferrocadJs` with
//! four functions — `active`, `commands`, `evaluate`, `runCommand` — and an
//! `onEvent` callback; this crate calls them and maps the answers onto the seam's
//! types. Nothing here knows GPUI or the document: a command reaches the palette
//! because [`ScriptHost::commands`] is merged into the backend's list, and it runs
//! because [`ScriptHost::run_command`] reaches the page.
//!
//! **Spike scope.** The command list is read once, at boot, from the page; dynamic
//! registration, a JS tool session and the full `mod-events` contract are the next
//! step. `evaluate` and the event listener are real.

#![cfg(target_family = "wasm")]

use std::sync::OnceLock;

use ferrocad_inventor::{Directives, Event, EventListener, Hit, Scene};
use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_api::commands::{CommandInfo, CommandOutcome};
use ferrocad_shell_api::types::{EvalResult, WorkbenchReport};
use serde::Deserialize;
use wasm_bindgen::prelude::*;

/// The page's contract: `window.__ferrocadJs`, defined by the ES module the page
/// loads. Every call is `catch`-guarded, so a missing module is a value, not a
/// panic.
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__ferrocadJs"], js_name = active, catch)]
    fn js_active() -> Result<bool, JsValue>;

    /// A JSON array of `{ id, label, tooltip }`.
    #[wasm_bindgen(js_namespace = ["window", "__ferrocadJs"], js_name = commands, catch)]
    fn js_commands() -> Result<String, JsValue>;

    #[wasm_bindgen(js_namespace = ["window", "__ferrocadJs"], js_name = evaluate, catch)]
    fn js_evaluate(code: &str) -> Result<String, JsValue>;

    /// `true` when the command ran; `false` when it refused.
    #[wasm_bindgen(js_namespace = ["window", "__ferrocadJs"], js_name = runCommand, catch)]
    fn js_run_command(id: &str) -> Result<bool, JsValue>;

    #[wasm_bindgen(js_namespace = ["window", "__ferrocadJs"], js_name = onEvent, catch)]
    fn js_on_event(kind: &str, x: i32, y: i32, entity: u32) -> Result<(), JsValue>;
}

/// Whether the page installed its JS surface. The host asks this before it adds a
/// [`JsScriptHost`], so a page without the module gets the plain Rust backend.
pub fn available() -> bool {
    js_active().unwrap_or(false)
}

/// The script half of the JS backend.
pub struct JsScriptHost;

impl ScriptHost for JsScriptHost {
    fn evaluate(&self, code: &str) -> Result<EvalResult, String> {
        match js_evaluate(code) {
            Ok(output) => Ok(EvalResult { ok: true, output }),
            Err(error) => Ok(EvalResult {
                ok: false,
                output: format!("{error:?}"),
            }),
        }
    }

    fn load_workbenches(&self) -> Result<WorkbenchReport, String> {
        // ES modules are loaded by the page before the shell boots; there is no
        // workbench script to execute here.
        Ok(WorkbenchReport::default())
    }

    fn tool(&self, name: &str) -> Result<Box<dyn ferrocad_stage::Tool>, String> {
        Err(format!("js: no tool {name} in the spike"))
    }

    fn tool_command(&self, _name: &str, _verb: &str) -> bool {
        false
    }

    fn inventor_scene(&self) -> Result<Scene, String> {
        Ok(Scene::default())
    }

    fn event_listener(&self) -> Box<dyn EventListener> {
        Box::new(JsEventListener)
    }

    fn commands(&self) -> Vec<CommandInfo> {
        page_commands().clone()
    }

    fn run_command(&self, id: &str) -> Option<CommandOutcome> {
        if !page_commands().iter().any(|command| command.id == id) {
            return None;
        }
        Some(match js_run_command(id) {
            Ok(true) => CommandOutcome {
                ok: true,
                status: format!("js: ran {id}"),
                ..Default::default()
            },
            Ok(false) => CommandOutcome {
                ok: false,
                error: format!("js: {id} refused"),
                ..Default::default()
            },
            Err(error) => CommandOutcome {
                ok: false,
                error: format!("js: {error:?}"),
                ..Default::default()
            },
        })
    }
}

/// One entry of the page's command list.
#[derive(Deserialize)]
struct PageCommand {
    id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    tooltip: String,
}

impl PageCommand {
    fn into_command(self) -> CommandInfo {
        CommandInfo {
            menu: if self.label.is_empty() {
                self.id.clone()
            } else {
                self.label
            },
            tooltip: self.tooltip,
            cmd_type: "ForEdit".to_string(),
            group: "JS".to_string(),
            surface: "hidden".to_string(),
            enabled: true,
            id: self.id,
            ..Default::default()
        }
    }
}

/// The page's commands, read once. They are static for the session: the page
/// declares them before the module boots.
fn page_commands() -> &'static Vec<CommandInfo> {
    static COMMANDS: OnceLock<Vec<CommandInfo>> = OnceLock::new();
    COMMANDS.get_or_init(|| {
        let json = match js_commands() {
            Ok(json) => json,
            Err(error) => {
                log::error!("ferrocad js: commands failed: {error:?}");
                return Vec::new();
            }
        };
        match serde_json::from_str::<Vec<PageCommand>>(&json) {
            Ok(entries) => entries.into_iter().map(PageCommand::into_command).collect(),
            Err(error) => {
                log::error!("ferrocad js: bad command list: {error}");
                Vec::new()
            }
        }
    })
}

/// Forwards viewport events to the page.
///
/// `entity` is the picked stage entity (`0` for a miss); the page maps it to an
/// object name if it wants one. Returning `false` means the scene is unchanged.
struct JsEventListener;

impl EventListener for JsEventListener {
    fn on_event(
        &mut self,
        event: &Event,
        hit: Option<&Hit>,
        _scene: &mut Scene,
        _directives: &Directives,
    ) -> bool {
        let kind = match event {
            Event::MouseMove { .. } => "move",
            Event::MouseButton { down: true, .. } => "press",
            Event::MouseButton { down: false, .. } => "release",
            Event::MouseLeave => "leave",
        };
        let [x, y] = event.position().unwrap_or([0, 0]);
        let entity = hit.map(|hit| hit.id.0).unwrap_or(0);
        if let Err(error) = js_on_event(kind, x, y, entity) {
            log::error!("ferrocad js: onEvent failed: {error:?}");
            return false;
        }
        false
    }
}
