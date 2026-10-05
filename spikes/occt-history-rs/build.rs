// Compiles the OCCT shim and links the OCCT toolkits it needs.
//
// Override the defaults if OCCT lives elsewhere:
//   OCCT_INCLUDE_DIR=/path/to/opencascade/include
//   OCCT_LIB_DIR=/path/to/opencascade/lib
use std::env;

fn main() {
    let include = env::var("OCCT_INCLUDE_DIR")
        .unwrap_or_else(|_| "/usr/include/opencascade".to_string());
    let lib_dir =
        env::var("OCCT_LIB_DIR").unwrap_or_else(|_| "/usr/lib/x86_64-linux-gnu".to_string());

    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .include("shim")
        .include(&include)
        .file("shim/occt_shim.cpp")
        .compile("occt_shim");

    println!("cargo:rerun-if-changed=shim/occt_shim.cpp");
    println!("cargo:rerun-if-changed=shim/occt_shim.h");
    println!("cargo:rustc-link-search=native={lib_dir}");

    // Toolkits touched by the shim. Adjust for your OCCT version/install.
    for lib in [
        "TKernel",
        "TKMath",
        "TKG2d",
        "TKG3d",
        "TKGeomBase",
        "TKBRep",
        "TKTopAlgo",
        "TKPrim",
        "TKBO",
        "TKFillet",
        "TKShHealing",
    ] {
        println!("cargo:rustc-link-lib=dylib={lib}");
    }
}
