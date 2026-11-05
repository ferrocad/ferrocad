//! Embedded SVG pictograms for chrome that has no room for a text label.
//!
//! The assets are the outline glyphs selected in
//! `docs/developer/interaction/iconography.md` (the IconMCP set, plus the authored
//! `workplane` and the composed `gizmo`/`snap`); they are embedded with
//! [`include_bytes!`] and rendered by gpui's SVG element, so there is no asset
//! source, filesystem lookup or network dependency. The renderer produces an alpha
//! mask and the ink is supplied by the caller: greyscale when off (`MUTED`), accent
//! when on (`HOVER_RIM`).
//!
//! The elements are **presentational**: they draw the glyph. The host owns the click
//! target, its hover and its tooltip.
//!
//! Two entry points:
//!
//! - [`bug`], [`grid`], [`axes`], [`workplane`], [`gizmo`], [`snap`] are the header's
//!   fixed controls, each with a `{key}-icon` / `{key}-bug` debug selector.
//! - [`named`] resolves an **icon key** (a command's curated icon) to a glyph, so a
//!   menu row, a dock button or a wheel slot can carry whichever icon its command
//!   declares. An unknown key returns `None` and the caller keeps the label alone.

use gpui::{AnyElement, IntoElement, SharedString, div, prelude::*, px, rgb, svg};

use ferrocad_theme::ui::{HOVER_RIM, MUTED};

/// The Dev Tools bug's side.
pub const BUG: f32 = 16.0;

/// A header control glyph's side.
pub const CONTROL: f32 = 16.0;

/// A menu row's or dock button's glyph side.
pub const ROW: f32 = 14.0;

/// A property row's glyph side, matching its `text_xs` label.
pub const SMALL: f32 = 12.0;

/// A radial-wheel slot's glyph side.
pub const WHEEL: f32 = 14.0;

const BUG_SVG: &[u8] = include_bytes!("../assets/icons/bug.svg");
const GRID_SVG: &[u8] = include_bytes!("../assets/icons/grid-4x4.svg");
const AXES_SVG: &[u8] = include_bytes!("../assets/icons/axes-3d.svg");
const WORKPLANE_SVG: &[u8] = include_bytes!("../assets/icons/workplane.svg");
const GIZMO_SVG: &[u8] = include_bytes!("../assets/icons/gizmo.svg");
const SNAP_SVG: &[u8] = include_bytes!("../assets/icons/snap.svg");

/// Every vendored glyph, by icon key.
///
/// The key is the IconMCP icon name for a direct match, or the local name for the
/// glyphs FerroCAD authors (`workplane`) or composes (`gizmo`, `snap`). The curated
/// alias table in the shell maps a command id to one of these keys; this table is
/// what actually ships.
const ASSETS: &[(&str, &[u8])] = &[
    ("bug", BUG_SVG),
    ("grid-4x4", GRID_SVG),
    ("axes-3d", AXES_SVG),
    ("workplane", WORKPLANE_SVG),
    ("gizmo", GIZMO_SVG),
    ("snap", SNAP_SVG),
    ("menu", include_bytes!("../assets/icons/menu.svg")),
    ("search", include_bytes!("../assets/icons/search.svg")),
    (
        "folder-open",
        include_bytes!("../assets/icons/folder-open.svg"),
    ),
    ("save", include_bytes!("../assets/icons/save.svg")),
    ("move", include_bytes!("../assets/icons/move.svg")),
    (
        "rotate-object",
        include_bytes!("../assets/icons/rotate-object.svg"),
    ),
    ("trash", include_bytes!("../assets/icons/trash.svg")),
    ("cube", include_bytes!("../assets/icons/cube.svg")),
    (
        "shape-polyline",
        include_bytes!("../assets/icons/shape-polyline.svg"),
    ),
    (
        "layers-plus",
        include_bytes!("../assets/icons/layers-plus.svg"),
    ),
    (
        "anchor-point",
        include_bytes!("../assets/icons/anchor-point.svg"),
    ),
    (
        "cursor-crosshair",
        include_bytes!("../assets/icons/cursor-crosshair.svg"),
    ),
    (
        "intersection",
        include_bytes!("../assets/icons/intersection.svg"),
    ),
    (
        "perpendicular",
        include_bytes!("../assets/icons/perpendicular.svg"),
    ),
    ("parallel", include_bytes!("../assets/icons/parallel.svg")),
    ("angle", include_bytes!("../assets/icons/angle.svg")),
    (
        "angle-right",
        include_bytes!("../assets/icons/angle-right.svg"),
    ),
    ("ruler", include_bytes!("../assets/icons/ruler.svg")),
    ("star", include_bytes!("../assets/icons/star.svg")),
    // Model-tree object types.
    (
        "pencil-ruler",
        include_bytes!("../assets/icons/pencil-ruler.svg"),
    ),
    ("group", include_bytes!("../assets/icons/group.svg")),
    (
        "chain-link",
        include_bytes!("../assets/icons/chain-link.svg"),
    ),
    ("table", include_bytes!("../assets/icons/table.svg")),
    ("shapes", include_bytes!("../assets/icons/shapes.svg")),
    ("cylinder", include_bytes!("../assets/icons/cylinder.svg")),
    ("sphere", include_bytes!("../assets/icons/sphere.svg")),
    ("cone", include_bytes!("../assets/icons/cone.svg")),
    ("torus", include_bytes!("../assets/icons/torus.svg")),
    ("prism", include_bytes!("../assets/icons/prism.svg")),
    (
        "spring-coil",
        include_bytes!("../assets/icons/spring-coil.svg"),
    ),
    ("spiral", include_bytes!("../assets/icons/spiral.svg")),
    (
        "circle-sector",
        include_bytes!("../assets/icons/circle-sector.svg"),
    ),
    (
        "line-segment",
        include_bytes!("../assets/icons/line-segment.svg"),
    ),
    ("bullet", include_bytes!("../assets/icons/bullet.svg")),
    ("circle", include_bytes!("../assets/icons/circle.svg")),
    // Property-row types.
    ("palette", include_bytes!("../assets/icons/palette.svg")),
    ("hash", include_bytes!("../assets/icons/hash.svg")),
    ("type", include_bytes!("../assets/icons/type.svg")),
    ("list", include_bytes!("../assets/icons/list.svg")),
    ("toggle-on", include_bytes!("../assets/icons/toggle-on.svg")),
    // Dialog / active-action verbs.
    ("close", include_bytes!("../assets/icons/close.svg")),
    ("check", include_bytes!("../assets/icons/check.svg")),
];

