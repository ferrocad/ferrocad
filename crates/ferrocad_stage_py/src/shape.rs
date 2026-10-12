//! `ShapeItem`: a tessellated CAD shape with its display attributes.
//!
//! Geometry is ingested with a **buffer fast path**: an object exposing a contiguous
//! `float32`/`uint32` buffer (a NumPy array, `array.array`, `memoryview`) is read as one
//! C-level copy through `memoryview.tobytes()` rather than element by element; anything
//! else falls back to iterating Python sequences, so a plain list of `(x, y, z)` tuples
//! works too. (The stable ABI below 3.11 has no `PyBuffer`, so this uses `memoryview`.)

use ferrocad_stage as fs;
use ferrocad_types::Matrix4;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::enums::DrawMode;

/// A declarative visual item (a `ViewProvider`'s shape).
#[pyclass(module = "ferrocad_stage", name = "ShapeItem")]
#[derive(Clone)]
pub struct ShapeItem {
    pub(crate) inner: fs::ShapeItem,
}

impl ShapeItem {
    /// Wrap a native item.
    pub(crate) fn wrap(inner: fs::ShapeItem) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl ShapeItem {
    /// A new, empty shape carrying the pick id `entity_id`.
    #[new]
    #[pyo3(signature = (entity_id))]
    fn new(entity_id: u32) -> Self {
        Self {
            inner: fs::ShapeItem::new(entity_id),
        }
    }

    /// Replace the tessellation.
    ///
    /// `positions` is `N` vertices; `normals` is either `N` per-vertex normals or empty
    /// (flat-shaded); `triangles` and `edges` are flat index arrays. A contiguous
    /// `f32`/`u32` buffer is ingested zero-copy.
    #[pyo3(signature = (positions, normals=None, triangles=None, edges=None))]
    fn set_shape(
        &mut self,
        py: Python<'_>,
        positions: &Bound<'_, PyAny>,
        normals: Option<&Bound<'_, PyAny>>,
        triangles: Option<&Bound<'_, PyAny>>,
        edges: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let positions = ingest_vec3(py, positions, "positions")?;
        let normals = opt_vec3(py, normals, "normals")?;
        let triangles = opt_u32(py, triangles, "triangles")?;
        let edges = opt_u32(py, edges, "edges")?;
        self.inner.set_shape(positions, normals, triangles, edges);
        Ok(())
    }

    /// The pick id.
    #[getter]
    fn entity_id(&self) -> u32 {
        self.inner.entity_id()
    }

    /// The display mode.
    #[getter]
    fn draw_mode(&self) -> DrawMode {
        self.inner.draw_mode().into()
    }

    #[setter]
    fn set_draw_mode(&mut self, mode: DrawMode) {
        self.inner = self.inner.clone().with_draw_mode(mode.into());
    }

    /// The surface colour (RGBA).
    #[getter]
    fn surface_color(&self) -> (f32, f32, f32, f32) {
        quad(self.inner.surface_color())
    }

    #[setter]
    fn set_surface_color(&mut self, color: (f32, f32, f32, f32)) -> PyResult<()> {
        self.inner.set_surface_color([color.0, color.1, color.2, color.3]);
        Ok(())
    }

    /// The edge colour (RGBA).
    #[getter]
    fn edge_color(&self) -> (f32, f32, f32, f32) {
        quad(self.inner.edge_color())
    }

    #[setter]
    fn set_edge_color(&mut self, color: (f32, f32, f32, f32)) -> PyResult<()> {
        self.inner.set_edge_color([color.0, color.1, color.2, color.3]);
        Ok(())
    }

    /// The edge width (stored, not yet consumed by the renderer).
    #[getter]
    fn edge_width(&self) -> f32 {
        self.inner.edge_width()
    }

    #[setter]
    fn set_edge_width(&mut self, width: f32) {
        self.inner = self.inner.clone().with_edge_width(width);
    }

    /// Whether the shape participates in picking.
    #[getter]
    fn selectable(&self) -> bool {
        self.inner.selectable()
    }

    #[setter]
    fn set_selectable(&mut self, selectable: bool) {
        self.inner = self.inner.clone().with_selectable(selectable);
    }

    /// The temporary preview transform (16 row-major floats), if any.
    #[getter]
    fn temporary_transform(&self) -> Option<Vec<f64>> {
        self.inner.temporary_transform().map(|m| m.values().to_vec())
    }

    #[setter]
    fn set_temporary_transform(&mut self, transform: Option<Vec<f64>>) -> PyResult<()> {
        let transform = transform.map(|values| matrix(&values, "temporary_transform")).transpose()?;
        self.inner.set_temporary_transform(transform);
        Ok(())
    }

    /// The vertex positions (copies).
    #[getter]
    fn positions(&self) -> Vec<(f32, f32, f32)> {
        self.inner.positions().iter().map(|p| triple(*p)).collect()
    }

    /// The per-vertex normals (empty when flat-shaded).
    #[getter]
    fn normals(&self) -> Vec<(f32, f32, f32)> {
        self.inner.normals().iter().map(|p| triple(*p)).collect()
    }

    /// The flat triangle indices, three per triangle.
    #[getter]
    fn triangles(&self) -> Vec<u32> {
        self.inner.triangles().to_vec()
    }

    /// The flat edge segment indices, two per segment.
    #[getter]
    fn edges(&self) -> Vec<u32> {
        self.inner.edges().to_vec()
    }

    fn __repr__(&self) -> String {
        format!(
            "ShapeItem(entity_id={}, draw_mode={:?})",
            self.inner.entity_id(),
            self.inner.draw_mode()
        )
    }
}

/// Register this module's classes.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<ShapeItem>()?;
    Ok(())
}

