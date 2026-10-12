//! G-buffer picking: render pick ids offscreen, read them back per pixel.
//!
//! The scene is drawn once more with the pick pipeline, encoding each packet's
//! [`StateSnapshot::id`](ferrocad_inventor::StateSnapshot::id) into a colour target.
//! A click reads one texel to learn which object is under the cursor, which is the
//! selection path workbench code relies on.

/// The offscreen target the pick pass renders into.
pub struct PickTarget {
    /// The id-encoding colour target.
    pub texture: wgpu::Texture,
    /// The view the pick pipeline renders into.
    pub view: wgpu::TextureView,
}

/// Read the pick id at pixel `(x, y)` of `target`, or `None` for empty space.
///
/// TODO: copy the texel back with `wgpu::CommandEncoder::copy_texture_to_buffer`
/// (or a compute read) and decode it; `None` until the pick pass lands.
pub fn pick_id_at(target: &PickTarget, x: u32, y: u32) -> Option<u32> {
    let _ = (target, x, y);
    None
}
