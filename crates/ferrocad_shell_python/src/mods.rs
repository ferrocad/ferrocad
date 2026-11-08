//! What the mod directories declare, read once before the hosts are chosen.
//!
//! B9a: `mods/<Name>/` says what a workbench needs by its entry file, so the script hosts
//! are derived from the set rather than typed on the command line. The directory list is
//! [`crate::python::mods_dirs`], so the derivation and the Python loader cannot disagree
//! about where the mods are.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ferrocad_shell_api::EntryKind;

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

/// The kinds `package` declares, by its top-level entry files.
fn collect(package: &Path, seen: &mut BTreeSet<EntryKind>) {
    let Ok(files) = std::fs::read_dir(package) else {
        return;
    };
    for file in files.flatten() {
        if let Some(name) = file.file_name().to_str() {
            if let Some(kind) = EntryKind::of_file_name(name) {
                seen.insert(kind);
            }
        }
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
}
