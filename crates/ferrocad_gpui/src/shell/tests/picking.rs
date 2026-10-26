//! Shell tests: select mode (whole-body vs sub-element selection).

use super::*;
use crate::translate_gizmo;
use ferrocad_stage::{ShapeItem, Stage};

/// A shell whose display map resolves the viewport's pick id `7` to the sample
/// object `Params`, with the item origin the triad should hug.
fn shell_with_stage(
    cx: &mut TestAppContext,
    origin: [f32; 3],
) -> (Entity<Shell>, Arc<Mutex<ShellModel>>) {
    let model = boot_shell();
    let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));
    shell.update(cx, |shell, _cx| {
        shell.display.build([StageMesh {
            entity: 7,
            object: "Params".into(),
            label: String::new(),
            positions: vec![[0.0, 0.0, 0.0]],
            normals: Vec::new(),
            triangles: Vec::new(),
            triangle_parts: Vec::new(),
            edges: Vec::new(),
            segment_parts: Vec::new(),
            vertices: Vec::new(),
            vertex_parts: Vec::new(),
            parts: vec!["Face1".into()],
            origin: Some(origin),
            color: None,
            display: None,
        }]);
        shell.set_contributions(Contributions::new().with_gizmo(translate_gizmo));
    });
    (shell, model)
}

/// The viewport's event for a click on the sample slab's `Face1`.
fn picked(click_count: usize) -> SelectionEvent {
    SelectionEvent::Picked {
        reference: SelectionRef::element(7, "Face1"),
        anchor: None,
        click_count,
    }
}

fn selection(model: &Arc<Mutex<ShellModel>>) -> Vec<SelectionItem> {
    model.lock().unwrap().selection.clone()
}

/// A stage carrying just the selected body, with its own origin.
fn body_stage(origin: [f32; 3]) -> Stage {
    Stage::new().with_item(ShapeItem::new(7).with_origin(Some(origin)))
}

/// A single click keeps the sub-element the picker resolved. The second click of a
/// double click upgrades the same pick to the whole body; a further click stays whole
/// rather than toggling, and a plain click afterwards returns to sub-element scope.
#[gpui::test]
fn a_single_click_keeps_scope_and_a_double_click_upgrades_to_the_body(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (shell, model) = shell_with_stage(cx, [2.0, 3.0, 4.0]);

    // First click: the face is selected, not the body.
    shell.update(cx, |shell, cx| shell.on_selection(&picked(1), cx));
    let items = selection(&model);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].object, "Params");
    assert_eq!(items[0].elements, vec!["Face1".to_string()]);

    // The second click of the double click upgrades to the whole body.
    shell.update(cx, |shell, cx| shell.on_selection(&picked(2), cx));
    let items = selection(&model);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].object, "Params");
    assert!(
        items[0].is_whole(),
        "a double click selects the body: {items:?}"
    );

    // A third click in the same run is still a whole-body selection, not a toggle.
    shell.update(cx, |shell, cx| shell.on_selection(&picked(3), cx));
    let items = selection(&model);
    assert!(items[0].is_whole(), "a triple click stays whole: {items:?}");

    // A later single click on the face returns to sub-element scope.
    shell.update(cx, |shell, cx| shell.on_selection(&picked(1), cx));
    let items = selection(&model);
    assert_eq!(items[0].elements, vec!["Face1".to_string()]);
}

/// The upgraded selection re-arms the triad at the object's own origin: the session
/// pivot from a re-pivot is dropped with the old selection subject, and a face
/// selection still anchors at the body origin rather than the click point.
#[gpui::test]
fn a_whole_body_selection_arms_the_gizmo_at_the_object_origin(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (shell, _model) = shell_with_stage(cx, [2.0, 3.0, 4.0]);
    let anchor = Vector3::new(2.0, 3.0, 4.0);

    // A session pivot stands in for a previous re-pivot.
    let pivot = Vector3::new(9.0, 9.0, 9.0);
    shell.update(cx, |shell, cx| {
        shell.on_gizmo_origin(pivot, SnapKind::Vertex, cx);
    });
    assert_eq!(shell.read_with(cx, |shell, _| shell.user_origin), Some(pivot));

    // A double click promotes Face1 to the body; the new subject clears the pivot.
    shell.update(cx, |shell, cx| shell.on_selection(&picked(2), cx));
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.user_origin),
        None,
        "a whole-body selection re-arms from the object origin"
    );

    let stage = body_stage([2.0, 3.0, 4.0]);
    let whole = shell
        .read_with(cx, |shell, _| shell.armed_gizmo(&SelectionRef::whole(7), &stage))
        .expect("the whole-body selection arms the triad");
    assert_eq!(whole.origin(), anchor);

    let face = shell
        .read_with(cx, |shell, _| {
            shell.armed_gizmo(&SelectionRef::element(7, "Face1"), &stage)
        })
        .expect("a face selection arms the body's triad");
    assert_eq!(
        face.origin(),
        anchor,
        "a sub-element still anchors at the object origin, not the click"
    );
}
