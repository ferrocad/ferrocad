//! The navigation cube's appearance: the semantic role a piece takes, and the theme
//! that maps roles to colours.
//!
//! Appearance is **not** part of the draw contract: a producer never picks a colour. It
//! tags each piece with a [`NavicubeRole`], and a [`NavicubeTheme`] supplies the colour
//! (see `docs/navicube-spec.md` §5 and `docs/directives.md` §5). The default theme
//! reproduces the built palette.

use ferrocad_types::Vector3;

use super::model::{Piece, PieceKind};

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
    /// A bevel adjacent to a hovered corner: it lights up softly so the glow reads across
    /// the chamfer without flooding the face.
    EdgeNeighbour,
    /// A corner cube at rest.
    Corner,
    /// A corner cube that is hovered.
    CornerHighlighted,
}

impl Piece {
    /// The role `self` takes given the hovered piece and the away-facing faces — the
    /// pure role rule, priority highlight then away. A bevel adjacent to a hovered corner
    /// is a neighbour and lights softly.
    pub fn role(self, hilite: Option<Piece>, away: &[Piece]) -> NavicubeRole {
        let highlighted = hilite == Some(self);
        let neighbour = self.kind() == PieceKind::Edge
            && hilite.is_some_and(|h| {
                h.kind() == PieceKind::Corner
                    && (0..3).all(|i| self.axes[i] == 0 || self.axes[i] == h.axes[i])
            });
        match self.kind() {
            PieceKind::Face if highlighted => NavicubeRole::FaceHighlighted,
            PieceKind::Face if away.contains(&self) => NavicubeRole::FaceAway,
            PieceKind::Face => NavicubeRole::Face,
            PieceKind::Edge if highlighted => NavicubeRole::EdgeHighlighted,
            PieceKind::Edge if neighbour => NavicubeRole::EdgeNeighbour,
            PieceKind::Edge => NavicubeRole::Edge,
            PieceKind::Corner if highlighted => NavicubeRole::CornerHighlighted,
            PieceKind::Corner => NavicubeRole::Corner,
        }
    }
}

/// The cube's resting **glass facet** colour: a faint dark tint that gives the facet
/// volume without hiding the model behind it.
pub const BASE_COLOR: [f32; 4] = [0.18, 0.23, 0.30, 0.16];
/// The soft cyan a hovered bevel or corner takes.
pub const HILITE_COLOR: [f32; 4] = [0.28, 0.90, 1.0, 0.60];
/// A bevel adjacent to a hovered corner: a softer cyan than the hovered piece itself.
pub const NEIGHBOUR_COLOR: [f32; 4] = [0.28, 0.65, 0.95, 0.20];
/// The still fainter tint of an away-facing face.
pub const INACTIVE_COLOR: [f32; 4] = [0.14, 0.18, 0.24, 0.06];
/// A resting bevel quad: the bright machined chamfer between two face octagons.
pub const BEVEL_COLOR: [f32; 4] = [0.86, 0.92, 1.0, 0.45];
/// A bevel quad once the pointer is in the gizmo.
pub const BEVEL_PROXIMITY: [f32; 4] = [0.93, 0.96, 1.0, 0.60];
/// A bevel behind the cube: dimmed so the front wireframe dominates.
pub const BEVEL_AWAY: [f32; 4] = [0.48, 0.53, 0.60, 0.10];