/// The embedded SVG for an icon key, if it is vendored.
pub fn asset(name: &str) -> Option<&'static [u8]> {
    ASSETS
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, data)| *data)
}

/// Render the glyph named by an icon key at `size`, in `ink`.
///
/// Returns `None` for a key that is not vendored, so a caller can keep its label
/// alone rather than reserving an empty slot. The element carries no debug selector:
/// the caller wraps it, because the same icon may appear more than once on screen.
pub fn named(name: &str, size: f32, ink: u32) -> Option<AnyElement> {
    let data = asset(name)?;
    Some(
        div()
            .w(px(size))
            .h(px(size))
            .flex_none()
            .child(svg().data(data).w_full().h_full().text_color(rgb(ink)))
            .into_any_element(),
    )
}

/// The literal bug that opens the Dev Tools window.
pub fn bug(key: impl Into<SharedString>, active: bool) -> impl IntoElement {
    glyph(key, BUG, "bug", ink(active), BUG_SVG)
}

/// The 4 × 4 reference grid.
pub fn grid(key: impl Into<SharedString>, active: bool) -> impl IntoElement {
    glyph(key, CONTROL, "icon", ink(active), GRID_SVG)
}

/// The three-axis world triad.
pub fn axes(key: impl Into<SharedString>, active: bool) -> impl IntoElement {
    glyph(key, CONTROL, "icon", ink(active), AXES_SVG)
}

/// A plane in perspective with a normal arrow.
pub fn workplane(key: impl Into<SharedString>, active: bool) -> impl IntoElement {
    glyph(key, CONTROL, "icon", ink(active), WORKPLANE_SVG)
}

/// The transform gizmo: the axis triad with the four-way move arrows.
pub fn gizmo(key: impl Into<SharedString>, active: bool) -> impl IntoElement {
    glyph(key, CONTROL, "icon", ink(active), GIZMO_SVG)
}

/// The global snap switch: a magnet with a lock.
pub fn snap(key: impl Into<SharedString>, active: bool) -> impl IntoElement {
    glyph(key, CONTROL, "icon", ink(active), SNAP_SVG)
}

/// The ink a boolean control uses: the accent when on, muted when off.
fn ink(active: bool) -> u32 {
    if active { HOVER_RIM } else { MUTED }
}

/// A presentational glyph with a `{key}-{suffix}` debug selector.
fn glyph(
    key: impl Into<SharedString>,
    size: f32,
    suffix: &'static str,
    ink: u32,
    data: &'static [u8],
) -> impl IntoElement {
    let key = key.into();
    let selector = key.clone();
    div()
        .id(key)
        .debug_selector(move || format!("{selector}-{suffix}"))
        .w(px(size))
        .h(px(size))
        .flex_none()
        .child(svg().data(data).w_full().h_full().text_color(rgb(ink)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext, VisualTestContext, Window};

    struct Harness;

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(bug("bug-test", false))
                .child(grid("grid-test", false))
                .child(axes("axis-test", true))
                .child(workplane("wp-test", true))
                .child(gizmo("gizmo-test", false))
                .child(snap("snap-test", true))
        }
    }

    #[test]
    fn every_vendored_asset_is_an_svg_with_strokes() {
        for (name, data) in ASSETS {
            assert!(data.starts_with(b"<svg"), "{name} is an SVG");
            assert!(
                data.windows(6).any(|window| window == b"stroke".as_slice()),
                "{name} carries strokes"
            );
        }
    }

    #[test]
    fn the_icon_keys_resolve_and_unknown_keys_do_not() {
        for key in [
            "menu",
            "grid-4x4",
            "axes-3d",
            "move",
            "trash",
            "cube",
            "shape-polyline",
            "snap",
        ] {
            assert!(asset(key).is_some(), "{key} is vendored");
        }
        assert!(asset("no-such-icon").is_none());
    }

    #[gpui::test]
    fn the_bug_renders_at_its_side(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_window, cx| cx.new(|_| Harness))
                .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let bounds = visual
            .debug_bounds("bug-test-bug")
            .expect("the bug is painted");
        assert_eq!(bounds.size.width, px(BUG));
        assert_eq!(bounds.size.height, px(BUG));
    }

    #[gpui::test]
    fn the_control_glyphs_render_at_their_side(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_window, cx| cx.new(|_| Harness))
                .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });
        for name in [
            "grid-test-icon",
            "axis-test-icon",
            "wp-test-icon",
            "gizmo-test-icon",
            "snap-test-icon",
        ] {
            let bounds = visual
                .debug_bounds(name)
                .unwrap_or_else(|| panic!("{name} is painted"));
            assert_eq!(bounds.size.width, px(CONTROL), "{name} width");
            assert_eq!(bounds.size.height, px(CONTROL), "{name} height");
        }
    }
}
