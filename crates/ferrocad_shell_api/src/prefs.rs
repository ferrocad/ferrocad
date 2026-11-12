//! B17a: the preferences the host reads **before** the backend exists.
//!
//! The About card can show what a boot composed, but changing a *mod* or a *bundle*
//! cannot be a runtime operation: `mod_roots` selects the set and `load_workbenches`
//! runs at boot, both before the interpreter (which owns the normal `Settings` store)
//! exists. This module is the small, host-readable store that decision needs.
//!
//! Two rules keep it honest:
//!
//! - **A disabled entry is a refusal; absence means enabled.** The file records what the
//!   user turned *off*, so a file from a newer build, or one that names a mod this build
//!   does not have, cannot switch anything on by surprise; deleting the file restores the
//!   defaults.
//! - **A malformed file is the defaults.** This is on the boot path: refusing to start the
//!   window over a stray byte would be worse than ignoring it.
//!
//! Providers are the live half: [`Prefs::disabled_providers`] hides a backend from the
//! popovers and the Dev Tools list and refuses activation in the same run. Mods are the
//! restart half, because their set is fixed at boot.

use std::cell::RefCell;
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// The format version written to (and accepted from) the store.
pub const VERSION: u32 = 1;

/// What the user turned off. Empty means the defaults (everything on).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// The format version. An older/newer file still loads: the fields are additive and
    /// anything unknown is ignored.
    pub version: u32,
    /// Mod/package names the user turned off (`GridOverlay`, `Fasteners`, …).
    pub disabled_mods: BTreeSet<String>,
    /// Provider backends the user turned off, keyed `feature:backend`
    /// (`grid:coin`, `gizmo:ferrocad-shader`).
    pub disabled_providers: BTreeSet<String>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            version: VERSION,
            disabled_mods: BTreeSet::new(),
            disabled_providers: BTreeSet::new(),
        }
    }
}

impl Prefs {
    /// Read a store, tolerating anything that is not the shape written above.
    pub fn parse(text: &str) -> Prefs {
        serde_json::from_str(text).unwrap_or_default()
    }

    /// The text a store writes.
    pub fn to_json(&self) -> String {
        let mut prefs = self.clone();
        prefs.version = VERSION;
        let mut text = serde_json::to_string_pretty(&prefs).unwrap_or_else(|_| "{}".to_string());
        text.push('\n');
        text
    }

    /// Whether a mod loads. Absence means enabled.
    pub fn mod_enabled(&self, name: &str) -> bool {
        !self.disabled_mods.contains(name)
    }

    /// Turn a mod on or off; turning it on removes the refusal rather than storing a
    /// redundant "enabled" entry.
    pub fn set_mod_enabled(&mut self, name: &str, enabled: bool) {
        if enabled {
            self.disabled_mods.remove(name);
        } else {
            self.disabled_mods.insert(name.to_string());
        }
    }

    /// The key [`Prefs::disabled_providers`] uses.
    pub fn provider_key(feature: &str, backend: &str) -> String {
        format!("{feature}:{backend}")
    }

    /// Whether a provider backend is offered. Absence means enabled.
    pub fn provider_enabled(&self, feature: &str, backend: &str) -> bool {
        !self
            .disabled_providers
            .contains(&Self::provider_key(feature, backend))
    }

    /// Turn a provider backend on or off.
    pub fn set_provider_enabled(&mut self, feature: &str, backend: &str, enabled: bool) {
        let key = Self::provider_key(feature, backend);
        if enabled {
            self.disabled_providers.remove(&key);
        } else {
            self.disabled_providers.insert(key);
        }
    }
}

/// What a host read before boot, and the set of names the card can offer.
///
/// The host installs this beside its [`Composition`](crate::types::Composition)
/// (`ferrocad_shell_core::app::install_prefs`), so boot can copy it into the model without
/// the shell re-scanning anything.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrefsReport {
    pub prefs: Prefs,
    /// Every mod the boot could see, by name — enabled and disabled alike. The set is the
    /// host's (a scanned `mods/` tree, or a fetched index), not the backend's.
    pub mods: Vec<String>,
    /// Where `prefs` are stored, in words a user can act on (`session only` when no store
    /// is installed).
    pub store: String,
}

/// How a host reads and writes [`Prefs`].
///
/// `load` is called before the window exists, so it must not need an `App`; `save` is called
/// from a click and reports a failure rather than aborting the session (the in-memory change
/// still applies).
pub trait PrefsStore {
    fn load(&self) -> Prefs;
    fn save(&self, prefs: &Prefs) -> Result<(), String>;
    /// A one-line description for the About card (`/home/me/.config/ferrocad/prefs.json`,
    /// `localStorage`).
    fn location(&self) -> String;
}

/// A store that keeps everything in this process: tests, and any host without storage.
#[derive(Debug, Default)]
pub struct MemoryPrefs {
    inner: RefCell<Prefs>,
}

impl MemoryPrefs {
    pub fn new(prefs: Prefs) -> Self {
        Self {
            inner: RefCell::new(prefs),
        }
    }
}

impl PrefsStore for MemoryPrefs {
    fn load(&self) -> Prefs {
        self.inner.borrow().clone()
    }

    fn save(&self, prefs: &Prefs) -> Result<(), String> {
        *self.inner.borrow_mut() = prefs.clone();
        Ok(())
    }