/// A resting 1px outline. Front and back are deliberately far apart in alpha so the
/// wireframe cannot flip into a Necker cube; the line pipeline is 1px, so depth is
/// carried by opacity rather than stroke weight.
pub const EDGE_COLOR: [f32; 4] = [0.84, 0.88, 0.94, 0.62];
/// The outline once the pointer is in the gizmo (proximity, not a hovered piece).
pub const EDGE_PROXIMITY: [f32; 4] = [0.90, 0.94, 1.0, 0.9];
/// The outline of the hovered piece.
pub const EDGE_HILITE: [f32; 4] = [0.4, 0.84, 1.0, 1.0];
/// The outline of a bevel adjacent to a hovered corner.
pub const EDGE_NEIGHBOUR: [f32; 4] = [0.4, 0.84, 1.0, 0.80];
/// The outline of an away-facing face: a ghost line, not a second front edge.
pub const EDGE_AWAY: [f32; 4] = [0.55, 0.60, 0.68, 0.10];
/// A resting face label: bright, high-contrast ink for the filled glyphs and the compass
/// letters.
pub const LABEL_COLOR: [f32; 4] = [0.92, 0.95, 1.0, 0.80];
/// A label once the pointer is in the gizmo.
pub const LABEL_PROXIMITY: [f32; 4] = [0.98, 0.99, 1.0, 0.95];
/// The label of the hovered piece.
pub const LABEL_HILITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// The soft cyan ribbon behind a hovered edge. The renderer has no bloom pass and no wide
/// lines, so "glow" is a translucent screen-space quad around the edge.
pub const GLOW_COLOR: [f32; 4] = [0.4, 0.84, 1.0, 0.22];
/// The compass ring at rest. It lies on the ground plane, so it must read as a turntable
/// rather than disappear into the viewport.
pub const RING_COLOR: [f32; 4] = [0.62, 0.68, 0.76, 0.70];
/// The ring once the pointer is in the gizmo.
pub const RING_PROXIMITY: [f32; 4] = [0.75, 0.81, 0.88, 0.90];
/// The ring while hovered or scrubbed.
pub const RING_HILITE: [f32; 4] = [0.4, 0.84, 1.0, 0.95];
/// The dark floor disc under the cube: it grounds the glass body against the viewport.
/// Values are **linear**, so this is much darker than its nominal `#141820` sRGB look.
pub const TURNTABLE_COLOR: [f32; 4] = [0.008, 0.010, 0.016, 0.72];
/// The thick rim band between the turntable's inner and outer loops.
pub const RING_FILL: [f32; 4] = [0.59, 0.725, 0.824, 0.35];
/// The rim band once the pointer is in the gizmo.
pub const RING_FILL_PROXIMITY: [f32; 4] = [0.68, 0.80, 0.89, 0.45];
/// The rim band while hovered or scrubbed.
pub const RING_FILL_HILITE: [f32; 4] = [0.4, 0.84, 1.0, 0.55];

/// The role→appearance mapping. The Rust core owns styling, so a producer resolves a
/// [`NavicubeRole`] through a theme rather than choosing colours itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavicubeTheme {
    /// The resting colour of a face, edge or corner.
    pub base: [f32; 4],
    /// The colour of any hovered piece.
    pub highlighted: [f32; 4],
    /// A bevel adjacent to a hovered corner.
    pub neighbour: [f32; 4],
    /// The still fainter tint of an away-facing face.
    pub away: [f32; 4],
    /// A resting bevel quad.
    pub bevel: [f32; 4],
    /// A bevel once the pointer is in the gizmo.
    pub bevel_proximity: [f32; 4],
    /// A bevel behind the cube.
    pub bevel_away: [f32; 4],
    /// A resting 1px outline.
    pub edge: [f32; 4],
    /// The outline while the pointer is in the gizmo.
    pub edge_proximity: [f32; 4],
    /// The outline of the hovered piece.
    pub edge_highlighted: [f32; 4],
    /// The outline of a bevel adjacent to a hovered corner.
    pub edge_neighbour: [f32; 4],
    /// The outline of an away-facing face.
    pub edge_away: [f32; 4],
    /// A resting face/Home label.
    pub label: [f32; 4],
    /// A label while the pointer is in the gizmo.
    pub label_proximity: [f32; 4],
    /// The label of the hovered piece.
    pub label_highlighted: [f32; 4],
    /// The ribbon behind a hovered edge.
    pub glow: [f32; 4],
    /// The compass ring at rest.
    pub ring: [f32; 4],
    /// The compass ring while the pointer is in the gizmo.
    pub ring_proximity: [f32; 4],
    /// The compass ring while hovered or scrubbed.
    pub ring_highlighted: [f32; 4],
    /// The dark floor disc under the cube.
    pub turntable: [f32; 4],
    /// The turntable's rim band at rest / with proximity.
    pub ring_fill: [f32; 4],
    pub ring_fill_proximity: [f32; 4],
    /// The rim band while hovered or scrubbed.
    pub ring_fill_highlighted: [f32; 4],
    /// Line/edge width, in logical pixels. Stored for fidelity; the renderer draws 1-px
    /// lines until a wide-line pipeline lands, so this has no visible effect yet.
    pub edge_width: f32,
}

