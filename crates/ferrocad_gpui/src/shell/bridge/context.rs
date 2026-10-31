//! The interaction context and the action gate.
//!
//! One place decides what the user may do *now*. Every entry point — a pick, a drag, a
//! loadout key, a registry command — asks the same question, so a new gesture cannot
//! forget to check the state of the one it competes with. The rules are few and named;
//! adding a context (or refusing one more action in it) is a change here, not a hunt for
//! scattered guards.
//!
//! See `docs/developer/interaction/selection-gizmos-and-forms.md` §5.2.

use crate::shell::*;

/// What the shell is doing, in precedence order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InteractionContext {
    /// A modal surface (command or form dialog, palette, settings card) owns the input.
    Modal,
    /// An interactive tool owns the canvas.
    Tool,
    /// A move is pending: the `✗`/`✓` pair is on screen and the value is undecided.
    ///
    /// `target` is the stage entity the session owns (the dirty object), so a pick can
    /// be judged against *which* subject it would replace rather than "any selection".
    PendingMove { target: Option<u32> },
    /// A rotation is pending: the same finish pair, but the value is a sweep rather than
    /// a translation. It is a distinct context so the refusal names the right verb and
    /// the angular row keeps its own readout.
    PendingRotation { target: Option<u32> },
    /// The Move hand has a selected target and its handles are armed.
    Armed,
    /// A selection exists without handles.
    Selection,
    /// Nothing is selected.
    Idle,
}

/// Something the user can ask the shell to do.
///
/// The completion actions (`✓`/`✗`, `Enter`/`Esc`) and field editing are deliberately
/// not here: they are how a context is *left*, and they are always allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action<'a> {
    /// Replace, clear or toggle the selection (a pick, the tree, a script).
    ///
    /// `target` is the stage entity the action would select, `None` for a clear.
    Select { target: Option<u32> },
    /// Equip a loadout hand (a key, the bar, a wheel verb) — pausing included.
    SwitchHand,
    /// Change the `Select ▾` topology scope.
    SwitchScope,
    /// Change the transform handle set (translate / rotate).
    SwitchHandleSet,
    /// Start an interactive tool.
    StartTool,
    /// Run a registry command; `edit` is true unless it declares `CmdType: ForView`.
    RunCommand { id: &'a str, edit: bool },
    /// Arm, hover-preview or start a drag on another body.
    Arm,
}

impl Action<'_> {
    /// The phrase a refusal completes: "✓ or ✗ before you …".
    fn verb(self) -> &'static str {
        match self {
            Action::Select { .. } => "select another object",
            Action::SwitchHand => "switch tools",
            Action::SwitchScope => "change the selection scope",
            Action::SwitchHandleSet => "switch the handle set",
            Action::StartTool => "start a tool",
            Action::RunCommand { .. } => "run a command",
            Action::Arm => "drag another object",
        }
    }
}

impl InteractionContext {
    /// Whether `action` is legal in this context.
    ///
    /// Only the pending move narrows the surface today: it is an explicit, unfinished
    /// decision, so it owns the subject and the transform until it is finished. A tool
    /// and a modal already own their own input paths; their handles, picks and commands
    /// are gated where they are routed.
    fn allows(self, action: Action<'_>) -> bool {
        match self {
            // A pending transform accepts only the completion keys, field editing and
            // view-only commands (grid, projection, …). It refuses everything that would
            // spend or drop it, or that would make another body the subject.
            InteractionContext::PendingMove { .. }
            | InteractionContext::PendingRotation { .. } => {
                matches!(action, Action::RunCommand { edit: false, .. })
            }
            InteractionContext::Modal
            | InteractionContext::Tool
            | InteractionContext::Armed
            | InteractionContext::Selection
            | InteractionContext::Idle => true,
        }
    }
}

impl Shell {
    /// The current interaction context.
    pub(in crate::shell) fn context(&self, cx: &App) -> InteractionContext {
        if self.modal_open() {
            return InteractionContext::Modal;
        }
        if self.active_tool.is_some() {
            return InteractionContext::Tool;
        }
        if self.move_is_pending(cx) {
            // The subject is the session's primary target (the body the triad hugs).
            // Typed-only pending state before a stage exists has no entity to name.
            let target = self.session.as_ref().map(|session| session.primary());
            let rotate = self
                .session
                .as_ref()
                .is_some_and(|session| session.mode() == GizmoMode::Rotate);
            return if rotate {
                InteractionContext::PendingRotation { target }
            } else {
                InteractionContext::PendingMove { target }
            };
        }
        let (armed, selected) = {
            let model = self.model.lock().unwrap();
            (model.gizmo_armed, !model.selection.is_empty())
        };
        if armed {
            InteractionContext::Armed
        } else if selected {
            InteractionContext::Selection
        } else {
            InteractionContext::Idle
        }
    }

    /// Gate `action` against [`context`](Self::context).
    ///
    /// Returns whether the action was **refused**; a refusal writes the reason to the
    /// status line so a click that does nothing says why.
    pub(in crate::shell) fn gate(&mut self, action: Action<'_>, cx: &mut Context<Self>) -> bool {
        let context = self.context(cx);
        if context.allows(action) {
            return false;
        }
        let verb = match action {
            Action::RunCommand { id, .. } => format!("run “{}”", self.command_label(id)),
            other => other.verb().to_string(),
        };
        let status = match context {
            InteractionContext::PendingMove { .. } => {
                format!("move pending — ✓ or ✗ before you {verb}")
            }
            InteractionContext::PendingRotation { .. } => {
                format!("rotation pending — ✓ or ✗ before you {verb}")
            }
            InteractionContext::Tool => format!("a tool is active — finish it before you {verb}"),
            InteractionContext::Modal => format!("close the dialog before you {verb}"),
            InteractionContext::Armed
            | InteractionContext::Selection
            | InteractionContext::Idle => format!("not now — {verb}"),
        };
        self.model.lock().unwrap().status = status;
        cx.notify();
        true
    }

    /// Whether a registry command only affects the view (`CmdType: ForView`).
    ///
    /// A command the shell has not seen in its registry snapshot is treated as an edit:
    /// without the declaration it cannot be proved harmless under a pending move.
    pub(in crate::shell) fn command_is_view(&self, id: &str) -> bool {
        let model = self.model.lock().unwrap();
        commands::find(&model.commands, id)
            .is_some_and(|command| command.cmd_type.eq_ignore_ascii_case("ForView"))
    }

    /// A command's display label, falling back to its id.
    fn command_label(&self, id: &str) -> String {
        let model = self.model.lock().unwrap();
        commands::find(&model.commands, id)
            .map(|command| command.label().to_string())
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| id.to_string())
    }
}
