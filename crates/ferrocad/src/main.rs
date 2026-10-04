//! The base FerroCAD application (the general edition).
//!
//! It is the smallest possible edition: it calls the shared shell library with
//! the base app's configuration (its Python entry module, and later its workbench
//! directories). A redistribution such as *FerroCAD: Architecture* is the same
//! shape with different scripts and mods (see `docs/repackaging.md`).
//!
//! Run it with `cargo run -p ferrocad` (needs a display).

fn main() {
    let config = ferrocad_gpui::HostConfig {
        app_name: "FerroCAD".to_string(),
        entry_module: "ferrocad_shell".to_string(),
        ..Default::default()
    };
    ferrocad_gpui::run_with(config);
}
