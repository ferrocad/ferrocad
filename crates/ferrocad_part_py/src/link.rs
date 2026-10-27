//! The `App::Link` base: shared link properties and the Part-side execute path.
//!
//! Upstream `App::Link` is a real object type with a property base
//! (`Placement`, `LinkPlacement`, `Scale`, `LinkTransform`, `LinkedObject`,
//! `ShowElement`, the element list, …) whose `execute` copies or transforms the
//! linked object's shape. `Part::LinkArray*` derive from it.
//!
//! FerroCAD keeps the geometry-free defaults in `ferrocad_core::typeregistry`
//! so an `App::Link` can be created without a kernel, and registers the type
//! here, where the geometry backend exists. Core's `typeregistry` module is
//! private to that crate, so [`link_properties`] mirrors its `App::Link` entry
//! and adds the shape-valued `Shape`. That helper is the one property list the
//! link and the `Part::LinkArray*` types share.
//!
//! This is slices L1–L4 of `docs/project/record/tasks/links.md`. Element expansion is real:
//! `ShowElement` creates [`ELEMENT_TYPE`] children with a `Placement`, a link
//! back to the source and a derived `Shape`, and collapsing removes them again.
//! L4 adds a non-uniform `ScaleVector` (through the backend's general-affine
//! transform) and the [`LINK_GROUP_TYPE`]/[`LINK_LIST_TYPE`] containers, whose
//! `ElementList` is their `Group` membership.
//!
//! **`LinkMode` and `LinkCopyOnChange` are stored but inert.** A FerroCAD link is
//! a name reference to one object (or, for a container, a list of names), which
//! is upstream's `KeepLink` behaviour; `AutoDelete` would need a delete cascade
//! and `Translate` a standalone copy, neither of which exists. Copy-on-change
//! needs an object clone/copy facility that this tree does not have, so none of
//! its modes is honoured. `execute` never reads either property. Lineage-backed
//! sub-element resolution (L5) is also still absent.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use ferrocad_core::geometry::{Matrix4, Placement, Rotation, Vector3};
use ferrocad_core::object_registry::{self, ObjectType};
use ferrocad_core::{prop_status, Document, ObjectId, Property};
use ferrocad_geom::Shape;
use ferrocad_part::{make_shape_property, shape_of, ShapeProperty};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};

/// The `App::Link` type id. `Part::LinkArray*` are treated as its peers by the
/// query layer's link table.
pub(crate) const LINK_TYPE: &str = "App::Link";

/// The generated child of an expanded link or link array (upstream
/// `App::LinkElement`).
pub(crate) const ELEMENT_TYPE: &str = "App::LinkElement";

/// A link to many objects (upstream `App::LinkGroup`). Its `ElementList` is its
/// `Group` membership, so `Element1` resolves to the first member.
pub(crate) const LINK_GROUP_TYPE: &str = "App::LinkGroup";

/// The list form of a link container (upstream `App::LinkList`). Registered with
/// the same behaviour as [`LINK_GROUP_TYPE`]: it follows its link list.
pub(crate) const LINK_LIST_TYPE: &str = "App::LinkList";

/// The element type FerroCAD used before L3. It is registered with the same
/// behaviour as [`ELEMENT_TYPE`] so documents saved by earlier versions still
/// load and recompute.
pub(crate) const LEGACY_ELEMENT_TYPE: &str = "Part::LinkArrayElement";

/// Status flags for the generated properties (`ElementCount`, `ElementList`,
/// `PlacementList`, `Shape`): computed output, never restored, never recomputed
/// from the GUI.
pub(crate) const HIDDEN_OUTPUT: u32 =
    prop_status::HIDDEN | prop_status::OUTPUT | prop_status::NORECOMPUTE | prop_status::READONLY;

/// Register `App::Link`, its containers, `App::LinkElement` and the legacy
/// element type id. Idempotent.
pub(crate) fn register() {
    object_registry::register(LINK_TYPE, Arc::new(LinkObject));
    let elements: Arc<dyn ObjectType> = Arc::new(LinkElementObject);
    object_registry::register(ELEMENT_TYPE, elements.clone());
    object_registry::register(LEGACY_ELEMENT_TYPE, elements);
    let containers: Arc<dyn ObjectType> = Arc::new(LinkContainerObject);
    object_registry::register(LINK_GROUP_TYPE, containers.clone());
    object_registry::register(LINK_LIST_TYPE, containers);
    ferrocad_py::register_extension_provider(LINK_EXTENSION, provide_link_extension);
    ferrocad_py::register_extension_method(
        LINK_EXTENSION,
        "configLinkProperty",
        config_link_property_method,
    );
}

// ---------------------------------------------------------------------------
// App::LinkExtensionPython
// ---------------------------------------------------------------------------

