//! The workbench contributions an edition makes to the shell.
//!
//! The base app links no workbench and registers nothing. An edition adds the
//! manipulators its workbench needs (the Part edge fillet arc, a BIM thickness grip,
//! …) through a **factory**: the shell calls it for each selection and hosts the first
//! match in the viewport.
//!
//! A commit routes back through the app script's `interaction` hook, so the shell still
//! names no workbench and links no kernel. A workbench that has no manipulators — BIM
//! has no dress-up — simply registers none, and never carries the Part code.

use std::sync::Arc;

use ferrocad_geom::SelectionRef;
use ferrocad_stage::{Manipulator, Stage, Tool, TransformGizmo};

/// The manipulator a selection asks for, if the workbench has one.
///
/// Implemented for any matching closure, so an edition can write
/// `Contributions::default().with_manipulator(ferrocad_part_gui::fillet_manipulator)`.
pub trait ManipulatorFactory: Send + Sync {
    /// The manipulator this workbench arms for `selection`, if it has one.
    fn create(&self, selection: &SelectionRef, stage: &Stage) -> Option<Box<dyn Manipulator>>;
}

impl<F> ManipulatorFactory for F
where
    F: Fn(&SelectionRef, &Stage) -> Option<Box<dyn Manipulator>> + Send + Sync,
{
    fn create(&self, selection: &SelectionRef, stage: &Stage) -> Option<Box<dyn Manipulator>> {
        self(selection, stage)
    }
}

/// The transform gizmo a selection asks for, if the workbench offers one.
///
/// The gizmo is the generic move/rotate handle; a workbench decides whether its
/// objects respond to it (a sketch may want it, a spreadsheet cell may not).
pub trait GizmoFactory: Send + Sync {
    /// The gizmo to show for `selection`, if this workbench offers one.
    fn create(&self, selection: &SelectionRef, stage: &Stage) -> Option<TransformGizmo>;
}

impl<F> GizmoFactory for F
where
    F: Fn(&SelectionRef, &Stage) -> Option<TransformGizmo> + Send + Sync,
{
    fn create(&self, selection: &SelectionRef, stage: &Stage) -> Option<TransformGizmo> {
        self(selection, stage)
    }
}

/// The interactive tool a command id starts, if the edition offers one.
///
/// A command's `{"tool": id}` is presentation-level: the shell resolves the id here
/// and hands the tool to the viewport. The registry is Rust (a tool owns pointer
/// events and transient geometry); Python only names it.
pub trait ToolFactory: Send + Sync {
    /// A fresh tool session.
    fn create(&self) -> Box<dyn Tool>;
}

impl<F> ToolFactory for F
where
    F: Fn() -> Box<dyn Tool> + Send + Sync,
{
    fn create(&self) -> Box<dyn Tool> {
        self()
    }
}

/// The manipulators an edition contributes, in registration order.
#[derive(Clone, Default)]
pub struct Contributions {
    factories: Vec<Arc<dyn ManipulatorFactory>>,
    gizmos: Vec<Arc<dyn GizmoFactory>>,
    /// `(tool id, factory)`, in registration order.
    tools: Vec<(String, Arc<dyn ToolFactory>)>,
}

impl Contributions {
    /// No contributions (the base app).
    pub fn new() -> Self {
        Self::default()
    }

    /// The builder form of [`register`](Self::register).
    pub fn with_manipulator(mut self, factory: impl ManipulatorFactory + 'static) -> Self {
        self.register(factory);
        self
    }

    /// Register a manipulator factory. Later registrations are tried first.
    pub fn register(&mut self, factory: impl ManipulatorFactory + 'static) {
        self.factories.push(Arc::new(factory));
    }

    /// The builder form of [`register_gizmo`](Self::register_gizmo).
    pub fn with_gizmo(mut self, factory: impl GizmoFactory + 'static) -> Self {
        self.register_gizmo(factory);
        self
    }

    /// Register a transform-gizmo factory. Later registrations are tried first.
    pub fn register_gizmo(&mut self, factory: impl GizmoFactory + 'static) {
        self.gizmos.push(Arc::new(factory));
    }

    /// The builder form of [`register_tool`](Self::register_tool).
    pub fn with_tool(mut self, id: impl Into<String>, factory: impl ToolFactory + 'static) -> Self {
        self.register_tool(id, factory);
        self
    }

    /// Register an interactive tool under a command-facing id. A later registration
    /// with the same id wins.
    pub fn register_tool(&mut self, id: impl Into<String>, factory: impl ToolFactory + 'static) {
        self.tools.push((id.into(), Arc::new(factory)));
    }

    /// A fresh tool session for `id`, if the edition registered one.
    pub fn tool_for(&self, id: &str) -> Option<Box<dyn Tool>> {
        self.tools
            .iter()
            .rev()
            .find(|(registered, _)| registered == id)
            .map(|(_, factory)| factory.create())
    }

    /// The manipulator for `selection`: the last registered factory that claims it wins.
    pub fn manipulator_for(
        &self,
        selection: &SelectionRef,
        stage: &Stage,
    ) -> Option<Box<dyn Manipulator>> {
        self.factories
            .iter()
            .rev()
            .find_map(|factory| factory.create(selection, stage))
    }

