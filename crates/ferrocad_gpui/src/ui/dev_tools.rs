//! The Dev Tools window (P0.5).
//!
//! A second, GIMP-toolbox-style window over the **same** [`Shell`] entity: the actions
//! bar, the provider and GPU switches, the model inspector, the property editor and
//! the console. It owns no model and
//! never boots an interpreter. The main window keeps the viewport, the status line, the
//! transient tool strip and the modal dialogs, so a command run here opens its dialog
//! over the canvas the user is looking at.
//!
//! The viewport entity is deliberately absent: gpui can render one entity in several
//! windows, but a 3D viewport must be drawn exactly once.

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, IntoElement, Render, Subscription, Window,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowKind, WindowOptions, bounds,
    point, px, size,
};

use ferrocad_widgets::window_frame;

use crate::shell::Shell;

/// The command that opens, focuses or closes this window.
pub const TOGGLE_COMMAND: &str = "Dev_ToggleTools";

/// The window's size and title.
const WIDTH: f32 = 420.;
const HEIGHT: f32 = 760.;
pub const TITLE: &str = "FerroCAD Dev Tools";

/// Default bounds: a narrow tall window beside the main one (which opens at 80,80 and
/// is 1180 wide).
///
/// `WindowKind::Floating` is the **always-on-top** option: on macOS it sets the native
/// floating window level, and on X11/Wayland it makes the window transient for the main
/// one, so a window manager that honours the hint keeps it above its parent while the
/// user orbits the model. Windows has no such hint in this gpui version and treats it
/// as a normal window.
pub fn window_options() -> WindowOptions {
    WindowOptions {
        titlebar: Some(gpui::TitlebarOptions {
            title: Some(TITLE.into()),
            appears_transparent: true,
            ..Default::default()
        }),
        window_background: WindowBackgroundAppearance::Opaque,
        window_decorations: Some(WindowDecorations::Client),
        kind: WindowKind::Floating,
        is_movable: true,
        is_resizable: true,
        app_owns_titlebar_drag: true,
        window_bounds: Some(WindowBounds::Windowed(bounds(
            point(px(1272.), px(80.)),
            size(px(WIDTH), px(HEIGHT)),
        ))),
        ..Default::default()
    }
}

/// The Dev Tools root view: a thin shell around the shared [`Shell`].
pub struct DevTools {
    shell: Entity<Shell>,
    focus: FocusHandle,
    /// Repaint when the shell's model changes (the shell notifies after every edit,
    /// selection and command).
    _shell_changes: Subscription,
}

impl DevTools {
    pub fn new(shell: Entity<Shell>, cx: &mut Context<Self>) -> Self {
        let _shell_changes = cx.observe(&shell, |_this, _shell, cx| cx.notify());
        Self {
            shell,
            focus: cx.focus_handle(),
            _shell_changes,
        }
    }
}

impl Focusable for DevTools {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DevTools {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The panels are built from the shell's context so their listeners run against
        // the same entity the main window renders. This happens at frame time, never
        // inside a shell update, so the borrow is not re-entrant.
        let body = self.shell.update(cx, |shell, cx| {
            crate::shell::dev_tools_body(shell, &self.focus, window, cx)
        });
        window_frame(window, TITLE, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Dev Tools window is a tool: it floats above the main window instead of
    /// hiding behind it while the user works in the viewport.
    #[test]
    fn the_window_floats_above_the_main_one() {
        let options = window_options();
        assert_eq!(options.kind, WindowKind::Floating);
        assert!(options.is_movable, "the user can park it");
        assert!(options.is_resizable, "the panes can be given room");
        assert_eq!(
            options.titlebar.as_ref().and_then(|t| t.title.as_deref()),
            Some(TITLE)
        );
    }
}