/// The script-added link extension (`obj.addExtension("App::LinkExtensionPython")`
/// then `obj.configLinkProperty(...)`), used by Draft's link-aware arrays.
pub(crate) const LINK_EXTENSION: &str = "App::LinkExtensionPython";

/// The properties `App::LinkExtension` adds.
///
/// This is [`link_properties`] without the shape-valued `Shape` (the object
/// already owns one) plus `ColoredElements`, the element colour list upstream's
/// link base carries. `Placement` is in the list too, but the provider only adds
/// the properties the object does not already have, so a `Part::FeaturePython`'s
/// own `Placement` is kept rather than shadowed.
fn extension_properties() -> Vec<(String, Property, u32)> {
    let mut properties: Vec<(String, Property, u32)> = link_properties()
        .into_iter()
        .filter(|(name, _, _)| name != "Shape")
        .collect();
    properties.push((
        "ColoredElements".to_string(),
        Property::LinkSub(String::new(), Vec::new()),
        prop_status::HIDDEN,
    ));
    properties
}

/// The provider `addExtension("App::LinkExtensionPython")` calls: add every link
/// property the object is missing, leaving existing values (notably `Placement`)
/// untouched.
fn provide_link_extension(doc: &mut Document, id: ObjectId) -> bool {
    let missing: Vec<(String, Property, u32)> = {
        let Some(object) = doc.object(id) else {
            return false;
        };
        extension_properties()
            .into_iter()
            .filter(|(name, _, _)| object.properties.get(name).is_none())
            .collect()
    };
    // A fresh attachment starts from a clean configuration; the mapping is
    // in-session state, not document state.
    link_configs().lock().unwrap().remove(&(document_key(doc), id));
    for (name, value, status) in missing {
        if doc
            .add_property(id, &name, value, status, "Link", "")
            .is_err()
        {
            return false;
        }
    }
    true
}

/// What `configLinkProperty` recorded for one object.
#[derive(Default)]
struct LinkConfig {
    /// Object properties the extension treats as link properties (positional
    /// arguments of `configLinkProperty`).
    linked: Vec<String>,
    /// Extension property name → the object property that supplies it, e.g.
    /// `LinkedObject` → `Base` for Draft's `configLinkProperty(LinkedObject="Base")`.
    sources: BTreeMap<String, String>,
}

fn link_configs() -> &'static Mutex<BTreeMap<(usize, ObjectId), LinkConfig>> {
    static CONFIGS: OnceLock<Mutex<BTreeMap<(usize, ObjectId), LinkConfig>>> = OnceLock::new();
    CONFIGS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// A document's identity for the in-session configuration table. A `Document`
/// lives behind the Python binding's `Arc<Mutex<...>>`, so its address is stable
/// for as long as the configuration can be consulted.
fn document_key(doc: &Document) -> usize {
    doc as *const Document as usize
}

/// Validate and record `configLinkProperty(properties, **links)`.
///
/// `properties` are positional names of the object's own properties that the
/// extension should treat as link properties. `links` maps an extension property
/// to the object property that supplies it, as in Draft's
/// `configLinkProperty(LinkedObject="Base")` (the object's `Base` property is the
/// link target). Every named *object* property must exist: a miss is an error
/// naming it, never a silent no-op.
fn configure_link_property(
    doc: &Document,
    id: ObjectId,
    properties: &[String],
    links: &[(String, String)],
) -> Result<(), String> {
    let Some(object) = doc.object(id) else {
        return Err(format!("no object {id}"));
    };
    for name in properties {
        if object.properties.get(name).is_none() {
            return Err(format!(
                "configLinkProperty: object '{}' has no property '{name}'",
                object.name
            ));
        }
    }
    for (link, source) in links {
        if object.properties.get(source).is_none() {
            return Err(format!(
                "configLinkProperty: object '{}' has no property '{source}' \
                 for the link property '{link}'",
                object.name
            ));
        }
    }
    let mut configs = link_configs().lock().unwrap();
    let config = configs.entry((document_key(doc), id)).or_default();
    for name in properties {
        if !config.linked.contains(name) {
            config.linked.push(name.clone());
        }
    }
    for (link, source) in links {
        config.sources.insert(link.clone(), source.clone());
    }
    Ok(())
}

