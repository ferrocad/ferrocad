//! A wasm32 link smoke test for the OCCT Part backend.
//!
//! It exercises the full path a browser app needs: acquire the kernel, build
//! geometry, run a real operation, tessellate. The exports use the plain C ABI
//! so the module can be driven without wasm-bindgen.
//!
//! ```text
//! occt_init()                  // once, before anything else
//! filleted_box_triangles()     // triangles of a filleted 10 mm box, 0 on error
//! ```

use ferrocad_geom::{ElementKind, GeomError, GeometryBackend, Tessellation, TessellationOptions};
use ferrocad_occt::OcctBackend;

unsafe extern "C" {
    /// Emitted by wasm-ld: runs the module's static constructors. OCCT's C++
    /// initialisers must run before any kernel call.
    #[cfg(target_arch = "wasm32")]
    fn __wasm_call_ctors();
}

/// Run the module's constructors. Call once, before any other export.
///
/// On non-wasm targets this is a no-op, so the crate still builds natively.
#[unsafe(no_mangle)]
pub extern "C" fn occt_init() {
    // SAFETY: wasm-ld emits this symbol for every linked module, and calling it
    // more than once is harmless.
    #[cfg(target_arch = "wasm32")]
    unsafe {
        __wasm_call_ctors()
    };
}

/// Build a 10 mm box, fillet one edge, and return the tessellated triangle
/// count. `0` means the operation failed.
#[unsafe(no_mangle)]
pub extern "C" fn filleted_box_triangles() -> u32 {
    match filleted_box() {
        Ok(mesh) => (mesh.triangles.len() / 3) as u32,
        Err(_) => 0,
    }
}

fn filleted_box() -> Result<Tessellation, GeomError> {
    let backend = OcctBackend::new();
    let shape = backend.make_box(10.0, 10.0, 10.0)?;
    let edges = backend.elements(&shape, ElementKind::Edge);
    let edge = edges.first().ok_or("the box has no edges")?;
    let filleted = backend.fillet(&shape, std::slice::from_ref(edge), 1.0)?;
    Ok(backend.tessellate(&filleted.shape, &TessellationOptions::absolute(0.2))?)
}
