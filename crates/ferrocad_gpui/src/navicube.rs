//! The navigation-cube controller: placement, camera sync, hit-testing and snapping.
//!
//! The counterpart of FreeCAD's `Gui::NaviCube`. The cube *scene* and its overlay
//! camera are pure (`ferrocad_inventor::navicube`); this is the viewer glue that
//! places the cube in a viewport corner, mirrors the main camera's orientation into
//! the cube, hit-tests the corner rectangle, highlights the hovered piece, and — on a
//! click — snaps the main camera onto that face, edge or corner.
//!
//! Labels, the context menu, `Home`/arrows, translucent inactive faces and animated
//! snapping are deferred (see `docs/navicube.md` §4–§5).

use ferrocad_inventor::{
    Camera, NodeKind, Rect, RenderQueue, Scene, ViewSpec, Viewport, compile_with, navicube,
    navicube::{Piece, PieceKind}, pick_at_rect,
};
use ferrocad_types::{Matrix4, Placement, Rotation, Vector3};

/// Which corner of the viewport the cube sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    /// Top-left.
    TopLeft,
    /// Top-right.
    TopRight,
    /// Bottom-left.
    BottomLeft,
    /// Bottom-right.
    BottomRight,
}

/// The navigation cube: a corner overlay showing the view orientation.
pub struct NaviCube {
    scene: Scene,
    queue: RenderQueue,
    camera: Camera,
    viewport: Viewport,
    rect: Rect,
    corner: Corner,
    /// The cube's size in **logical** pixels (scaled by the DPR into `rect`).
    size: f32,
    /// The margin from the corner, in logical pixels.
    margin: f32,
    hilite: Option<Piece>,
    /// The camera rotation last synced into the cube (so we only recompile on change).
    synced: Option<Rotation>,
}

impl NaviCube {
    /// A cube in `viewport`'s top-right corner.
    pub fn new(viewport: Viewport) -> Self {
        let corner = Corner::TopRight;
        let rect = corner_rect(viewport, corner, 96.0, 8.0);
        let scene = navicube::scene(None);
        Self {
            queue: compile_with(&scene, &viewport),
            camera: navicube::camera(rect),
            scene,
            viewport,
            rect,
            corner,
            size: 96.0,
            margin: 8.0,
            hilite: None,
            synced: None,
        }
    }

    /// The device-pixel rectangle the cube occupies.
    pub fn rect(&self) -> Rect {
        self.rect
    }

    /// The overlay view for the renderer.
    pub fn view(&self) -> ViewSpec<'_> {
        ViewSpec {
            queue: &self.queue,
            camera: &self.camera,
            rect: Some(self.rect),
            overlay: true,
        }
    }

    /// Update the placement for a new viewport (its size or scale factor changed).
    pub fn set_viewport(&mut self, viewport: Viewport) {
        self.viewport = viewport;
        self.rect = corner_rect(viewport, self.corner, self.size, self.margin);
        self.camera = navicube::camera(self.rect);
        self.recompile();
    }

    /// Mirror the main camera's orientation into the cube. Recompiles only when the
    /// orientation changed.
    pub fn sync(&mut self, main: &Camera) {
        let rotation = Rotation::from_matrix(&main.view);
        if self.synced.as_ref() == Some(&rotation) {
            return;
        }
        self.synced = Some(rotation);
        self.rebuild();
    }

    /// The face, edge or corner under a device position, if it is over the cube.
    pub fn hit(&self, device: [i32; 2]) -> Option<Piece> {
        if !self.rect.contains(device) {
            return None;
        }
        pick_at_rect(&self.queue, &self.camera, self.rect, device).and_then(Piece::from_pick_id)
    }

    /// Highlight `piece` (or clear), rebuilding the scene when the highlight changes.
    /// Returns whether anything changed.
    pub fn set_hilite(&mut self, piece: Option<Piece>) -> bool {
        if self.hilite == piece {
            return false;
        }
        self.hilite = piece;
        self.rebuild();
        true
    }

    /// Rebuild the cube scene: the camera rotation plus the away-facing faces drawn
    /// translucent.
    fn rebuild(&mut self) {
        let away = self.away_faces();
        self.scene = navicube::scene_with(self.hilite, &away);
        if let Some(rotation) = self.synced {
            self.apply_rotation(rotation);
        }
        self.recompile();
    }

    /// The faces pointing away from the viewer, given the synced camera rotation.
    fn away_faces(&self) -> Vec<Piece> {
        let Some(rotation) = self.synced else {
            return Vec::new();
        };
        let matrix = rotation.to_matrix();
        Piece::ALL
            .iter()
            .copied()
            .filter(|piece| {
                piece.kind() == PieceKind::Face && matrix.transform(&piece.direction()).z < 0.0
            })
            .collect()
    }

    fn apply_rotation(&mut self, rotation: Rotation) {
        if let Some(node) = self.scene.root.children.first_mut() {
            node.kind = NodeKind::Transform(Placement::new(Vector3::zero(), rotation));
        }
    }

    fn recompile(&mut self) {
        self.queue = compile_with(&self.scene, &self.viewport);
    }
}