/// `configLinkProperty(property, **links)` as an extension method.
///
/// Upstream's signature is positional property names plus keyword link mappings.
/// The binding hands them over as the positional tuple with a final `dict` when
/// kwargs were given, e.g. `("Placement", {"LinkedObject": "Base"})`.
fn config_link_property_method(
    doc: &mut Document,
    id: ObjectId,
    _method: &str,
    args: &Bound<'_, PyAny>,
) -> PyResult<PyObject> {
    let py = args.py();
    let mut properties: Vec<String> = Vec::new();
    let mut links: Vec<(String, String)> = Vec::new();
    for item in args.downcast::<PyTuple>()?.iter() {
        if let Ok(kwargs) = item.downcast::<PyDict>() {
            for (key, value) in kwargs.iter() {
                links.push((key.extract()?, value.extract()?));
            }
        } else {
            properties.push(item.extract()?);
        }
    }
    configure_link_property(doc, id, &properties, &links).map_err(PyValueError::new_err)?;
    Ok(py.None())
}

// ---------------------------------------------------------------------------
// Shared property base
// ---------------------------------------------------------------------------

/// The property base shared by `App::Link` and (upstream) `Part::LinkArray*`.
///
/// This mirrors core's private `typeregistry` entry for the `App::Link` type id,
/// which a document without `Part` uses. Keeping the list in one place for both
/// the link and the arrays is what this helper is for. The shape-valued `Shape`
/// is added here because core cannot construct a `Part::PropertyPartShape`.
pub(crate) fn link_properties() -> Vec<(String, Property, u32)> {
    let mut properties: Vec<(String, Property, u32)> = vec![
        (
            "Placement".to_string(),
            Property::Placement(Placement::identity()),
            prop_status::NONE,
        ),
        (
            "LinkPlacement".to_string(),
            Property::Placement(Placement::identity()),
            prop_status::NONE,
        ),
        ("Scale".to_string(), Property::Float(1.0), prop_status::NONE),
        (
            "ScaleVector".to_string(),
            Property::Vector(Vector3::new(1.0, 1.0, 1.0)),
            prop_status::NONE,
        ),
        (
            "LinkTransform".to_string(),
            Property::Bool(false),
            prop_status::NONE,
        ),
        // Stored for upstream compatibility, never read by `execute`: a link is
        // a name reference (`LinkedObject`), i.e. `KeepLink`; `AutoDelete` and
        // `Translate` need a delete cascade / copy facility that does not exist.
        // See the module docs.
        (
            "LinkMode".to_string(),
            Property::Enumeration(
                ["AutoDelete", "KeepLink", "Translate"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                0,
            ),
            prop_status::NONE,
        ),
        // Inert: there is no object clone/copy facility, so none of the modes can
        // be honoured. Do not invent copy-on-change semantics here.
        (
            "LinkCopyOnChange".to_string(),
            Property::Enumeration(
                ["Disabled", "Enabled", "Owned", "Tracking"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                0,
            ),
            prop_status::NONE,
        ),
        (
            "LinkedObject".to_string(),
            Property::Link(String::new()),
            prop_status::NONE,
        ),
        ("ShowElement".to_string(), Property::Bool(true), prop_status::NONE),
        (
            "ElementCount".to_string(),
            Property::Integer(0),
            HIDDEN_OUTPUT,
        ),
        (
            "PlacementList".to_string(),
            Property::PlacementList(Vec::new()),
            HIDDEN_OUTPUT,
        ),
        (
            "ElementList".to_string(),
            Property::LinkList(Vec::new()),
            HIDDEN_OUTPUT,
        ),
    ];
    properties.push((
        "Shape".to_string(),
        make_shape_property(ShapeProperty::empty()),
        HIDDEN_OUTPUT,
    ));
    properties
}

/// The property base of `App::LinkGroup` / `App::LinkList`.
///
/// A container is a link whose element set is its `Group` membership rather than
/// one transformed source, so it gets the whole link base plus `Group`. Its
/// `ElementList` is the same membership (upstream's `isLinkGroup()` convention),
/// which is why no generated `App::LinkElement` children are created for it.
pub(crate) fn container_properties() -> Vec<(String, Property, u32)> {
    let mut properties = link_properties();
    properties.push((
        "Group".to_string(),
        Property::LinkList(Vec::new()),
        prop_status::NONE,
    ));
    properties
}

/// The property base of a generated element.
///
/// Upstream's `App::LinkElement` is a link in its own right: it carries a
/// `Placement` (element-local), the same `LinkedObject` as its owner, the link
/// transform properties and (through `SuppressibleExtension`) `Suppressed`.
/// `Shape` is added here because core cannot construct a
/// `Part::PropertyPartShape`.
pub(crate) fn element_properties() -> Vec<(String, Property, u32)> {
    vec![
        (
            "Placement".to_string(),
            Property::Placement(Placement::identity()),
            prop_status::NONE,
        ),
        (
            "LinkedObject".to_string(),
            Property::Link(String::new()),
            prop_status::NONE,
        ),
        ("Scale".to_string(), Property::Float(1.0), prop_status::NONE),
        (
            "ScaleVector".to_string(),
            Property::Vector(Vector3::new(1.0, 1.0, 1.0)),
            prop_status::HIDDEN,
        ),
        (
            "LinkTransform".to_string(),
            Property::Bool(false),
            prop_status::NONE,
        ),
        (
            "LinkPlacement".to_string(),
            Property::Placement(Placement::identity()),
            prop_status::NONE,
        ),
        (
            "Suppressed".to_string(),
            Property::Bool(false),
            prop_status::NONE,
        ),
        (
            "Shape".to_string(),
            make_shape_property(ShapeProperty::empty()),
            HIDDEN_OUTPUT,
        ),
    ]
}

// ---------------------------------------------------------------------------
// Property access
// ---------------------------------------------------------------------------

fn property<'a>(doc: &'a Document, id: ObjectId, name: &str) -> Option<&'a Property> {
    doc.object(id)?.properties.get(name)
}

