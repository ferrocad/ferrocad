//! Font parsing, metrics and the legacy `kern` table.
//!
//! Everything here is expressed in **font units**; the caller applies the one
//! scale factor. That keeps the FreeType emulation in one place (see the crate
//! docs) and keeps this module testable without a size.

use read_fonts::tables::kern::SubtableKind;
use read_fonts::{FontRef, TableProvider};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{GlyphId, MetadataProvider};

use crate::outline::TextError;

/// A parsed font, borrowed from its bytes.
pub(crate) struct Face<'a> {
    font: FontRef<'a>,
}

impl<'a> Face<'a> {
    /// Parse `bytes` as a single-face font.
    pub(crate) fn new(bytes: &'a [u8]) -> Result<Self, TextError> {
        let font = FontRef::new(bytes).map_err(|error| TextError::Unreadable(error.to_string()))?;
        Ok(Self { font })
    }

    /// FreeType's `face->height` in font units: ascender − descender + line gap.
    ///
    /// `skrifa` chooses the same source FreeType does (OS/2 typographic metrics
    /// when `USE_TYPO_METRICS` is set, else `hhea`), so this is the divisor
    /// upstream's `scalefactor` uses.
    pub(crate) fn line_height(&self) -> f64 {
        let metrics = self.font.metrics(Size::unscaled(), LocationRef::new(&[]));
        (metrics.ascent - metrics.descent + metrics.leading) as f64
    }

    /// The glyph for `ch`, or glyph 0 (`.notdef`).
    ///
    /// FreeType's `FT_Load_Char` does the same: a character the cmap does not
    /// map is rendered as `.notdef`, not rejected.
    pub(crate) fn glyph(&self, ch: char) -> GlyphId {
        self.font.charmap().map(ch).unwrap_or(GlyphId::new(0))
    }

    /// The advance of `glyph` in font units.
    pub(crate) fn advance(&self, glyph: GlyphId) -> f64 {
        self.font
            .glyph_metrics(Size::unscaled(), LocationRef::new(&[]))
            .advance_width(glyph)
            .unwrap_or(0.0) as f64
    }

    /// The horizontal kerning of `left` and `right`, in font units.
    ///
    /// Only the legacy `kern` table is consulted, because that is all
    /// `FT_Get_Kerning` reads. A font without one (or without a horizontal
    /// subtable) returns 0.0.
    pub(crate) fn kerning(&self, left: GlyphId, right: GlyphId) -> f64 {
        let Ok(kern) = self.font.kern() else {
            return 0.0;
        };
        for subtable in kern.subtables().flatten() {
            if !subtable.is_horizontal() {
                continue;
            }
            let Ok(kind) = subtable.kind() else {
                continue;
            };
            let value = match kind {
                SubtableKind::Format0(table) => table.kerning(left, right),
                SubtableKind::Format2(table) => table.kerning(left, right),
                // Format 1 is an AAT state table and format 3 has no pairs;
                // neither is what FreeType applies for a horizontal advance.
                SubtableKind::Format1(_) | SubtableKind::Format3(_) => None,
            };
            if let Some(value) = value {
                return value as f64;
            }
        }
        0.0
    }

    /// Emit `glyph`'s outline, in font units, to `pen`.
    ///
    /// A glyph with no outline (a space) emits nothing.
    pub(crate) fn outline(&self, glyph: GlyphId, pen: &mut impl OutlinePen) -> Result<(), TextError> {
        let Some(outline) = self.font.outline_glyphs().get(glyph) else {
            return Ok(());
        };
        let settings = DrawSettings::unhinted(Size::unscaled(), LocationRef::new(&[]));
        outline
            .draw(settings, pen)
            .map_err(|error| TextError::Unreadable(error.to_string()))?;
        Ok(())
    }
}