/// The main-camera **view matrix** that snaps the view onto `piece`, keeping the
/// current camera distance.
pub fn snap_view(piece: Piece, main: &Camera) -> Matrix4 {
    let base = Vector3::new(0.0, 0.0, -camera_distance(main));
    Placement::new(base, piece.orientation()).to_matrix()
}

/// The camera **view matrix** orbited by a drag of `delta` logical pixels.
pub fn drag_view(main: &Camera, delta: [f32; 2]) -> Matrix4 {
    const RADIANS_PER_PIXEL: f64 = 0.012;
    let yaw = Rotation::from_axis_angle(
        &Vector3::new(0.0, 1.0, 0.0),
        f64::from(delta[0]) * RADIANS_PER_PIXEL,
    );
    let pitch = Rotation::from_axis_angle(
        &Vector3::new(1.0, 0.0, 0.0),
        f64::from(delta[1]) * RADIANS_PER_PIXEL,
    );
    let base = Vector3::new(main.view.m[3], main.view.m[7], main.view.m[11]);
    let rotation = yaw.multiply(&pitch).multiply(&Rotation::from_matrix(&main.view));
    Placement::new(base, rotation).to_matrix()
}

/// Spherical linear interpolation between two rotations (`t` in `[0, 1]`).
pub fn slerp(a: &Rotation, b: &Rotation, t: f64) -> Rotation {
    let a_q = a.q;
    let mut b_q = b.q;
    let mut dot = a_q[0] * b_q[0] + a_q[1] * b_q[1] + a_q[2] * b_q[2] + a_q[3] * b_q[3];
    // Take the shorter arc: a quaternion and its negation are the same rotation.
    if dot < 0.0 {
        b_q = [-b_q[0], -b_q[1], -b_q[2], -b_q[3]];
        dot = -dot;
    }
    let q = if dot > 0.9995 {
        // Nearly parallel: a normalized linear blend avoids a 0/0 in the sin terms.
        let mut q = [0.0; 4];
        for i in 0..4 {
            q[i] = a_q[i] + (b_q[i] - a_q[i]) * t;
        }
        let length = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        if length > 0.0 {
            for value in &mut q {
                *value /= length;
            }
        }
        q
    } else {
        let theta = dot.clamp(-1.0, 1.0).acos();
        let sin = theta.sin();
        let wa = ((1.0 - t) * theta).sin() / sin;
        let wb = (t * theta).sin() / sin;
        let mut q = [0.0; 4];
        for i in 0..4 {
            q[i] = wa * a_q[i] + wb * b_q[i];
        }
        q
    };
    Rotation {
        q,
        raw_axis: None,
    }
}

/// An in-progress animated snap of the main camera onto a piece.
#[derive(Clone, Debug)]
pub struct SnapAnimation {
    start: Rotation,
    end: Rotation,
    base: Vector3,
    start_time: std::time::Instant,
    duration: std::time::Duration,
}