fn placement_property(doc: &Document, id: ObjectId, name: &str) -> Placement {
    match property(doc, id, name) {
        Some(Property::Placement(placement)) => *placement,
        _ => Placement::identity(),
    }
}

fn float_property(doc: &Document, id: ObjectId, name: &str, default: f64) -> f64 {
    match property(doc, id, name) {
        Some(Property::Float(value)) => *value,
        Some(Property::Integer(value)) => *value as f64,
        _ => default,
    }
}

fn int_property(doc: &Document, id: ObjectId, name: &str, default: i64) -> i64 {
    match property(doc, id, name) {
        Some(Property::Integer(value)) => *value,
        Some(Property::IntegerConstraint { value, .. }) => *value,
        _ => default,
    }
}

fn bool_property(doc: &Document, id: ObjectId, name: &str, default: bool) -> bool {
    match property(doc, id, name) {
        Some(Property::Bool(value)) => *value,
        _ => default,
    }
}

fn vector_property(doc: &Document, id: ObjectId, name: &str, default: Vector3) -> Vector3 {
    match property(doc, id, name) {
        Some(Property::Vector(value)) => *value,
        _ => default,
    }
}

/// The target object name stored in `LinkedObject`, which may be a plain link or
/// a link-with-sub-elements. A `LinkSub`'s sub-names are ignored for now
/// (sub-elements still resolve positionally, and properly only with lineage, L5).
fn link_target(doc: &Document, id: ObjectId) -> Option<String> {
    match property(doc, id, "LinkedObject") {
        Some(Property::Link(name)) if !name.is_empty() => Some(name.clone()),
        Some(Property::LinkSub(name, _)) if !name.is_empty() => Some(name.clone()),
        _ => None,
    }
}

/// The transform a link applies to its target's geometry.
///
/// `general` records whether the 4x4 is an affine transform that the similarity
/// path (an OCCT `gp_Trsf`) cannot represent — a non-uniform `ScaleVector`.
/// Uniform scale stays on the cheaper similarity path.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LinkTransform {
    pub(crate) matrix: Matrix4,
    pub(crate) general: bool,
}

impl LinkTransform {
    /// The identity transform (the arrays compose their own element placements).
    pub(crate) fn identity() -> Self {
        Self {
            matrix: Matrix4::identity(),
            general: false,
        }
    }

    /// Compose a local placement after this transform.
    fn then(&self, local: &Matrix4) -> Self {
        Self {
            matrix: self.matrix.mul(local),
            general: self.general,
        }
    }
}

/// The transform an `App::Link` applies to its target's shape.
///
/// - `LinkTransform = false` (default): the link's own `Placement`.
/// - `LinkTransform = true`: `LinkPlacement`, the link's global placement.
///
/// `Scale` multiplies `ScaleVector`. A uniform vector folds into one uniform
/// factor; a non-uniform vector becomes a diagonal affine scale applied after the
/// placement, which [`transformed`] sends through the kernel's general-transform
/// path (`BRepBuilderAPI_GTransform`) because a similarity cannot express it.
fn link_transform(doc: &Document, id: ObjectId) -> LinkTransform {
    let placement = if bool_property(doc, id, "LinkTransform", false) {
        placement_property(doc, id, "LinkPlacement")
    } else {
        placement_property(doc, id, "Placement")
    };
    let matrix = placement.to_matrix();

    let scale = float_property(doc, id, "Scale", 1.0);
    let vector = vector_property(doc, id, "ScaleVector", Vector3::new(1.0, 1.0, 1.0));
    let uniform = (vector.x - vector.y).abs() <= 1e-12 && (vector.x - vector.z).abs() <= 1e-12;
    if uniform {
        let factor = scale * vector.x;
        LinkTransform {
            matrix: if (factor - 1.0).abs() > 1e-12 {
                matrix.mul(&uniform_scale_matrix(factor))
            } else {
                matrix
            },
            general: false,
        }
    } else {
        let mut values = Matrix4::identity().m;
        values[0] = scale * vector.x;
        values[5] = scale * vector.y;
        values[10] = scale * vector.z;
        LinkTransform {
            matrix: matrix.mul(&Matrix4::from_values(values)),
            general: true,
        }
    }
}

