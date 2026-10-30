//! Part's container objects: booleans (`Part::Fuse`/`Cut`/`Common`/`MultiFuse`)
//! and `Part::Compound`.
//!
//! Each follows the same shape as a dress-up feature: a `Base`/`Tools` (or `Shapes`)
//! link set, an output `Shape`, and `execute` that resolves the links through the
//! injected backend. The kernel does the work; this module only maps the document
//! properties onto [`GeometryBackend::fuse`](ferrocad_geom::GeometryBackend::fuse)
//! and friends.

use ferrocad_core::object_registry::ObjectType;
use ferrocad_core::{Document, ObjectId, Property, prop_status};
use ferrocad_geom::{History, Lineage, MapVersion, Shape};
use ferrocad_types::Placement;

use crate::shape_property::{ShapeProperty, make_shape_property, shape_of};

pub const FUSE_TYPE: &str = "Part::Fuse";
pub const CUT_TYPE: &str = "Part::Cut";
pub const COMMON_TYPE: &str = "Part::Common";
pub const MULTIFUSE_TYPE: &str = "Part::MultiFuse";
pub const MULTICOMMON_TYPE: &str = "Part::MultiCommon";
pub const COMPOUND_TYPE: &str = "Part::Compound";

const SHAPE: &str = "Shape";

/// Which boolean a [`Boolean`] object performs.
#[derive(Clone, Copy)]
pub(crate) enum BooleanKind {
    Fuse,
    Cut,
    Common,
}

impl BooleanKind {
    fn label(self) -> &'static str {
        match self {
            BooleanKind::Fuse => "fuse",
            BooleanKind::Cut => "cut",
            BooleanKind::Common => "common",
        }
    }

    fn operation(self, shape: &Shape, other: &Shape) -> Result<ferrocad_geom::OpResult, ferrocad_geom::GeomError> {
        match self {
            BooleanKind::Fuse => crate::backend().fuse(shape, other),
            BooleanKind::Cut => crate::backend().cut(shape, other),
            BooleanKind::Common => crate::backend().common(shape, other),
        }
    }
}

/// `Part::Fuse` / `Part::Cut` / `Part::Common`: `Base` minus/plus `Tools`.
pub(crate) struct Boolean {
    pub(crate) kind: BooleanKind,
}

/// `Part::MultiFuse` / `Part::MultiCommon`: fold `Shapes` with `fuse`/`common`.
pub(crate) struct Multi {
    pub(crate) kind: BooleanKind,
}

pub(crate) struct Compound;

fn output_shape() -> Property {
    make_shape_property(ShapeProperty::empty())
}

fn output_property() -> (String, Property, u32) {
    (SHAPE.to_string(), output_shape(), prop_status::OUTPUT)
}

fn link_properties() -> Vec<(String, Property, u32)> {
    // Upstream's `Part::Fuse`/`Cut`/`Common` take one `Base` and one `Tool`;
    // `Part::MultiFuse`/`MultiCommon` are the list forms.
    vec![
        (
            "Base".to_string(),
            Property::Link(String::new()),
            prop_status::NONE,
        ),
        (
            "Tool".to_string(),
            Property::Link(String::new()),
            prop_status::NONE,
        ),
        ("Refine".to_string(), Property::Bool(false), prop_status::NONE),
        // Upstream derives every Part feature from `Part::Feature`, so a boolean carries
        // a `Placement` too. Without it the shell has no anchor for the object (no move
        // gizmo) and a move commit has no property to write.
        placement_property(),
        output_property(),
    ]
}

/// The `Placement` every Part feature-shaped object carries.
///
/// It is applied by the display hook, not by `execute`: a boolean's operands are read
/// in their own frames, and the result is placed when it is drawn (the same contract a
/// `Part::Feature` has).
fn placement_property() -> (String, Property, u32) {
    (
        "Placement".to_string(),
        Property::Placement(Placement::identity()),
        prop_status::NONE,
    )
}


