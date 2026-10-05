//! # FerroCAD OCCT backend (`ferrocad_occt`)
//!
//! The real geometry kernel behind the [`ferrocad_geom`] seam. It implements
//! [`GeometryBackend`](ferrocad_geom::GeometryBackend) with OCCT
//! via [`opencascade_sys`](https://crates.io/crates/opencascade-sys), plus a
//! **sibling cxx bridge** (`include/fc_history.hxx`) that adds the shape-history
//! calls the crate does not bridge — no fork, no patch.
//!
//! The seam means nothing below the application depends on this crate: the document
//! core talks to a `dyn GeometryBackend`, and the application installs
//! [`OcctBackend`] at startup. See `docs/occt-integration.md`.
//!
//! # Building
//!
//! This crate needs OCCT 7.8+ and is **not** in the workspace's `default-members`,
//! so an ordinary `cargo build`/`cargo test` does not require a kernel. To build it,
//! point two environment variables at an OCCT prefix (see `docs/occt-bundling.md`):
//!
//! ```sh
//! export OCCT_INCLUDE_DIR=<prefix>/include/opencascade   # for build.rs
//! export OpenCASCADE_DIR=<prefix>/lib/cmake/opencascade  # for opencascade-sys
//! export LD_LIBRARY_PATH=<prefix>/lib:$LD_LIBRARY_PATH   # for running tests
//! cargo test -p ferrocad_occt
//! ```
//!
//! # Example
//!
//! ```
//! use ferrocad_geom::GeometryBackend;
//! use ferrocad_occt::OcctBackend;
//!
//! let backend = OcctBackend::new();
//! let a = backend.make_box(10.0, 10.0, 10.0).unwrap();
//! let b = backend.make_box(10.0, 10.0, 10.0).unwrap();
//! let fused = backend.fuse(&a, &b).unwrap();
//! assert!(backend.resolve(&fused.shape, "Face1").is_some());
//! ```

mod backend;
mod bridge;
mod error;
mod shape;

pub use backend::OcctBackend;
pub use error::OcctError;
pub use shape::OcctShape;

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_geom::GeometryBackend;

    #[test]
    fn make_box_has_six_faces() {
        let backend = OcctBackend::new();
        let cube = backend.make_box(10.0, 10.0, 10.0).unwrap();
        assert!(backend.resolve(&cube, "Face1").is_some());
        assert!(backend.resolve(&cube, "Face6").is_some());
        assert!(backend.resolve(&cube, "Face7").is_none());
    }

    #[test]
    fn fuse_reports_face_lineage() {
        let backend = OcctBackend::new();
        let a = backend.make_box(10.0, 10.0, 10.0).unwrap();
        let b = backend.make_box(10.0, 10.0, 10.0).unwrap();
        let out = backend.fuse(&a, &b).unwrap();
        // Two coincident cubes: every input face survives or is merged, so the
        // history is non-empty and no face is deleted.
        assert!(!out.history.is_empty());
        assert!(out.history.deleted.is_empty());
    }

    #[test]
    fn cut_removes_material() {
        let backend = OcctBackend::new();
        let a = backend.make_box(10.0, 10.0, 10.0).unwrap();
        let b = backend.make_box(10.0, 10.0, 10.0).unwrap();
        let out = backend.cut(&a, &b).unwrap();
        assert!(!out.history.is_empty());
    }

    #[test]
    fn place_and_resolve() {
        use ferrocad_types::{Placement, Rotation, Vector3};
        let backend = OcctBackend::new();
        let cube = backend.make_box(1.0, 1.0, 1.0).unwrap();
        let moved = backend
            .place(
                &cube,
                &Placement {
                    base: Vector3::new(5.0, 0.0, 0.0),
                    rotation: Rotation::identity(),
                },
            )
            .unwrap();
        assert!(backend.resolve(&moved, "Face1").is_some());
        assert!(!cube.is_same(&moved));
    }

    #[test]
    fn rejects_a_foreign_shape() {
        use ferrocad_geom::NullBackend;
        let occt = OcctBackend::new();
        let foreign = NullBackend.make_box(1.0, 1.0, 1.0).unwrap();
        assert_eq!(occt.resolve(&foreign, "Face1"), None);
    }
}
