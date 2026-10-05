//! # ferrocad
//!
//! The base FerroCAD application. The binary (`cargo install ferrocad`) boots an
//! embedded CPython interpreter and opens the `bite-gpui` shell; the engine is
//! [`ferrocad_core`](https://crates.io/crates/ferrocad_core) and the shell library
//! is [`ferrocad_gpui`](https://crates.io/crates/ferrocad_gpui).
//!
//! ## The payload
//!
//! The app is useless without its Python facade and workbenches. A development
//! checkout has them in the workspace; `cargo install` does not, so the payload
//! is embedded into the binary and unpacked on first run. See [`payload`] for the
//! resolution order and the packaging story in `docs/distribution.md`.

pub mod payload;
