fn main() {
    println!("cargo:rerun-if-env-changed=OCCT_INCLUDE_DIR");
    println!("cargo:rerun-if-changed=include/fc_history.hxx");
    println!("cargo:rerun-if-changed=src/lib.rs");

    let occt_include = std::env::var("OCCT_INCLUDE_DIR").expect(
        "set OCCT_INCLUDE_DIR to the OCCT 'include/opencascade' directory \
         (e.g. <prefix>/include/opencascade)",
    );

    cxx_build::bridge("src/lib.rs")
        .include(&occt_include)
        .std("c++14")
        .compile("occt-history-bridge");
}
