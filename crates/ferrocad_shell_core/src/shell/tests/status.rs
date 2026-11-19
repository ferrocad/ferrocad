//! Shell tests: the transient status toast.
//!
//! The bottom status strip is gone, so action feedback is a HUD stack on the right:
//! confirmations fade on their own, warnings and errors stay until dismissed, and each
//! pill carries an inline Undo that runs the same command the title bar does.

use std::time::Duration;

use super::*;

fn toast_count(shell: &Entity<Shell>, cx: &VisualTestContext) -> usize {
    shell.read_with(cx, |shell, _| shell.status_toasts.len())
}

/// The newest toast's id, for the per-toast debug selectors.
fn newest_id(shell: &Entity<Shell>, cx: &VisualTestContext) -> u64 {
    shell.read_with(cx, |shell, _| shell.status_toasts.last().expect("a toast").id)
}

/// A `prefix-id` debug selector (the per-toast ones are dynamic).
fn toast_selector(prefix: &str, id: u64) -> &'static str {
    Box::leak(format!("{prefix}-{id}").into_boxed_str())
}

fn toast_window(cx: &mut TestAppContext) -> (VisualTestContext, Entity<Shell>) {
    let model = boot_shell(cx);
    let window = cx.update(|cx| {
        let model = model.clone();
        cx.open_window(Default::default(), move |_window, cx| {
            cx.new(|cx| Shell::from_model(model.clone(), cx))
        })
        .unwrap()
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let shell: Entity<Shell> = window.root(&mut cx).unwrap();
    draw(&mut cx);
    (cx, shell)
}

/// A confirmation appears on the right, offers Undo, and does not need a dismiss.
#[gpui::test]
fn a_confirmation_toasts_with_an_undo_link(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell) = toast_window(cx);

    assert!(
        cx.debug_bounds("status-toasts").is_none(),
        "the boot status is not an action"
    );

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            // A real edit moves the history; the Undo chip follows that, not merely a
            // non-empty label.
            {
                let mut model = shell.model.lock().unwrap();
                model.undo_depth += 1;
                model.undo_label = "Move 1 object".to_string();
            }
            shell.set_status("moved 1 object", cx);
        });
    });
    draw(&mut cx);

    let stack = cx.debug_bounds("status-toasts").expect("the confirmation");
    let id = newest_id(&shell, &cx);
    assert!(
        cx.debug_bounds(toast_selector("toast-undo", id)).is_some(),
        "the pill can undo the action it reports"
    );
    // The stack sits in the viewport's own corner, sixteen pixels in from each edge.
    let canvas = cx.debug_bounds("main-canvas").expect("the canvas");
    assert!(
        stack.origin.x + stack.size.width >= canvas.origin.x + canvas.size.width - px(24.),
        "the stack hugs the right edge ({stack:?} / {canvas:?})"
    );
    // The corner clears the bottom-left tool dock: on a narrow window that row reaches
    // under the corner, so its top border has to stay below the newest card.
    let dock = cx.debug_bounds("loadout-bar").expect("the tool dock");
    assert!(
        stack.origin.y + stack.size.height <= dock.origin.y,
        "the feed clears the tool dock ({stack:?} / {dock:?})"
    );

    // A click on the card itself puts it away early; there is no `×` control to aim at.
    click_at(&mut cx, toast_selector("status-toast", id));
    draw(&mut cx);
    assert!(
        cx.debug_bounds(toast_selector("status-toast", id)).is_none(),
        "a click dismissed the confirmation"
    );
}

