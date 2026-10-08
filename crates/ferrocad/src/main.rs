//! The base FerroCAD application (the general edition).
//!
//! It is the smallest possible edition: it calls the shared shell library with
//! the base app's configuration (its Python entry module, and later its workbench
//! directories). A redistribution such as *FerroCAD: Architecture* is the same
//! shape with different scripts and mods (see `docs/repackaging.md`).
//!
//! Run it with `cargo run -p ferrocad` (needs a display); `cargo install
//! ferrocad` produces the same binary with its Python payload embedded.

fn main() {
    // `--check-init` boots the interpreter and the payload and exits without a
    // window: it verifies a packaged layout (native dependencies, facade,
    // workbenches) on a machine with no display. `run` is the normal, windowed
    // path. See `docs/distribution.md`.
    if std::env::args().skip(1).any(|arg| arg == "--check-init") {
        match ferrocad::check_init() {
            Ok(()) => println!("ferrocad: init OK"),
            Err(e) => {
                eprintln!("ferrocad: init failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
    ferrocad::run();
}