fn uniform_scale_matrix(factor: f64) -> Matrix4 {
    let mut values = Matrix4::identity().m;
    values[0] = factor;
    values[5] = factor;
    values[10] = factor;
    Matrix4::from_values(values)
}

/// Write (or clear) an object's `Shape`. Shared with the link arrays so both
/// keep the same "shape is world" convention.
pub(crate) fn write_shape(doc: &mut Document, id: ObjectId, shape: Option<Shape>) {
    let property = match shape {
        Some(shape) => make_shape_property(ShapeProperty::new(shape)),
        None => make_shape_property(ShapeProperty::empty()),
    };
    doc.set_property_raw(id, "Shape", property);
}

// ---------------------------------------------------------------------------
// Element management
// ---------------------------------------------------------------------------

/// The generated-element object names of `id`, resolved and in element order.
pub(crate) fn element_ids(doc: &Document, id: ObjectId) -> Vec<ObjectId> {
    match property(doc, id, "ElementList") {
        Some(Property::LinkList(names)) => names
            .iter()
            .filter_map(|name| doc.get_by_name(name))
            .collect(),
        _ => Vec::new(),
    }
}

fn placement_list(doc: &Document, id: ObjectId, name: &str) -> Vec<Placement> {
    match property(doc, id, name) {
        Some(Property::PlacementList(values)) => values.clone(),
        _ => Vec::new(),
    }
}

fn linked_shape(doc: &Document, name: &str) -> Option<Shape> {
    let id = doc.get_by_name(name)?;
    property(doc, id, "Shape")
        .and_then(shape_of)
        .and_then(|property| property.shape())
}

/// Transform `source` by `transform`, cloning it when the transform is the
/// identity and choosing the similarity or general-affine kernel path.
fn transformed(source: &Shape, transform: &LinkTransform) -> Option<Shape> {
    if transform.matrix.is_unity(1e-12) {
        Some(source.clone())
    } else if transform.general {
        ferrocad_part::backend()
            .general_transform(source, &transform.matrix)
            .ok()
    } else {
        ferrocad_part::backend()
            .transform(source, &transform.matrix)
            .ok()
    }
}

/// Upstream's default element grid (`Link.cpp`, `onChanged(ElementCount)`):
/// `(i % 10, (i / 10) % 10, i / 100)`.
fn grid_placement(index: usize) -> Placement {
    Placement {
        base: Vector3::new(
            (index % 10) as f64,
            ((index / 10) % 10) as f64,
            (index / 100) as f64,
        ),
        rotation: Rotation::identity(),
    }
}

/// The placements an `App::Link` should show.
///
/// The element set is the one the link already has (when expanded), otherwise
/// `PlacementList`. Reading the elements even while collapsing is deliberate:
/// their `Placement` is the user-visible position that must move into
/// `PlacementList`. `ElementCount` is the requested size: a larger count is
/// filled with upstream's default grid, a smaller one truncates. Arrays compute
/// their own placements and never call this.
fn app_link_placements(doc: &Document, id: ObjectId) -> Vec<Placement> {
    let count = int_property(doc, id, "ElementCount", 0).max(0) as usize;
    let existing = element_ids(doc, id);

    let mut placements: Vec<Placement> = if !existing.is_empty() {
        existing
            .iter()
            .map(|element| placement_property(doc, *element, "Placement"))
            .collect()
    } else {
        placement_list(doc, id, "PlacementList")
    };
    if placements.len() > count {
        placements.truncate(count);
    }
    for index in placements.len()..count {
        placements.push(grid_placement(index));
    }
    placements
}

