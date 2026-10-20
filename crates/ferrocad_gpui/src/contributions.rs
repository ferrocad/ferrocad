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
use ferrocad_stage::{Manipulator, Stage};

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

/// The manipulators an edition contributes, in registration order.
#[derive(Clone, Default)]
pub struct Contributions {
    factories: Vec<Arc<dyn ManipulatorFactory>>,
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

    /// Whether the edition contributes any manipulator.
    pub fn is_empty(&self) -> bool {
        self.factories.is_empty()
    }
}

impl std::fmt::Debug for Contributions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Contributions")
            .field("factories", &self.factories.len())
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
}
