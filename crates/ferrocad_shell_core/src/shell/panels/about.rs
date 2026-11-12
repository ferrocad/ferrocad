//! The About card: what this build is running.
//!
//! The window's only honest answer to "what did this build load". It is a **card, not a
//! backend namespace**: every row is synthesised from state the shell already holds —
//! the boot's [`Composition`](crate::types::Composition), the workbench report from
//! `load_workbenches`, and the provider registry — so no mod, no script half and no
//! backend has to answer a `Settings/About` property question. It renders through the
//! settings card's row list ([`crate::shell::panels::properties`]), which is why it gets
//! scrolling, dragging and `Esc` without a second surface.
//!
//! Provider and mod controls are interactive (B17a): a provider row is a **command row**
//! (`ferrocad-provider:<feature>:<backend>`) and a preference row is one too
//! (`ferrocad-prefs:mod:<name>` / `ferrocad-prefs:provider:<feature>:<backend>`), so the
//! card is the runtime control as well as the inventory. Providers are live; a mod
//! change is a refusal the next boot reads, so the card says so.

use gpui::App;

use crate::shell::Shell;
use crate::types::PropRow;

/// The synthetic node the About card shows.
///
/// `Settings/` keeps it in the settings card's family (the title, the overlay, the
/// dismissal), but it is **not** a backend property namespace: [`rows`] builds it.
pub(in crate::shell) const ABOUT: &str = "Settings/About";

/// The synthetic command id the app menu's `About FerroCAD` row runs.
///
/// Intercepted by [`Shell::run_command`](crate::shell::Shell) like the other generated
/// chrome (`select:`, `ferrocad-provider:`): it is not a registry command, so no
/// workbench, no Python registry entry and no `catalog.json` row is involved.
pub(crate) const OPEN_ID: &str = "ferrocad-about";

/// A plain read-only row: a key and its value.
fn value_row(name: &str, value: impl Into<String>) -> PropRow {
    PropRow {
        name: name.to_string(),
        label: String::new(),
        type_id: String::new(),
        value: value.into(),
        status: String::new(),
        editable: false,
        command: None,
        kind: "text".to_string(),
        min: None,
        max: None,
        unit: None,
        choices: Vec::new(),
    }
}

/// A row with a status tag (`active`, `failed`), rendered as `value [status]`.
fn tagged_row(name: &str, value: impl Into<String>, status: &str) -> PropRow {
    PropRow {
        status: status.to_string(),
        ..value_row(name, value)
    }
}

/// A row that runs a generated command when clicked (the provider switch).
fn command_row(name: &str, label: impl Into<String>, command: impl Into<String>) -> PropRow {
    PropRow {
        label: label.into(),
        command: Some(command.into()),
        ..value_row(name, "")
    }
}

/// The About card's rows, in display order.
///
/// Reads the boot's report rather than asking the backend again: the card answers
/// "what is loaded", which is a fact about boot, not a live query.
pub(in crate::shell) fn rows(shell: &Shell, cx: &App) -> Vec<PropRow> {
    let model = shell.snapshot();
    let composition = &model.composition;
    let mut rows = Vec::new();

    rows.push(value_row(
        "Application",
        if composition.app.is_empty() {
            "FerroCAD".to_string()
        } else {
            composition.app.clone()
        },
    ));
    rows.push(value_row(
        "Version",
        format!("{} · {} backend", model.version, model.backend),
    ));
    rows.push(value_row(
        "Platform",
        if composition.platform.is_empty() {
            "not reported".to_string()
        } else {
            composition.platform.clone()
        },
    ));
    // The engine's own line names the interpreter that answered, which is the number
    // that has to agree with the halves list below.
    rows.push(value_row(
        "Engine",
        crate::app::hello(cx).unwrap_or_else(|error| format!("unavailable: {error}")),
    ));
    rows.push(value_row(
        "Script halves",
        if composition.runtimes.is_empty() {
            "none — the document half alone".to_string()
        } else {
            composition.runtimes.join(", ")
        },
    ));
    rows.push(value_row(
        "Addons",
        if composition.addons.is_empty() {
            "none".to_string()
        } else {
            composition.addons.join(", ")
        },
    ));

    let loaded = &model.workbenches.loaded;
    rows.push(tagged_row(
        "Mods",
        if loaded.is_empty() {
            "none loaded".to_string()
        } else {
            format!("{} script(s) loaded", loaded.len())
        },
        if model.workbenches.errors.is_empty() {
            ""
        } else {
            "with failures"
        },
    ));
    for script in loaded {
        rows.push(value_row(script, String::new()));
    }
    for error in &model.workbenches.errors {
        rows.push(tagged_row(
            &format!("{}/{}", error.workbench, error.script),
            "did not load",
            "failed",
        ));
    }

    // Preferences (B17a): where a change survives, and what this boot could see.
    let prefs = &model.prefs;
    rows.push(value_row(
        "Preferences",
        if prefs.store.is_empty() {
            "session only".to_string()
        } else {
            prefs.store.clone()
        },
    ));
    if prefs.mods.is_empty() {
        // An explicit `--addons`/`?addons=` set is this run's choice, not a stored one;
        // nothing to offer until a host names the mods it could load.
        rows.push(value_row(
            "Change the set",
            "halves and bundles are chosen at launch (`--addons`, `?addons=`)",
        ));
    } else {
        for name in &prefs.mods {
            let enabled = prefs.prefs.mod_enabled(name);
            rows.push(command_row(
                &format!("mod · {name}"),
                if enabled { "disable" } else { "enable" },
                format!("ferrocad-prefs:mod:{name}"),
            ));
        }
        rows.push(value_row("Mod changes", "apply at the next start"));
    }

    // Providers: one row per backend. The active one is tagged; the others name the
    // switch they would run, so this list is also the runtime control. The row carries the
    // feature name because two features can offer providers with the same label. A refused
    // backend stays listed — this is where it is turned back on — while everything else
    // hides it.
    for feature in shell.provider_features() {
        for provider in &feature.providers {
            let label = if provider.label.is_empty() {
                provider.backend.clone()
            } else {
                provider.label.clone()
            };
            let name = format!("{} · {label}", feature.feature);
            let offered = prefs
                .prefs
                .provider_enabled(&feature.feature, &provider.backend);
            let toggle = format!(
                "ferrocad-prefs:provider:{}:{}",
                feature.feature, provider.backend
            );
            if !offered {
                rows.push(command_row(&name, "show", toggle));
            } else if provider.active {
                // Hiding the backend that is drawing would leave the feature on nothing;
                // `toggle_provider` refuses it, so the row does not offer it.
                rows.push(tagged_row(&name, String::new(), "active"));
            } else {
                rows.push(command_row(
                    &name,
                    "activate",
                    format!("ferrocad-provider:{}:{}", feature.feature, provider.backend),
                ));
                rows.push(command_row(&name, "hide", toggle));
            }
        }
    }

    rows
}
