//! Render one FerroCAD documentation snapshot: `ferrocad_snapshot <spec> <out.png>`.
//!
//! The spec is the JSON a documentation workflow records for an `image()` op (see
//! [`ferrocad_wgpu::snapshot`]). This binary is the small, supported bridge between
//! the Python tools and the offscreen renderer: `tools/userdocs.py --capture` and
//! `tools/screenshot.py` both invoke it, and neither links wgpu itself.
//!
//! Exit codes let the caller tell the failure kinds apart:
//!
//! * `0` — the PNG was written (or the spec validated with `--validate`);
//! * `2` — a usage, spec, or I/O error (the caller reports it and stops);
//! * `3` — this machine has no usable GPU adapter (the caller leaves the committed
//!   images alone).
//!
//! Usage:
//!
//! ```text
//! ferrocad_snapshot <spec.json> <out.png>
//! ferrocad_snapshot --validate <spec.json>
//! ```
//!
//! `-` reads the spec from stdin or writes the PNG to stdout.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ferrocad_wgpu::snapshot::{SnapshotSpec, encode_png, render};

const USAGE: &str = "usage: ferrocad_snapshot <spec.json> <out.png>\n       ferrocad_snapshot --validate <spec.json>\n\n'-' reads the spec from stdin or writes the PNG to stdout.";

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let first = match args.next() {
        Some(value) => value.to_string_lossy().into_owned(),
        None => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    if first == "-h" || first == "--help" {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    let (validate, spec_arg, out_arg) = if first == "--validate" {
        match (args.next(), args.next()) {
            (Some(spec), None) => (true, spec.to_string_lossy().into_owned(), None),
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    } else {
        match (args.next(), args.next(), args.next()) {
            (Some(out), None, None) => (false, first, Some(PathBuf::from(out))),
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    };

    let text = match read_spec(Path::new(&spec_arg)) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("ferrocad_snapshot: {error}");
            return ExitCode::from(2);
        }
    };
    let spec: SnapshotSpec = match serde_json::from_str(&text) {
        Ok(spec) => spec,
        Err(error) => {
            eprintln!(
                "ferrocad_snapshot: {} is not a snapshot spec: {error}",
                display_path(Path::new(&spec_arg))
            );
            return ExitCode::from(2);
        }
    };

    if validate {
        match spec.validate() {
            Ok(()) => {
                println!(
                    "ok: {} shape(s), {}×{} px",
                    spec.shapes.len(),
                    spec.size[0],
                    spec.size[1]
                );
                return ExitCode::SUCCESS;
            }
            Err(error) => {
                eprintln!("ferrocad_snapshot: {error}");
                return ExitCode::from(2);
            }
        }
    }

    let out = out_arg.expect("a render always has an output path");
    let image = match render(&spec) {
        Ok(image) => image,
        Err(error) if error.is_adapter_missing() => {
            eprintln!("ferrocad_snapshot: no GPU adapter available: {error}");
            return ExitCode::from(3);
        }
        Err(error) => {
            eprintln!("ferrocad_snapshot: {error}");
            return ExitCode::from(2);
        }
    };
    let png = match encode_png(&image) {
        Ok(png) => png,
        Err(error) => {
            eprintln!("ferrocad_snapshot: {error}");
            return ExitCode::from(2);
        }
    };
    if let Err(error) = write_png(&out, &png) {
        eprintln!("ferrocad_snapshot: {error}");
        return ExitCode::from(2);
    }
    if out != Path::new("-") {
        println!(
            "wrote {} ({}×{}, {} bytes)",
            out.display(),
            image.width,
            image.height,
            png.len()
        );
    }
    ExitCode::SUCCESS
}

fn read_spec(path: &Path) -> Result<String, String> {
    if path == Path::new("-") {
        let mut text = String::new();
        io::stdin()
            .read_to_string(&mut text)
            .map_err(|error| format!("cannot read the spec from stdin: {error}"))?;
        return Ok(text);
    }
    fs::read_to_string(path).map_err(|error| format!("cannot read {}: {error}", path.display()))
}

fn write_png(path: &Path, png: &[u8]) -> Result<(), String> {
    if path == Path::new("-") {
        let mut stdout = io::stdout().lock();
        stdout
            .write_all(png)
            .and_then(|()| stdout.flush())
            .map_err(|error| format!("cannot write the PNG to stdout: {error}"))?;
        return Ok(());
    }
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("{} has no file name", path.display()))?;
    let temporary = path.with_file_name(format!(".{}.tmp", file_name.to_string_lossy()));
    fs::write(&temporary, png)
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("cannot replace {}: {error}", path.display()));
    }
    Ok(())
}

fn display_path(path: &Path) -> String {
    if path == Path::new("-") {
        "stdin".to_string()
    } else {
        path.display().to_string()
    }
}
