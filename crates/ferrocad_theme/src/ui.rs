//! The tokens at the gpui boundary: packed sRGB.
//!
//! gpui binds packed sRGB, so these values are used **as stored** by the shell and
//! the widgets. They are the same visual language as [`crate::draw`], expressed in
//! the convention that boundary wants; do not feed them to the renderer.
//!
//! Opaque tokens are `0xRRGGBB` and go through `rgb()`; translucent tokens are
//! `0xRRGGBBAA` and go through `rgba()`. gpui reads the last three bytes as RGB, so
//! the two forms are not interchangeable.

// -- Surfaces -----------------------------------------------------------------

/// The window void (`#0F1218`).
pub const BG: u32 = 0x0f1218;
/// A raised panel (`#16181D`).
pub const PANEL: u32 = 0x16181d;
/// A second panel step (`#1D2026`), used for idle controls and field rows.
pub const PANEL_ALT: u32 = 0x1d2026;
/// The floating pill's glass (`#151A24` at 0.80).
pub const GLASS: u32 = 0x151a24cc;
/// The popover panel (`#1D2026` at 0.95).
pub const POPOVER_BG: u32 = 0x1d2026f2;
/// The popover border.
pub const POPOVER_BORDER: u32 = 0xffffff26;
/// The action dock's glass strip (`#16181D` at 0.90).
pub const DOCK_BG: u32 = 0x16181de6;
/// The action dock's border (`#FFFFFF` at 12%).
pub const DOCK_BORDER: u32 = 0xffffff1f;
/// The radial menu's dielectric torus: the canvas glass at 75%.
pub const WHEEL_GLASS: u32 = 0x0f1218bf;
/// A control's selected fill (`#2F4A6D`).
pub const ACCENT: u32 = 0x2f4a6d;

// -- Ink ----------------------------------------------------------------------

/// App text (`#D8DBE0`).
pub const FG: u32 = 0xd8dbe0;
/// Muted text and inactive marks (`#8B919A`).
pub const MUTED: u32 = 0x8b919a;
/// The chrome accent (`#67D7FF`): the NaviCube bar and the Dev Tools `active` badge.
pub const CHROME_ACTIVE: u32 = 0x67d7ff;
/// The chrome accent's fill (`#1B2B38` at 33%): behind a pressed chrome control, such as
/// the NaviCube's active projection toggle.
pub const CHROME_ACTIVE_BG: u32 = 0x1b2b3855;
/// The state accent (`#38BDF8`): the status dot on the pill and on a checked row.
pub const ACTIVE_ACCENT: u32 = 0x38bdf8;
/// The inactive half of a toggle's dot slot: [`MUTED`] at 30%.
pub const DOT_INACTIVE: u32 = 0x8b919a4d;
/// The bright rim for a hovered structural piece (`#48E5FF`).
pub const HOVER_RIM: u32 = 0x48e5ff;
/// Additive operations (`#F59E0B`): push/pull, extrude, pad.
pub const PUSH_PULL_COLOR: u32 = 0xf59e0b;
/// Destructive operations (`#EF4444`): delete, pocket, subtract.
pub const SUBTRACT_COLOR: u32 = 0xef4444;
/// A hovered wheel wedge's tint: [`ACTIVE_ACCENT`] at 12%.
pub const HIGHLIGHT_TINT: u32 = 0x38bdf81f;
/// The X axis token in a floating HUD (`#F87171`, a coral that reads as X without
/// implying the destructive tone).
pub const AXIS_X: u32 = 0xf87171;
/// The Y axis token in a floating HUD (`#4ADE80`).
pub const AXIS_Y: u32 = 0x4ade80;
/// The Z axis token in a floating HUD (`#38BDF8`): the same state accent as the rest
/// of the chrome.
pub const AXIS_Z: u32 = 0x38bdf8;

// -- Dielectric lines ---------------------------------------------------------

/// The 1 px dielectric hairline (`#FFFFFF` at 8%).
pub const CHROME_LINE: u32 = 0xffffff14;
/// The additive row hover fill, named for its role (same value as the hairline).
pub const ROW_HOVER: u32 = 0xffffff14;
