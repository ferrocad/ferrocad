//! What the mod directories declare, read once before the hosts are chosen.
//!
//! B9a: `mods/<Name>/` says what a workbench needs by its entry file, so the script hosts
//! are derived from the set rather than typed on the command line. The directory list is
//! [`crate::python::mods_dirs`], so the derivation and the Python loader cannot disagree
//! about where the mods are.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ferrocad_shell_api::{DocumentGrant, EntryKind, Language, Runtime};

/// The entry kinds present under `roots`, in [`EntryKind::ALL`] order.
///
/// A root is a package, a directory of packages, or one bundle file (see [`packages`]);
/// the selection is what `--addons` names, and the whole mods directory when it names
/// nothing. The result is ordered so the derivation never depends on how the filesystem
/// enumerates.
pub fn entry_kinds(roots: &[PathBuf]) -> Vec<EntryKind> {
    let mut seen = BTreeSet::new();
    for root in roots {
        for package in packages(root) {
            seen.extend(package_kinds(&package));
        }
    }
    EntryKind::ALL
        .into_iter()
        .filter(|kind| seen.contains(kind))
        .collect()
}

/// The packages one mod-set root names.
///
/// A root is one of three things, and every host's scanner applies the same rule — so
/// `--addons mods/GridOverlay` selects one mod for the derivation, the Python loader and
/// the bundles alike:
///
/// - a **package** directory (it declares an entry file): the root itself;
/// - a **directory of packages** (`mods/`): its subdirectories, sorted;
/// - one **bundle file** (`mods/Toy/addon.wasm`): the file's directory, which is its
///   package — the same selection as naming the directory.
pub fn packages(root: &Path) -> Vec<PathBuf> {
    if root.is_file() {
        return root
            .parent()
            .map(|dir| vec![dir.to_path_buf()])
            .unwrap_or_default();
    }
    if is_package(root) {
        return vec![root.to_path_buf()];
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut packages: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    packages.sort();
    packages
}

/// Whether `dir` is itself a package: it holds at least one entry file.
///
/// `mods/` holds none (only package directories), so it stays "a directory of packages";
/// a root that does hold one is a selection of exactly that mod.
pub fn is_package(dir: &Path) -> bool {
    !package_kinds(dir).is_empty()
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
pub fn requests(roots: &[PathBuf]) -> Vec<Request> {
    let mut requests = Vec::new();
    for root in roots {
        for path in packages(root) {
            let kinds = package_kinds(&path);
            let Some(grant) = requested_grant(&path) else {
                continue;
            };
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
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
    fn a_package_root_selects_one_mod() {
        // `--addons mods/Draft`: the root *is* the package, so the set is that mod alone
        // — not the whole directory it sits in. This is what makes the flag a selection
        // for the derivation, the loader and the bundles at once.
        let temp = mods(&[
            ("Draft", &["InitGui.py"]),
            ("Toy", &["InitGui.js"]),
            ("Sand", &["addon.wasm"]),
        ]);
        let draft = temp.path().join("Draft");
        assert_eq!(entry_kinds(&[draft.clone()]), [EntryKind::Python]);
        assert_eq!(packages(&draft), [draft.clone()], "the package itself");
        assert!(is_package(&draft));
        assert!(!is_package(temp.path()), "mods/ holds packages, it is not one");
        assert_eq!(packages(temp.path()).len(), 3, "a directory of packages");
    }

    #[test]
    fn a_bundle_file_names_its_package() {
        // `--addons mods/Sand/addon.wasm` is the same selection as `--addons mods/Sand`,
        // which is also where its manifest lives.
        let temp = mods(&[("Sand", &["addon.wasm"])]);
        let bundle = temp.path().join("Sand").join("addon.wasm");
        assert_eq!(packages(&bundle), [temp.path().join("Sand")]);
        assert_eq!(entry_kinds(&[bundle]), [EntryKind::Wasm]);
    }

    #[test]
    fn a_selection_keeps_the_order_it_was_named_in() {
        let temp = mods(&[("Draft", &["InitGui.py"]), ("Toy", &["InitGui.js"])]);
        let draft = temp.path().join("Draft");
        let toy = temp.path().join("Toy");
        assert_eq!(
            entry_kinds(&[toy.clone(), draft.clone()]),
            [EntryKind::Python, EntryKind::JavaScript],
            "ordered by kind, whatever the selection order"
        );
        assert_eq!(packages(&toy), [toy]);
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
    fn a_read_write_request_is_reported_when_no_python_host_can_write() {
        let temp = tempfile::tempdir().expect("a temporary directory");
        with_manifest(
            &temp,
            "Draft",
            &["InitGui.py"],
            r#"{"document": "read-write"}"#,
        );
        let requests = requests_in(&temp);

        // No host of the language at all: the request has nothing that could serve it.
        let warnings = under_granted(&requests, &[]);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("Draft"), "{warnings:?}");
        assert!(warnings[0].contains("read-write"), "{warnings:?}");
        assert!(warnings[0].contains("no script host"), "{warnings:?}");

        // Granted where it can be, and the check follows the host that would run it.
        // B18b item 5: the pywasm host writes through the doorway too, so a Python mod's
        // read-write request is served in the browser and by the desktop preview.
        assert!(under_granted(&requests, &[Runtime::Pywasm]).is_empty());
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
