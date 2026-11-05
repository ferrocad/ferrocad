//! Temporary measurement: the Rust backend on a big `.FCStd`.
//!
//! ```sh
//! cargo run -p ferrocad_shell_rust --example timing -- /path/to/model.FCStd
//! ```

use ferrocad_shell_api::app::AppBackend;
use ferrocad_shell_rust::RustAppBackend;

fn main() -> Result<(), String> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| "usage: timing <model.FCStd>".to_string())?;
    let mut backend = RustAppBackend::new();
    backend.bootstrap()?;

    let started = std::time::Instant::now();
    let opened = backend.open_path(std::path::Path::new(&path))?;
    eprintln!(
        "rust: open {:?} ({} objects)",
        started.elapsed(),
        opened.objects
    );

    for index in 0..3 {
        let started = std::time::Instant::now();
        let meshes = backend.stage_meshes()?;
        let triangles: usize = meshes.iter().map(|mesh| mesh.triangles.len() / 3).sum();
        eprintln!(
            "rust: stage_meshes {index} {:?} ({} items, {triangles} triangles)",
            started.elapsed(),
            meshes.len()
        );
    }
    Ok(())
}