/// Bring the generated-element objects in line with `placements`.
///
/// With `ShowElement` set, one `App::LinkElement` exists per placement and owns
/// the element-local `Placement` plus a `Shape` already composed with
/// `parent_matrix`; `PlacementList` is cleared (upstream reads the placements
/// back from the elements). With `ShowElement` clear, the placements move to
/// `PlacementList` and every child is removed from the document, so collapsing
/// leaves no stale objects.
///
/// Returns `Ok(true)` when expanded and `Ok(false)` when collapsed; `Err` means
/// the kernel refused one of the element transforms.
pub(crate) fn sync_elements(
    doc: &mut Document,
    owner: ObjectId,
    placements: &[Placement],
    flags: Option<&[bool]>,
    parent: &LinkTransform,
) -> Result<bool, ()> {
    doc.set_property_raw(
        owner,
        "ElementCount",
        Property::Integer(placements.len() as i64),
    );
    let mut ids = element_ids(doc, owner);

    if !bool_property(doc, owner, "ShowElement", true) {
        if placement_list(doc, owner, "PlacementList") != placements {
            doc.set_property_raw(
                owner,
                "PlacementList",
                Property::PlacementList(placements.to_vec()),
            );
        }
        doc.set_property_raw(owner, "ElementList", Property::LinkList(Vec::new()));
        for element in ids {
            doc.remove_object(element);
        }
        return Ok(false);
    }

    let owner_name = doc
        .object(owner)
        .map(|object| object.name.clone())
        .unwrap_or_else(|| "Link".to_string());
    if ids.len() > placements.len() {
        for element in ids.split_off(placements.len()) {
            doc.remove_object(element);
        }
    }
    while ids.len() < placements.len() {
        let name = format!("{owner_name}_Element{}", ids.len());
        ids.push(doc.add_object_with(&name, ELEMENT_TYPE, true));
    }

    let source_name = link_target(doc, owner);
    let source = source_name.as_deref().and_then(|name| linked_shape(doc, name));
    // Resolve every shape before writing anything: a kernel failure must not
    // leave a half-updated element set behind.
    let shapes: Vec<Option<Shape>> = match &source {
        Some(source) => placements
            .iter()
            .map(|placement| {
                let element = parent.then(&placement.to_matrix());
                transformed(source, &element).ok_or(())
            })
            .collect::<Result<Vec<Shape>, ()>>()?
            .into_iter()
            .map(Some)
            .collect(),
        None => placements.iter().map(|_| None).collect(),
    };

    for (index, element) in ids.iter().enumerate() {
        let placement = placements[index];
        doc.set_property_raw(*element, "Placement", Property::Placement(placement));
        if let Some(name) = &source_name {
            doc.set_property_raw(*element, "LinkedObject", Property::Link(name.clone()));
        } else {
            doc.set_property_raw(*element, "LinkedObject", Property::Link(String::new()));
        }
        if let Some(flags) = flags {
            let suppressed = flags.get(index).copied().unwrap_or(false);
            doc.set_property_raw(*element, "Suppressed", Property::Bool(suppressed));
        }
        write_shape(doc, *element, shapes[index].clone());
    }

    let names: Vec<String> = ids
        .iter()
        .filter_map(|element| doc.object(*element).map(|object| object.name.clone()))
        .collect();
    doc.set_property_raw(owner, "ElementList", Property::LinkList(names));
    if !placement_list(doc, owner, "PlacementList").is_empty() {
        doc.set_property_raw(owner, "PlacementList", Property::PlacementList(Vec::new()));
    }
    Ok(true)
}

/// Remove every generated child and clear the element outputs.
fn clear_elements(doc: &mut Document, owner: ObjectId) {
    for element in element_ids(doc, owner) {
        doc.remove_object(element);
    }
    doc.set_property_raw(owner, "ElementCount", Property::Integer(0));
    doc.set_property_raw(owner, "ElementList", Property::LinkList(Vec::new()));
    doc.set_property_raw(owner, "PlacementList", Property::PlacementList(Vec::new()));
}

// ---------------------------------------------------------------------------
// The object type
// ---------------------------------------------------------------------------

struct LinkObject;

impl ObjectType for LinkObject {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        link_properties()
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        // No target, a dangling target name, or a target whose `Shape` is unset
        // all mean "nothing to show", not a failure: the link stays valid with an
        // empty shape (upstream's `ShowElement`/collapsed-link behaviour).
        let Some(target_name) = link_target(doc, id) else {
            clear_elements(doc, id);
            write_shape(doc, id, None);
            return;
        };
        let Some(target_id) = doc.get_by_name(&target_name) else {
            clear_elements(doc, id);
            write_shape(doc, id, None);
            return;
        };
        let Some(source) = doc
            .object(target_id)
            .and_then(|object| object.properties.get("Shape"))
            .and_then(shape_of)
            .and_then(|property| property.shape())
        else {
            clear_elements(doc, id);
            write_shape(doc, id, None);
            return;
        };

        let transform = link_transform(doc, id);
        let placements = app_link_placements(doc, id);
        if placements.is_empty() {
            // No element expansion: the link is a single transformed instance.
            clear_elements(doc, id);
            match transformed(&source, &transform) {
                Some(shape) => write_shape(doc, id, Some(shape)),
                None => doc.mark_invalid(id),
            }
            return;
        }