/// Several actions keep their pills; a repeat refreshes the newest, and the stack is
/// capped so it cannot grow up the whole viewport.
#[gpui::test]
fn confirmations_stack_and_repeats_refresh(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell) = toast_window(cx);

    for text in ["created Box_01", "moved Box_02", "deleted Box_03"] {
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.set_status(text, cx));
        });
        draw(&mut cx);
    }
    assert_eq!(toast_count(&shell, &cx), 3, "three actions, three pills");
    let stack = cx.debug_bounds("status-toasts").expect("the stack");
    assert!(
        stack.size.height > px(70.),
        "the pills stack vertically ({stack:?})"
    );

    // Repeating the newest message refreshes it rather than duplicating it.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_status("deleted Box_03", cx));
    });
    draw(&mut cx);
    assert_eq!(toast_count(&shell, &cx), 3, "the repeat refreshed the newest");

    // The stack is capped: a new action displaces the oldest information.
    for index in 0..5 {
        let text = format!("created Box_{index}");
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.set_status(text, cx));
        });
        draw(&mut cx);
    }
    assert_eq!(
        toast_count(&shell, &cx),
        crate::shell::MAX_STATUS_TOASTS,
        "the stack keeps only the newest pills"
    );
}

/// A card is only as wide as its message, up to the cap: a short note stays compact, and a
/// long solver sentence wraps inside its card instead of stretching across the canvas.
#[gpui::test]
fn a_card_shrinks_to_its_message_and_wraps_at_the_cap(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell) = toast_window(cx);

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_status("tool: box", cx));
    });
    draw(&mut cx);
    let short = newest_id(&shell, &cx);
    let short = cx
        .debug_bounds(toast_selector("status-toast", short))
        .expect("the note");

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.set_status(
                "interaction error: the pocket cut on Face1 failed because the sweep met \
                 no material; the body is unchanged",
                cx,
            )
        });
    });
    draw(&mut cx);
    let long = newest_id(&shell, &cx);
    let long = cx
        .debug_bounds(toast_selector("status-toast", long))
        .expect("the alert");

    let stack = cx.debug_bounds("status-toasts").expect("the corner feed");
    assert!(
        short.size.width < long.size.width,
        "the short note did not stretch to the long card's width ({short:?} / {long:?})"
    );
    assert!(
        short.size.width < px(crate::chrome::TOAST_MAX_WIDTH),
        "the short note is compact ({short:?})"
    );
    assert!(
        long.size.width <= px(crate::chrome::TOAST_MAX_WIDTH) + px(1.),
        "the long card stops at the cap ({long:?})"
    );
    assert_eq!(
        short.origin.x + short.size.width,
        long.origin.x + long.size.width,
        "both cards hug the same right edge ({short:?} / {long:?})"
    );
    assert_eq!(
        short.origin.x + short.size.width,
        stack.origin.x + stack.size.width,
        "the feed's right edge is the cards' right edge ({short:?} / {stack:?})"
    );
    assert!(
        long.size.height > short.size.height,
        "the long sentence wrapped instead of widening ({long:?} / {short:?})"
    );
}

/// The newest message holds the bottom-right corner and pushes the older cards up.
#[gpui::test]
fn the_newest_card_holds_the_corner(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell) = toast_window(cx);

    for text in ["created Box_01", "moved Box_02"] {
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.set_status(text, cx));
        });
        draw(&mut cx);
    }
    let ids = shell.read_with(&cx, |shell, _| {
        shell
            .status_toasts
            .iter()
            .map(|toast| toast.id)
            .collect::<Vec<_>>()
    });
    let oldest = cx
        .debug_bounds(toast_selector("status-toast", ids[0]))
        .expect("the first card");
    let newest = cx
        .debug_bounds(toast_selector("status-toast", ids[1]))
        .expect("the last card");
    assert_eq!(
        oldest.origin.x + oldest.size.width,
        newest.origin.x + newest.size.width,
        "the cards share one right edge ({oldest:?} / {newest:?})"
    );
    assert!(
        newest.origin.y > oldest.origin.y,
        "the newest card sits nearest the corner ({newest:?} / {oldest:?})"
    );

    // The feed is pinned to the viewport's right edge, sixteen pixels in.
    let canvas = cx.debug_bounds("main-canvas").expect("the canvas");
    let stack = cx.debug_bounds("status-toasts").expect("the feed");
    assert_eq!(
        stack.origin.x + stack.size.width,
        canvas.origin.x + canvas.size.width - px(16.),
        "the feed hugs the viewport's right edge ({stack:?} / {canvas:?})"
    );
}

