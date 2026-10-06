//! The OCCT-side shape handle and helpers to build/borrow sub-shape maps.

use cxx::UniquePtr;
use ferrocad_geom::Shape;
use opencascade_sys as ffi;

use ffi::top_abs::TopAbs_ShapeEnum;
use ffi::top_tools::TopTools_IndexedMapOfShape;
use ffi::topo_ds::TopoDS_Shape;

use crate::error::OcctError;

/// The backend-specific value the OCCT backend erases into a
/// [`ferrocad_geom::Shape`]. Owning the `TopoDS_Shape` as a `UniquePtr` copies the
/// handle (OCCT shapes are reference-counted), so the shape outlives the operation
/// object that produced it.
pub struct OcctShape(pub(crate) UniquePtr<TopoDS_Shape>);

impl OcctShape {
    /// Take ownership of a copy of `shape`.
    pub fn from_ref(shape: &TopoDS_Shape) -> Self {
        OcctShape(ffi::topo_ds::TopoDS_Shape_to_owned(shape))
    }

    /// The wrapped shape.
    ///
    /// No lock: the handle only needs to be `Send`, and every call already runs under
    /// the owning document's `Mutex`.
    pub fn borrow(&self) -> &UniquePtr<TopoDS_Shape> {
        &self.0
    }
}

impl Clone for OcctShape {
    /// Copy the OCCT handle (reference-counted), so the copy outlives this value.
    fn clone(&self) -> Self {
        OcctShape(ffi::topo_ds::TopoDS_Shape_to_owned(&self.0))
    }
}

impl std::fmt::Debug for OcctShape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OcctShape")
    }
}

/// Recover the OCCT shape behind a [`ferrocad_geom::Shape`], or [`OcctError::WrongBackend`]
/// if it was produced by a different backend.
pub(crate) fn downcast(shape: &Shape) -> Result<&OcctShape, OcctError> {
    shape
        .downcast_ref::<OcctShape>()
        .ok_or(OcctError::WrongBackend)
}

/// Wrap `shape` as an owned [`ferrocad_geom::Shape`].
pub(crate) fn erase(shape: &TopoDS_Shape) -> Shape {
    Shape::new(OcctShape::from_ref(shape))
}

/// A fresh, empty `TopTools_IndexedMapOfShape`.
pub(crate) fn new_map() -> UniquePtr<TopTools_IndexedMapOfShape> {
    ffi::top_tools::new_indexed_map_of_shape()
}

/// Map the sub-shapes of `shape` of `kind` (1-based, OCCT order).
pub(crate) fn sub_shape_map(shape: &TopoDS_Shape, kind: TopAbs_ShapeEnum) -> UniquePtr<TopTools_IndexedMapOfShape> {
    let mut map = new_map();
    ffi::top_exp::TopExp::MapShapes(shape, kind, map.pin_mut());
    map
}

/// Serialise a shape to BREP bytes (in memory; the crate only bridges file I/O).
pub fn write_brep(shape: &Shape) -> Option<Vec<u8>> {
    let occt = downcast(shape).ok()?;
    let guard = occt.borrow();
    let inner = guard.as_ref()?;
    let mut bytes = Vec::new();
    crate::bridge::fc_brep_write(inner, &mut bytes);
    Some(bytes)
}

/// Read a shape from BREP bytes, or `None` if they are not valid BREP.
pub fn read_brep(bytes: &[u8]) -> Option<Shape> {
    let shape = crate::bridge::fc_brep_read(bytes);
    if shape.is_null() {
        return None;
    }
    Some(Shape::new(OcctShape(shape)))
}
