//! The workbench contributions an edition makes to the shell.
//!
//! The base app links no workbench and registers nothing. An edition adds the
//! manipulators its workbench needs (the Part edge fillet arc, a BIM thickness grip,
//! …) through a **factory**: the shell calls it for each selection and hosts the first
//! match in the viewport.
//!
//! Gizmos are a **set**, not a single match: each factory declares the transform kinds
//! it offers, [`Contributions::gizmos_for`] returns them in a stable hit-priority order
//! (most recently registered first, specific kinds before the composed `universal`
//! one), and the shell narrows the set to the session kind a wheel verb names. A
//! workbench gizmo and the shared triad therefore coexist without one shadowing the
//! other.
//!
//! A commit routes back through the app script's `interaction` hook, so the shell still
//! names no workbench and links no kernel. A workbench that has no manipulators — BIM
//! has no dress-up — simply registers none, and never carries the Part code.

use std::sync::Arc;

use ferrocad_geom::SelectionRef;
use ferrocad_stage::{GizmoKind, Manipulator, Stage, Tool, TransformGizmo};
use ferrocad_types::Vector3;

/// The generic transform handle: any selection on a displayed item gets a triad.
///
/// This is the default every edition wants, so an edition only has to register it
/// (`Contributions::new().with_gizmo(translate_gizmo)`). The prototype is a **translate**
/// gizmo; the factory declares every transform kind ([`GizmoKind::ALL`]), so the
/// registry's set can also serve a `Rotate` or `Universal` session, and
/// [`gizmo_for`](Contributions::gizmo_for) still returns the translate entry first. A
/// workbench that wants only arrows registers
/// [`with_gizmo_kinds(&[GizmoKind::Translate], …)`](Contributions::with_gizmo_kinds).
///
/// The gizmo targets the whole item even when the selection names a sub-element: a
/// click on a face or an edge is a **spatial** selection, and the triad still acts on
/// the body. (Declining sub-elements here made the Part edition look like it had no gizmo at all: its meshes carry `Face1`/`Edge1`/… names, so every click
/// resolved to an element, while the toy's single untagged part resolved to the whole
/// body.)
///
/// The triad **hugs the object's own origin** (`ShapeItem::anchor`): the display
/// provider may report the placement's base point, which doubles as a visual origin
/// indicator, and a rotate gizmo pivots about it. A mesh with no declared origin falls
/// back to its bounding box's base corner.
pub fn translate_gizmo(selection: &SelectionRef, stage: &Stage) -> Option<TransformGizmo> {
    let anchor = stage.item(selection.object)?.anchor()?;
    Some(TransformGizmo::translate(selection.object, anchor))
}

/// The world-aligned base corner (minimum X/Y/Z) of a body's bounding box.
///
/// `None` for an entity the stage does not carry, or one with no positions. This is
/// the datum a non-spatial (tree or command) selection uses when it has no click
/// anchor.
pub fn body_corner(entity: u32, stage: &Stage) -> Option<Vector3> {
    let item = stage.item(entity)?;
    let positions = item.positions();
    if positions.is_empty() {
        return None;
    }
    let mut min = Vector3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    for [x, y, z] in positions {
        min.x = min.x.min(f64::from(*x));
        min.y = min.y.min(f64::from(*y));
        min.z = min.z.min(f64::from(*z));
    }
    min.x.is_finite().then_some(min)
}

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
    /// The representative gizmo for `selection`: its target, frame and style. The
    /// registry derives one entry per offered kind from it, so a factory only builds
    /// the datum once.
    fn create(&self, selection: &SelectionRef, stage: &Stage) -> Option<TransformGizmo>;

    /// The kinds this factory offers, in **hit-priority order**.
    ///
    /// The default is every transform kind, specific before composed
    /// ([`GizmoKind::ALL`]). A workbench that only wants arrows returns
    /// `&[GizmoKind::Translate]`; a kind it does not list is genuinely absent (its
    /// wheel verb cannot arm it), not silently substituted.
    fn kinds(&self) -> &'static [GizmoKind] {
        &GizmoKind::ALL
    }
}

impl<F> GizmoFactory for F
where
    F: Fn(&SelectionRef, &Stage) -> Option<TransformGizmo> + Send + Sync,
{
    fn create(&self, selection: &SelectionRef, stage: &Stage) -> Option<TransformGizmo> {
        self(selection, stage)
    }
}

/// A gizmo factory limited to the kinds an edition composes.
struct KindLimited<F> {
    factory: F,
    kinds: &'static [GizmoKind],
}

