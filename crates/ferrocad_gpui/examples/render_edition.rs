//! Render an edition's sample document headlessly to a PNG.
//!
//! The editions themselves open a window, but the model path and the renderer are
//! separable: boot the app script **without** a display, ask it for the tessellated
//! document shapes (`stage_meshes`), compile the stage, and draw one offscreen frame.
//! That is the same three-call path the window's viewport uses (`build_stage` →
//! `Stage::to_queue` → `WgpuRenderer`), minus gpui. The editions expose it as
//! `--render`; this example is the library-level form.
//!
//! ```sh
//! # the kernel-free toy edition, which needs no OCCT:
//! VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
//!   cargo run -p ferrocad_gpui --example render_edition -- \
//!   --entry ferrocad_toy --out target/demo/toy.png
//! ```
//!
//! Add `--script model.py` to build the model after the sample document is
//! created, `--open model.FCStd` to load a saved document instead, and `--stl
//! model.stl` to export the same tessellation as binary STL. The entry module
//! must be on `sys.path` (the workspace `python/` directory, found automatically
//! in a development checkout). `ferrocad_parametric` additionally needs `Part`
//! registered in the **same image** as the core bindings, which only the edition
//! binary does; use `cargo run -p ferrocad_parametric -- --render …` for a Part
//! model.

use ferrocad_gpui::HostConfig;
use ferrocad_gpui::headless;

const DEFAULT_SIZE: [u32; 2] = [800, 600];

fn main() {
    let mut entry = "ferrocad_toy".to_string();
    let mut out = "target/demo/model.png".to_string();
    let mut script = None;
    let mut open = None;
    let mut stl = None;
    let mut size = DEFAULT_SIZE;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--entry" => entry = args.next().unwrap_or(entry),
            "--out" => out = args.next().unwrap_or(out),
            "--script" => script = args.next().map(std::path::PathBuf::from),
            "--open" => open = args.next().map(std::path::PathBuf::from),
            "--stl" => stl = args.next().map(std::path::PathBuf::from),
            "--size" => {
                if let Some(value) = args.next() {
                    size = parse_size(&value).unwrap_or_else(|error| {
                        eprintln!("ferrocad: {error}");
                        std::process::exit(2);
                    });
                }
            }
            other => {
                eprintln!("ferrocad: unknown argument {other:?}");
                std::process::exit(2);
            }
        }
    }

    let config = HostConfig {
        entry_module: entry,
        open_document: open,
        ..HostConfig::default()
    };
    match headless::run(
        &config,
        Some(std::path::Path::new(&out)),
        stl.as_deref(),
        size,
        script.as_deref(),
    ) {
        Ok(report) => {
            println!(
                "ferrocad: {} object(s) [{}] from {}",
                report.objects.len(),
                report.objects.join(", "),
                report.document
            );
            if let Some(size) = report.size {
                println!("ferrocad: wrote PNG {out} ({}x{})", size[0], size[1]);
            }
            if let Some(stl) = &report.stl {
                println!(
                    "ferrocad: wrote STL {} ({} triangles)",
                    stl.path.display(),
                    stl.triangles
                );
            }
        }
        Err(error) => {
            eprintln!("ferrocad: headless run failed: {error}");
            std::process::exit(1);
        }
    }
}

/// `800x600` → `[800, 600]`.
fn parse_size(value: &str) -> Result<[u32; 2], String> {
    let (width, height) = value
        .split_once('x')
        .ok_or_else(|| format!("--size wants WIDTHxHEIGHT, got {value:?}"))?;
    let parse = |part: &str| {
        part.parse::<u32>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| format!("--size wants positive integers, got {value:?}"))
    };
    Ok([parse(width)?, parse(height)?])
}
