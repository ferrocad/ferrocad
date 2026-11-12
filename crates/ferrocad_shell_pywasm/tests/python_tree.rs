//! B18b item 3: a native RustPython host imports `python/**` from disk.
//!
//! `host_env` gives the interpreter `os`/`_io`, so the shared facade can be a real
//! package tree rather than a synthetic module built from `include_str!`. That is what
//! makes an edit to `python/FreeCAD` visible to the next desktop boot without a Rust
//! rebuild. The browser has no filesystem and keeps the embedded copy.
//!
//! Both facts are pinned here: a default boot finds the repository's tree, and
//! `FERROCAD_PYTHON_DIR` makes the console run a different tree.

#![cfg(not(target_family = "wasm"))]

use ferrocad_shell_pywasm::PywasmScriptHost;
use serde_json::Value;

/// Run `f` against a freshly booted interpreter on a large stack (RustPython's compiler
/// recurses; see `properties.rs`).
fn with_host<R: Send + 'static>(f: impl FnOnce(&PywasmScriptHost) -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || {
            let host = ferrocad_shell_pywasm::new().expect("the interpreter boots");
            f(&host)
        })
        .expect("a worker thread")
        .join()
        .expect("no panic")
}

fn output(host: &PywasmScriptHost, code: &str) -> String {
    let raw = host.call("evaluate", &[code]).expect("the console runs");
    let answer: Value = serde_json::from_str(&raw).expect("evaluate answers JSON");
    assert_eq!(answer["ok"], true, "{code}: {answer}");
    answer["output"].as_str().unwrap_or_default().to_string()
}

/// The two tests below share the process-wide `FERROCAD_PYTHON_DIR`, so they must not
/// overlap: the default-discovery test would see the override tree otherwise.
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The tree this repository ships, resolved from the test's own crate.
fn shipped_python() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python")
}

/// Copy one package directory, skipping `__pycache__` (a CPython byte-cache is not a
/// RustPython one and must not be mistaken for source).
fn copy_package(source: &std::path::Path, target: &std::path::Path) {
    std::fs::create_dir_all(target).expect("a package directory");
    for entry in std::fs::read_dir(source).expect("the shipped package") {
        let entry = entry.expect("a directory entry");
        let name = entry.file_name();
        if name == "__pycache__" {
            continue;
        }
        let to = target.join(&name);
        if entry.path().is_dir() {
            copy_package(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).expect("copy a package file");
        }
    }
}

#[test]
fn a_native_boot_imports_the_tree_from_disk() {
    let _guard = env_lock();
    // The default boot finds `python/` above the test executable (or, in a dev build,
    // beside the crate manifest), so the facade's `__file__` is a real file.
    let file = with_host(|host| output(host, "print(FreeCAD.__file__)"));
    assert!(
        file.ends_with("python/FreeCAD/__init__.py"),
        "the shipped tree is what runs: {file}"
    );
    assert!(
        std::path::Path::new(&file).is_absolute(),
        "a disk import has an absolute __file__: {file}"
    );

    // `host_env` is what makes that path usable: the real `os`/`pathlib`/`tempfile` are
    // here, not `bootstrap.py`'s wasm substitutes (whose `Path.exists` says `False`).
    let stdlib = with_host(|host| {
        output(
            host,
            "import os, pathlib, tempfile\n\
             print(os.path.isabs('/x'), hasattr(tempfile, 'mkstemp'), \
             pathlib.Path(FreeCAD.__file__).is_file())",
        )
    });
    assert_eq!(
        stdlib, "True True True",
        "host_env supplies the real stdlib layer"
    );
}

#[test]
fn the_override_tree_is_what_the_console_runs() {
    let _guard = env_lock();
    let root = std::env::temp_dir().join(format!("ferrocad-python-tree-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("a temp tree");
    for package in [
        "FreeCAD",
        "FreeCADGui",
        "FerroCAD",
        "FerroCADGui",
        "ferrocad_shell",
    ] {
        copy_package(&shipped_python().join(package), &root.join(package));
    }
    // A marker only the copied tree has: if the console sees it, the boot read `root`,
    // not the repository and not the embedded copy.
    let init = root.join("FerroCAD/__init__.py");
    let mut source = std::fs::read_to_string(&init).expect("the copied package");
    source.push_str("\nMARKER_FROM_DISK = 'ferrocad-tree-test'\n");
    std::fs::write(&init, source).expect("write the marker");

    // SAFETY: the environment lock above makes this the only boot in flight, and the
    // override is removed before the test ends.
    unsafe { std::env::set_var("FERROCAD_PYTHON_DIR", &root) };
    let marker =
        with_host(|host| output(host, "import FerroCAD\nprint(FerroCAD.MARKER_FROM_DISK)"));
    unsafe { std::env::remove_var("FERROCAD_PYTHON_DIR") };

    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(marker, "ferrocad-tree-test");
}
