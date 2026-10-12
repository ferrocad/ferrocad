//! The navigation cube's appearance: the semantic role a piece takes, and the theme
//! that maps roles to colours.
//!
//! Appearance is **not** part of the draw contract: a producer never picks a colour. It
//! tags each piece with a [`NavicubeRole`], and a [`NavicubeTheme`] supplies the colour
//! (see `docs/navicube-spec.md` §5 and `docs/directives.md` §5). The default theme
//! reproduces the built palette.

use super::model::{NaviTarget, Piece, PieceKind};

/// The semantic role a piece takes this frame — the input to a [`NavicubeTheme`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavicubeRole {
    /// A cube face at rest.
    Face,
    /// A cube face that is hovered.
    FaceHighlighted,
    /// A cube face pointing away from the viewer (emitted first).
    FaceAway,
    /// An edge segment at rest.
    Edge,
    /// An edge segment that is hovered.
    EdgeHighlighted,
    /// A corner cube at rest.
    Corner,
    /// A corner cube that is hovered.
    CornerHighlighted,
    /// The Home button at rest.
    Home,
    /// The Home button that is hovered.
    HomeHighlighted,
}

impl NaviTarget {
    /// The role `self` takes given the hovered target and the away-facing faces — the
    /// pure role rule, priority highlight then away.
    pub fn role(self, hilite: Option<NaviTarget>, away: &[Piece]) -> NavicubeRole {
        let highlighted = hilite == Some(self);
        match self {
            NaviTarget::Piece(piece) => match piece.kind() {
                PieceKind::Face if highlighted => NavicubeRole::FaceHighlighted,
                PieceKind::Face if away.contains(&piece) => NavicubeRole::FaceAway,
                PieceKind::Face => NavicubeRole::Face,
                PieceKind::Edge if highlighted => NavicubeRole::EdgeHighlighted,
                PieceKind::Edge => NavicubeRole::Edge,
                PieceKind::Corner if highlighted => NavicubeRole::CornerHighlighted,
                PieceKind::Corner => NavicubeRole::Corner,
            },
            NaviTarget::Home if highlighted => NavicubeRole::HomeHighlighted,
            NaviTarget::Home => NavicubeRole::Home,
        }
    }
}

/// The cube's resting piece colour (the default theme's `base`).
pub const BASE_COLOR: [f32; 4] = [0.55, 0.6, 0.66, 1.0];
/// The colour any hovered piece takes (the default theme's `highlighted`).
pub const HILITE_COLOR: [f32; 4] = [0.2, 0.6, 0.95, 1.0];
/// The translucent colour of an away-facing face (the default theme's `away`).
pub const INACTIVE_COLOR: [f32; 4] = [0.38, 0.42, 0.48, 0.5];
/// The Home button's resting colour (the default theme's `home`).
pub const HOME_COLOR: [f32; 4] = [0.5, 0.55, 0.62, 1.0];

/// The role→appearance mapping. The Rust core owns styling, so a producer resolves a
/// [`NavicubeRole`] through a theme rather than choosing colours itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavicubeTheme {
    /// The resting colour of a face, edge or corner.
    pub base: [f32; 4],
    /// The colour of any hovered piece.
    pub highlighted: [f32; 4],
    /// The colour of an away-facing face (emitted first).
    pub away: [f32; 4],
    /// The Home button's resting colour.
    pub home: [f32; 4],
    /// Line/edge width, in logical pixels. Stored for fidelity; the renderer draws 1-px
    /// lines until a wide-line pipeline lands, so this has no visible effect yet.
    pub edge_width: f32,
}

impl NavicubeTheme {
    /// The colour for a role.
    pub fn color(&self, role: NavicubeRole) -> [f32; 4] {
        match role {
            NavicubeRole::Face | NavicubeRole::Edge | NavicubeRole::Corner => self.base,
            NavicubeRole::FaceHighlighted
            | NavicubeRole::EdgeHighlighted
            | NavicubeRole::CornerHighlighted
            | NavicubeRole::HomeHighlighted => self.highlighted,
            NavicubeRole::FaceAway => self.away,
            NavicubeRole::Home => self.home,
        }
    }
}

impl Default for NavicubeTheme {
    fn default() -> Self {
        Self {
            base: BASE_COLOR,
            highlighted: HILITE_COLOR,
            away: INACTIVE_COLOR,
            home: HOME_COLOR,
            edge_width: 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_role_rule_is_highlight_then_away() {
        let piece = Piece::TOP;
        let back = Piece::BACK;
        let target = NaviTarget::Piece(piece);
        assert_eq!(
            target.role(Some(target), &[back]),
            NavicubeRole::FaceHighlighted
        );
        assert_eq!(target.role(None, &[back]), NavicubeRole::Face);
        assert_eq!(
            NaviTarget::Piece(back).role(None, &[back]),
            NavicubeRole::FaceAway
        );
        assert_eq!(
            NaviTarget::Piece(Piece::ALL[6]).role(None, &[]),
            NavicubeRole::Edge
        );
        assert_eq!(
            NaviTarget::Piece(Piece::ALL[6]).role(Some(NaviTarget::Piece(Piece::ALL[6])), &[]),
            NavicubeRole::EdgeHighlighted
        );
        assert_eq!(NaviTarget::Home.role(None, &[]), NavicubeRole::Home);
        assert_eq!(
            NaviTarget::Home.role(Some(NaviTarget::Home), &[]),
            NavicubeRole::HomeHighlighted
        );
    }

    #[test]
    fn the_default_theme_matches_the_built_palette() {
        let theme = NavicubeTheme::default();
        assert_eq!(theme.color(NavicubeRole::Face), BASE_COLOR);
        assert_eq!(theme.color(NavicubeRole::FaceHighlighted), HILITE_COLOR);
        assert_eq!(theme.color(NavicubeRole::HomeHighlighted), HILITE_COLOR);
        assert_eq!(theme.color(NavicubeRole::FaceAway), INACTIVE_COLOR);
        assert_eq!(theme.color(NavicubeRole::Home), HOME_COLOR);
    }
}
