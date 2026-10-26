//! `Shell` methods: devtools.
//!
//! Split out of `shell.rs`; the paths and behaviour are unchanged.

use super::*;

impl Shell {
    /// Whether the Dev Tools window is currently open.
    pub fn dev_tools_open(&self, cx: &App) -> bool {
        self.dev_tools
            .and_then(|handle| cx.window_handle(handle.window_id()))
            .is_some()
    }

    /// Open, focus or close the Dev Tools window (`Dev_ToggleTools`, `Ctrl+Shift+D`).
    ///
    /// The window is a second view over this same `Shell` entity — it never owns the
    /// model and never boots a second interpreter. Opening is deferred by one effect
    /// cycle: building the window draws its first frame, and that frame reads this
    /// entity, so it must not happen while the caller already holds it mutably (the
    /// command path does).
    ///
    /// Returns the status line for the toggle; the deferred open replaces it with
    /// `dev tools: open` once the window exists.
    pub fn toggle_dev_tools(&mut self, cx: &mut Context<Self>) -> String {
        if self.dev_tools_open(cx) {
            let Some(handle) = self.dev_tools.take() else {
                return "dev tools: closed".to_string();
            };
            return match handle.update(cx, |_view, window, _cx| window.remove_window()) {
                Ok(()) => "dev tools: closed".to_string(),
                Err(_) => "dev tools: window already closed".to_string(),
            };
        }
        // The handle may name a window the user closed with the title bar.
        self.dev_tools = None;
        self.open_dev_tools(cx)
    }

    /// Create the Dev Tools window around this shell entity.
    pub(super) fn open_dev_tools(&mut self, cx: &mut Context<Self>) -> String {
        // The providers and GPU panels read their state once per open, not per frame.
        self.provider_features = python::provider_features().ok();
        self.gpu_preference = Some(python::gpu_adapter().unwrap_or_else(|_| "auto".to_string()));
        self.gpu_adapters = Some(ferrocad_wgpu::adapter::adapters());
        let shell = cx.entity();
        cx.defer(move |cx| {
            let handle = cx.open_window(crate::dev_tools::window_options(), {
                let shell = shell.clone();
                move |_window, cx| cx.new(|cx| DevTools::new(shell, cx))
            });
            match handle {
                Ok(handle) => {
                    let _ = handle.update(cx, |view, window, cx| {
                        window.activate_window();
                        window.focus(&view.focus_handle(cx), cx);
                    });
                    shell.update(cx, |shell, cx| {
                        shell.dev_tools = Some(handle);
                        shell.model.lock().unwrap().status = "dev tools: open".to_string();
                        cx.notify();
                    });
                }
                Err(error) => {
                    shell.update(cx, |shell, cx| {
                        let status = format!("dev tools: cannot open a window ({error})");
                        shell.model.lock().unwrap().status = status.clone();
                        shell.console_line(status, cx);
                        cx.notify();
                    });
                }
            }
        });
        "dev tools: opening".to_string()
    }

}