/// Ingest `N` 3-vectors: a contiguous `float32` buffer (a fast copy) or a sequence of
/// triples.
fn ingest_vec3(py: Python<'_>, obj: &Bound<'_, PyAny>, what: &str) -> PyResult<Vec<[f32; 3]>> {
    if let Some(values) = f32_buffer(py, obj)? {
        if values.len() % 3 != 0 {
            return Err(PyValueError::new_err(format!(
                "{what} needs a multiple of 3 floats, got {}",
                values.len()
            )));
        }
        return Ok(values.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect());
    }
    let mut out = Vec::new();
    for item in obj.try_iter()? {
        let values: Vec<f32> = item?.extract()?;
        if values.len() != 3 {
            return Err(PyValueError::new_err(format!(
                "each {what} entry needs 3 floats, got {}",
                values.len()
            )));
        }
        out.push([values[0], values[1], values[2]]);
    }
    Ok(out)
}

/// Ingest a flat `u32` index array (a contiguous buffer, or any int sequence).
fn ingest_u32(py: Python<'_>, obj: &Bound<'_, PyAny>, what: &str) -> PyResult<Vec<u32>> {
    if let Some(values) = u32_buffer(py, obj)? {
        return Ok(values);
    }
    let mut out = Vec::new();
    for item in obj.try_iter()? {
        let value = item?.extract::<u32>().map_err(|_| {
            PyValueError::new_err(format!("{what} must contain non-negative integers"))
        })?;
        out.push(value);
    }
    Ok(out)
}

/// Read a contiguous `float32` buffer (`memoryview.format == "f"`) into a `Vec<f32>`.
///
/// `None` when `obj` is not a buffer or is not 4-byte floats (the caller then iterates).
fn f32_buffer(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<Option<Vec<f32>>> {
    let Some((format, itemsize, bytes)) = buffer_bytes(py, obj)? else {
        return Ok(None);
    };
    if format != "f" || itemsize != 4 {
        return Ok(None);
    }
    Ok(Some(decode(&bytes, f32::from_ne_bytes)))
}

/// Read a contiguous `uint32` buffer (`memoryview.format == "I"/"L"`) into a `Vec<u32>`.
fn u32_buffer(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<Option<Vec<u32>>> {
    let Some((format, itemsize, bytes)) = buffer_bytes(py, obj)? else {
        return Ok(None);
    };
    if !matches!(format.as_str(), "I" | "L") || itemsize != 4 {
        return Ok(None);
    }
    Ok(Some(decode(&bytes, u32::from_ne_bytes)))
}

/// A `(format, itemsize, bytes)` view of `obj`'s buffer, or `None` when it has none.
fn buffer_bytes(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<Option<(String, usize, Vec<u8>)>> {
    let view = match py.import("builtins")?.getattr("memoryview")?.call1((obj,)) {
        Ok(view) => view,
        Err(_) => return Ok(None), // not a buffer-protocol object
    };
    let format: String = view.getattr("format")?.extract()?;
    let itemsize: usize = view.getattr("itemsize")?.extract()?;
    let bytes: Vec<u8> = view.call_method0("tobytes")?.extract()?;
    Ok(Some((format, itemsize, bytes)))
}

/// Decode a byte buffer as `T`, four bytes (native order) at a time.
fn decode<T>(bytes: &[u8], from: fn([u8; 4]) -> T) -> Vec<T> {
    bytes
        .chunks_exact(4)
        .map(|c| from([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn opt_vec3(
    py: Python<'_>,
    obj: Option<&Bound<'_, PyAny>>,
    what: &str,
) -> PyResult<Vec<[f32; 3]>> {
    match obj {
        Some(obj) if !obj.is_none() => ingest_vec3(py, obj, what),
        _ => Ok(Vec::new()),
    }
}

fn opt_u32(py: Python<'_>, obj: Option<&Bound<'_, PyAny>>, what: &str) -> PyResult<Vec<u32>> {
    match obj {
        Some(obj) if !obj.is_none() => ingest_u32(py, obj, what),
        _ => Ok(Vec::new()),
    }
}

/// A row-major 4×4 matrix from 16 floats.
fn matrix(values: &[f64], what: &str) -> PyResult<Matrix4> {
    let values: [f64; 16] = values.try_into().map_err(|_| {
        PyValueError::new_err(format!("{what} must be 16 floats, got {}", values.len()))
    })?;
    Ok(Matrix4::from_values(values))
}

fn quad(color: [f32; 4]) -> (f32, f32, f32, f32) {
    (color[0], color[1], color[2], color[3])
}

fn triple(p: [f32; 3]) -> (f32, f32, f32) {
    (p[0], p[1], p[2])
}
