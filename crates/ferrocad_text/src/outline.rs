//! Glyph contours in millimetres: the pen, the public shapes and the one entry
//! point.

use std::fmt;

use skrifa::outline::OutlinePen;

use crate::font::Face;

/// Two points closer than this (in mm) are the same point.
///
/// Font outlines close exactly, so this only absorbs the f32 → f64 round trip
/// and a curve that ends where it started.
const SAME_POINT: f64 = 1e-9;

/// A point in millimetres. `y` grows up; the baseline is `y = 0`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point2 {
    pub x: f64,
    pub y: f64,
}

impl Point2 {
    /// A point.
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// The distance to `other`.
    pub fn distance(&self, other: &Self) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}

/// One outline command, from the current point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathCommand {
    /// A straight segment to the point.
    LineTo(Point2),
    /// A quadratic Bézier: one control point, then the endpoint (TrueType).
    QuadTo { control: Point2, to: Point2 },
    /// A cubic Bézier: two control points, then the endpoint (CFF/PostScript).
    CubicTo { c1: Point2, c2: Point2, to: Point2 },
}

/// A closed contour: a start point and the commands that return to it.
#[derive(Clone, Debug, PartialEq)]
pub struct Contour {
    /// Where the contour starts.
    pub start: Point2,
    /// The commands, in draw order. A closed contour's last command ends at
    /// [`start`](Self::start); an empty command list is a degenerate (dot)
    /// contour and is never emitted.
    pub commands: Vec<PathCommand>,
}

/// One character's outline, in the glyph's own space.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphOutline {
    /// The glyph id the character mapped to (0 is `.notdef`).
    pub glyph_id: u32,
    /// Where this glyph's origin sits relative to the text's origin, in mm.
    pub origin: Point2,
    /// The advance to the next glyph, in mm.
    pub advance: f64,
    /// The outline, with the pen origin at `x = 0` and the baseline at `y = 0`.
    pub contours: Vec<Contour>,
}

/// A laid-out string: one glyph per character, in text order.
#[derive(Clone, Debug, PartialEq)]
pub struct TextOutline {
    /// The glyphs.
    pub glyphs: Vec<GlyphOutline>,
    /// The total advance of the text in mm (kerning and advances, no tracking).
    pub advance: f64,
}

/// Why a string could not be turned into outlines.
#[derive(Clone, Debug, PartialEq)]
pub enum TextError {
    /// The bytes are not a font, or a table the outline needs is unreadable.
    Unreadable(String),
    /// The font has no positive line height, so there is no scale to apply.
    NoLineMetrics,
}

impl fmt::Display for TextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable(message) => write!(formatter, "cannot read the font: {message}"),
            Self::NoLineMetrics => write!(formatter, "the font has no positive line height"),
        }
    }
}

impl std::error::Error for TextError {}

/// Collects `skrifa`'s path commands into [`Contour`]s in millimetres.
struct ContourPen {
    /// Font units → mm.
    scale: f64,
    contours: Vec<Contour>,
    current: Option<Contour>,
    start: Point2,
    last: Point2,
}

impl ContourPen {
    fn new(scale: f64) -> Self {
        Self {
            scale,
            contours: Vec::new(),
            current: None,
            start: Point2::new(0.0, 0.0),
            last: Point2::new(0.0, 0.0),
        }
    }

    fn point(&self, x: f32, y: f32) -> Point2 {
        Point2::new(x as f64 * self.scale, y as f64 * self.scale)
    }

    /// Keep the contour being drawn, if it has any commands.
    fn finish(&mut self) {
        if let Some(contour) = self.current.take() {
            if !contour.commands.is_empty() {
                self.contours.push(contour);
            }
        }
    }

    /// Mark the current contour complete.
    ///
    /// The last point becomes the start when the outline did not close itself;
    /// font contours always close, but a font that omits the final segment would
    /// otherwise emit a wire that cannot be joined.
    fn close_current(&mut self) {
        if let Some(contour) = &mut self.current {
            if self.last.distance(&self.start) > SAME_POINT {
                contour.commands.push(PathCommand::LineTo(self.start));
                self.last = self.start;
            }
        }
        self.finish();
    }
}

impl OutlinePen for ContourPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.finish();
        let point = self.point(x, y);
        self.start = point;
        self.last = point;
        self.current = Some(Contour {
            start: point,
            commands: Vec::new(),
        });
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let point = self.point(x, y);
        if point.distance(&self.last) > SAME_POINT {
            if let Some(contour) = &mut self.current {
                contour.commands.push(PathCommand::LineTo(point));
            }
            self.last = point;
        }
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let control = self.point(cx, cy);
        let point = self.point(x, y);
        if let Some(contour) = &mut self.current {
            contour.commands.push(PathCommand::QuadTo { control, to: point });
        }
        self.last = point;
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let c1 = self.point(cx0, cy0);
        let c2 = self.point(cx1, cy1);
        let point = self.point(x, y);
        if let Some(contour) = &mut self.current {
            contour.commands.push(PathCommand::CubicTo { c1, c2, to: point });
        }
        self.last = point;
    }

    fn close(&mut self) {
        self.close_current();
    }
}

