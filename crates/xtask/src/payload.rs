//! The curated payload descriptor for an edition.
//!
//! An edition that bundles a *chosen set* (the bundle tier of
//! `docs/project/ideation/surface-and-freeform-modelling.md` §7) declares it in
//! `packaging/editions/<edition>.json`. `cargo xtask bundle` reads it and:
//!
//! * stages only the listed `mods/` directories, instead of every workbench the
//!   checkout happens to hold;
//! * copies any extra licence files the bundled add-ons require into `LICENSES/`;
//! * writes `PAYLOAD.json` at the payload root, so an artifact says what it is
//!   (`edition`, title, app script, staged mods, OCCT version) without the
//!   launcher having to guess.
//!
//! An edition with no descriptor stages every mod, which is the behaviour every
//! edition had before this file existed. `bundled_addons` is documentation plus a
//! validation hook: a listed add-on that is not present in `mods/` fails the
//! bundle, so the payload cannot silently ship without what the descriptor claims.
//!
//! The descriptor is deliberately not a general packaging configuration. It names
//! a **set**, not a layout; the layout stays in `bundle`'s code.

use std::path::{Path, PathBuf};

use crate::json::{self, Value};

/// Where an edition's descriptor lives, relative to the workspace root.
pub const DIRECTORY: &str = "packaging/editions";

/// One edition's curated payload.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Descriptor {
    /// The edition name the file declares (must match the file's stem).
    pub edition: String,
    /// The window title, recorded in `PAYLOAD.json`.
    pub title: Option<String>,
    /// The app-script module the shell imports (`HostConfig.entry_module`).
    pub app_script: Option<String>,
    /// The `mods/` directories to stage, in order. Empty means "everything".
    pub mods: Vec<String>,
    /// The third-party add-ons this edition bundles, for validation and licences.
    pub bundled_addons: Vec<Addon>,
    /// Extra licence files to copy into `LICENSES/` (`(source, destination)`).
    pub licenses: Vec<(String, String)>,
    /// Why an add-on is *not* bundled (recorded in `PAYLOAD.json`).
    pub excluded_addons: Vec<Addon>,
}

/// An add-on named by a descriptor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Addon {
    pub name: String,
    /// SPDX-ish licence id, as the bundling review recorded it.
    pub licence: String,
    /// Where it stands: `"bundled"`, `"planned"`, `"excluded: …"`.
    pub status: String,
}

