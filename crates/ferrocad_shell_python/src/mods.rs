//! What the mod directories declare, read once before the hosts are chosen.
//!
//! B9a: `mods/<Name>/` says what a workbench needs by its entry file, so the script hosts
//! are derived from the set rather than typed on the command line. The directory list is
//! [`crate::python::mods_dirs`], so the derivation and the Python loader cannot disagree
//! about where the mods are.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ferrocad_shell_api::{DocumentGrant, EntryKind, Language, Runtime};

/// The entry kinds present under `dirs`, in [`EntryKind::ALL`] order.
///
/// `<dir>/<Name>/` is one package and its files decide the kind; a package may declare
/// more than one. A directory that does not exist is skipped, because a payload may name
/// one that ships on only some targets, and the result is ordered so the derivation never
/// depends on how the filesystem enumerates.
pub fn entry_kinds(dirs: &[PathBuf]) -> Vec<EntryKind> {
    let mut seen = BTreeSet::new();
    for dir in dirs {
        let Ok(packages) = std::fs::read_dir(dir) else {
            continue;
        };
        for package in packages.flatten() {
            if !package.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            collect(&package.path(), &mut seen);
        }
    }
    EntryKind::ALL
        .into_iter()
        .filter(|kind| seen.contains(kind))
        .collect()
}

/// The wasm bundles under `dirs`, in name order: `<dir>/<Name>/addon.wasm`.
///
/// Empty without the `addons` feature, because a path the host cannot load is worse than
/// no path at all: the run is the same as a mod set with no bundles.
pub fn addon_packs(dirs: &[PathBuf]) -> Vec<PathBuf> {
    #[cfg(feature = "addons")]
    {
        ferrocad_shell_wasm::packs(dirs)
    }
    #[cfg(not(feature = "addons"))]
    {
        let _ = dirs;
        Vec::new()
    }
}

/// The kinds `package` declares, by its top-level entry files.
fn collect(package: &Path, seen: &mut BTreeSet<EntryKind>) {
    for kind in package_kinds(package) {
        seen.insert(kind);
    }
}

/// One package's manifest request.
/// The kinds one package declares.
fn package_kinds(package: &Path) -> BTreeSet<EntryKind> {
    let mut kinds = BTreeSet::new();
    let Ok(files) = std::fs::read_dir(package) else {
        return kinds;
    };
    for file in files.flatten() {
        if let Some(name) = file.file_name().to_str() {
            if let Some(kind) = EntryKind::of_file_name(name) {
                kinds.insert(kind);
            }
        }
    }
    kinds
}

/// One package's manifest request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// The package directory's name, which is what a log line can name.
    pub name: String,
    /// Which entry of the package the request is for.
    pub kind: EntryKind,
    /// What the manifest asks for.
    pub grant: DocumentGrant,
}

/// The manifest requests under `dirs`, one per declared entry.
///
/// A package names its request in `addon.json` — `{"document": "read-write"}`, the same
/// key a bundle already uses. A package with no manifest asks for nothing, and an unknown
/// word is likewise no request: the policy is to grant by default, so silence is the safe
/// reading rather than a refusal.
pub fn requests(dirs: &[PathBuf]) -> Vec<Request> {
    let mut requests = Vec::new();
    for dir in dirs {
        let Ok(packages) = std::fs::read_dir(dir) else {
            continue;
        };
        for package in packages.flatten() {
            let path = package.path();
            if !package.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let kinds = package_kinds(&path);
            let Some(grant) = requested_grant(&path) else {
                continue;
            };
            let name = package.file_name().to_string_lossy().into_owned();
            for kind in kinds {
                requests.push(Request {
                    name: name.clone(),
                    kind,
                    grant,
                });
            }
        }
    }
    requests
}

/// What `package`'s manifest asks for, if it has one that names a grant.
fn requested_grant(package: &Path) -> Option<DocumentGrant> {
    #[derive(serde::Deserialize)]
    struct Manifest {
        document: Option<String>,
    }

    let text = std::fs::read_to_string(package.join("addon.json")).ok()?;
    let manifest: Manifest = serde_json::from_str(&text).ok()?;
    manifest.document.as_deref().and_then(DocumentGrant::from_name)
}

