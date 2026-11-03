//! The bundled UI font for hosts with no system font database.
//!
//! Native windows resolve fonts through the platform (fontconfig, CoreText, DirectWrite),
//! so they add nothing here. The browser text system starts with an **empty** font
//! database ([`bite-gp-web`](https://crates.io/crates/bite-gp-web)), and GPUI panics
//! when `resolve_font` cannot resolve the requested family or any fallback. Shipping a
//! font whose family is one of GPUI's fallbacks (`DejaVu Sans`) makes the default
//! `.ZedSans` stack resolve without every element having to name a family.
//!
//! The font is DejaVu Sans (Bitstream Vera licence, in
//! `assets/DejaVuSans-LICENSE`). Embedding is `wasm`-only, so a native binary does not
//! carry it.

#[cfg(target_family = "wasm")]
use std::borrow::Cow;

use gpui::App;

/// The family the bundled font declares, one of GPUI's default fallbacks.
pub const FAMILY: &str = "DejaVu Sans";

#[cfg(target_family = "wasm")]
const FONT: &[u8] = include_bytes!("../assets/DejaVuSans.ttf");

/// Register the bundled font with the app's text system.
///
/// Call once, before opening a window. Native builds are a no-op: the platform already
/// has fonts, and adding one would change how native text resolves.
pub fn install(cx: &App) {
    #[cfg(target_family = "wasm")]
    {
        cx.text_system()
            .add_fonts(vec![Cow::Borrowed(FONT)])
            .expect("the bundled UI font is a valid TTF");
        log::info!("ferrocad: bundled UI font ({FAMILY}) registered");
    }
    #[cfg(not(target_family = "wasm"))]
    {
        let _ = cx;
    }
}
