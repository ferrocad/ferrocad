//! Re-export of the draw-contract tokens.
//!
//! The token table moved to [`ferrocad_theme::draw`] (V0 of
//! `docs/developer/interaction/visual-language.md`); this path stays so the renderer-side
//! call sites (`theme::GLASS_SHADE`, …) do not churn. The gpui boundary uses the
//! packed sRGB table in [`ferrocad_theme::ui`] instead.

pub use ferrocad_theme::draw::*;
