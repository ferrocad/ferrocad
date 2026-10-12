//! The state accumulator and the snapshot it resolves to.
//!
//! Coin's actions carry a working state as they traverse; a `SoMaterial` sets the
//! current colour, a `SoTransform` composes the current transform, and a leaf shape
//! reads the state in effect where it sits. [`State`] is that working state, the
//! traversal in [`compile`](crate::compile) advances it, and [`StateSnapshot`] is
//! the frozen result bound to a [`DrawPacket`](crate::DrawPacket).

use std::sync::Arc;

use ferrocad_types::Matrix4;

use crate::batch::PickId;
use crate::geometry::Texture2;

/// The inherited state at a point in the scene graph, as the traversal advances it.
#[derive(Clone, Debug)]
pub struct State {
    /// The world transform accumulated from enclosing `SoTransform` nodes.
    ///
    /// Held as a matrix, not a `Placement`, because `Placement::mul` is a POC stub
    /// that drops the rotation; composing matrices is correct and matches the shader.
    pub transform: Matrix4,
    /// The colour in effect (`SoMaterial`/`SoBaseColor`).
    pub color: [f32; 4],
    /// Depth bias for the following shapes (`SoPolygonOffset`).
    pub depth_bias: f32,
    /// The texture bound for the following shapes (`SoTexture2`).
    pub texture: Option<Arc<Texture2>>,
    /// The selection identity of the shape being traversed.
    pub id: PickId,
    /// Device pixels per logical pixel (the display scale factor).
    ///
    /// The analogue of Coin's `SoDevicePixelRatioElement`: screen-space nodes (labels,
    /// gizmos, annotation offsets) multiply authored logical sizes by this so they
    /// appear the right size on a physical viewport. World-space geometry ignores it.
    pub device_pixel_ratio: f32,
    /// Whether shapes here are rendered and pickable (`SoSwitch`, draw style).
    pub visible: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            transform: Matrix4::identity(),
            color: [1.0, 1.0, 1.0, 1.0],
            depth_bias: 0.0,
            texture: None,
            id: PickId::default(),
            device_pixel_ratio: 1.0,
            visible: true,
        }
    }
}

impl State {
    /// Freeze the current state into the snapshot a [`DrawPacket`] carries.
    ///
    /// [`DrawPacket`]: crate::DrawPacket
    pub fn snapshot(&self) -> StateSnapshot {
        StateSnapshot {
            world_transform: self.transform,
            color: self.color,
            depth_bias: self.depth_bias,
            texture: self.texture.clone(),
            id: self.id,
        }
    }
}

/// The resolved traversal state bound to a draw, independent of the node model.
#[derive(Clone, Debug)]
pub struct StateSnapshot {
    /// The world transform to apply to the mesh.
    pub world_transform: Matrix4,
    /// Linear RGBA colour.
    pub color: [f32; 4],
    /// Depth bias applied before rasterisation.
    pub depth_bias: f32,
    /// The texture to sample, if any (multiplied into the colour).
    pub texture: Option<Arc<Texture2>>,
    /// Selection identity, written to the picking buffer.
    pub id: PickId,
}
