//! The pack index the browser fetches (B19).
//!
//! `tools/mods_pack.py` writes `www/mods/index.json` beside one JSON pack per workbench
//! (`{"name", "version", "files"}`). The page has no filesystem, so the index is the one
//! list a browser can read: it names the packs, and the host fetches each of them and
//! hands it to the script half.
//!
//! Only two things live here, deliberately: parsing, and the status lines. The fetch loop
//! is the host's (`ferrocad_web_gpui`), because it needs the window and the async
//! executor; what a capture can assert on is the index it accepted and the text it showed.
//!
//! ```text
//! {"version": 1, "mods": [{"name": "GridOverlay", "pack": "GridOverlay.json"}]}
//! ```

use ferrocad_shell_api::prefs::Prefs;
use serde::Deserialize;

/// `www/mods/index.json`, as `tools/mods_pack.py` writes it.
///
/// `version` is a compatibility marker between packer and reader; the entries are the
/// whole contract, so a future packer may bump it and an older page still reads the
/// packs it knows. It is a **separate** version from a pack's own, because a reader may
/// support one shape for each.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ModIndex {
    #[serde(default)]
    pub version: u32,
    /// The packs, in load order. This is the order the packer sorted them in (name
    /// order), and it is the order the host fetches and runs them.
    pub mods: Vec<ModEntry>,
}

/// One pack in the index.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ModEntry {
    /// The workbench name, for the status line and the report.
    pub name: String,
    /// The pack's URL, relative to the index (the packer writes `<Name>.json`). An
    /// absolute URL is allowed: an index on a CDN may name packs elsewhere.
    pub pack: String,
}

/// Parse an index, or say what is wrong with it.
///
/// An index the packer cannot produce is an `Err`: it is a build artifact, so a shape it
/// does not write is a bug in the delivery (a hand-edited index, a proxy's error page),
/// and loading a subset of it silently would hide that. A *pack* that fails to fetch or
/// a script that raises is a different matter — those are recorded and the batch goes
/// on.
pub fn parse_index(text: &str) -> Result<ModIndex, String> {
    let index: ModIndex =
        serde_json::from_str(text).map_err(|error| format!("mods index: {error}"))?;
    let mut seen: Vec<&str> = Vec::new();
    for entry in &index.mods {
        if entry.name.is_empty() {
            return Err("mods index: an entry has no name".to_string());
        }
        if entry.pack.is_empty() {
            return Err(format!("mods index: `{}` has no pack", entry.name));
        }
        if seen.contains(&entry.name.as_str()) {
            return Err(format!("mods index: `{}` is listed twice", entry.name));
        }
        seen.push(&entry.name);
    }
    Ok(index)
}

/// The packs this page should fetch: the index minus the user's refusals (B17a).
///
/// A pack the user turned off in the About card is not skipped *after* the fetch — it is
/// never asked for, which is the point of a refusal that survives a reload. The returned
/// entries keep the index's order, which is the packer's name order.
pub fn selected<'a>(mods: &'a [ModEntry], prefs: &Prefs) -> Vec<&'a ModEntry> {
    mods.iter()
        .filter(|entry| prefs.mod_enabled(&entry.name))
        .collect()
}

/// `Loading mods 3/7 · Fasteners` — what the status line says while a pack runs.
///
/// `position` is 1-based: it counts the pack being loaded, so the last one reads
/// `7/7`.
pub fn loading_status(position: usize, total: usize, name: &str) -> String {
    format!("Loading mods {position}/{total} · {name}")
}

/// What the status line says once the batch is done.
///
/// The unit is a **script**: a pack with a working `Init.py` and a failing `InitGui.py`
/// is one loaded and one failed, which is what the report records and what the About
/// card lists. A pack that could not be fetched at all counts as one failure.
pub fn done_status(loaded: usize, failed: usize) -> String {
    match (loaded, failed) {
        (0, 0) => "mods: none".to_string(),
        (loaded, 0) => format!("mods: {loaded} loaded"),
        (0, failed) => format!("mods: 0 loaded, {failed} failed"),
        (loaded, failed) => format!("mods: {loaded} loaded, {failed} failed"),
    }
}

/// Whether a `?mods=` value switches the fetch off.
///
/// `?mods=0`/`off`/`none` is a page that wants the window without a pack fetch; any
/// other value is the index URL (the bare `?mods=1` meaning [`DEFAULT_INDEX`]).
pub fn is_off(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "0" | "off" | "no" | "none" | "false"
    )
}

/// The index a bare `?mods=` (or an absent `?mods=`) fetches.
pub const DEFAULT_INDEX: &str = "mods/index.json";