        match sync_elements(doc, id, &placements, None, &transform) {
            Ok(true) => {
                // The generated children carry the geometry while expanded.
                write_shape(doc, id, None);
            }
            Ok(false) => {
                // Collapsed: one compound of every transformed instance.
                let mut shapes = Vec::with_capacity(placements.len());
                for placement in &placements {
                    let element = transform.then(&placement.to_matrix());
                    match transformed(&source, &element) {
                        Some(shape) => shapes.push(shape),
                        None => {
                            doc.mark_invalid(id);
                            return;
                        }
                    }
                }
                match ferrocad_part::backend().make_compound(&shapes) {
                    Ok(compound) => write_shape(doc, id, Some(compound)),
                    Err(_) => doc.mark_invalid(id),
                }
            }
            Err(()) => doc.mark_invalid(id),
        }
    }
}

// ---------------------------------------------------------------------------
// The generated element
// ---------------------------------------------------------------------------

/// `App::LinkElement`: the object an expanded link creates per element.
///
/// It links back to the *source object* (not the owning link, which would make
/// the dependency graph cyclic: the owner already lists the element), carries
/// the element-local `Placement`, and derives `Shape` from the source with its
/// own link transform. When the owning link or array recomputes it overwrites
/// both the placement and the shape, composing the owner's transform.
struct LinkElementObject;

impl ObjectType for LinkElementObject {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        element_properties()
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let Some(target_name) = link_target(doc, id) else {
            write_shape(doc, id, None);
            return;
        };
        let Some(target_id) = doc.get_by_name(&target_name) else {
            write_shape(doc, id, None);
            return;
        };
        let Some(source) = doc
            .object(target_id)
            .and_then(|object| object.properties.get("Shape"))
            .and_then(shape_of)
            .and_then(|property| property.shape())
        else {
            write_shape(doc, id, None);
            return;
        };
        let transform = link_transform(doc, id);
        match transformed(&source, &transform) {
            Some(shape) => write_shape(doc, id, Some(shape)),
            None => doc.mark_invalid(id),
        }
    }
}

// ---------------------------------------------------------------------------
// The link containers
// ---------------------------------------------------------------------------

/// `App::LinkGroup` / `App::LinkList`: a link to many objects.
///
/// Upstream marks these with `isLinkGroup()` and lets `ElementList` hold the
/// actual grouped objects, not generated `App::LinkElement` children. That is the
/// model here: `ElementList` is the resolved `Group` membership, `ElementCount`
/// its size, and `Shape` the compound of the members transformed by the
/// container's own link transform. `ShowElement` therefore has no effect on a
/// container (there are no generated children to show or hide).
///
/// `LinkedObject` remains the ordinary base link property; when it is empty,
/// `getLinkedObject` reports `None` exactly as it does for an empty `App::Link`.
struct LinkContainerObject;

impl ObjectType for LinkContainerObject {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        container_properties()
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        let members: Vec<String> = match property(doc, id, "Group") {
            Some(Property::LinkList(names)) => names
                .iter()
                .filter(|name| doc.get_by_name(name).is_some())
                .cloned()
                .collect(),
            _ => Vec::new(),
        };
        doc.set_property_raw(
            id,
            "ElementCount",
            Property::Integer(members.len() as i64),
        );
        doc.set_property_raw(id, "ElementList", Property::LinkList(members.clone()));
        doc.set_property_raw(id, "PlacementList", Property::PlacementList(Vec::new()));