/// A wrapped message keeps its `Undo` chip on the first line, where it is read, instead of
/// floating in the middle of the card.
#[gpui::test]
fn a_wrapped_card_keeps_its_chip_on_the_first_line(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell) = toast_window(cx);

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            {
                let mut model = shell.model.lock().unwrap();
                model.undo_depth += 1;
                model.undo_label = "Move 1 object".to_string();
            }
            shell.set_status(
                "moved every dependent body of the assembly onto the active workplane",
                cx,
            );
        });
    });
    draw(&mut cx);

    let id = newest_id(&shell, &cx);
    let card = cx
        .debug_bounds(toast_selector("status-toast", id))
        .expect("the card");
    let chip = cx
        .debug_bounds(toast_selector("toast-undo", id))
        .expect("the chip");
    assert!(
        card.size.height > px(32.),
        "the message wrapped onto two lines ({card:?})"
    );
    assert!(
        chip.origin.y - card.origin.y < px(16.),
        "the chip stays on the first line ({chip:?} / {card:?})"
    );
}

/// A full feed gives up the oldest information, never a solver alert.
#[gpui::test]
fn a_full_feed_keeps_its_alerts(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell) = toast_window(cx);

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_status("fillet radius too large", cx));
    });
    draw(&mut cx);
    for text in ["tool: box", "tool: line", "tool: circle"] {
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| shell.set_status(text, cx));
        });
        draw(&mut cx);
    }

    assert_eq!(
        toast_count(&shell, &cx),
        crate::shell::MAX_STATUS_TOASTS,
        "the feed holds only its cap"
    );
    let texts = shell.read_with(&cx, |shell, _| {
        shell
            .status_toasts
            .iter()
            .map(|toast| toast.text.clone())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        texts,
        vec!["fillet radius too large", "tool: line", "tool: circle"],
        "the oldest note went, the alert stayed"
    );
}

/// A successful action is the "next valid modelling action" that clears a solver alert.
#[gpui::test]
fn a_success_clears_an_older_alert(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell) = toast_window(cx);

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_status("fillet radius too large", cx));
    });
    draw(&mut cx);
    assert_eq!(toast_count(&shell, &cx), 1);

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_status("moved Box_01", cx));
    });
    draw(&mut cx);
    assert_eq!(toast_count(&shell, &cx), 1, "the alert gave way");
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell.status_toasts[0].kind),
        crate::shell::toast::StatusKind::Success,
        "the remaining pill is the successful action"
    );
}

/// Undo belongs to the action that owns the current top of the stack. An older toast
/// loses its chip when a newer action lands, and a view-only status never gets one.
#[gpui::test]
fn only_the_current_top_offers_undo(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell) = toast_window(cx);

    // First edit.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            {
                let mut model = shell.model.lock().unwrap();
                model.undo_depth += 1;
                model.undo_label = "Move".to_string();
            }
            shell.set_status("moved Box_01", cx);
        });
    });
    draw(&mut cx);
    let first = newest_id(&shell, &cx);
    assert!(
        cx.debug_bounds(toast_selector("toast-undo", first)).is_some(),
        "the newest edit can be undone"
    );

    // A second edit moves the top: the first toast must no longer offer Undo.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            {
                let mut model = shell.model.lock().unwrap();
                model.undo_depth += 1;
                model.undo_label = "Fillet".to_string();
            }
            shell.set_status("filleted Box_01", cx);
        });
    });
    draw(&mut cx);
    let second = newest_id(&shell, &cx);
    assert!(
        cx.debug_bounds(toast_selector("toast-undo", second)).is_some(),
        "the new top carries the chip"
    );
    assert!(
        cx.debug_bounds(toast_selector("toast-undo", first)).is_none(),
        "the older toast cannot undo someone else's action"
    );

    // A view-only status does not touch history: it gets no chip, and the edit that
    // still owns the top keeps its own.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_status("grid: on", cx));
    });
    draw(&mut cx);
    let third = newest_id(&shell, &cx);
    assert!(
        cx.debug_bounds(toast_selector("toast-undo", third)).is_none(),
        "a view toggle is not an undoable action"
    );
    assert!(
        cx.debug_bounds(toast_selector("toast-undo", second)).is_some(),
        "the edit still owns the top"
    );
}

