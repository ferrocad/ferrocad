//! Temporary measurement: the Rust backend on a big `.FCStd`.
//!
//! ```sh
//! cargo run -p ferrocad_shell_rust --example timing -- /path/to/model.FCStd
//! ```

use ferrocad_shell_api::app::AppBackend;
use ferrocad_shell_rust::RustAppBackend;
use ferrocad_stage::{ShapeItem, Stage};

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
        // Where each object ended up, so the Python backend can be compared against it.
        for mesh in &meshes {
            let mut min = [f32::INFINITY; 3];
            let mut max = [f32::NEG_INFINITY; 3];
            for point in &mesh.positions {
                for axis in 0..3 {
                    min[axis] = min[axis].min(point[axis]);
                    max[axis] = max[axis].max(point[axis]);
                }
            }
            eprintln!(
                "rust: bbox {} [{:.1}, {:.1}, {:.1}] .. [{:.1}, {:.1}, {:.1}] {} tri",
                mesh.object,
                min[0],
                min[1],
                min[2],
                max[0],
                max[1],
                max[2],
                mesh.triangles.len() / 3
            );
        }
    }
    // What a camera move used to cost: `recompile` re-emitted the whole stage, and
    // `ShapeItem::emit` allocates a `Vertex` per position.
    let mut stage = Stage::new();
    for mesh in backend.stage_meshes()? {
        stage.add_item(ShapeItem::from_tessellation_owned(
            mesh.entity,
            mesh.into_tessellation(),
        ));
    }
    for index in 0..3 {
        let started = std::time::Instant::now();
        let queue = stage.to_queue();
        eprintln!(
            "rust: stage recompile {index} {:?} ({} packets)",
            started.elapsed(),
            queue.len()
        );
    }
    Ok(())
}
