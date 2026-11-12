//! B19c: fetch the mod packs and hand them to the script half.
//!
//! The page has no filesystem, so `load_workbenches` cannot run in a browser. It can
//! `fetch`: `tools/mods_pack.py` writes one JSON pack per `mods/<Name>/` plus an index,
//! and this module walks that index **after the window is up**, handing each pack to
//! `ferrocad_shell_core::app::load_workbench_pack`.
//!
//! Every step awaits (a fetch, or the interpreter), so the first frame paints before the
//! first pack runs — delivery must not delay the window. A pack that cannot be fetched
//! or loaded is reported and skipped: one broken mod must not cost the batch, and the
//! loader inside the script half already isolates a *script* that raises.
//!
//! The index is read only when a Python half is installed: a `?runtime=plain` or `?js`
//! page has nothing that could run an `Init.py`/`InitGui.py` pack, and asking anyway
//! would report an empty batch as if it had loaded.

use ferrocad_shell_core::Shell;
use ferrocad_shell_web::mods;
use gpui::{AsyncApp, WeakEntity};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Response, Url};

/// Fetch `index_url` (resolved against the page) and load every pack it names.
///
/// `index_url` is the page's choice (`?mods=`); a value that [`mods::is_off`] accepts
/// never reaches here. The task is detached — it is a background of the window, not
/// something a caller waits on.
pub fn start(
    app: AsyncApp,
    shell: WeakEntity<Shell>,
    index_url: String,
    prefs: ferrocad_shell_api::prefs::Prefs,
) {
    app.spawn(async move |app| {
        let base = match page_url() {
            Ok(base) => base,
            Err(error) => {
                log::warn!("ferrocad: mods: {error}");
                return;
            }
        };
        let index_url = match resolve(&index_url, &base) {
            Ok(url) => url,
            Err(error) => {
                log::warn!("ferrocad: mods: {error}");
                return;
            }
        };

        let index = match fetch_text(&index_url).await {
            Ok(text) => match mods::parse_index(&text) {
                Ok(index) => index,
                Err(error) => {
                    log::warn!("ferrocad: {error}");
                    return;
                }
            },
            Err(error) => {
                // A page served without packs (a checkout that never ran the packer)
                // is a configuration fact, not a crash: say so and keep the window.
                log::warn!("ferrocad: mods: {error}");
                return;
            }
        };
        // The card offers every pack the index names — including the refused ones, which is
        // where they are turned back on. The set the page can *see* is the index's.
        let names: Vec<String> = index.mods.iter().map(|entry| entry.name.clone()).collect();
        let _ =
            app.update(|cx| shell.update(cx, |shell, cx| shell.note_prefs_mods(names.clone(), cx)));

        // B17a: a refused pack is never fetched, not fetched-then-skipped.
        let selected = mods::selected(&index.mods, &prefs);
        let refused = index.mods.len() - selected.len();
        if refused > 0 {
            log::info!("ferrocad: mods: {refused} pack(s) off in Settings/About");
        }

        let total = selected.len();
        if total == 0 {
            log::info!("ferrocad: mods: nothing to load");
            return;
        }
        log::info!("ferrocad: mods: {total} pack(s) from {index_url}");

        let mut loaded = 0usize;
        let mut failed = 0usize;
        for (position, entry) in selected.into_iter().enumerate() {
            set_status(
                app,
                &shell,
                mods::loading_status(position + 1, total, &entry.name),
            );
            let pack_url = match resolve(&entry.pack, &index_url) {
                Ok(url) => url,
                Err(error) => {
                    log::warn!("ferrocad: mod {}: {error}", entry.name);
                    failed += 1;
                    continue;
                }
            };
            let pack = match fetch_text(&pack_url).await {
                Ok(pack) => pack,
                Err(error) => {
                    log::warn!("ferrocad: mod {}: {error}", entry.name);
                    failed += 1;
                    continue;
                }
            };
            let report =
                match app.update(|cx| ferrocad_shell_core::app::load_workbench_pack(cx, &pack)) {
                    Ok(report) => report,
                    Err(error) => {
                        log::warn!("ferrocad: mod {}: {error}", entry.name);
                        failed += 1;
                        continue;
                    }
                };
            if !report.errors.is_empty() {
                failed += report.errors.len();
            }
            loaded += report.loaded.len();
            log::info!(
                "ferrocad: mod {}: {} script(s) loaded, {} failed",
                entry.name,
                report.loaded.len(),
                report.errors.len()
            );
            // The same call the boot path makes for a scanned tree: the About card and
            // the console are the pack's report too.
            let _ =
                app.update(|cx| shell.update(cx, |shell, cx| shell.note_mod_report(&report, cx)));
        }

        let status = mods::done_status(loaded, failed);
        log::info!("ferrocad: {status}");
        let _ = app.update(|cx| {
            shell.update(cx, |shell, cx| {
                shell.refresh_after_mods(cx);
                shell.set_status(status.clone(), cx);
            })
        });
    })
    .detach();
}

/// Show `text` in the status bar, if the window still exists.
fn set_status(app: &AsyncApp, shell: &WeakEntity<Shell>, text: String) {
    let _ = app.update(|cx| shell.update(cx, |shell, cx| shell.set_status(text, cx)));
}

/// The page's own URL — the base every relative pack path is resolved against.
fn page_url() -> Result<String, String> {
    web_sys::window()
        .ok_or_else(|| "no window".to_string())
        .map(|window| window.location().href())
        .and_then(|href| href.map_err(|error| format!("page URL: {error:?}")))
}

/// Resolve `relative` (a URL from the page or the index) against `base`.
fn resolve(relative: &str, base: &str) -> Result<String, String> {
    Url::new_with_base(relative, base)
        .map(|url| url.href())
        .map_err(|error| format!("{relative}: {error:?}"))
}

/// `fetch(url)` and read the body as text.
async fn fetch_text(url: &str) -> Result<String, String> {
    let window = web_sys::window().ok_or_else(|| "no window".to_string())?;
    let response = JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|error| format!("{url}: fetch failed: {error:?}"))?;
    let response: Response = response
        .dyn_into()
        .map_err(|_| format!("{url}: fetch did not answer a response"))?;
    if !response.ok() {
        return Err(format!(
            "{url}: HTTP {} {}",
            response.status(),
            response.status_text()
        ));
    }
    let text = JsFuture::from(
        response
            .text()
            .map_err(|error| format!("{url}: body: {error:?}"))?,
    )
    .await
    .map_err(|error| format!("{url}: body: {error:?}"))?;
    text.as_string()
        .ok_or_else(|| format!("{url}: body is not text"))
}
