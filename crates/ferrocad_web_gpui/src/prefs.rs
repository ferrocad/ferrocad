//! B17a: the page's preferences store.
//!
//! The browser has no filesystem, so `localStorage` is the equivalent of the desktop's
//! `prefs.json`: one JSON value under a stable key, read **before** the window boots
//! (because it decides which packs are fetched) and written whenever the About card
//! changes a refusal.
//!
//! `localStorage` can be unavailable (a sandboxed iframe, a browser with storage
//! disabled). That is not fatal: `load` answers the defaults and `save` reports why, so the
//! session still honours the change and the card says it did not persist.

use ferrocad_shell_api::prefs::{Prefs, PrefsStore};

/// The key the page writes. Stable, so a later build reads the same list.
const KEY: &str = "ferrocad.prefs";

/// The page's store.
pub struct LocalStoragePrefs;

impl PrefsStore for LocalStoragePrefs {
    fn load(&self) -> Prefs {
        storage()
            .and_then(|storage| storage.get_item(KEY).ok().flatten())
            .map(|text| Prefs::parse(&text))
            .unwrap_or_default()
    }

    fn save(&self, prefs: &Prefs) -> Result<(), String> {
        let storage =
            storage().ok_or_else(|| "localStorage is unavailable in this page".to_string())?;
        storage
            .set_item(KEY, &prefs.to_json())
            .map_err(|error| format!("localStorage: {error:?}"))
    }

    fn location(&self) -> String {
        format!("localStorage · {KEY}")
    }
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}