/// Lay `text` out at `height` and return one outline per character.
///
/// `height` and `tracking` are millimetres. The scale is upstream's
/// (`height / (10 * line_height)`), see the [crate docs](crate) for why the
/// divisor is 10 and why it is not "fixed" here.
///
/// Characters the font does not map are drawn as `.notdef`, as FreeType does;
/// kerning comes from the legacy `kern` table only.
pub fn outline_text(
    font_bytes: &[u8],
    text: &str,
    height: f64,
    tracking: f64,
) -> Result<TextOutline, TextError> {
    let face = Face::new(font_bytes)?;
    let line_height = face.line_height();
    if !(line_height.is_finite() && line_height > 0.0) {
        return Err(TextError::NoLineMetrics);
    }
    let scale = height / (10.0 * line_height);

    let mut pen = 0.0_f64;
    let mut previous: Option<skrifa::GlyphId> = None;
    let mut glyphs = Vec::with_capacity(text.chars().count());
    for (index, character) in text.chars().enumerate() {
        let glyph_id = face.glyph(character);
        if let Some(left) = previous {
            pen += face.kerning(left, glyph_id);
        }
        let advance = face.advance(glyph_id);
        let mut outline = ContourPen::new(scale);
        face.outline(glyph_id, &mut outline)?;
        glyphs.push(GlyphOutline {
            glyph_id: glyph_id.to_u32(),
            origin: Point2::new(pen * scale + index as f64 * tracking, 0.0),
            advance: advance * scale,
            contours: outline.contours,
        });
        pen += advance;
        previous = Some(glyph_id);
    }

    Ok(TextOutline {
        glyphs,
        advance: pen * scale,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FONT: &[u8] = include_bytes!("../tests/fixtures/fonts/LiberationMono-Regular.ttf");

    fn font_units() -> f64 {
        let face = Face::new(FONT).expect("the fixture parses");
        face.line_height()
    }

    #[test]
    fn the_fixture_is_a_font_with_line_metrics() {
        let line = font_units();
        // Liberation Mono: upem 2048, hhea ascender 1854, descender -434, gap 32.
        assert!(
            (line - 2320.0).abs() < 1.0,
            "line height in font units is {line}"
        );
    }

    #[test]
    fn a_string_yields_one_glyph_per_character() {
        let outline = outline_text(FONT, "AB", 10.0, 0.0).expect("outlines");
        assert_eq!(outline.glyphs.len(), 2);
        assert!(outline.glyphs.iter().all(|glyph| glyph.advance > 0.0));
        assert!(
            outline.glyphs.iter().all(|glyph| !glyph.contours.is_empty()),
            "letters have outlines"
        );
    }

    #[test]
    fn a_space_has_an_advance_and_no_contours() {
        let outline = outline_text(FONT, "A A", 10.0, 0.0).expect("outlines");
        assert!(outline.glyphs[1].advance > 0.0);
        assert!(outline.glyphs[1].contours.is_empty());
    }

    #[test]
    fn the_scale_is_upstreams_height_over_ten_line_heights() {
        let outline = outline_text(FONT, "I", 10.0, 0.0).expect("outlines");
        // Cap height of Liberation Mono is 1349 font units; the outline's top
        // must be that many units times the one scale factor.
        let top = outline.glyphs[0]
            .contours
            .iter()
            .flat_map(|contour| {
                std::iter::once(contour.start).chain(contour.commands.iter().flat_map(|command| {
                    match *command {
                        PathCommand::LineTo(to) => vec![to],
                        PathCommand::QuadTo { control, to } => vec![control, to],
                        PathCommand::CubicTo { c1, c2, to } => vec![c1, c2, to],
                    }
                }))
            })
            .map(|point| point.y)
            .fold(f64::MIN, f64::max);
        let expected = 1349.0 * 10.0 / (10.0 * font_units());
        assert!(
            (top - expected).abs() < 1e-6,
            "top is {top}, expected {expected}"
        );
    }

    #[test]
    fn origins_advance_and_tracking_shift_the_next_glyph() {
        let plain = outline_text(FONT, "AB", 10.0, 0.0).expect("outlines");
        assert!((plain.glyphs[0].origin.x - 0.0).abs() < 1e-12);
        assert!((plain.glyphs[1].origin.x - plain.glyphs[0].advance).abs() < 1e-9);

        let tracked = outline_text(FONT, "AB", 10.0, 1.0).expect("outlines");
        assert!((tracked.glyphs[1].origin.x - plain.glyphs[1].origin.x - 1.0).abs() < 1e-9);
        assert!((tracked.advance - plain.advance).abs() < 1e-12, "tracking is placement, not advance");
    }

    #[test]
    fn every_contour_is_closed() {
        let outline = outline_text(FONT, "B", 10.0, 0.0).expect("outlines");
        for contour in &outline.glyphs[0].contours {
            let end = contour.commands.last().expect("a contour has commands");
            let end = match *end {
                PathCommand::LineTo(to) => to,
                PathCommand::QuadTo { to, .. } | PathCommand::CubicTo { to, .. } => to,
            };
            assert!(
                end.distance(&contour.start) <= SAME_POINT,
                "{end:?} does not return to {:?}",
                contour.start
            );
        }
    }

    #[test]
    fn an_unmapped_character_draws_notdef_rather_than_failing() {
        let outline = outline_text(FONT, "\u{1F600}", 10.0, 0.0).expect("outlines");
        assert_eq!(outline.glyphs.len(), 1);
        assert_eq!(outline.glyphs[0].glyph_id, 0);
    }

    #[test]
    fn empty_text_is_an_empty_outline() {
        let outline = outline_text(FONT, "", 10.0, 0.0).expect("outlines");
        assert!(outline.glyphs.is_empty());
        assert_eq!(outline.advance, 0.0);
    }

    #[test]
    fn junk_bytes_are_an_error() {
        let error = outline_text(b"not a font", "A", 10.0, 0.0).expect_err("must fail");
        assert!(matches!(error, TextError::Unreadable(_)), "{error:?}");
    }
}
