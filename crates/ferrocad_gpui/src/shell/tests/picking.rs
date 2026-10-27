//! Shell tests: select mode (whole-body vs sub-element selection).

use super::*;
use crate::python::SelectionRequest;
use crate::shell::picking::apply_selection_request;
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

/// The shell arms the registry **set**, not one narrowed gizmo: a workbench gizmo and
/// the shared triad coexist, the most recently registered workbench answers first, a
/// verb re-narrows the whole set without dropping entries, and a session pivot moves
/// only the controls anchored at the object datum.
#[gpui::test]
fn the_hosted_set_keeps_a_workbench_gizmo_beside_the_shared_triad(cx: &mut TestAppContext) {
    use ferrocad_stage::{GizmoKind, GizmoSet, TransformGizmo};

    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let model = boot_shell();
    let shell = cx.update(|cx| cx.new(|cx| Shell::from_model(model.clone(), cx)));

    // The workbench's own datum, distinct from the body origin (2, 3, 4).
    let workbench = Vector3::new(5.0, 0.0, 0.0);
    shell.update(cx, |shell, _cx| {
        shell.set_contributions(
            Contributions::new()
                .with_gizmo(translate_gizmo)
                .with_gizmo(move |_selection: &SelectionRef, _stage: &Stage| {
                    Some(TransformGizmo::translate(7, workbench))
                }),
        );
    });

    let stage = body_stage([2.0, 3.0, 4.0]);
    let set = shell.read_with(cx, |shell, _| {
        shell.arm_gizmos(&SelectionRef::whole(7), &stage)
    });
    assert_eq!(
        set.len(),
        6,
        "both factories contribute one entry per offered kind"
    );
    assert_eq!(
        set.primary().map(|index| set.gizmos()[index].origin()),
        Some(workbench),
        "the most recently registered factory answers the pointer first"
    );
    assert_eq!(
        set.offered().len(),
        2,
        "the workbench gizmo and the triad coexist at their own anchors"
    );

    // A verb re-narrows the same six entries; none is dropped.
    let mut set = set;
    assert!(set.set_session(GizmoKind::Rotate));
    assert_eq!(set.len(), 6);
    assert_eq!(
        set.offered().len(),
        2,
        "the rotate entries replace the translate ones in place"
    );

    // A session pivot re-anchors only what sat at the object datum.
    shell.update(cx, |shell, cx| {
        shell.on_gizmo_origin(Vector3::new(9.0, 9.0, 9.0), SnapKind::Vertex, cx);
    });
    let set: GizmoSet = shell.read_with(cx, |shell, _| {
        shell.arm_gizmos(&SelectionRef::whole(7), &stage)
    });
    assert_eq!(
        set.gizmos()[0].origin(),
        workbench,
        "a workbench gizmo keeps its own datum"
    );
    assert_eq!(
        set.primary().map(|index| set.gizmos()[index].origin()),
        Some(workbench)
    );
    assert_eq!(
        set.gizmos()[3].origin(),
        Vector3::new(9.0, 9.0, 9.0),
        "the triad follows the session pivot"
    );
}