/// A commit reports its status before the document refresh lands. The chip appears
/// once the refresh records the transaction, and still points at that action.
#[gpui::test]
fn a_late_refresh_resolves_the_undo_chip(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell) = toast_window(cx);
    give_params_a_placement();

    // The status arrives first.
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_status("moved Box_01", cx));
    });
    draw(&mut cx);
    let id = newest_id(&shell, &cx);
    assert!(
        cx.debug_bounds(toast_selector("toast-undo", id)).is_none(),
        "no chip until the history says the action was undoable"
    );

    // The transaction and its refresh land afterwards.
    let edit = crate::python::evaluate(
        r#"
import FreeCAD
doc = FreeCAD.ActiveDocument
doc.openTransaction('Late undo')
doc.getObject('Params').Placement = FreeCAD.Placement(
    FreeCAD.Vector(5, 0, 0), FreeCAD.Rotation()
)
doc.commitTransaction()
doc.recompute()
"#,
    )
    .expect("write the late transaction");
    assert!(edit.ok, "{}", edit.output);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.refresh(cx));
    });
    draw(&mut cx);
    assert!(
        cx.debug_bounds(toast_selector("toast-undo", id)).is_some(),
        "the late refresh attaches the chip to the action it records"
    );

    // Leave the shared sample document as it was.
    let cleanup = crate::python::evaluate(
        "import FreeCAD\ndoc = FreeCAD.ActiveDocument\ndoc.undo()\ndoc.recompute()",
    )
    .expect("undo the late transaction");
    assert!(cleanup.ok, "{}", cleanup.output);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.refresh(cx));
    });
}

/// The chip is its own press. The card behind it dismisses on a click, so this pins the
/// nesting: a click on the chip must run `App_Undo`, not merely put the message away.
#[gpui::test]
fn clicking_the_chip_undoes_rather_than_dismissing(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell) = toast_window(cx);
    give_params_a_placement();

    let edit = crate::python::evaluate(
        r#"
import FreeCAD
doc = FreeCAD.ActiveDocument
doc.openTransaction('Chip undo')
doc.getObject('Params').Placement = FreeCAD.Placement(
    FreeCAD.Vector(5, 0, 0), FreeCAD.Rotation()
)
doc.commitTransaction()
doc.recompute()
"#,
    )
    .expect("write the chip transaction");
    assert!(edit.ok, "{}", edit.output);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.refresh(cx));
    });

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.set_status("moved Params", cx));
    });
    draw(&mut cx);
    let id = newest_id(&shell, &cx);
    assert!(
        cx.debug_bounds(toast_selector("toast-undo", id)).is_some(),
        "the edit owns the chip"
    );

    click_at(&mut cx, toast_selector("toast-undo", id));
    draw(&mut cx);

    assert!(
        shell
            .read_with(&cx, |shell, _| shell.model.lock().unwrap().status.clone())
            .starts_with("undo"),
        "the chip ran the undo command, it did not just dismiss"
    );
    let undone = crate::python::evaluate(
        "import FreeCAD\n\
         names = FreeCAD.ActiveDocument.UndoNames\n\
         assert 'Chip undo' not in names, names",
    )
    .expect("read the history");
    assert!(undone.ok, "{}", undone.output);
}