impl Descriptor {
    /// Read `packaging/editions/<edition>.json`, or `None` when the edition has
    /// no descriptor (every edition before the bundle tier did).
    pub fn load(root: &Path, edition: &str) -> Result<Option<Self>, String> {
        let path = root.join(DIRECTORY).join(format!("{edition}.json"));
        if !path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        let value = json::parse(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        Self::from_json(&value, edition).map(Some)
    }

    fn from_json(value: &Value, edition: &str) -> Result<Self, String> {
        let declared = value
            .field("edition")
            .and_then(Value::as_str)
            .ok_or_else(|| "the descriptor has no `edition` string".to_string())?;
        if declared != edition {
            return Err(format!(
                "the descriptor declares edition `{declared}`, but it is {}",
                format!("{DIRECTORY}/{declared}.json")
            ));
        }
        let mods = strings(value.field("mods"))?;
        let licenses = pairs(value.field("licenses"))?;
        Ok(Descriptor {
            edition: declared.to_string(),
            title: text(value.field("title")),
            app_script: text(value.field("app_script")),
            mods,
            bundled_addons: addons(value.field("bundled_addons"))?,
            licenses,
            excluded_addons: addons(value.field("excluded_addons"))?,
        })
    }

    /// The `mods/` directories this edition stages, or every one present.
    ///
    /// A named mod must exist: a descriptor that claims an add-on the payload
    /// does not ship is the failure mode this check exists for.
    pub fn stage_mods(&self, root: &Path) -> Result<Vec<String>, String> {
        let mods_dir = root.join("mods");
        let available = || -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(&mods_dir)
                .map(|entries| {
                    entries
                        .filter_map(|entry| entry.ok())
                        .filter(|entry| entry.path().is_dir())
                        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            names
        };
        if self.mods.is_empty() {
            return Ok(available());
        }
        let present = available();
        for mod_name in &self.mods {
            if !present.iter().any(|name| name == mod_name) {
                return Err(format!(
                    "the {} payload names mod `{mod_name}`, but {}/{} does not exist \
                     (run `cargo xtask mods` first for an upstream workbench)",
                    self.edition,
                    mods_dir.display(),
                    mod_name
                ));
            }
        }
        Ok(self.mods.clone())
    }

    /// The `PAYLOAD.json` body for this edition.
    ///
    /// `staged_mods` is what actually went in, which may differ from `mods` when
    /// the descriptor lists none and every checkout mod is staged.
    pub fn manifest(&self, staged_mods: &[String]) -> String {
        let mut out = String::from("{\n");
        out.push_str(&format!("  \"edition\": {},\n", json::quote(&self.edition)));
        if let Some(title) = &self.title {
            out.push_str(&format!("  \"title\": {},\n", json::quote(title)));
        }
        if let Some(script) = &self.app_script {
            out.push_str(&format!("  \"app_script\": {},\n", json::quote(script)));
        }
        out.push_str("  \"mods\": [");
        for (index, name) in staged_mods.iter().enumerate() {
            if index > 0 {
                out.push_str(", ");
            }
            out.push_str(&json::quote(name));
        }
        out.push_str("],\n");
        out.push_str(&addons_json("bundled_addons", &self.bundled_addons));
        out.push_str(",\n");
        out.push_str(&addons_json("excluded_addons", &self.excluded_addons));
        out.push_str("\n}\n");
        out
    }
}

/// The `PAYLOAD.json` body for an edition without a descriptor (every pre-bundle
/// edition): the mods it staged and nothing it does not know.
pub fn open_manifest(edition: &str, staged_mods: &[String]) -> String {
    let mut out = String::from("{\n");
    out.push_str(&format!("  \"edition\": {},\n", json::quote(edition)));
    out.push_str("  \"mods\": [");
    for (index, name) in staged_mods.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&json::quote(name));
    }
    out.push_str("]\n}\n");
    out
}

fn addons_json(key: &str, addons: &[Addon]) -> String {
    if addons.is_empty() {
        return format!("  \"{key}\": []");
    }
    let mut out = format!("  \"{key}\": [\n");
    for (index, addon) in addons.iter().enumerate() {
        out.push_str(&format!(
            "    {{\"name\": {}, \"licence\": {}, \"status\": {}}}",
            json::quote(&addon.name),
            json::quote(&addon.licence),
            json::quote(&addon.status)
        ));
        out.push_str(if index + 1 == addons.len() { "\n" } else { ",\n" });
    }
    out.push_str("  ]");
    out
}

fn text(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_string)
}

fn strings(value: Option<&Value>) -> Result<Vec<String>, String> {
    match value {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| "expected a string in the array".to_string())
            })
            .collect(),
        Some(_) => Err("expected an array of strings".to_string()),
    }
}

fn pairs(value: Option<&Value>) -> Result<Vec<(String, String)>, String> {
    match value {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                let source = item
                    .field("source")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "a licence entry needs a `source`".to_string())?;
                let destination = item
                    .field("destination")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "a licence entry needs a `destination`".to_string())?;
                Ok((source.to_string(), destination.to_string()))
            })
            .collect(),
        Some(_) => Err("expected an array of licence entries".to_string()),
    }
}

fn addons(value: Option<&Value>) -> Result<Vec<Addon>, String> {
    match value {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                let name = item
                    .field("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "an add-on entry needs a `name`".to_string())?;
                Ok(Addon {
                    name: name.to_string(),
                    licence: text(item.field("licence")).unwrap_or_default(),
                    status: text(item.field("status")).unwrap_or_default(),
                })
            })
            .collect(),
        Some(_) => Err("expected an array of add-on entries".to_string()),
    }
}