    fn location(&self) -> String {
        "session only".to_string()
    }
}

/// The environment variable that names the file outright, for a test or a portable app.
pub const FILE_ENV: &str = "FERROCAD_PREFS_FILE";

/// The desktop store: one JSON file beside the user's parameter file.
///
/// Native only: the page has no filesystem and installs its own store
/// (`localStorage`) in the browser host.
#[cfg(not(target_family = "wasm"))]
#[derive(Debug, Clone)]
pub struct FilePrefs {
    path: std::path::PathBuf,
}

#[cfg(not(target_family = "wasm"))]
impl FilePrefs {
    /// The store at `path`.
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// `FERROCAD_PREFS_FILE`, else `<XDG_CONFIG_HOME>/ferrocad/prefs.json` (falling back
    /// to `~/.config`, then `%APPDATA%`) — the same rule the parameter store uses, so the
    /// two files sit together.
    pub fn default_path() -> std::path::PathBuf {
        if let Some(path) = std::env::var_os(FILE_ENV).filter(|value| !value.is_empty()) {
            return std::path::PathBuf::from(path);
        }
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config"))
            })
            .or_else(|| std::env::var_os("APPDATA").map(std::path::PathBuf::from))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        base.join("ferrocad").join("prefs.json")
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

#[cfg(not(target_family = "wasm"))]
impl Default for FilePrefs {
    fn default() -> Self {
        Self::new(Self::default_path())
    }
}

#[cfg(not(target_family = "wasm"))]
impl PrefsStore for FilePrefs {
    fn load(&self) -> Prefs {
        std::fs::read_to_string(&self.path)
            .map(|text| Prefs::parse(&text))
            .unwrap_or_default()
    }

    fn save(&self, prefs: &Prefs) -> Result<(), String> {
        let path = &self.path;
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        // Write beside the target and rename: a window closed mid-write must not leave a
        // truncated file that the next boot parses as "everything enabled".
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, prefs.to_json())
            .map_err(|error| format!("{}: {error}", temporary.display()))?;
        std::fs::rename(&temporary, path).map_err(|error| format!("{}: {error}", path.display()))
    }

    fn location(&self) -> String {
        self.path.display().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Absence is the default: the file records refusals, not a complete list.
    #[test]
    fn absence_means_enabled_and_a_refusal_is_removed_on_enable() {
        let mut prefs = Prefs::default();
        assert!(prefs.mod_enabled("GridOverlay"));
        assert!(prefs.provider_enabled("grid", "coin"));

        prefs.set_mod_enabled("GridOverlay", false);
        prefs.set_provider_enabled("grid", "coin", false);
        assert!(!prefs.mod_enabled("GridOverlay"));
        assert!(!prefs.provider_enabled("grid", "coin"));
        assert_eq!(prefs.to_json().matches("GridOverlay").count(), 1);

        prefs.set_mod_enabled("GridOverlay", true);
        prefs.set_provider_enabled("grid", "coin", true);
        assert!(prefs.disabled_mods.is_empty(), "enabling removes the entry");
        assert!(prefs.disabled_providers.is_empty());
        assert!(prefs.mod_enabled("GridOverlay"));
    }

    /// A file from a newer build, or one the user edited, still loads what this build
    /// understands; anything else is the defaults rather than a boot failure.
    #[test]
    fn a_malformed_or_newer_file_still_loads() {
        assert_eq!(Prefs::parse("not json"), Prefs::default());
        assert_eq!(Prefs::parse(""), Prefs::default());

        let newer = r#"{"version": 9, "disabled_mods": ["GridOverlay"], "future": true}"#;
        let prefs = Prefs::parse(newer);
        assert!(!prefs.mod_enabled("GridOverlay"));
        let round = Prefs::parse(&prefs.to_json());
        assert_eq!(round.disabled_mods, prefs.disabled_mods, "it round-trips");
        assert_eq!(
            round.version, VERSION,
            "the store writes this build's version"
        );
    }

    /// The file store round-trips, creates its directory, and leaves no temporary behind.
    #[test]
    fn the_file_store_round_trips() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = FilePrefs::new(dir.path().join("nested").join("prefs.json"));
        assert_eq!(
            store.load(),
            Prefs::default(),
            "a missing file is the defaults"
        );

        let mut prefs = Prefs::default();
        prefs.set_mod_enabled("Fasteners", false);
        prefs.set_provider_enabled("gizmo", "ferrocad-shader", false);
        store.save(&prefs).expect("save");

        assert_eq!(FilePrefs::new(store.path()).load(), prefs);
        assert!(
            !dir.path().join("nested").join("prefs.json.tmp").exists(),
            "the write is renamed into place"
        );
        assert!(store.location().ends_with("prefs.json"));
    }

    /// The in-memory store is the no-filesystem host's default and the tests' store.
    #[test]
    fn the_memory_store_keeps_the_value() {
        let store = MemoryPrefs::default();
        assert_eq!(store.load(), Prefs::default());
        let mut prefs = Prefs::default();
        prefs.set_mod_enabled("Spring", false);
        store.save(&prefs).expect("save");
        assert!(!store.load().mod_enabled("Spring"));
        assert_eq!(store.location(), "session only");
    }
}