/// The requests the installed runtimes cannot honour, as log lines.
///
/// **Granting is the policy for now**: nothing here refuses a package or downgrades it. A
/// mismatch is only said out loud, because an author who tested on the desktop otherwise
/// finds out in the browser at the first refused write. A bundle's own manifest is not
/// checked here — its document API already enforces it.
pub fn under_granted(requests: &[Request], runtimes: &[Runtime]) -> Vec<String> {
    let mut lines = Vec::new();
    for request in requests {
        if !request.grant.allows_write() {
            continue;
        }
        let language = Language::of_kind(request.kind);
        if language == Language::Wasm {
            continue;
        }
        let host = runtimes
            .iter()
            .copied()
            .find(|runtime| Language::of(*runtime) == Some(language));
        let granted = host
            .map(Runtime::document_grant)
            .unwrap_or(DocumentGrant::Read);
        if granted.allows_write() {
            continue;
        }
        let engine = host.map(Runtime::id).unwrap_or("no script host");
        lines.push(format!(
            "{} asks for {}; {engine} grants {granted}",
            request.name, request.grant
        ));
    }
    lines
}

#[cfg(all(test, feature = "addons"))]
mod addon_tests {
    use super::*;

    fn bundle(package: &Path) {
        std::fs::create_dir_all(package).expect("the package directory");
        std::fs::write(package.join("addon.wasm"), b"\0asm").expect("the bundle");
    }

    fn names(packs: Vec<PathBuf>) -> Vec<String> {
        packs
            .iter()
            .map(|pack| {
                pack.parent()
                    .and_then(Path::file_name)
                    .map(|name| name.to_string_lossy().into_owned())
                    .expect("a package directory")
            })
            .collect()
    }

    #[test]
    fn the_bundles_are_the_packages_with_one_in_name_order() {
        let temp = tempfile::tempdir().expect("a temporary directory");
        bundle(&temp.path().join("Zeta"));
        bundle(&temp.path().join("Alpha"));
        // A package with another language's entry is not a bundle.
        let python = temp.path().join("PythonOnly");
        std::fs::create_dir_all(&python).expect("the package directory");
        std::fs::write(python.join("InitGui.py"), "").expect("the entry file");

        assert_eq!(
            names(addon_packs(std::slice::from_ref(&temp.path().to_path_buf()))),
            ["Alpha", "Zeta"]
        );
    }

    #[test]
    fn a_directory_with_no_bundle_yields_none() {
        let temp = tempfile::tempdir().expect("a temporary directory");
        assert!(addon_packs(&[temp.path().to_path_buf()]).is_empty());
    }
}

#[cfg(all(test, not(feature = "addons")))]
mod no_addon_tests {
    use super::*;