/// The descriptor path for `edition`, for messages.
pub fn path_for(root: &Path, edition: &str) -> PathBuf {
    root.join(DIRECTORY).join(format!("{edition}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCULPT: &str = r#"{
  "edition": "sculpt",
  "title": "FerroCAD: Sculpt",
  "app_script": "ferrocad_sculpt",
  "mods": ["AxisOverlay", "GridOverlay"],
  "bundled_addons": [
    {"name": "SubD Modeler", "licence": "MIT", "status": "planned"}
  ],
  "licenses": [
    {"source": "mods/SubD/LICENSE", "destination": "SubD-MIT.txt"}
  ],
  "excluded_addons": [
    {"name": "Silk", "licence": "GPL-3.0-only", "status": "excluded: incompatible with an LGPL edition"}
  ]
}"#;

    #[test]
    fn a_descriptor_reads_its_curated_set() {
        let descriptor =
            Descriptor::from_json(&json::parse(SCULPT).unwrap(), "sculpt").expect("parse");
        assert_eq!(descriptor.title.as_deref(), Some("FerroCAD: Sculpt"));
        assert_eq!(descriptor.app_script.as_deref(), Some("ferrocad_sculpt"));
        assert_eq!(descriptor.mods, vec!["AxisOverlay", "GridOverlay"]);
        assert_eq!(descriptor.bundled_addons[0].name, "SubD Modeler");
        assert_eq!(descriptor.licenses[0].1, "SubD-MIT.txt");
        assert_eq!(descriptor.excluded_addons[0].licence, "GPL-3.0-only");
    }

    #[test]
    fn the_manifest_names_the_staged_set_and_the_review() {
        let descriptor =
            Descriptor::from_json(&json::parse(SCULPT).unwrap(), "sculpt").expect("parse");
        let manifest = descriptor.manifest(&descriptor.mods.clone());
        let parsed = json::parse(&manifest).expect("the manifest is valid JSON");
        assert_eq!(
            parsed.field("edition").and_then(Value::as_str),
            Some("sculpt")
        );
        assert_eq!(parsed.field("mods").and_then(Value::as_array).map(<[Value]>::len), Some(2));
        assert_eq!(
            parsed.field("excluded_addons").and_then(Value::as_array).map(<[Value]>::len),
            Some(1)
        );
    }

    #[test]
    fn a_descriptor_for_another_edition_is_refused() {
        let error = Descriptor::from_json(&json::parse(SCULPT).unwrap(), "parametric")
            .expect_err("the edition name must match");
        assert!(error.contains("declares edition `sculpt`"), "{error}");
    }

    #[test]
    fn a_named_mod_must_exist_in_the_checkout() {
        let root = std::env::temp_dir().join(format!("ferrocad-payload-{}", std::process::id()));
        let mods = root.join("mods");
        std::fs::create_dir_all(mods.join("AxisOverlay")).unwrap();
        let descriptor =
            Descriptor::from_json(&json::parse(SCULPT).unwrap(), "sculpt").expect("parse");
        let error = descriptor
            .stage_mods(&root)
            .expect_err("GridOverlay is not present");
        assert!(error.contains("GridOverlay"), "{error}");

        std::fs::create_dir_all(mods.join("GridOverlay")).unwrap();
        assert_eq!(
            descriptor.stage_mods(&root).unwrap(),
            vec!["AxisOverlay", "GridOverlay"]
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn no_descriptor_means_every_mod() {
        let root = std::env::temp_dir().join(format!("ferrocad-payload-open-{}", std::process::id()));
        std::fs::create_dir_all(root.join("mods").join("B")).unwrap();
        std::fs::create_dir_all(root.join("mods").join("A")).unwrap();
        let descriptor = Descriptor {
            edition: "parametric".to_string(),
            ..Descriptor::default()
        };
        assert_eq!(descriptor.stage_mods(&root).unwrap(), vec!["A", "B"]);
        let manifest = json::parse(&open_manifest(
            "parametric",
            &["A".to_string(), "B".to_string()],
        ))
        .unwrap();
        assert_eq!(manifest.field("edition").and_then(Value::as_str), Some("parametric"));
        std::fs::remove_dir_all(&root).ok();
    }
}
