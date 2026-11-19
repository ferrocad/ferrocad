//! B17a: the About card's preferences controls.
//!
//! The card can turn two things off, and they apply at different times:
//!
//! - **Providers** are live. A disabled backend disappears from the header popovers and
//!   the Dev Tools list, and activating it is refused, in the same run — the registry is a
//!   runtime object, so nothing has to restart.
//! - **Mods** are the next boot. Their set is resolved by `mod_roots` and loaded by
//!   `load_workbenches` before the backend exists, so a mod change is written to the
//!   host's store and the card says so.
//!
//! Both are written as **refusals** (`Prefs::disabled_*`): absence means enabled, so a
//! store from another build cannot switch on something this one does not know about.

use gpui::Context;

use crate::Shell;
use crate::types::{GridProvider, Prefs, ProviderFeature};

/// The provider registry with the user's refusals applied.
///
/// A feature whose backends are all refused disappears rather than showing as an empty
/// group: the popover then offers no switch for it, which is what "hidden" means.
pub(crate) fn visible_features(
    features: &[ProviderFeature],
    prefs: &Prefs,
) -> Vec<ProviderFeature> {
    features
        .iter()
        .map(|feature| ProviderFeature {
            feature: feature.feature.clone(),
            providers: feature
                .providers
                .iter()
                .filter(|provider| prefs.provider_enabled(&feature.feature, &provider.backend))
                .cloned()
                .collect(),
        })
        .filter(|feature| !feature.providers.is_empty())
        .collect()
}

/// One summary's provider rows with the user's refusals applied.
pub(crate) fn visible_providers(
    feature: &str,
    providers: &[GridProvider],
    prefs: &Prefs,
) -> Vec<GridProvider> {
    providers
        .iter()
        .filter(|provider| prefs.provider_enabled(feature, &provider.backend))
        .cloned()
        .collect()
}

impl Shell {
    /// The provider registry as the window should *offer* it.
    ///
    /// The About card is the exception: it lists the refused backends too, because that is
    /// where they are turned back on. Everything else (the header popovers, the Dev Tools
    /// panel, the losing-provider command filter) sees only the visible set.
    pub(crate) fn visible_provider_features(&self) -> Vec<ProviderFeature> {
        let prefs = self.model.lock().unwrap().prefs.prefs.clone();
        visible_features(self.provider_features(), &prefs)
    }

    /// Tell the card which mods exist (the browser learns this from the fetched index).
    ///
    /// The desktop host installs the list with its `PrefsReport`; a page has no filesystem
    /// and fetches the index after the first frame, so the set it can offer arrives here.
    pub fn note_prefs_mods(&mut self, mods: Vec<String>, cx: &mut Context<Self>) {
        self.model.lock().unwrap().prefs.mods = mods;
        self.refresh_settings_card(cx);
        cx.notify();
    }

    /// Flip one preference row (`ferrocad-prefs:mod:<name>` / `ferrocad-prefs:provider:<f>:<b>`).
    ///
    /// The session state changes first and is persisted afterwards: a store that cannot
    /// write reports it in the status line instead of pretending the click did nothing.
    pub(in crate::shell) fn toggle_pref(&mut self, rest: &str, cx: &mut Context<Self>) {
        let status = if let Some(name) = rest.strip_prefix("mod:") {
            Some(self.toggle_mod(name))
        } else if let Some(key) = rest.strip_prefix("provider:") {
            key.split_once(':')
                .map(|(feature, backend)| self.toggle_provider(feature, backend))
        } else {
            None
        };
        let Some(status) = status else {
            self.console_line(format!("prefs: unknown row {rest:?}"), cx);
            return;
        };

        let prefs = self.model.lock().unwrap().prefs.prefs.clone();
        let status = match crate::app::persist_prefs(cx, &prefs) {
            Ok(()) => status,
            Err(error) => format!("{status}; not saved: {error}"),
        };
        self.model.lock().unwrap().note_status(status.clone());
        self.console_line(status, cx);
        // The rows this click changed are the card's own.
        self.refresh_settings_card(cx);
        cx.notify();
    }

    /// Turn a mod on or off for the **next** boot.
    fn toggle_mod(&mut self, name: &str) -> String {
        let enabled = {
            let mut model = self.model.lock().unwrap();
            let enabled = !model.prefs.prefs.mod_enabled(name);
            model.prefs.prefs.set_mod_enabled(name, enabled);
            enabled
        };
        if enabled {
            format!("mods: {name} loads again after a restart")
        } else {
            format!("mods: {name} stays off after a restart")
        }
    }

    /// Turn a provider backend on or off, live.
    ///
    /// Hiding the backend that is currently drawing would leave the feature on nothing, so
    /// that one refusal is refused: switch away first, then hide it.
    fn toggle_provider(&mut self, feature: &str, backend: &str) -> String {
        let active = self
            .provider_features()
            .iter()
            .filter(|candidate| candidate.feature == feature)
            .flat_map(|candidate| &candidate.providers)
            .any(|provider| provider.backend == backend && provider.active);
        let enabled = {
            let mut model = self.model.lock().unwrap();
            let enabled = !model.prefs.prefs.provider_enabled(feature, backend);
            if !enabled && active {
                return format!("{feature}: switch away from {backend} before hiding it");
            }
            model
                .prefs
                .prefs
                .set_provider_enabled(feature, backend, enabled);
            enabled
        };
        if enabled {
            format!("{feature}: {backend} is offered again")
        } else {
            format!("{feature}: {backend} hidden")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feature(name: &str, backends: &[(&str, bool)]) -> ProviderFeature {
        ProviderFeature {
            feature: name.to_string(),
            providers: backends
                .iter()
                .map(|(backend, active)| GridProvider {
                    backend: backend.to_string(),
                    label: backend.to_string(),
                    active: *active,
                    command: None,
                })
                .collect(),
        }
    }

    /// A refused backend disappears from the offered list; a feature with none left
    /// disappears with it, rather than showing as an empty group.
    #[test]
    fn a_refused_backend_is_not_offered() {
        let features = vec![
            feature("grid", &[("ferrocad", true), ("coin", false)]),
            feature("gizmo", &[("ferrocad-shader", false)]),
        ];
        let mut prefs = Prefs::default();
        prefs.set_provider_enabled("grid", "coin", false);
        prefs.set_provider_enabled("gizmo", "ferrocad-shader", false);

        let visible = visible_features(&features, &prefs);
        assert_eq!(visible.len(), 1, "the empty gizmo group is dropped");
        assert_eq!(visible[0].feature, "grid");
        assert_eq!(
            visible[0]
                .providers
                .iter()
                .map(|provider| provider.backend.as_str())
                .collect::<Vec<_>>(),
            ["ferrocad"]
        );
    }

    /// The same rule for the Grid/Axis summaries, which carry their own provider lists.
    #[test]
    fn a_summary_hides_a_refused_backend() {
        let providers = [
            GridProvider {
                backend: "ferrocad".to_string(),
                ..Default::default()
            },
            GridProvider {
                backend: "coin".to_string(),
                ..Default::default()
            },
        ];
        let mut prefs = Prefs::default();
        prefs.set_provider_enabled("grid", "coin", false);
        let visible = visible_providers("grid", &providers, &prefs);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].backend, "ferrocad");
    }
}