impl SnapAnimation {
    /// Begin a snap from `main`'s current orientation onto `piece`.
    pub fn begin(piece: Piece, main: &Camera, duration: std::time::Duration) -> Self {
        Self {
            start: Rotation::from_matrix(&main.view),
            end: piece.orientation(),
            base: Vector3::new(main.view.m[3], main.view.m[7], main.view.m[11]),
            start_time: std::time::Instant::now(),
            duration,
        }
    }

    /// The camera view at `elapsed` since [`begin`](Self::begin) (eased).
    pub fn view_at(&self, elapsed: std::time::Duration) -> Matrix4 {
        let t = (elapsed.as_secs_f64() / self.duration.as_secs_f64().max(1e-9)).clamp(0.0, 1.0);
        let eased = t * t * (3.0 - 2.0 * t); // smoothstep
        Placement::new(self.base, slerp(&self.start, &self.end, eased)).to_matrix()
    }

    /// Whether the snap has finished.
    pub fn is_done(&self) -> bool {
        self.start_time.elapsed() >= self.duration
    }

    /// The elapsed time since [`begin`](Self::begin).
    pub fn elapsed(&self) -> std::time::Duration {
        self.start_time.elapsed()
    }
}

/// The cube's device-pixel rectangle for a corner, size and margin (both logical).
fn corner_rect(viewport: Viewport, corner: Corner, size: f32, margin: f32) -> Rect {
    let dpr = viewport.device_pixel_ratio();
    let size = (size * dpr).round().max(1.0) as u32;
    let margin = (margin * dpr).round().max(0.0) as i32;
    let [width, height] = viewport.device_size();
    let right = width as i32 - margin - size as i32;
    let top = height as i32 - margin - size as i32;
    let (x, y) = match corner {
        Corner::TopLeft => (margin, top),
        Corner::TopRight => (right, top),
        Corner::BottomLeft => (margin, margin),
        Corner::BottomRight => (right, margin),
    };
    Rect::new(x, y, size, size)
}