        let transform = link_transform(doc, id);
        let mut shapes = Vec::with_capacity(members.len());
        for name in &members {
            let Some(source) = linked_shape(doc, name) else {
                continue;
            };
            match transformed(&source, &transform) {
                Some(shape) => shapes.push(shape),
                None => {
                    doc.mark_invalid(id);
                    return;
                }
            }
        }
        if shapes.is_empty() {
            write_shape(doc, id, None);
            return;
        }
        match ferrocad_part::backend().make_compound(&shapes) {
            Ok(compound) => write_shape(doc, id, Some(compound)),
            Err(_) => doc.mark_invalid(id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_properties_cover_the_core_defaults_and_shape() {
        let properties = link_properties();
        let names: Vec<&str> = properties.iter().map(|(name, _, _)| name.as_str()).collect();
        for expected in [
            "LinkedObject",
            "Placement",
            "LinkPlacement",
            "Scale",
            "ScaleVector",
            "LinkTransform",
            "LinkMode",
            "LinkCopyOnChange",
            "ShowElement",
            "ElementCount",
            "PlacementList",
            "ElementList",
            "Shape",
        ] {
            assert!(names.contains(&expected), "missing {expected}");
        }
    }

    #[test]
    fn container_properties_add_group_to_the_link_base() {
        let names: Vec<String> = container_properties()
            .into_iter()
            .map(|(name, _, _)| name)
            .collect();
        for expected in [
            "Group",
            "LinkedObject",
            "Placement",
            "ElementCount",
            "PlacementList",
            "ElementList",
            "Shape",
        ] {
            assert!(names.iter().any(|name| name == expected), "missing {expected}");
        }
    }

    #[test]
    fn element_properties_cover_the_link_and_suppression_surface() {
        let names: Vec<String> = element_properties()
            .into_iter()
            .map(|(name, _, _)| name)
            .collect();
        for expected in [
            "Placement",
            "LinkedObject",
            "Scale",
            "ScaleVector",
            "LinkTransform",
            "LinkPlacement",
            "Suppressed",
            "Shape",
        ] {
            assert!(names.iter().any(|name| name == expected), "missing {expected}");
        }
    }

    #[test]
    fn default_element_grid_matches_upstream() {
        let placement = grid_placement(23);
        assert!((placement.base.x - 3.0).abs() < 1e-12);
        assert!((placement.base.y - 2.0).abs() < 1e-12);
        assert!((placement.base.z).abs() < 1e-12);
    }

    #[test]
    fn extension_properties_cover_the_link_surface_without_shape() {
        let names: Vec<String> = extension_properties()
            .into_iter()
            .map(|(name, _, _)| name)
            .collect();
        for expected in [
            "LinkedObject",
            "Placement",
            "LinkPlacement",
            "Scale",
            "ScaleVector",
            "LinkTransform",
            "LinkMode",
            "LinkCopyOnChange",
            "ShowElement",
            "ElementCount",
            "PlacementList",
            "ElementList",
            "ColoredElements",
        ] {
            assert!(names.iter().any(|name| name == expected), "missing {expected}");
        }
        assert!(
            !names.iter().any(|name| name == "Shape"),
            "the extension must not re-add the object's Shape"
        );
    }

    #[test]
    fn link_extension_provider_adds_missing_properties_and_keeps_placement() {
        let mut doc = Document::new();
        let id = doc.add_object("Array", "Part::FeaturePython");
        let placement = Placement {
            base: Vector3::new(1.0, 2.0, 3.0),
            rotation: ferrocad_core::Rotation::identity(),
        };
        doc.set_property(id, "Placement", Property::Placement(placement))
            .unwrap();

        assert!(provide_link_extension(&mut doc, id));
        let object = doc.object(id).unwrap();
        assert!(matches!(
            object.properties.get("LinkedObject"),
            Some(Property::Link(_))
        ));
        assert!(matches!(
            object.properties.get("LinkTransform"),
            Some(Property::Bool(false))
        ));
        assert!(object.properties.get("ShowElement").is_some());
        assert!(object.properties.get("PlacementList").is_some());
        assert_eq!(
            object.properties.get("Placement"),
            Some(&Property::Placement(placement)),
            "the object's own Placement value is kept"
        );

        // Idempotent: a second attach adds nothing and does not fail.
        assert!(provide_link_extension(&mut doc, id));
    }

    #[test]
    fn config_link_property_records_links_and_rejects_missing_properties() {
        let mut doc = Document::new();
        let id = doc.add_object("Array", "Part::FeaturePython");
        doc.set_property(id, "Base", Property::Link(String::new()))
            .unwrap();
        doc.set_property(id, "Placement", Property::Placement(Placement::identity()))
            .unwrap();
        assert!(provide_link_extension(&mut doc, id));

        configure_link_property(
            &doc,
            id,
            &["Placement".to_string()],
            &[("LinkedObject".to_string(), "Base".to_string())],
        )
        .expect("Base exists, so the mapping is accepted");
        {
            let configs = link_configs().lock().unwrap();
            let config = configs.get(&(document_key(&doc), id)).unwrap();
            assert_eq!(config.linked, vec!["Placement".to_string()]);
            assert_eq!(
                config.sources.get("LinkedObject").map(String::as_str),
                Some("Base")
            );
        }

        // An unknown source property is an error, not a silent no-op.
        let error = configure_link_property(
            &doc,
            id,
            &[],
            &[("ShowElement".to_string(), "Missing".to_string())],
        )
        .unwrap_err();
        assert!(error.contains("Missing"), "{error}");

        // An unknown positional property is an error too.
        let error = configure_link_property(&doc, id, &["Missing".to_string()], &[])
            .unwrap_err();
        assert!(error.contains("Missing"), "{error}");
    }

    #[test]
    fn uniform_scale_is_applied_after_the_placement() {
        let matrix = Placement {
            base: Vector3::new(2.0, 0.0, 0.0),
            rotation: ferrocad_core::Rotation::identity(),
        }
        .to_matrix()
        .mul(&uniform_scale_matrix(2.0));
        let moved = matrix.transform(&Vector3::new(1.0, 1.0, 1.0));
        assert!((moved.x - 4.0).abs() < 1e-12);
        assert!((moved.y - 2.0).abs() < 1e-12);
        assert!((moved.z - 2.0).abs() < 1e-12);
    }
}
