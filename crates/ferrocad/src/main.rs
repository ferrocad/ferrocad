//! The base FerroCAD application (the general edition).
//!
//! It is the smallest possible edition: it calls the shared host with the default
//! configuration. A redistribution such as *FerroCAD: Architecture* is the same
//! shape with a different `HostConfig` (see `docs/repackaging.md`).
//!
//! Run it with `cargo run -p ferrocad` (needs a display).

fn main() {
    ferrocad_host::run();
}
