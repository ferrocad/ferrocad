//! Boot the Rust document backend natively and print what it holds.
//!
//! ```sh
//! cargo run -p ferrocad_shell_rust --example headless
//! ```
//!
//! This is the desktop half of the backend's proof: no window, no interpreter, and
//! the same `AppBackend` the browser host installs. It needs OCCT; the workspace's
//! `.cargo/config.toml` points at `.occt/prefix` (see
//! `docs/developer/architecture/occt-bundling.md`).

use ferrocad_shell_api::app::AppBackend;
use ferrocad_shell_rust::RustAppBackend;

fn main() -> Result<(), String> {
    let mut backend = RustAppBackend::new();
    let boot = backend.bootstrap()?;
    println!("boot: {} · backend {}", boot.version, boot.backend);

    for document in backend.model_tree()? {
        println!(
            "document {} ({} object(s))",
            document.name,
            document.objects.len()
        );
        for object in &document.objects {
            println!("  {} · {}", object.name, object.type_id);
        }
    }

    for mesh in backend.stage_meshes()? {
        println!(
            "stage: {} · {} triangles, {} edges",
            mesh.object,
            mesh.triangles.len() / 3,
            mesh.edges.len() / 2
        );
    }
    Ok(())
}