/// The model carries the top of the document's undo stack, which is what the Undo
/// tooltip and the toast's inline Undo read.
#[gpui::test]
fn the_model_follows_the_document_history(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell) = toast_window(cx);
    give_params_a_placement();

    let edit = crate::python::evaluate(
        r#"
import FreeCAD
doc = FreeCAD.ActiveDocument
doc.openTransaction('Probe undo')
doc.getObject('Params').Placement = FreeCAD.Placement(
    FreeCAD.Vector(5, 0, 0), FreeCAD.Rotation()
)
doc.commitTransaction()
doc.recompute()
"#,
    )
    .expect("write the probe transaction");
    assert!(edit.ok, "{}", edit.output);

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.refresh(cx));
    });
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell
            .model
            .lock()
            .unwrap()
            .undo_label
            .clone()),
        "Probe undo",
        "the model names the newest transaction"
    );

    // Leave the shared sample document as it was.
    let cleanup = crate::python::evaluate(
        "import FreeCAD\ndoc = FreeCAD.ActiveDocument\ndoc.undo()\ndoc.recompute()",
    )
    .expect("undo the probe transaction");
    assert!(cleanup.ok, "{}", cleanup.output);
    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| shell.refresh(cx));
    });
    assert_eq!(
        shell.read_with(&cx, |shell, _| shell
            .model
            .lock()
            .unwrap()
            .undo_label
            .clone()),
        "",
        "the cleanup transaction is gone"
    );
}

/// A solver failure is sticky: it survives the confirmation's lifetime and leaves only
/// when the user clicks the card away.
#[gpui::test]
fn a_solver_error_sticks_until_dismissed(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell) = toast_window(cx);

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.set_status("fillet radius too large", cx);
        });
    });
    draw(&mut cx);
    assert!(cx.debug_bounds("status-toasts").is_some());
    let id = newest_id(&shell, &cx);
    assert!(
        cx.debug_bounds(toast_selector("toast-dismiss", id)).is_none(),
        "the card is the dismiss target, not a `×`"
    );
    assert!(
        cx.debug_bounds(toast_selector("status-toast", id)).is_some(),
        "the alert's card is there to click"
    );

    // Well past a confirmation's life, the alert is still there.
    cx.executor().advance_clock(Duration::from_secs(10));
    cx.run_until_parked();
    draw(&mut cx);
    assert!(
        cx.debug_bounds("status-toasts").is_some(),
        "an error does not fade on its own"
    );

    click_at(&mut cx, toast_selector("status-toast", id));
    draw(&mut cx);
    assert!(
        cx.debug_bounds("status-toasts").is_none(),
        "the dismiss closes it"
    );
}

/// A confirmation is crisp for its hold, then fades out and is removed.
#[gpui::test]
fn a_confirmation_fades_after_its_hold(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut cx, shell) = toast_window(cx);

    cx.update(|_window, cx| {
        shell.update(cx, |shell, cx| {
            shell.set_status("created Box_01", cx);
        });
    });
    draw(&mut cx);
    assert!(cx.debug_bounds("status-toasts").is_some());

    // Step through the entrance, spend the hold, then step through the fade. Each step
    // is longer than the task's 40/50 ms sleep so the loop can reach its final clear.
    for _ in 0..(crate::shell::toast::TOAST_ENTER_STEPS + 3) {
        cx.executor().advance_clock(Duration::from_millis(60));
        cx.run_until_parked();
    }
    cx.executor()
        .advance_clock(crate::shell::toast::TOAST_LIFE);
    cx.run_until_parked();
    for _ in 0..(crate::shell::toast::TOAST_FADE_STEPS + 2) {
        cx.executor().advance_clock(Duration::from_millis(60));
        cx.run_until_parked();
    }
    draw(&mut cx);
    assert!(
        cx.debug_bounds("status-toasts").is_none(),
        "the confirmation faded away"
    );
}
