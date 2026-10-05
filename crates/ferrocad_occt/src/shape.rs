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

    /// Borrow the underlying OCCT shape.
    pub fn as_shape(&self) -> &TopoDS_Shape {
        match self.0.as_ref() {
            Some(shape) => shape,
            None => panic!("OCCT shape is null"),
        }
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