/// S4.3 write path: a script's `Gui.Selection.addSelection` reaches the shell.
///
/// The facade queues the write because Python cannot command the viewport; the shell
/// drains the queue into **its own** selection, pushes the applied selection back so
/// the two sides agree, and re-arms the primary item's manipulators (which is what
/// makes a scripted face selection arm the Part push/pull stem).
#[gpui::test]
fn a_scripted_selection_reaches_the_shell(cx: &mut TestAppContext) {
    let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (shell, model) = shell_with_stage(cx, [0.0, 0.0, 0.0]);

    // Start from an empty selection with an empty queue.
    let reset = crate::python::evaluate(
        "import FreeCADGui\n\
         FreeCADGui.Selection._set([])\n\
         FreeCADGui.Selection._take_requests()\n",
    )
    .expect("reset the facade selection");
    assert!(reset.ok, "{}", reset.output);
    shell.update(cx, |shell, cx| shell.drain_selection_requests(cx));
    assert!(selection(&model).is_empty());

    // A script selects a face on the sample document's object.
    let added = crate::python::evaluate(
        "import FreeCADGui\n\
         FreeCADGui.Selection.addSelection('Params', 'Face6')\n",
    )
    .expect("add a selection");
    assert!(added.ok, "{}", added.output);
    shell.update(cx, |shell, cx| shell.drain_selection_requests(cx));
    assert_eq!(
        selection(&model),
        vec![SelectionItem::with_elements("Params", vec!["Face6".into()])],
        "the shell's selection reflects the script's write"
    );

    // The applied selection is pushed back down, so `getSelectionEx` agrees.
    let facade = crate::python::evaluate(
        "import FreeCADGui\n\
         print(repr([item.SubElementNames for item in FreeCADGui.Selection.getSelectionEx('Params')]))\n",
    )
    .expect("read the facade selection");
    assert_eq!(facade.output.trim(), "[['Face6']]");
    let queue = crate::python::evaluate(
        "import FreeCADGui\nprint(repr(FreeCADGui.Selection._take_requests()))\n",
    )
    .expect("read the queue");
    assert_eq!(queue.output.trim(), "[]", "applying a write does not requeue it");

    // Adding a second object makes it the primary (last) item.
    crate::python::evaluate(
        "import FreeCADGui\nFreeCADGui.Selection.addSelection('Derived')\n",
    )
    .expect("add the second object");
    shell.update(cx, |shell, cx| shell.drain_selection_requests(cx));
    assert_eq!(
        selection(&model)
            .iter()
            .map(|item| item.object.as_str())
            .collect::<Vec<_>>(),
        vec!["Params", "Derived"]
    );

    // A remove drops just the named object.
    crate::python::evaluate(
        "import FreeCADGui\nFreeCADGui.Selection.removeSelection('Derived')\n",
    )
    .expect("remove the second object");
    shell.update(cx, |shell, cx| shell.drain_selection_requests(cx));
    assert_eq!(
        selection(&model),
        vec![SelectionItem::with_elements("Params", vec!["Face6".into()])]
    );

    // A clear empties the shell's selection too.
    crate::python::evaluate("import FreeCADGui\nFreeCADGui.Selection.clearSelection()\n")
        .expect("clear the selection");
    shell.update(cx, |shell, cx| shell.drain_selection_requests(cx));
    assert!(selection(&model).is_empty());
    assert!(
        shell
            .read_with(cx, |shell, _| shell.user_origin)
            .is_none(),
        "a cleared selection drops the session pivot"
    );
}

/// The queue is applied in order, and a record that changes nothing is a no-op.
#[test]
fn a_selection_request_is_applied_with_the_facades_rules() {
    let mut items = Vec::new();
    apply_selection_request(
        &mut items,
        SelectionRequest {
            op: "add".into(),
            object: "Body".into(),
            elements: vec!["Face6".into()],
            points: vec![(0.0, 0.0, 0.0)],
        },
    );
    assert_eq!(
        items,
        vec![SelectionItem::with_elements("Body", vec!["Face6".into()])]
    );

    // A second add merges sub-elements instead of duplicating the object.
    apply_selection_request(
        &mut items,
        SelectionRequest {
            op: "add".into(),
            object: "Body".into(),
            elements: vec!["Face6".into(), "Edge2".into()],
            points: Vec::new(),
        },
    );
    assert_eq!(
        items,
        vec![SelectionItem::with_elements(
            "Body",
            vec!["Face6".into(), "Edge2".into()]
        )]
    );

    // A whole-object add replaces the element list and makes it primary.
    apply_selection_request(
        &mut items,
        SelectionRequest {
            op: "add".into(),
            object: "Slab".into(),
            elements: Vec::new(),
            points: Vec::new(),
        },
    );
    apply_selection_request(
        &mut items,
        SelectionRequest {
            op: "add".into(),
            object: "Body".into(),
            elements: Vec::new(),
            points: Vec::new(),
        },
    );
    assert_eq!(items.len(), 2);
    assert_eq!(items.last().map(|item| item.object.as_str()), Some("Body"));
    assert!(items.last().is_some_and(SelectionItem::is_whole));

    // Removing the last sub-element drops the object; an unknown name is a no-op.
    apply_selection_request(
        &mut items,
        SelectionRequest {
            op: "remove".into(),
            object: "Body".into(),
            elements: vec!["Face6".into()],
            points: Vec::new(),
        },
    );
    assert_eq!(items.len(), 1);
    apply_selection_request(
        &mut items,
        SelectionRequest {
            op: "remove".into(),
            object: "Missing".into(),
            elements: Vec::new(),
            points: Vec::new(),
        },
    );
    assert_eq!(items.len(), 1);

    // Hover and an unknown op never touch the selection (forward compatible).
    apply_selection_request(
        &mut items,
        SelectionRequest {
            op: "preselect".into(),
            object: "Slab".into(),
            elements: vec!["Face1".into()],
            points: Vec::new(),
        },
    );
    apply_selection_request(
        &mut items,
        SelectionRequest {
            op: "something-new".into(),
            object: "Slab".into(),
            elements: Vec::new(),
            points: Vec::new(),
        },
    );
    assert_eq!(items.len(), 1);

    apply_selection_request(
        &mut items,
        SelectionRequest {
            op: "clear".into(),
            object: String::new(),
            elements: Vec::new(),
            points: Vec::new(),
        },
    );
    assert!(items.is_empty());
}
