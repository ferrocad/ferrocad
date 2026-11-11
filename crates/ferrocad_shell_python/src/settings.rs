//! The desktop's settings backend: the FreeCAD parameter store.
//!
//! The values and group names are the same ones `FreeCAD.ParamGet` already uses
//! (`Mods/grid/ferrocad`, `enabled`, `minor`, …), so `FerroCAD.Grid`, the
//! `GridOverlay_Settings` dialog and the native settings card read and write one store.
//! The bridge is deliberately small: four typed getters, four setters and the group's
//! name lists.

use ferrocad_shell_api::settings::{SettingKind, SettingValue, SettingsBackend};
use pyo3::prelude::*;

/// The FreeCAD parameter store, addressed exactly as `FerroCAD._grid` addresses it.
#[derive(Debug, Default, Clone, Copy)]
pub struct PythonSettings;

impl PythonSettings {
    fn group_path(group: &str) -> String {
        format!("User parameter:BaseApp/Preferences/{group}")
    }

    fn import(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
        Ok(py.import("FreeCAD")?.into_any())
    }

    fn group<'py>(py: Python<'py>, group: &str) -> PyResult<Bound<'py, PyAny>> {
        Self::import(py)?.call_method1("ParamGet", (Self::group_path(group),))
    }

    fn names_method(kind: SettingKind) -> &'static str {
        match kind {
            SettingKind::Bool => "GetBools",
            SettingKind::Int => "GetInts",
            SettingKind::Float => "GetFloats",
            SettingKind::String => "GetStrings",
        }
    }

    fn remove_method(kind: SettingKind) -> &'static str {
        match kind {
            SettingKind::Bool => "RemBool",
            SettingKind::Int => "RemInt",
            SettingKind::Float => "RemFloat",
            SettingKind::String => "RemString",
        }
    }
}

impl SettingsBackend for PythonSettings {
    fn get(&self, group: &str, name: &str, kind: SettingKind) -> Option<SettingValue> {
        Python::with_gil(|py| -> Option<SettingValue> {
            let grp = Self::group(py, group).ok()?;
            let names: Vec<String> = grp
                .call_method0(Self::names_method(kind))
                .ok()?
                .extract()
                .ok()?;
            if !names.iter().any(|candidate| candidate == name) {
                return None;
            }
            match kind {
                SettingKind::Bool => Some(SettingValue::Bool(
                    grp.call_method1("GetBool", (name,)).ok()?.extract().ok()?,
                )),
                SettingKind::Int => Some(SettingValue::Int(
                    grp.call_method1("GetInt", (name,)).ok()?.extract().ok()?,
                )),
                SettingKind::Float => Some(SettingValue::Float(
                    grp.call_method1("GetFloat", (name,)).ok()?.extract().ok()?,
                )),
                SettingKind::String => Some(SettingValue::String(
                    grp.call_method1("GetString", (name,))
                        .ok()?
                        .extract()
                        .ok()?,
                )),
            }
        })
    }

    fn set(&self, group: &str, name: &str, value: SettingValue) -> Result<(), String> {
        Python::with_gil(|py| -> PyResult<()> {
            let grp = Self::group(py, group)?;
            match value {
                SettingValue::Bool(value) => {
                    grp.call_method1("SetBool", (name, value))?;
                }
                SettingValue::Int(value) => {
                    grp.call_method1("SetInt", (name, value))?;
                }
                SettingValue::Float(value) => {
                    grp.call_method1("SetFloat", (name, value))?;
                }
                SettingValue::String(value) => {
                    grp.call_method1("SetString", (name, value))?;
                }
            }
            Ok(())
        })
        .map_err(|error| error.to_string())
    }

    fn remove(&self, group: &str, name: &str, kind: SettingKind) -> Result<(), String> {
        Python::with_gil(|py| -> PyResult<()> {
            let grp = Self::group(py, group)?;
            grp.call_method1(Self::remove_method(kind), (name,))?;
            Ok(())
        })
        .map_err(|error| error.to_string())
    }

    fn names(&self, group: &str) -> Vec<String> {
        Python::with_gil(|py| -> Option<Vec<String>> {
            let grp = Self::group(py, group).ok()?;
            let mut names = Vec::new();
            for method in ["GetBools", "GetInts", "GetFloats", "GetStrings"] {
                let mut entries: Vec<String> = grp.call_method0(method).ok()?.extract().ok()?;
                names.append(&mut entries);
            }
            names.sort();
            names.dedup();
            Some(names)
        })
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_python_store_round_trips_typed_values() {
        let _guard = crate::PYTHON_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let python_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../python");
        Python::with_gil(|py| {
            let sys = py.import("sys").expect("Python has sys");
            sys.getattr("path")
                .expect("Python has a path")
                .call_method1("insert", (0, python_dir.to_string_lossy().to_string()))
                .expect("the development python/ directory joins sys.path");
        });
        let settings = PythonSettings;
        let group = "Mods/grid/ferrocad-test";

        settings
            .set(group, "enabled", SettingValue::Bool(true))
            .expect("the parameter store accepts a bool");
        settings
            .set(group, "minor", SettingValue::Float(20.0))
            .expect("the parameter store accepts a float");
        settings
            .set(group, "color", SettingValue::String("#94a3b8".to_string()))
            .expect("the parameter store accepts a string");

        assert_eq!(
            settings.get(group, "enabled", SettingKind::Bool),
            Some(SettingValue::Bool(true))
        );
        assert_eq!(
            settings.get(group, "minor", SettingKind::Float),
            Some(SettingValue::Float(20.0))
        );
        assert_eq!(
            settings.get(group, "color", SettingKind::String),
            Some(SettingValue::String("#94a3b8".to_string()))
        );
        assert_eq!(settings.names(group), ["color", "enabled", "minor"]);

        settings
            .remove(group, "enabled", SettingKind::Bool)
            .expect("the parameter store accepts a remove");
        assert_eq!(settings.get(group, "enabled", SettingKind::Bool), None);
    }
}
