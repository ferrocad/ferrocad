//! Text outlines: a font's glyphs as 2-D contours in millimetres.
//!
//! This crate exists so `Part.makeWireString` can be built on the Rust text stack
//! (`skrifa` + `read-fonts`) instead of FreeType. It is deliberately **kernel
//! free**: it names no [`Shape`](ferrocad_geom::Shape), no OCCT type and no
//! Python type, so the curve/extrusion work is the only place that turns a
//! contour into an edge. That mirrors `ferrocad_curves`, the other leaf crate.
//!
//! # The scale is upstream's, not a nicer one
//!
//! Upstream's `FT2FC.cpp` sets FreeType to a fixed 480 pt, then scales with
//! `height / face->height / 10`. The 480 pt size and the units-per-em cancel
//! exactly, so what actually reaches the geometry is
//!
//! ```text
//! mm = font_unit * height / (10 * line_height)
//! ```
//!
//! where `line_height` is the font's ascender − descender + line gap in font
//! units (FreeType's `face->height`, chosen the same way: OS/2 typographic
//! metrics when `USE_TYPO_METRICS` is set, else `hhea`). This crate reproduces
//! that formula, **including the `/ 10`**, because the project's contract is that
//! an upstream-shaped script gets the same shape; see
//! [`tasks/simplecad-bringup.md`](../../../docs/project/tasks/simplecad-bringup.md)
//! SC2 for the open question about whether upstream's `/ 10` is a bug.
//!
//! # What it returns
//!
//! One [`GlyphOutline`] per character, in text order. The contours are in the
//! glyph's own space (pen origin at `x = 0`, baseline at `y = 0`) and the
//! placement is [`GlyphOutline::origin`], so a caller can translate instead of
//! re-deriving advances. Upstream returns `list[list[Wire]]` — one inner list per
//! character — and the binding mirrors that.
//!
//! ```
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let font = std::fs::read(concat!(
//! #     env!("CARGO_MANIFEST_DIR"),
//! #     "/tests/fixtures/fonts/LiberationMono-Regular.ttf"
//! # ))?;
//! let outline = ferrocad_text::outline_text(&font, "AB", 10.0, 0.0)?;
//! assert_eq!(outline.glyphs.len(), 2);
//! assert!(outline.glyphs[0].advance > 0.0);
//! # Ok(())
//! # }
//! ```

mod font;
mod outline;

pub use outline::{Contour, GlyphOutline, PathCommand, Point2, TextError, TextOutline, outline_text};
