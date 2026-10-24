//! Compile the sibling cxx bridge over `opencascade-sys`'s types.
//!
//! The bridge reuses the crate's opaque OCCT types
//! (`type TopoDS_Shape = opencascade_sys::...`) so we add the missing history /
//! element-map calls without patching or forking `opencascade-sys`.
//!
//! Needs `OCCT_INCLUDE_DIR` (the OCCT `include/opencascade` directory) on the
//! include path. Set it to a fetched OCCT prefix; see docs/architecture/occt-bundling.md.

fn main() {
    println!("cargo:rerun-if-env-changed=OCCT_INCLUDE_DIR");
    println!("cargo:rerun-if-changed=include/fc_history.hxx");
    println!("cargo:rerun-if-changed=include/fc_topology.hxx");
    println!("cargo:rerun-if-changed=include/fc_curve.hxx");
    println!("cargo:rerun-if-changed=include/fc_shapefix.hxx");
    println!("cargo:rerun-if-changed=include/fc_chfi2d.hxx");
    println!("cargo:rerun-if-changed=src/bridge.rs");
    println!("cargo:rerun-if-changed=src/shapefix.rs");
    println!("cargo:rerun-if-changed=src/chfi2d.rs");

    let occt_include = std::env::var("OCCT_INCLUDE_DIR").expect(
        "set OCCT_INCLUDE_DIR to the OCCT 'include/opencascade' directory \
         (e.g. <prefix>/include/opencascade)",
    );

    cxx_build::bridges(["src/bridge.rs", "src/shapefix.rs", "src/chfi2d.rs"])
        .include(&occt_include)
        .std("c++14")
        .compile("ferrocad-occt-bridge");
}