/// The index URL for a `?mods=` value: the default for an "on" value, else the value.
pub fn index_url(value: &str) -> &str {
    match value.trim() {
        "" | "1" | "on" | "yes" | "true" => DEFAULT_INDEX,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The packer's shape, in its order.
    #[test]
    fn the_packers_index_parses_in_load_order() {
        let index = parse_index(
            r#"{
                "version": 1,
                "mods": [
                    {"name": "AxisOverlay", "pack": "AxisOverlay.json"},
                    {"name": "GridOverlay", "pack": "GridOverlay.json"}
                ]
            }"#,
        )
        .expect("the packer's shape");
        assert_eq!(index.version, 1);
        assert_eq!(
            index
                .mods
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["AxisOverlay", "GridOverlay"]
        );
        assert_eq!(index.mods[1].pack, "GridOverlay.json");
    }

    /// The packer also names JS workbench modules in the index; the Python loader reads
    /// the `mods` list and leaves the new key to the page.
    #[test]
    fn a_js_list_does_not_disturb_the_pack_list() {
        let index = parse_index(
            r#"{
                "version": 1,
                "mods": [{"name": "AxisOverlay", "pack": "AxisOverlay.json"}],
                "js": [{"name": "AxisOverlayJs", "module": "AxisOverlayJs.js"}]
            }"#,
        )
        .expect("an index with JS workbenches");
        assert_eq!(index.mods.len(), 1);
        assert_eq!(index.mods[0].name, "AxisOverlay");
    }

    /// An empty list is a legitimate index (a checkout with no authored mods), not a
    /// malformed one.
    #[test]
    fn an_empty_index_is_valid_and_loads_nothing() {
        let index = parse_index(r#"{"version": 1, "mods": []}"#).expect("no packs");
        assert!(index.mods.is_empty());
        assert_eq!(done_status(0, 0), "mods: none");
    }

    /// Every shape the packer cannot write is refused with the reason, because the index
    /// is a build artifact: a subset loaded silently would hide a broken delivery.
    #[test]
    fn a_malformed_index_names_the_problem() {
        let error = parse_index("<!doctype html>").unwrap_err();
        assert!(error.starts_with("mods index: "), "{error}");

        let error = parse_index(r#"{"version": 1}"#).unwrap_err();
        assert!(error.contains("mods"), "the missing list is named: {error}");

        let error = parse_index(r#"{"version": 1, "mods": [{"pack": "A.json"}]}"#).unwrap_err();
        assert!(error.contains("name"), "{error}");

        let error = parse_index(r#"{"version": 1, "mods": [{"name": "A"}]}"#).unwrap_err();
        assert!(error.contains("pack"), "{error}");

        let error = parse_index(
            r#"{"version": 1, "mods": [{"name": "A", "pack": "a.json"}, {"name": "A", "pack": "b.json"}]}"#,
        )
        .unwrap_err();
        assert!(error.contains("twice"), "{error}");
    }

    /// The switch: a bare/on value is the default index, an off value never gets here,
    /// and anything else is the URL the page named.
    #[test]
    fn the_mods_switch_names_the_index() {
        assert_eq!(index_url("1"), DEFAULT_INDEX);
        assert_eq!(index_url("true"), DEFAULT_INDEX);
        assert_eq!(index_url("mods/index.json"), "mods/index.json");
        assert_eq!(
            index_url("https://cdn.example/mods.json"),
            "https://cdn.example/mods.json"
        );
        for off in ["0", "off", "NONE", " no ", "false"] {
            assert!(is_off(off), "{off:?} switches the fetch off");
        }
        assert!(!is_off("1"));
    }

    /// The status lines read as one sentence, and the failing count never goes negative.
    #[test]
    fn the_status_lines_name_the_progress() {
        assert_eq!(
            loading_status(3, 7, "Fasteners"),
            "Loading mods 3/7 · Fasteners"
        );
        assert_eq!(done_status(12, 0), "mods: 12 loaded");
        assert_eq!(done_status(6, 6), "mods: 6 loaded, 6 failed");
        assert_eq!(done_status(0, 1), "mods: 0 loaded, 1 failed");
        assert_eq!(done_status(0, 0), "mods: none");
    }

    /// B17a: a refused pack is never fetched, and the rest keep the index's order.
    #[test]
    fn a_refused_pack_is_not_selected() {
        let index = parse_index(
            r#"{"version": 1, "mods": [
                {"name": "AxisOverlay", "pack": "AxisOverlay.json"},
                {"name": "GridOverlay", "pack": "GridOverlay.json"},
                {"name": "Spring", "pack": "Spring.json"}
            ]}"#,
        )
        .expect("the packer's shape");
        let mut prefs = Prefs::default();
        prefs.set_mod_enabled("GridOverlay", false);

        let picked = selected(&index.mods, &prefs);
        assert_eq!(
            picked
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["AxisOverlay", "Spring"]
        );
        assert_eq!(selected(&index.mods, &Prefs::default()).len(), 3);
    }
}