impl ObjectType for Boolean {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        link_properties()
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some((
            "Boolean".to_string(),
            format!("Part {} {name}", self.kind.label()),
        ))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let result = (|| -> Result<(Shape, Lineage), String> {
            let base = link(doc, id, "Base")
                .ok_or_else(|| "the boolean has no Base link".to_string())?;
            let shape = linked_shape(doc, &base)?;
            let tool = link(doc, id, "Tool")
                .ok_or_else(|| "the boolean has no Tool link".to_string())?;
            let other = linked_shape(doc, &tool)?;
            self.kind
                .operation(&shape, &other)
                .map(|out| (out.shape, lineage_from(&out.history)))
                .map_err(|error| error.to_string())
        })();
        store(doc, id, result);
    }
}

impl ObjectType for Multi {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            (
                "Shapes".to_string(),
                Property::LinkList(Vec::new()),
                prop_status::NONE,
            ),
            placement_property(),
            output_property(),
        ]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        let kind = if matches!(self.kind, BooleanKind::Common) {
            "multi-common"
        } else {
            "multi-fuse"
        };
        Some(("Boolean".to_string(), format!("Part {kind} {name}")))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let result = (|| -> Result<(Shape, Lineage), String> {
            let names = links(doc, id, "Shapes");
            let mut shapes = names.iter().map(|name| linked_shape(doc, name));
            let mut shape = shapes
                .next()
                .ok_or_else(|| "the boolean has no Shapes".to_string())??;
            // Each fold is one operation with its own history; fold them forward into
            // one lineage, the same way the geometry is folded.
            let mut lineage = Lineage::default();
            for other in shapes {
                let out = self
                    .kind
                    .operation(&shape, &other?)
                    .map_err(|error| error.to_string())?;
                lineage.apply(&out.history);
                shape = out.shape;
            }
            Ok((shape, lineage))
        })();
        store(doc, id, result);
    }
}

impl ObjectType for Compound {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        vec![
            (
                "Links".to_string(),
                Property::LinkList(Vec::new()),
                prop_status::NONE,
            ),
            placement_property(),
            output_property(),
        ]
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        Some(("Compound".to_string(), format!("Part compound {name}")))
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let result = (|| -> Result<(Shape, Lineage), String> {
            let names = links(doc, id, "Links");
            if names.is_empty() {
                return Err("the compound has no Links".to_string());
            }
            let shapes = names
                .iter()
                .map(|name| linked_shape(doc, name))
                .collect::<Result<Vec<_>, _>>()?;
            // `make_compound` reports no history, so the lineage stays empty rather
            // than inventing names for sub-shapes the kernel did not relate.
            let shape = crate::backend()
                .make_compound(&shapes)
                .map_err(|error| error.to_string())?;
            Ok((shape, Lineage::default()))
        })();
        store(doc, id, result);
    }
}

/// The object name of a `Link` property, when set.
fn link(doc: &Document, id: ObjectId, name: &str) -> Option<String> {
    match doc.object(id)?.properties.get(name)? {
        Property::Link(object) if !object.is_empty() => Some(object.clone()),
        _ => None,
    }
}

/// The object names of a `LinkList` property.
fn links(doc: &Document, id: ObjectId, name: &str) -> Vec<String> {
    match doc.object(id).and_then(|object| object.properties.get(name)) {
        Some(Property::LinkList(objects)) => objects.clone(),
        _ => Vec::new(),
    }
}

/// The shape a linked object currently holds.
fn linked_shape(doc: &Document, name: &str) -> Result<Shape, String> {
    let id = doc
        .get_by_name(name)
        .ok_or_else(|| format!("no object named {name}"))?;
    doc.object(id)
        .and_then(|object| object.properties.get(SHAPE))
        .and_then(shape_of)
        .and_then(|property| property.shape())
        .ok_or_else(|| format!("object {name} has no shape"))
}

/// Fold one operation's history into a fresh lineage under the default rules.
///
/// An empty history yields an empty lineage: no relation, no name.
fn lineage_from(history: &History) -> Lineage {
    Lineage::from_history(history, MapVersion::default())
}

/// Store a rebuild's outcome: the shape (and its lineage), or an empty output plus
/// the invalid flag.
fn store(doc: &mut Document, id: ObjectId, result: Result<(Shape, Lineage), String>) {
    match result {
        Ok((shape, lineage)) => {
            doc.set_property_raw(
                id,
                SHAPE,
                make_shape_property(ShapeProperty::with_lineage(shape, lineage)),
            );
        }
        Err(_) => {
            doc.mark_invalid(id);
            doc.set_property_raw(id, SHAPE, output_shape());
        }
    }
}
