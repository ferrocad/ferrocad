//! Thin binary for the host crate, kept so `cargo run -p ferrocad_host` works for
//! development. The application entry point is [`ferrocad_host::run`]; the base
//! app (`cargo run -p ferrocad`) and the editions call the same library.

fn main() {
    ferrocad_host::run();
}
