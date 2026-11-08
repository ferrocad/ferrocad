//! The browser module's stack size.
//!
//! `wasm-ld`'s default is 1 MiB. The interpreter half (`?runtime=pywasm`) compiles
//! `python/ferrocad_shell/__init__.py` with RustPython's recursive compiler, which
//! needs far more than that — the native test in `ferrocad_shell_pywasm` runs on a
//! 16 MiB thread for exactly the same reason. Without this, the console traps on the
//! first `evaluate` and the module looks broken.
//!
//! A **link argument**, not `RUSTFLAGS`: the cross image supplies its wasi-sdk paths
//! through `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS`, and a plain `RUSTFLAGS`
//! in the environment would replace them (see the comment in `.cargo/config.toml`).
//! `rustc-link-arg` reaches only this package's cdylib and composes with whatever the
//! image already passes.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // 16 MiB: the value the interpreter's own tests need, and a fraction of the
    // module's linear memory. Two arguments, the form rustc itself passes: `-z`
    // takes `stack-size=<bytes>` either joined or separate.
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=stack-size=16777216");
}
