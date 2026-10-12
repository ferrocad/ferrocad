//! The navigation cube: a small orientation gizmo for the viewer corner.
//!
//! The counterpart of FreeCAD's `Gui::NaviCube` + `SoNaviCube`: a cube whose faces
//! point along the world axes, drawn as a screen-space overlay and rotated to mirror
//! the main camera's orientation. Clicking a face reorients the view onto it.
//!
//! This module is the **pure** part — the cube scene and its orthographic overlay
//! camera. The controller (placement, camera sync, hit-testing, snapping) lives in
//! `ferrocad_gpui::navicube`, and the overlay render pass in `ferrocad_wgpu`.
//!
//! Our axes follow the engine's default view: **+Z toward the viewer**, **+Y up**,
//! **+X right**. (FreeCAD is Z-up; the face *names* differ, the behaviour does not.)

use ferrocad_types::{Matrix4, Placement, Rotation, Vector3};

use crate::batch::PickId;
use crate::camera::Camera;
use crate::geometry::Topology;
use crate::node::{Node, NodeKind, Scene};
use crate::pixels::{Rect, Viewport};

/// Half the extent of the cube (each face sits at ±`CUBE_HALF`).
const CUBE_HALF: f64 = 0.5;
/// The half-extent of each face quad, inset from the cube edge so the gaps read as
/// edges.
const FACE_INSET: f64 = 0.42;
/// The orthographic camera's half-extent: how much of the cube space fills the rect.
const ORTHO_EXTENT: f64 = 1.1;
/// How far the fixed overlay camera sits along +Z.
const CAMERA_DISTANCE: f64 = 3.0;

/// The cube's resting face colour.
pub const BASE_COLOR: [f32; 4] = [0.55, 0.6, 0.66, 1.0];
/// The colour a face takes while hovered.
pub const HILITE_COLOR: [f32; 4] = [0.2, 0.6, 0.95, 1.0];

/// A cube face, named by the world direction it faces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    /// Toward the viewer (+Z).
    Front,
    /// Away from the viewer (-Z).
    Back,
    /// Up (+Y).
    Top,
    /// Down (-Y).
    Bottom,
    /// Right (+X).
    Right,
    /// Left (-X).
    Left,
}

impl Face {
    /// Every face, in pick-id order.
    pub const ALL: [Face; 6] = [
        Face::Front,
        Face::Back,
        Face::Top,
        Face::Bottom,
        Face::Right,
        Face::Left,
    ];

    /// The outward world normal.
    pub fn normal(self) -> Vector3 {
        match self {
            Face::Front => Vector3::new(0.0, 0.0, 1.0),
            Face::Back => Vector3::new(0.0, 0.0, -1.0),
            Face::Top => Vector3::new(0.0, 1.0, 0.0),
            Face::Bottom => Vector3::new(0.0, -1.0, 0.0),
            Face::Right => Vector3::new(1.0, 0.0, 0.0),
            Face::Left => Vector3::new(-1.0, 0.0, 0.0),
        }
    }

    /// The pick id this face carries (1-based; the scene's default id is 0).
    pub fn pick_id(self) -> PickId {
        PickId(match self {
            Face::Front => 1,
            Face::Back => 2,
            Face::Top => 3,
            Face::Bottom => 4,
            Face::Right => 5,
            Face::Left => 6,
        })
    }

    /// The face carrying `id`, if any.
    pub fn from_pick_id(id: PickId) -> Option<Face> {
        Face::ALL.into_iter().find(|face| face.pick_id() == id)
    }

    /// The camera **view rotation** that turns this face toward the viewer (maps the
    /// face normal onto +Z).
    pub fn orientation(self) -> Rotation {
        Rotation::from_vectors(self.normal(), Vector3::new(0.0, 0.0, 1.0))
    }

    /// Two in-plane unit axes `(u, v)` with `u × v = normal`.
    fn tangents(self) -> (Vector3, Vector3) {
        let x = Vector3::new(1.0, 0.0, 0.0);
        let y = Vector3::new(0.0, 1.0, 0.0);
        let z = Vector3::new(0.0, 0.0, 1.0);
        match self {
            Face::Front => (x, y),
            Face::Back => (x, y.neg()),
            Face::Right => (y, z),
            Face::Left => (y, z.neg()),
            Face::Top => (z, x),
            Face::Bottom => (z, x.neg()),
        }
    }