    #[test]
    fn without_the_feature_nothing_is_discovered() {
        // The bundle would only be skipped by the composition later; not naming it keeps
        // the run identical to a mod set that has none.
        let temp = tempfile::tempdir().expect("a temporary directory");
        let package = temp.path().join("Toy");
        std::fs::create_dir_all(&package).expect("the package directory");
        std::fs::write(package.join("addon.wasm"), b"\0asm").expect("the bundle");

        assert!(addon_packs(&[temp.path().to_path_buf()]).is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary `mods/` directory with `packages`, each holding `files`.
    fn mods(packages: &[(&str, &[&str])]) -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("a temporary directory");
        for (name, files) in packages {
            let package = temp.path().join(name);
            std::fs::create_dir_all(&package).expect("the package directory");
            for file in *files {
                std::fs::write(package.join(file), "").expect("the entry file");
            }
        }
        temp
    }

    fn kinds(temp: &tempfile::TempDir) -> Vec<EntryKind> {
        entry_kinds(&[temp.path().to_path_buf()])
    }

    #[test]
    fn a_package_is_classified_by_its_entry_file() {
        let temp = mods(&[
            ("Draft", &["InitGui.py"]),
            ("Toy", &["InitGui.js"]),
            ("Sand", &["addon.wasm"]),
        ]);
        assert_eq!(
            kinds(&temp),
            [EntryKind::Python, EntryKind::JavaScript, EntryKind::Wasm]
        );
    }

    #[test]
    fn a_package_with_no_entry_file_declares_nothing() {
        let temp = mods(&[("Notes", &["README.md"]), ("Empty", &[])]);
        assert!(kinds(&temp).is_empty());
    }

    #[test]
    fn one_package_may_declare_two_kinds() {
        let temp = mods(&[("Both", &["InitGui.py", "addon.wasm"])]);
        assert_eq!(kinds(&temp), [EntryKind::Python, EntryKind::Wasm]);
    }

    #[test]
    fn the_order_is_the_kinds_not_the_filesystem() {
        // Written in an order that does not match `EntryKind::ALL`, so a scan that returned
        // whatever `read_dir` produced would fail here on most filesystems.
        let temp = mods(&[
            ("Zed", &["addon.wasm"]),
            ("Alpha", &["InitGui.js"]),
            ("Mid", &["InitGui.py"]),
        ]);
        assert_eq!(
            kinds(&temp),
            [EntryKind::Python, EntryKind::JavaScript, EntryKind::Wasm]
        );
    }

    #[test]
    fn a_missing_directory_is_skipped() {
        let temp = tempfile::tempdir().expect("a temporary directory");
        let absent = temp.path().join("absent");
        assert!(entry_kinds(&[absent]).is_empty());
    }

    #[test]
    fn a_nested_entry_file_is_not_a_package_entry() {
        // Only the package's own files count; a deeper `mods/A/B/InitGui.py` belongs to B.
        let temp = mods(&[("A", &[])]);
        std::fs::create_dir_all(temp.path().join("A").join("B")).expect("a nested directory");
        std::fs::write(temp.path().join("A").join("B").join("InitGui.py"), "")
            .expect("a nested entry");
        assert!(kinds(&temp).is_empty());
    }

    /// A package with `files` and a manifest.
    fn with_manifest(temp: &tempfile::TempDir, name: &str, files: &[&str], manifest: &str) {
        let package = temp.path().join(name);
        std::fs::create_dir_all(&package).expect("the package directory");
        for file in files {
            std::fs::write(package.join(file), "").expect("the entry file");
        }
        std::fs::write(package.join("addon.json"), manifest).expect("the manifest");
    }

    fn requests_in(temp: &tempfile::TempDir) -> Vec<Request> {
        requests(&[temp.path().to_path_buf()])
    }

    #[test]
    fn a_package_without_a_manifest_asks_for_nothing() {
        let temp = mods(&[("Draft", &["InitGui.py"])]);
        assert!(requests_in(&temp).is_empty());
    }

    #[test]
    fn a_manifest_applies_to_every_entry_its_package_declares() {
        let temp = tempfile::tempdir().expect("a temporary directory");
        with_manifest(
            &temp,
            "Both",
            &["InitGui.py", "addon.wasm"],
            r#"{"document": "read-write"}"#,
        );

        let requests = requests_in(&temp);
        assert_eq!(requests.len(), 2);
        assert!(requests.iter().all(|request| request.name == "Both"));
        assert!(
            requests
                .iter()
                .all(|request| request.grant == DocumentGrant::ReadWrite)
        );
    }

    #[test]
    fn an_unknown_grant_word_asks_for_nothing() {
        // Granting is the policy, so a word the seam does not know is silence, not a
        // refusal and not a default the author did not ask for.
        let temp = tempfile::tempdir().expect("a temporary directory");
        with_manifest(
            &temp,
            "Odd",
            &["InitGui.py"],
            r#"{"document": "everything"}"#,
        );
        assert!(requests_in(&temp).is_empty());
    }

    #[test]
    fn a_read_write_request_is_reported_when_the_python_engine_cannot_write() {
        let temp = tempfile::tempdir().expect("a temporary directory");
        with_manifest(
            &temp,
            "Draft",
            &["InitGui.py"],
            r#"{"document": "read-write"}"#,
        );
        let requests = requests_in(&temp);

        // The browser's Python, and `--script-host=pywasm` on the desktop.
        let warnings = under_granted(&requests, &[Runtime::Pywasm]);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("Draft"), "{warnings:?}");
        assert!(warnings[0].contains("read-write"), "{warnings:?}");
        assert!(warnings[0].contains("pywasm"), "{warnings:?}");

        // Granted where it can be, and the check follows the host that would run it.
        assert!(under_granted(&requests, &[Runtime::Cpython]).is_empty());
        assert!(under_granted(&requests, &[Runtime::Cpython, Runtime::Js]).is_empty());
    }

    #[test]
    fn a_bundles_request_is_left_to_the_bundle() {
        // The document API already enforces a bundle's manifest; the advisory must not repeat it.
        let temp = tempfile::tempdir().expect("a temporary directory");
        with_manifest(
            &temp,
            "Sand",
            &["addon.wasm"],
            r#"{"document": "read-write"}"#,
        );
        let requests = requests_in(&temp);
        assert_eq!(requests.len(), 1);
        assert!(under_granted(&requests, &[Runtime::Cpython]).is_empty());
    }
}
