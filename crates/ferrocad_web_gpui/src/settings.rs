//! The browser's settings backend: `localStorage`, with an in-memory fallback.
//!
//! The page has no filesystem, so the desktop's `FreeCAD.ParamGet` file backend cannot
//! be reused directly. The keys, groups and typed values are the same, though: each
//! group is one JSON object under `ferrocad.settings.<group>`, and the native grid
//! settings card reads and writes through this store. Storage can be unavailable
//! (private mode, a denied origin); the fallback keeps the session working and the
//! shared store trait means the rest of the shell does not care.

use std::collections::BTreeMap;

use ferrocad_shell_api::settings::{MemorySettings, SettingKind, SettingValue, SettingsBackend};
use js_sys::{Function, Reflect};
use wasm_bindgen::{JsCast, JsValue};

/// The browser store: `localStorage` first, memory when it is unavailable.
pub struct WebSettings {
    fallback: MemorySettings,
}

impl Default for WebSettings {
    fn default() -> Self {
        Self {
            fallback: MemorySettings::default(),
        }
    }
}

impl WebSettings {
    fn key(group: &str) -> String {
        format!("ferrocad.settings.{group}")
    }

    fn storage() -> Option<JsValue> {
        let global = js_sys::global();
        let storage = Reflect::get(&global, &JsValue::from_str("localStorage")).ok()?;
        (!storage.is_null() && !storage.is_undefined()).then_some(storage)
    }

    fn get_item(storage: &JsValue, key: &str) -> Option<String> {
        let function = Reflect::get(storage, &JsValue::from_str("getItem")).ok()?;
        let function: Function = function.dyn_into().ok()?;
        function
            .call1(storage, &JsValue::from_str(key))
            .ok()?
            .as_string()
    }

    fn set_item(storage: &JsValue, key: &str, value: &str) -> bool {
        let Ok(function) = Reflect::get(storage, &JsValue::from_str("setItem")) else {
            return false;
        };
        let Ok(function) = function.dyn_into::<Function>() else {
            return false;
        };
        function
            .call2(storage, &JsValue::from_str(key), &JsValue::from_str(value))
            .is_ok()
    }

    fn load(&self, group: &str) -> Option<BTreeMap<String, SettingValue>> {
        let storage = Self::storage()?;
        let raw = Self::get_item(&storage, &Self::key(group))?;
        serde_json::from_str(&raw).ok()
    }

    fn save(&self, group: &str, values: &BTreeMap<String, SettingValue>) -> bool {
        let Some(storage) = Self::storage() else {
            return false;
        };
        let Ok(raw) = serde_json::to_string(values) else {
            return false;
        };
        Self::set_item(&storage, &Self::key(group), &raw)
    }
}

impl SettingsBackend for WebSettings {
    fn get(&self, group: &str, name: &str, kind: SettingKind) -> Option<SettingValue> {
        self.load(group)
            .and_then(|values| {
                values
                    .get(name)
                    .filter(|value| value.kind() == kind)
                    .cloned()
            })
            .or_else(|| self.fallback.get(group, name, kind))
    }

    fn set(&self, group: &str, name: &str, value: SettingValue) -> Result<(), String> {
        let mut values = self.load(group).unwrap_or_default();
        values.insert(name.to_string(), value.clone());
        self.save(group, &values);
        // Keep the session readable even when storage is denied.
        self.fallback.set(group, name, value)
    }

    fn remove(&self, group: &str, name: &str, kind: SettingKind) -> Result<(), String> {
        if let Some(mut values) = self.load(group) {
            if values.get(name).is_some_and(|value| value.kind() == kind) {
                values.remove(name);
                self.save(group, &values);
            }
        }
        self.fallback.remove(group, name, kind)
    }

    fn names(&self, group: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .load(group)
            .map(|values| values.into_keys().collect())
            .unwrap_or_default();
        names.extend(self.fallback.names(group));
        names.sort();
        names.dedup();
        names
    }

    fn groups(&self) -> Vec<String> {
        let mut groups = self.fallback.groups();
        if let Some(storage) = Self::storage() {
            let length = Reflect::get(&storage, &JsValue::from_str("length"))
                .ok()
                .and_then(|value| value.as_f64())
                .unwrap_or(0.0) as u32;
            let key = Reflect::get(&storage, &JsValue::from_str("key"))
                .ok()
                .and_then(|function| function.dyn_into::<Function>().ok());
            if let Some(key) = key {
                for index in 0..length {
                    let name = key
                        .call1(&storage, &JsValue::from_f64(index as f64))
                        .ok()
                        .and_then(|value| value.as_string());
                    if let Some(group) = name
                        .as_deref()
                        .and_then(|name| name.strip_prefix("ferrocad.settings."))
                    {
                        groups.push(group.to_string());
                    }
                }
            }
        }
        groups.sort();
        groups.dedup();
        groups
    }
}