impl NavicubeTheme {
    /// The colour for a role: corners and faces are glass, bevels have their own brighter
    /// machined surface, and a hovered piece takes the cyan highlight.
    pub fn color(&self, role: NavicubeRole) -> [f32; 4] {
        match role {
            NavicubeRole::Face | NavicubeRole::Corner => self.base,
            NavicubeRole::FaceHighlighted | NavicubeRole::CornerHighlighted => self.highlighted,
            NavicubeRole::FaceAway => self.away,
            NavicubeRole::Edge => self.bevel,
            NavicubeRole::EdgeHighlighted => self.highlighted,
            NavicubeRole::EdgeNeighbour => self.neighbour,
        }
    }

    /// The directional glass fill for a **face** in view space: an upper-left studio light
    /// gives the top facet a bright silver tint, the left facet a mid grey, and the right
    /// facet a dark slate that reads as solid volume against the viewport. A hovered face
    /// keeps this crisp fill (the cyan goes on its outline and bevels); away faces keep
    /// the faint glass.
    pub fn face_fill(&self, role: NavicubeRole, normal_view: &Vector3) -> [f32; 4] {
        match role {
            NavicubeRole::FaceAway => self.away,
            NavicubeRole::Face | NavicubeRole::FaceHighlighted => self.face_lit(normal_view),
            _ => self.base,
        }
    }

    /// The lit face tint. `smoothstep` steepens the ramp so the unlit side is genuinely
    /// dark (`#1E2634`-ish) instead of another pale grey.
    fn face_lit(&self, normal_view: &Vector3) -> [f32; 4] {
        let light = Vector3::new(-0.45, 0.8, 0.55).normalize();
        let lit = normal_view.dot(&light).clamp(0.0, 1.0) as f32;
        let t = lit * lit * (3.0 - 2.0 * lit);
        let dark = [0.06, 0.08, 0.12];
        let bright = [0.86, 0.92, 1.0];
        [
            dark[0] + (bright[0] - dark[0]) * t,
            dark[1] + (bright[1] - dark[1]) * t,
            dark[2] + (bright[2] - dark[2]) * t,
            0.36 + 0.06 * t,
        ]
    }

    /// The filled surface of a bevel quad (the chamfer between two face octagons).
    pub fn bevel_color(&self, role: NavicubeRole, back: bool, proximity: bool) -> [f32; 4] {
        match role {
            NavicubeRole::EdgeHighlighted => self.highlighted,
            NavicubeRole::EdgeNeighbour => self.neighbour,
            _ if back => self.bevel_away,
            _ if proximity => self.bevel_proximity,
            _ => self.bevel,
        }
    }

    /// The 1px outline for a role. A hovered piece wins over proximity; an away face
    /// stays dimmer than a resting piece.
    pub fn edge_color(&self, role: NavicubeRole, proximity: bool) -> [f32; 4] {
        match role {
            NavicubeRole::FaceHighlighted
            | NavicubeRole::EdgeHighlighted
            | NavicubeRole::CornerHighlighted => self.edge_highlighted,
            NavicubeRole::EdgeNeighbour => self.edge_neighbour,
            NavicubeRole::FaceAway => self.edge_away,
            _ if proximity => self.edge_proximity,
            _ => self.edge,
        }
    }

    /// The label colour for a role. The filled glyphs are light-on-transparent, so this
    /// is the ink colour times the glyph.
    pub fn label_color(&self, role: NavicubeRole, proximity: bool) -> [f32; 4] {
        match role {
            NavicubeRole::FaceHighlighted => self.label_highlighted,
            _ if proximity => self.label_proximity,
            _ => self.label,
        }
    }

    /// The compass ring's colour: active (hover/scrub) beats proximity beats rest.
    pub fn ring_color(&self, proximity: bool, active: bool) -> [f32; 4] {
        if active {
            self.ring_highlighted
        } else if proximity {
            self.ring_proximity
        } else {
            self.ring
        }
    }

    /// The turntable's thick rim band: same active > proximity > rest priority.
    pub fn ring_fill_color(&self, proximity: bool, active: bool) -> [f32; 4] {
        if active {
            self.ring_fill_highlighted
        } else if proximity {
            self.ring_fill_proximity
        } else {
            self.ring_fill
        }
    }
}