impl<F: GizmoFactory> GizmoFactory for KindLimited<F> {
    fn create(&self, selection: &SelectionRef, stage: &Stage) -> Option<TransformGizmo> {
        self.factory.create(selection, stage)
    }

    fn kinds(&self) -> &'static [GizmoKind] {
        self.kinds
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

    /// The builder form of [`register_gizmo_kinds`](Self::register_gizmo_kinds).
    pub fn with_gizmo_kinds(
        mut self,
        kinds: &'static [GizmoKind],
        factory: impl GizmoFactory + 'static,
    ) -> Self {
        self.register_gizmo_kinds(kinds, factory);
        self
    }

    /// Register a gizmo factory limited to the `kinds` an edition composes, in the
    /// given priority order.
    pub fn register_gizmo_kinds(
        &mut self,
        kinds: &'static [GizmoKind],
        factory: impl GizmoFactory + 'static,
    ) {
        self.gizmos.push(Arc::new(KindLimited { factory, kinds }));
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

    /// The transform gizmos for `selection`, in **hit-priority order**.
    ///
    /// Each factory that claims the selection contributes one entry per kind it offers.
    /// The order is stable and documented:
    ///
    /// 1. factories are visited **most recently registered first** (the same rule the
    ///    manipulator registry uses, so a workbench gizmo shadows the shared triad);
    /// 2. within one factory, `kinds()` decides, and the default order is the specific
    ///    kinds before the composed [`Universal`](GizmoKind::Universal) one.
    ///
    /// The set order **is** the hit priority: a host that can draw several submits the
    /// pointer to the first entry that claims it (see [`GizmoSet`](ferrocad_stage::GizmoSet)).
    /// `gizmo_for` is the single-host convenience form and returns the first entry.
    pub fn gizmos_for(
        &self,
        selection: &SelectionRef,
        stage: &Stage,
    ) -> Vec<TransformGizmo> {
        let mut gizmos = Vec::new();
        for factory in self.gizmos.iter().rev() {
            let Some(prototype) = factory.create(selection, stage) else {
                continue;
            };
            for &kind in factory.kinds() {
                gizmos.push(prototype.clone().with_kind(kind));
            }
        }
        gizmos
    }

    /// The highest-priority transform gizmo for `selection`, if a workbench offers one.
    ///
    /// This is [`gizmos_for`](Self::gizmos_for)'s first entry, with the factory's
    /// declared kind order applied. A session verb that needs another kind narrows the
    /// full set instead ([`GizmoSet::narrow`](ferrocad_stage::GizmoSet::narrow)).
    pub fn gizmo_for(&self, selection: &SelectionRef, stage: &Stage) -> Option<TransformGizmo> {
        self.gizmos_for(selection, stage).into_iter().next()
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
        fn id(&self) -> &str {
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

    /// A whole-object gizmo factory at `origin`, so the tests can tell registrations
    /// apart by their datum.
    fn gizmo_factory(origin: Vector3) -> impl GizmoFactory + 'static {
        move |_: &SelectionRef, _: &Stage| Some(TransformGizmo::translate(1, origin))
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
    fn the_default_translate_gizmo_claims_whole_objects_and_sub_elements() {
        use ferrocad_stage::ShapeItem;

        let mut stage = Stage::new();
        stage.add_item(ShapeItem::new(3).with_shape(
            vec![[0.0, 0.0, 0.0], [2.0, 2.0, 4.0]],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ));
        let gizmo = translate_gizmo(&SelectionRef::whole(3), &stage).expect("a gizmo");
        assert_eq!(gizmo.target_id(), 3);
        assert_eq!(
            gizmo.origin(),
            Vector3::zero(),
            "the base corner is the datum"
        );

        // A sub-element still gets the triad (the shell re-anchors it at the click);
        // an entity that is not displayed does not.
        let face = translate_gizmo(&SelectionRef::element(3, "Face1"), &stage)
            .expect("a face selection arms the triad for its body");
        assert_eq!(face.target_id(), 3);
        assert_eq!(face.origin(), Vector3::zero());
        assert!(translate_gizmo(&SelectionRef::whole(9), &stage).is_none());

        // A provider-declared origin wins over the bounding-box corner: this is what
        // makes the triad hug the object's own origin and double as an indicator.
        let mut placed = Stage::new();
        placed.add_item(
            ShapeItem::new(4)
                .with_shape(
                    vec![[2.0, 3.0, 4.0], [4.0, 5.0, 6.0]],
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                )
                .with_origin(Some([10.0, 20.0, 30.0])),
        );
        let hugged = translate_gizmo(&SelectionRef::whole(4), &placed).expect("a gizmo");
        assert_eq!(hugged.origin(), Vector3::new(10.0, 20.0, 30.0));
    }

    #[test]
    fn a_registered_gizmo_factory_claims_its_objects() {
        use ferrocad_stage::TransformGizmo;
        use ferrocad_types::Vector3;

        let contributions =
            Contributions::new().with_gizmo(|selection: &SelectionRef, _stage: &Stage| {
                selection.is_whole().then(|| {
                    TransformGizmo::translate(selection.object, Vector3::zero())
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

    #[test]
    fn the_gizmo_registry_is_ordered_latest_first_then_specific_before_composed() {
        let contributions = Contributions::new()
            .with_gizmo(gizmo_factory(Vector3::new(1.0, 0.0, 0.0)))
            .with_gizmo(gizmo_factory(Vector3::new(2.0, 0.0, 0.0)));
        let gizmos = contributions.gizmos_for(&SelectionRef::whole(7), &stage());

        // Six entries: three kinds per registration. The later registration is first
        // because that is the hit priority, and within it the specific kinds precede
        // the universal composition.
        assert_eq!(gizmos.len(), 6);
        assert_eq!(
            gizmos.iter().map(|gizmo| gizmo.origin().x).collect::<Vec<_>>(),
            vec![2.0, 2.0, 2.0, 1.0, 1.0, 1.0]
        );
        assert_eq!(
            gizmos.iter().map(|gizmo| gizmo.kind()).collect::<Vec<_>>(),
            vec![
                GizmoKind::Translate,
                GizmoKind::Rotate,
                GizmoKind::Universal,
                GizmoKind::Translate,
                GizmoKind::Rotate,
                GizmoKind::Universal,
            ]
        );
    }

    #[test]
    fn a_session_verb_narrows_the_registry_set_to_its_kind() {
        use ferrocad_stage::GizmoSet;

        let contributions = Contributions::new()
            .with_gizmo(gizmo_factory(Vector3::zero()))
            .with_gizmo(gizmo_factory(Vector3::new(1.0, 0.0, 0.0)));
        let set = GizmoSet::new(contributions.gizmos_for(&SelectionRef::whole(7), &stage()));

        let moved = set.narrow(GizmoKind::Translate).expect("a Move verb");
        assert_eq!(moved.kind(), GizmoKind::Translate);
        assert_eq!(moved.origin().x, 1.0, "the priority entry still wins");

        let rotated = set.narrow(GizmoKind::Rotate).expect("a Rotate verb");
        assert_eq!(rotated.kind(), GizmoKind::Rotate);
        assert_eq!(rotated.origin().x, 1.0);
    }

    #[test]
    fn a_kind_the_edition_does_not_offer_is_absent_when_its_verb_is_chosen() {
        use ferrocad_stage::GizmoSet;

        // An edition that composes only the move handles. Rotate is *not offered*, so
        // the rotate verb has to leave the selection without a gizmo rather than
        // substitute the arrows (or silently force rings onto them).
        let contributions = Contributions::new().with_gizmo_kinds(
            &[GizmoKind::Translate],
            gizmo_factory(Vector3::zero()),
        );
        let gizmos = contributions.gizmos_for(&SelectionRef::whole(7), &stage());
        assert_eq!(gizmos.len(), 1);
        assert_eq!(gizmos[0].kind(), GizmoKind::Translate);

        let set = GizmoSet::new(gizmos);
        assert!(set.narrow(GizmoKind::Rotate).is_none());
        assert!(set.narrow(GizmoKind::Universal).is_none());
        assert_eq!(
            set.narrow(GizmoKind::Translate)
                .expect("the offered kind still arms")
                .kind(),
            GizmoKind::Translate
        );
    }

    #[test]
    fn the_shared_factory_offers_every_transform_kind() {
        use ferrocad_stage::ShapeItem;

        let mut stage = Stage::new();
        stage.add_item(ShapeItem::new(3).with_shape(
            vec![[0.0, 0.0, 0.0], [2.0, 2.0, 4.0]],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ));
        let gizmos = Contributions::new()
            .with_gizmo(translate_gizmo)
            .gizmos_for(&SelectionRef::whole(3), &stage);
        assert_eq!(
            gizmos.iter().map(|gizmo| gizmo.kind()).collect::<Vec<_>>(),
            vec![
                GizmoKind::Translate,
                GizmoKind::Rotate,
                GizmoKind::Universal
            ]
        );
        assert_eq!(
            gizmos[0].origin(),
            Vector3::zero(),
            "every kind keeps the factory datum"
        );
    }
}