    /// The four corners of this face's (inset) quad, wound so the compiler's flat
    /// normal points outward.
    fn quad(self) -> Vec<[f32; 3]> {
        let center = self.normal().scale(CUBE_HALF);
        let (u, v) = self.tangents();
        let corner = |su: f64, sv: f64| {
            let p = center
                .add(&u.scale(su * FACE_INSET))
                .add(&v.scale(sv * FACE_INSET));
            [p.x as f32, p.y as f32, p.z as f32]
        };
        vec![
            corner(-1.0, -1.0),
            corner(1.0, -1.0),
            corner(1.0, 1.0),
            corner(-1.0, 1.0),
        ]
    }
}

/// Build the cube scene: a synced transform, then one inset quad per face, each
/// carrying its [`Face::pick_id`]. The face in `hilite` is drawn in [`HILITE_COLOR`].
///
/// The first child is always the [`NodeKind::Transform`] the controller writes the
/// camera orientation into.
pub fn scene(hilite: Option<Face>) -> Scene {
    let mut children = vec![Node {
        kind: NodeKind::Transform(Placement::identity()),
        children: vec![],
    }];
    for face in Face::ALL {
        let color = if Some(face) == hilite {
            HILITE_COLOR
        } else {
            BASE_COLOR
        };
        children.push(field(NodeKind::PickId(face.pick_id().0)));
        children.push(field(NodeKind::Material(vec![color])));
        children.push(field(NodeKind::Coordinate3(face.quad())));
        children.push(field(NodeKind::Mesh {
            indices: vec![0, 1, 2, 0, 2, 3],
            normal_indices: vec![],
            topology: Topology::Triangles,
        }));
    }
    Scene {
        root: Node {
            kind: NodeKind::Separator,
            children,
        },
    }
}

fn field(kind: NodeKind) -> Node {
    Node {
        kind,
        children: vec![],
    }
}

/// The fixed orthographic overlay camera for `rect`, looking down −Z at the cube at
/// the origin.
pub fn camera(rect: Rect) -> Camera {
    Camera {
        view: Placement::new(Vector3::new(0.0, 0.0, -CAMERA_DISTANCE), Rotation::identity())
            .to_matrix(),
        projection: ortho(ORTHO_EXTENT),
        viewport: Viewport::new([rect.width.max(1), rect.height.max(1)], 1.0),
    }
}

/// A symmetric orthographic projection mapping `[-extent, extent]` to the clip volume
/// (wgpu's `z ∈ [0, 1]`).
fn ortho(extent: f64) -> Matrix4 {
    let (near, far) = (0.1_f64, 100.0_f64);
    let mut m = [0.0_f64; 16];
    m[0] = 1.0 / extent;
    m[5] = 1.0 / extent;
    m[10] = 1.0 / (near - far);
    m[11] = near / (near - far);
    m[15] = 1.0;
    Matrix4::from_values(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::compile;

    #[test]
    fn face_ids_round_trip() {
        for face in Face::ALL {
            assert_eq!(Face::from_pick_id(face.pick_id()), Some(face));
        }
        assert_eq!(Face::from_pick_id(PickId(0)), None);
        assert_eq!(Face::from_pick_id(PickId(99)), None);
    }

    #[test]
    fn face_orientation_turns_the_normal_to_the_viewer() {
        for face in Face::ALL {
            let m = face.orientation().to_matrix();
            let rotated = m.transform(&face.normal());
            assert!(
                rotated.sub(&Vector3::new(0.0, 0.0, 1.0)).length() < 1e-9,
                "{face:?} maps to {rotated:?}"
            );
        }
    }

    #[test]
    fn scene_compiles_to_six_distinct_faces() {
        let queue = compile(&scene(None));
        assert_eq!(queue.len(), 6, "one packet per face");
        let ids: Vec<u32> = queue.packets.iter().map(|p| p.state.id.0).collect();
        assert_eq!(ids, vec![1, 2, 3, 4, 5, 6]);
        // The highlight recolours exactly one face.
        let queue = compile(&scene(Some(Face::Top)));
        let top = queue.packets.iter().find(|p| p.state.id == Face::Top.pick_id());
        assert_eq!(top.unwrap().state.color, HILITE_COLOR);
    }
}