impl Default for NavicubeTheme {
    fn default() -> Self {
        Self {
            base: BASE_COLOR,
            highlighted: HILITE_COLOR,
            neighbour: NEIGHBOUR_COLOR,
            away: INACTIVE_COLOR,
            bevel: BEVEL_COLOR,
            bevel_proximity: BEVEL_PROXIMITY,
            bevel_away: BEVEL_AWAY,
            edge: EDGE_COLOR,
            edge_proximity: EDGE_PROXIMITY,
            edge_highlighted: EDGE_HILITE,
            edge_neighbour: EDGE_NEIGHBOUR,
            edge_away: EDGE_AWAY,
            label: LABEL_COLOR,
            label_proximity: LABEL_PROXIMITY,
            label_highlighted: LABEL_HILITE,
            glow: GLOW_COLOR,
            ring: RING_COLOR,
            ring_proximity: RING_PROXIMITY,
            ring_highlighted: RING_HILITE,
            turntable: TURNTABLE_COLOR,
            ring_fill: RING_FILL,
            ring_fill_proximity: RING_FILL_PROXIMITY,
            ring_fill_highlighted: RING_FILL_HILITE,
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
        assert_eq!(
            piece.role(Some(piece), &[back]),
            NavicubeRole::FaceHighlighted
        );
        assert_eq!(piece.role(None, &[back]), NavicubeRole::Face);
        assert_eq!(back.role(None, &[back]), NavicubeRole::FaceAway);
        assert_eq!(Piece::ALL[6].role(None, &[]), NavicubeRole::Edge);
        assert_eq!(
            Piece::ALL[6].role(Some(Piece::ALL[6]), &[]),
            NavicubeRole::EdgeHighlighted
        );
        // An edge incident to a hovered corner lights softly as a neighbour.
        assert_eq!(
            Piece::ALL[6].role(Some(Piece { axes: [1, 1, 1] }), &[]),
            NavicubeRole::EdgeNeighbour
        );
        // ...but an incident face does not become a neighbour.
        assert_eq!(
            Piece::TOP.role(Some(Piece { axes: [1, 1, 1] }), &[]),
            NavicubeRole::Face
        );
    }

    #[test]
    fn proximity_brightens_at_rest_tokens_but_not_highlights_or_away_pieces() {
        let theme = NavicubeTheme::default();
        assert_eq!(theme.edge_color(NavicubeRole::Face, false), EDGE_COLOR);
        assert_eq!(theme.edge_color(NavicubeRole::Face, true), EDGE_PROXIMITY);
        assert_eq!(theme.label_color(NavicubeRole::Face, false), LABEL_COLOR);
        assert_eq!(theme.label_color(NavicubeRole::Face, true), LABEL_PROXIMITY);
        // A hovered piece wins over proximity; an away face stays dim.
        assert_eq!(
            theme.edge_color(NavicubeRole::FaceHighlighted, false),
            EDGE_HILITE
        );
        assert_eq!(theme.edge_color(NavicubeRole::FaceAway, true), EDGE_AWAY);
    }

    #[test]
    fn the_ring_brightens_with_proximity_and_activity() {
        let theme = NavicubeTheme::default();
        assert_eq!(theme.ring_color(false, false), RING_COLOR);
        assert_eq!(theme.ring_color(true, false), RING_PROXIMITY);
        assert_eq!(theme.ring_color(false, true), RING_HILITE);
        assert_eq!(theme.ring_color(true, true), RING_HILITE);
    }

    #[test]
    fn the_default_theme_matches_the_built_palette() {
        let theme = NavicubeTheme::default();
        assert_eq!(theme.color(NavicubeRole::Face), BASE_COLOR);
        assert_eq!(theme.color(NavicubeRole::FaceHighlighted), HILITE_COLOR);
        assert_eq!(theme.color(NavicubeRole::FaceAway), INACTIVE_COLOR);
        assert_eq!(theme.color(NavicubeRole::Edge), BEVEL_COLOR);
        assert_eq!(theme.color(NavicubeRole::EdgeNeighbour), NEIGHBOUR_COLOR);
        assert_eq!(theme.glow, GLOW_COLOR);
        assert_eq!(theme.turntable, TURNTABLE_COLOR);
        assert_eq!(theme.ring_fill, RING_FILL);
    }
}
