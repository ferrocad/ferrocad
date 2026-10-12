//! The navigation-cube controller: placement, camera sync, hit-testing and snapping.
//!
//! The counterpart of FreeCAD's `Gui::NaviCube`. The cube *scene* and its overlay
//! camera are pure (`ferrocad_inventor::navicube`); this is the viewer glue that
//! places the cube in a viewport corner, mirrors the main camera's orientation into
//! the cube, hit-tests the corner rectangle, highlights the hovered face, and — on a
//! click — snaps the main camera onto that face.
//!
//! Labels, the edge/corner picks, drag-rotate, animation and `Home`/arrows are
//! deferred (see `docs/navicube.md` §4–§5).

use ferrocad_inventor::{
    Camera, NodeKind, Rect, RenderQueue, Scene, ViewSpec, Viewport, compile_with, navicube,
    navicube::Face, pick_at_rect,
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
    hilite: Option<Face>,
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
        self.apply_rotation(rotation);
        self.recompile();
    }

    /// The face under a device position, if it is over the cube.
    pub fn hit(&self, device: [i32; 2]) -> Option<Face> {
        if !self.rect.contains(device) {
            return None;
        }
        pick_at_rect(&self.queue, &self.camera, self.rect, device).and_then(Face::from_pick_id)
    }

    /// Highlight `face` (or clear), rebuilding the scene when the highlight changes.
    /// Returns whether anything changed.
    pub fn set_hilite(&mut self, face: Option<Face>) -> bool {
        if self.hilite == face {
            return false;
        }
        self.hilite = face;
        self.scene = navicube::scene(face);
        if let Some(rotation) = self.synced {
            self.apply_rotation(rotation);
        }
        self.recompile();
        true
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

/// The main-camera **view matrix** that snaps the view onto `face`, keeping the
/// current camera distance.
pub fn snap_view(face: Face, main: &Camera) -> Matrix4 {
    let base = Vector3::new(0.0, 0.0, -camera_distance(main));
    Placement::new(base, face.orientation()).to_matrix()
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
    use ferrocad_inventor::navicube::{HILITE_COLOR, BASE_COLOR};

    fn viewport() -> Viewport {
        Viewport::new([640, 480], 1.0)
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
        let rect = cube.rect();
        let centre = [rect.x + rect.width as i32 / 2, rect.y + rect.height as i32 / 2];
        assert_eq!(cube.hit(centre), Some(Face::Front));
        // Outside the rectangle there is no hit.
        assert_eq!(cube.hit([5, 5]), None);
    }

    #[test]
    fn hilite_changes_the_face_colour_and_only_recompiles_on_change() {
        let mut cube = NaviCube::new(viewport());
        assert!(cube.set_hilite(Some(Face::Top)));
        assert!(!cube.set_hilite(Some(Face::Top)), "no-op when unchanged");
        let top = cube
            .queue
            .packets
            .iter()
            .find(|packet| packet.state.id == Face::Top.pick_id())
            .expect("top packet");
        assert_eq!(top.state.color, HILITE_COLOR);
        let front = cube
            .queue
            .packets
            .iter()
            .find(|packet| packet.state.id == Face::Front.pick_id())
            .expect("front packet");
        assert_eq!(front.state.color, BASE_COLOR);
    }

    #[test]
    fn sync_mirrors_the_camera_orientation() {
        let mut cube = NaviCube::new(viewport());
        // A camera looking from +X makes the Right face point at the viewer.
        let camera = Camera {
            view: Placement::new(Vector3::new(0.0, 0.0, -4.0), Face::Right.orientation())
                .to_matrix(),
            ..Camera::identity()
        };
        cube.sync(&camera);
        let rect = cube.rect();
        let centre = [
            rect.x + rect.width as i32 / 2,
            rect.y + rect.height as i32 / 2,
        ];
        assert_eq!(cube.hit(centre), Some(Face::Right));

        // Snapping back to Front and re-syncing turns the Front face to the viewer.
        cube.sync(&Camera {
            view: snap_view(Face::Front, &Camera::identity()),
            ..Camera::identity()
        });
        assert_eq!(cube.hit(centre), Some(Face::Front));
    }

    #[test]
    fn snap_view_looks_along_the_face_normal() {
        let view = snap_view(Face::Right, &Camera::identity());
        // The view rotation maps the face normal onto +Z (toward the viewer).
        let rotated = Rotation::from_matrix(&view).to_matrix().transform(&Face::Right.normal());
        assert!(rotated.sub(&Vector3::new(0.0, 0.0, 1.0)).length() < 1e-9);
    }
}
