//! Host-neutral user-settings storage.
//!
//! The shell already owns the *meaning* of a setting: its schema, defaults and the
//! [`ViewRequest`](crate::view::ViewRequest) an edit produces. What differs by host is
//! only where the values live:
//!
//! * native: `FreeCAD.ParamGet` in the user's FreeCAD parameter store;
//! * browser: `localStorage` (falling back to memory when storage is unavailable);
//! * tests and a headless session: memory.
//!
//! [`SettingsBackend`] is that seam. A host installs one on the document backend; the
//! shared settings card reads and writes typed values, and the desktop Python API and
//! the browser therefore use the same keys and the same defaults instead of two
//! storage implementations drifting apart.
//!
//! The trait deliberately knows nothing about the grid. Feature modules name their
//! group and keys (`Mods/grid/ferrocad`, `enabled`, `minor`, …), exactly as the
//! FreeCAD parameter groups do, and keep defaults/validation in one place.

use std::collections::BTreeMap;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

/// The type of a setting value, needed because parameter stores are typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettingKind {
    Bool,
    Int,
    Float,
    String,
}

/// One typed setting value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SettingValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
}

impl SettingValue {
    /// The kind tag matching this value.
    pub fn kind(&self) -> SettingKind {
        match self {
            Self::Bool(_) => SettingKind::Bool,
            Self::Int(_) => SettingKind::Int,
            Self::Float(_) => SettingKind::Float,
            Self::String(_) => SettingKind::String,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Float(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        self.as_f64().map(|value| value as f32)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value.as_str()),
            _ => None,
        }
    }
}

impl From<bool> for SettingValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for SettingValue {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<f64> for SettingValue {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<String> for SettingValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for SettingValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

/// A host's settings store.
///
/// Groups and names use the FreeCAD parameter spelling without the
/// `User parameter:BaseApp/Preferences/` prefix, so a native adapter and a browser
/// store address exactly the same setting.
pub trait SettingsBackend: Send + Sync {
    /// Read one value, or `None` when it is absent or stored under another kind.
    fn get(&self, group: &str, name: &str, kind: SettingKind) -> Option<SettingValue>;

    /// Write one value. A store that cannot persist reports why.
    fn set(&self, group: &str, name: &str, value: SettingValue) -> Result<(), String>;

    /// Remove one value of `kind`; removing an absent value is a no-op.
    fn remove(&self, group: &str, name: &str, kind: SettingKind) -> Result<(), String>;

    /// The names with any value in `group`, in deterministic order.
    fn names(&self, group: &str) -> Vec<String>;

    /// The groups with any stored value, in deterministic order.
    ///
    /// A store that cannot enumerate its groups returns none; a script host with its
    /// own store then keeps its defaults instead of seeding.
    fn groups(&self) -> Vec<String> {
        Vec::new()
    }
}

/// The in-memory store: the headless/test default and the browser fallback.
///
/// Cloning shares the map, so a test can hand the store to a backend and still read
/// what was written to it (a provider switch, for one) without a second accessor.
#[derive(Clone, Default)]
pub struct MemorySettings {
    values: std::sync::Arc<RwLock<BTreeMap<(String, String), SettingValue>>>,
}

impl SettingsBackend for MemorySettings {
    fn get(&self, group: &str, name: &str, kind: SettingKind) -> Option<SettingValue> {
        let values = self
            .values
            .read()
            .unwrap_or_else(|error| error.into_inner());
        values
            .get(&(group.to_string(), name.to_string()))
            .filter(|value| value.kind() == kind)
            .cloned()
    }

    fn set(&self, group: &str, name: &str, value: SettingValue) -> Result<(), String> {
        let mut values = self
            .values
            .write()
            .unwrap_or_else(|error| error.into_inner());
        values.insert((group.to_string(), name.to_string()), value);
        Ok(())
    }

    fn remove(&self, group: &str, name: &str, kind: SettingKind) -> Result<(), String> {
        let mut values = self
            .values
            .write()
            .unwrap_or_else(|error| error.into_inner());
        let key = (group.to_string(), name.to_string());
        if values.get(&key).is_some_and(|value| value.kind() == kind) {
            values.remove(&key);
        }
        Ok(())
    }

    fn names(&self, group: &str) -> Vec<String> {
        let values = self
            .values
            .read()
            .unwrap_or_else(|error| error.into_inner());
        let mut names: Vec<String> = values
            .keys()
            .filter(|(stored_group, _)| stored_group == group)
            .map(|(_, name)| name.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    fn groups(&self) -> Vec<String> {
        let values = self
            .values
            .read()
            .unwrap_or_else(|error| error.into_inner());
        let mut groups: Vec<String> = values.keys().map(|(group, _)| group.clone()).collect();
        groups.sort();
        groups.dedup();
        groups
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_round_trips_with_its_kind() {
        let settings = MemorySettings::default();
        settings
            .set("Mods/grid/ferrocad", "enabled", true.into())
            .unwrap();
        settings
            .set("Mods/grid/ferrocad", "minor", 10.0.into())
            .unwrap();
        settings
            .set("Mods/grid/ferrocad", "color", "#94a3b8".into())
            .unwrap();

        assert_eq!(
            settings.get("Mods/grid/ferrocad", "enabled", SettingKind::Bool),
            Some(SettingValue::Bool(true))
        );
        assert_eq!(
            settings.get("Mods/grid/ferrocad", "minor", SettingKind::Float),
            Some(SettingValue::Float(10.0))
        );
        assert_eq!(
            settings.get("Mods/grid/ferrocad", "color", SettingKind::String),
            Some(SettingValue::String("#94a3b8".to_string()))
        );
    }

    #[test]
    fn a_wrong_kind_is_absent() {
        let settings = MemorySettings::default();
        settings
            .set("Mods/grid/ferrocad", "enabled", true.into())
            .unwrap();
        assert_eq!(
            settings.get("Mods/grid/ferrocad", "enabled", SettingKind::Float),
            None
        );
    }

    #[test]
    fn groups_and_names_are_separate() {
        let settings = MemorySettings::default();
        settings.set("grid", "a", true.into()).unwrap();
        settings.set("grid", "b", 1.0.into()).unwrap();
        settings.set("axis", "a", false.into()).unwrap();

        assert_eq!(settings.names("grid"), ["a", "b"]);
        assert_eq!(settings.names("axis"), ["a"]);
        assert!(settings.names("missing").is_empty());
    }

    #[test]
    fn remove_only_matches_the_requested_kind() {
        let settings = MemorySettings::default();
        settings.set("grid", "a", true.into()).unwrap();
        settings.remove("grid", "a", SettingKind::Float).unwrap();
        assert_eq!(
            settings.get("grid", "a", SettingKind::Bool),
            Some(SettingValue::Bool(true))
        );
        settings.remove("grid", "a", SettingKind::Bool).unwrap();
        assert_eq!(settings.get("grid", "a", SettingKind::Bool), None);
    }
}