    /// The transform gizmo for `selection`, if a workbench offers one.
    pub fn gizmo_for(&self, selection: &SelectionRef, stage: &Stage) -> Option<TransformGizmo> {
        self.gizmos
            .iter()
            .rev()
            .find_map(|factory| factory.create(selection, stage))
    }

    /// Whether the edition contributes any manipulator, gizmo or tool.
    pub fn is_empty(&self) -> bool {
        self.factories.is_empty() && self.gizmos.is_empty() && self.tools.is_empty()
    }
}

impl std::fmt::Debug for Contributions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Contributions")
            .field("factories", &self.factories.len())
            .field("gizmos", &self.gizmos.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_stage::InteractionCommit;

    /// A manipulator that commits nothing; only the factory routing is under test.
    struct Stub;

    impl Manipulator for Stub {
        fn id(&self) -> &'static str {
            "stub"
        }

        fn items(&self) -> &[ferrocad_stage::ShapeItem] {
            &[]
        }

        fn hit(&self, _camera: &ferrocad_inventor::Camera, _device: [i32; 2]) -> bool {
            false
        }

        fn drag_begin(&mut self, _camera: &ferrocad_inventor::Camera, _device: [i32; 2]) -> bool {
            false
        }

        fn drag_update(&mut self, _camera: &ferrocad_inventor::Camera, _device: [i32; 2]) -> bool {
            false
        }

        fn drag_end(&mut self) -> Option<InteractionCommit> {
            None
        }

        fn drag_cancel(&mut self) -> bool {
            false
        }

        fn is_dragging(&self) -> bool {
            false
        }

        fn set_hovered(&mut self, _hovered: bool) -> bool {
            false
        }

        fn refresh(&mut self, _camera: &ferrocad_inventor::Camera) -> bool {
            false
        }
    }

    fn stage() -> Stage {
        Stage::new()
    }

    /// A tool that draws nothing; only the registry routing is under test.
    struct StubTool;

    impl Tool for StubTool {
        fn on_event(
            &mut self,
            _event: &ferrocad_stage::PointerEvent,
            _snap: Option<ferrocad_stage::Snap>,
        ) {
        }

        fn items(&self) -> &[ferrocad_stage::ShapeItem] {
            &[]
        }
    }

    #[test]
    fn a_registered_tool_is_found_by_id() {
        let contributions =
            Contributions::new().with_tool("stub", || Box::new(StubTool) as Box<dyn Tool>);
        assert!(!contributions.is_empty());
        assert!(contributions.tool_for("stub").is_some());
        assert!(contributions.tool_for("missing").is_none());
    }

    #[test]
    fn a_later_tool_registration_wins() {
        let contributions = Contributions::new()
            .with_tool("stub", || Box::new(StubTool) as Box<dyn Tool>)
            .with_tool("stub", || Box::new(StubTool) as Box<dyn Tool>);
        assert!(contributions.tool_for("stub").is_some());
    }

    #[test]
    fn the_base_app_contributes_nothing() {
        let contributions = Contributions::new();
        assert!(contributions.is_empty());
        assert!(
            contributions
                .manipulator_for(&SelectionRef::element(1, "Edge1"), &stage())
                .is_none()
        );
    }

    #[test]
    fn a_registered_factory_claims_its_edges() {
        let contributions =
            Contributions::new().with_manipulator(|selection: &SelectionRef, _stage: &Stage| {
                selection
                    .elements
                    .first()
                    .filter(|element| element.starts_with("Edge"))
                    .map(|_| Box::new(Stub) as Box<dyn Manipulator>)
            });
        assert!(!contributions.is_empty());
        assert!(
            contributions
                .manipulator_for(&SelectionRef::element(1, "Edge1"), &stage())
                .is_some()
        );
        assert!(
            contributions
                .manipulator_for(&SelectionRef::element(1, "Face1"), &stage())
                .is_none()
        );
    }

    #[test]
    fn the_last_registered_factory_wins() {
        let contributions = Contributions::new()
            .with_manipulator(|_: &SelectionRef, _: &Stage| {
                Some(Box::new(Stub) as Box<dyn Manipulator>)
            })
            .with_manipulator(|_: &SelectionRef, _: &Stage| None);
        // The later factory declines, so the earlier one still gets its turn.
        assert!(
            contributions
                .manipulator_for(&SelectionRef::whole(1), &stage())
                .is_some()
        );
    }

    #[test]
    fn a_registered_gizmo_factory_claims_its_objects() {
        use ferrocad_stage::{GizmoMode, TransformGizmo};
        use ferrocad_types::Vector3;

        let contributions =
            Contributions::new().with_gizmo(|selection: &SelectionRef, _stage: &Stage| {
                selection.is_whole().then(|| {
                    TransformGizmo::new(selection.object, Vector3::zero(), GizmoMode::Translate)
                })
            });
        assert!(!contributions.is_empty());
        let gizmo = contributions
            .gizmo_for(&SelectionRef::whole(3), &stage())
            .expect("a gizmo");
        assert_eq!(gizmo.target_id(), 3);
        assert!(
            contributions
                .gizmo_for(&SelectionRef::element(3, "Face1"), &stage())
                .is_none()
        );
    }
}
