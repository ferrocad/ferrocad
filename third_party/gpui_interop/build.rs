#![allow(clippy::disallowed_methods, reason = "build scripts are exempt")]

// The Direct3D window the test opens lends `WindowsWindow`, whose prompt imports
// `TaskDialogIndirect`. Only the common-controls **v6** assembly exports that symbol, and a binary
// reaches it through its manifest: without one the loader binds System32's 5.82 `comctl32.dll`, the
// import does not resolve, and the process dies with `STATUS_ENTRYPOINT_NOT_FOUND` **before `main`**
// -- so the skip the test is built around never runs. `gpui` carries the same manifest, but only for
// its own targets, and that does not reach a downstream test binary.
//
// A resource compiler exists only on a Windows host, so this is where it runs and nowhere else.
#[cfg(windows)]
fn main() {
    let resource_dir = std::path::Path::new("resources/windows");
    let rc_file = resource_dir.join("gpui_interop.rc");
    println!("cargo:rerun-if-changed={}", rc_file.display());
    println!(
        "cargo:rerun-if-changed={}",
        resource_dir.join("gpui_interop.manifest.xml").display()
    );
    embed_resource::compile_for_everything(
        rc_file,
        embed_resource::ParamsIncludeDirs([resource_dir]),
    )
    .manifest_required()
    .unwrap();
}

#[cfg(not(windows))]
fn main() {}
