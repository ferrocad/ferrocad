//! The base FerroCAD application (the general edition).
//!
//! It is the smallest possible edition: it calls the shared shell library with
//! the base app's configuration (its Python entry module, and later its workbench
//! directories). A redistribution such as *FerroCAD: Architecture* is the same
//! shape with different scripts and mods (see `docs/developer/architecture/repackaging.md`).
//!
//! Run it with `cargo run -p ferrocad` (needs a display); `cargo install
//! ferrocad` produces the same binary with its Python payload embedded.
//!
//! The flags are the shared edition ones (`ferrocad::edition_args`): `--open` to
//! open a saved document, `--script-host=` to install another script half,
//! `--addons` to select the mods (every mod loads when it is absent), and
//! `--render`/`--stl` to draw the sample headlessly instead of opening a window.

fn main() {
    // Parse first: `--check-init` and `--render` both need the mod selection
    // (`--addons`) and the scripting configuration, and the windowed path needs
    // them too. Checking the default composition when the command line asked for
    // another one would report a layout that was never going to start.
    let args = match ferrocad::edition_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("ferrocad: {error}");
            std::process::exit(2);
        }
    };

    // `--check-init` boots the interpreter and the payload and exits without a
    // window: it verifies a packaged layout (native dependencies, facade,
    // workbenches) on a machine with no display. `run` is the normal, windowed
    // path. See `docs/developer/architecture/distribution.md`.
    if std::env::args().skip(1).any(|arg| arg == "--check-init") {
        match ferrocad::check_init_args(&args) {
            Ok(()) => println!("ferrocad: init OK"),
            Err(e) => {
                eprintln!("ferrocad: init failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    if args.render.is_some() || args.stl.is_some() {
        match ferrocad::render_edition_headless("FerroCAD", "ferrocad_shell", &args) {
            Ok(report) => {
                println!(
                    "ferrocad: {} object(s) [{}] from {}",
                    report.objects.len(),
                    report.objects.join(", "),
                    report.document
                );
                if let (Some(out), Some(size)) = (args.render.as_ref(), report.size) {
                    println!(
                        "ferrocad: wrote PNG {} ({}x{})",
                        out.display(),
                        size[0],
                        size[1]
                    );
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
        return;
    }

    ferrocad::run_edition_with_script(
        "FerroCAD",
        "ferrocad_shell",
        ferrocad::base_contributions(),
        args.open,
        args.script_hosts,
        args.addons,
    );
}
