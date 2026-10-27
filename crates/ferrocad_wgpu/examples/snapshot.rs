//! Render one documentation snapshot: `snapshot <spec.json> <out.png>`.
//!
//! The spec is the JSON `tools/workflows/_capture.py` records for an `image()` op
//! (see [`ferrocad_wgpu::snapshot`]). The documentation runner starts this binary
//! in its own process because the workflow itself is a kernel-free Python
//! subprocess with no Rust rendering available.
//!
//! Exit codes let the runner tell the two failure kinds apart:
//!
//! * `0` — the PNG was written;
//! * `2` — a usage, spec, or I/O error (the runner reports it and stops);
//! * `3` — this machine has no usable GPU adapter (the runner reports it and
//!   leaves the committed images alone).
//!
//! Usage: `cargo run --offline -p ferrocad_wgpu --example snapshot -- spec.json out.png`

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use ferrocad_wgpu::snapshot::{SnapshotSpec, encode_png, render};

const USAGE: &str = "usage: snapshot <spec.json> <out.png>";

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let (spec_path, out_path) = match (args.next(), args.next(), args.next()) {
        (Some(spec), Some(out), None) => (PathBuf::from(spec), PathBuf::from(out)),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    let text = match fs::read_to_string(&spec_path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("snapshot: cannot read {}: {error}", spec_path.display());
            return ExitCode::from(2);
        }
    };
    let spec: SnapshotSpec = match serde_json::from_str(&text) {
        Ok(spec) => spec,
        Err(error) => {
            eprintln!(
                "snapshot: {} is not a snapshot spec: {error}",
                spec_path.display()
            );
            return ExitCode::from(2);
        }
    };

    let image = match render(&spec) {
        Ok(image) => image,
        Err(error) if error.is_adapter_missing() => {
            eprintln!("snapshot: no GPU adapter available: {error}");
            return ExitCode::from(3);
        }
        Err(error) => {
            eprintln!("snapshot: {error}");
            return ExitCode::from(2);
        }
    };
    let png = match encode_png(&image) {
        Ok(png) => png,
        Err(error) => {
            eprintln!("snapshot: {error}");
            return ExitCode::from(2);
        }
    };
    if let Err(error) = fs::write(&out_path, &png) {
        eprintln!("snapshot: cannot write {}: {error}", out_path.display());
        return ExitCode::from(2);
    }
    println!(
        "wrote {} ({}×{}, {} bytes)",
        out_path.display(),
        image.width,
        image.height,
        png.len()
    );
    ExitCode::SUCCESS
}