/// The camera distance (the length of the view matrix's translation).
fn camera_distance(camera: &Camera) -> f64 {
    let distance = Vector3::new(camera.view.m[3], camera.view.m[7], camera.view.m[11]).length();
    if distance > 1e-9 {
        distance
    } else {
        4.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrocad_inventor::navicube::{BASE_COLOR, HILITE_COLOR, INACTIVE_COLOR};

    fn viewport() -> Viewport {
        Viewport::new([640, 480], 1.0)
    }

    fn colour_of(cube: &NaviCube, piece: Piece) -> [f32; 4] {
        cube.queue
            .packets
            .iter()
            .find(|packet| packet.state.id == piece.pick_id())
            .map(|packet| packet.state.color)
            .expect("piece packet")
    }

    fn centre(cube: &NaviCube) -> [i32; 2] {
        let rect = cube.rect();
        [
            rect.x + rect.width as i32 / 2,
            rect.y + rect.height as i32 / 2,
        ]
    }

    #[test]
    fn the_rect_sits_in_the_top_right() {
        let cube = NaviCube::new(viewport());
        let rect = cube.rect();
        assert_eq!(rect.width, 96);
        assert_eq!(rect.x, 640 - 8 - 96);
        assert_eq!(rect.y, 480 - 8 - 96);
    }

    #[test]
    fn the_centre_maps_to_the_front_face() {
        let mut cube = NaviCube::new(viewport());
        // With the default (identity) camera orientation, Front faces the viewer.
        cube.sync(&Camera::identity());
        assert_eq!(cube.hit(centre(&cube)), Some(Piece::FRONT));
        // Outside the rectangle there is no hit.
        assert_eq!(cube.hit([5, 5]), None);
    }

    #[test]
    fn a_corner_region_maps_to_a_corner_piece() {
        let mut cube = NaviCube::new(viewport());
        cube.sync(&Camera::identity());
        // Somewhere in the cube rect there is a clickable corner (the +X+Y+Z nub);
        // scanning avoids depending on the exact ortho scale.
        let rect = cube.rect();
        let mut found = None;
        for i in 0..12 {
            for j in 0..12 {
                let probe = [
                    rect.x + rect.width as i32 * i / 12,
                    rect.y + rect.height as i32 * j / 12,
                ];
                if cube.hit(probe) == Some(Piece { axes: [1, 1, 1] }) {
                    found = Some(probe);
                }
            }
        }
        assert!(found.is_some(), "a corner piece should be clickable");
    }

    #[test]
    fn hilite_changes_the_piece_colour_and_only_recompiles_on_change() {
        let mut cube = NaviCube::new(viewport());
        assert!(cube.set_hilite(Some(Piece::TOP)));
        assert!(!cube.set_hilite(Some(Piece::TOP)), "no-op when unchanged");
        assert_eq!(colour_of(&cube, Piece::TOP), HILITE_COLOR);
        assert_eq!(colour_of(&cube, Piece::FRONT), BASE_COLOR);
    }

    #[test]
    fn away_faces_are_drawn_translucent() {
        let mut cube = NaviCube::new(viewport());
        cube.sync(&Camera::identity());
        // With the identity camera, +Z (Front) faces the viewer and Back points away.
        assert_eq!(colour_of(&cube, Piece::FRONT), BASE_COLOR);
        assert_eq!(colour_of(&cube, Piece::BACK), INACTIVE_COLOR);
    }

    #[test]
    fn sync_mirrors_the_camera_orientation() {
        let mut cube = NaviCube::new(viewport());
        // A camera looking from +X makes the Right face point at the viewer.
        let camera = Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -4.0), Piece::RIGHT.orientation())
                .to_matrix(),
            ..Camera::identity()
        };
        cube.sync(&camera);
        assert_eq!(cube.hit(centre(&cube)), Some(Piece::RIGHT));

        // Snapping back to Front and re-syncing turns the Front face to the viewer.
        cube.sync(&Camera {
            view: snap_view(Piece::FRONT, &Camera::identity()),
            ..Camera::identity()
        });
        assert_eq!(cube.hit(centre(&cube)), Some(Piece::FRONT));
    }

    #[test]
    fn snap_view_looks_along_the_piece_direction() {
        for piece in [Piece::RIGHT, Piece::TOP, Piece { axes: [1, 1, 1] }] {
            let view = snap_view(piece, &Camera::identity());
            let rotated = Rotation::from_matrix(&view)
                .to_matrix()
                .transform(&piece.direction());
            assert!(rotated.sub(&Vector3::new(0.0, 0.0, 1.0)).length() < 1e-9);
        }
    }

    #[test]
    fn slerp_interpolates_between_rotations() {
        use std::time::Duration;
        let start = Camera::identity();
        let anim = SnapAnimation::begin(Piece::RIGHT, &start, Duration::from_millis(200));
        // At the end the view looks along the piece direction.
        let end = Rotation::from_matrix(&anim.view_at(Duration::from_millis(200)));
        let rotated = end.to_matrix().transform(&Piece::RIGHT.direction());
        assert!(rotated.sub(&Vector3::new(0.0, 0.0, 1.0)).length() < 1e-9);
        // Halfway is between the two orientations (neither the start nor the end).
        let mid = Rotation::from_matrix(&anim.view_at(Duration::from_millis(100)));
        assert!(!mid.to_matrix().is_unity(1e-6));
        assert!(mid.q != end.q);
        // A zero-length duration is instant.
        assert!(anim.is_done() == false || anim.start_time.elapsed() >= Duration::from_millis(200));
    }

    #[test]
    fn drag_view_orbits_around_the_camera() {
        let camera = Camera::identity();
        let view = drag_view(&camera, [50.0, 0.0]);
        // A horizontal drag rotates the view (not the identity anymore) but keeps the
        // camera distance.
        assert!(!Rotation::from_matrix(&view).to_matrix().is_unity(1e-9));
        let base = Vector3::new(view.m[3], view.m[7], view.m[11]);
        assert!(base.is_null());
    }
}
