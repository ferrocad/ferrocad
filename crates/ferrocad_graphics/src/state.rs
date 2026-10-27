//! The frozen per-draw state a renderer binds: transform, colour, texture, identity,
//! and the shader program.
//!
//! This is the neutral half of the state accumulator: the working [`State`] that a
//! traversal advances lives with the traversal (in `ferrocad_inventor`), but the
//! *snapshot* it resolves to — the thing a [`DrawPacket`](crate::DrawPacket) carries —
//! is shared by every producer.
//!
//! [`ShaderProgram`] lives here for the same reason [`Texture2`](crate::Texture2) does:
//! it is part of the neutral draw contract and `ferrocad_graphics` is the leaf both
//! producers compile down to. It carries **inline WGSL**, not a node model, so a
//! renderer never learns what a Coin shader node is.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use ferrocad_types::Matrix4;

use crate::batch::PickId;
use crate::geometry::Texture2;

/// The number of ground-shadow slots in the fixed program ABI.
///
/// Must match `SHADOW_MAX` in `mods/GridOverlayCoin/Resources/grid_overlay.wgsl`.
pub const SHADOW_MAX: usize = 64;

/// The number of floats per shadow slot.
///
/// Must match `SHADOW_STRIDE` in `mods/GridOverlayCoin/Resources/grid_overlay.wgsl`.
pub const SHADOW_STRIDE: usize = 5;

/// The number of `vec4`s the shadow block occupies in the uniform.
///
/// A WGSL uniform array element is 16-byte aligned, so the shader's 320 flat floats
/// are stored as 80 `vec4`s. Flat float `o` lives at `shadows[o / 4][o % 4]`.
pub const SHADOW_VECS: usize = SHADOW_MAX * SHADOW_STRIDE / 4;

// The shader's flat-float indexing assumes the slot count divides evenly into vec4s.
const _: () = assert!(SHADOW_MAX * SHADOW_STRIDE == SHADOW_VECS * 4);

/// The smallest half-extent the producer writes (world units).
///
/// The shader divides by the half-extent, so a degenerate (zero-area) footprint would
/// produce infinities; a hair of extent keeps the `sdf` finite.
const SHADOW_MIN_EXTENT: f32 = 1e-4;

/// One ground-shadow footprint: an axis-aligned ellipse on the world `z = 0` plane.
///
/// The slot layout is the shader's, not a new one: `center.xy`, `half_extent.xy`, then
/// `blur`, all in **world units**. The shader composites a soft ellipse under the grid
/// lines; `blur` is its world-space falloff radius.
///
/// The ABI has **one global strength** (`ProgramParameters::shadow_opacity`), not a
/// per-slot opacity, so a producer that knows per-object weights can only express them
/// through the shared knob. Do not repurpose [`blur`](Self::blur) for opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowFootprint {
    /// The ellipse centre in world `(x, y)`.
    pub center: [f32; 2],
    /// The ellipse semi-axes in world units (`x`, `y`).
    pub half_extent: [f32; 2],
    /// The world-space softness of the edge.
    pub blur: f32,
}

impl ShadowFootprint {
    /// A footprint covering the world-space box `min`..=`max` on the ground plane.
    ///
    /// Only `x`/`y` are significant: the shadow is cast onto `z = 0`, so a scene's
    /// vertical extent is flattened into its ground footprint.
    pub fn from_bounds(min: [f32; 2], max: [f32; 2], blur: f32) -> Self {
        Self {
            center: [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5],
            half_extent: [
                (max[0] - min[0]).abs() * 0.5,
                (max[1] - min[1]).abs() * 0.5,
            ],
            blur,
        }
    }

    /// Clamp a footprint into the range the shader can evaluate without infinities.
    fn sanitized(self) -> Self {
        let axis = |center: f32| if center.is_finite() { center } else { 0.0 };
        let extent = |half: f32| {
            if half.is_finite() {
                half.abs().max(SHADOW_MIN_EXTENT)
            } else {
                SHADOW_MIN_EXTENT
            }
        };
        Self {
            center: [axis(self.center[0]), axis(self.center[1])],
            half_extent: [extent(self.half_extent[0]), extent(self.half_extent[1])],
            blur: if self.blur.is_finite() {
                self.blur.max(0.0).max(1e-6)
            } else {
                1e-6
            },
        }
    }
}

/// Fill the fixed ABI's shadow block from `footprints` and return the slot count.
///
/// This is the **only** supported way to populate [`ProgramParameters::shadow_data`].
/// The producer is deterministic and bounded:
///
/// * Slots are filled in iteration order; the first [`SHADOW_MAX`] (64) footprints are
///   used and any extra are ignored (the count clamps, it never overflows the block).
/// * Every slot is cleared first, so a smaller scene cannot inherit stale footprints.
/// * [`ProgramParameters::shadow_count`] is set to the number of slots written, which is
///   what the shader's loop reads; it is `0` for an empty scene.
/// * Footprints are clamped to finite values with a minimum half-extent, so the shader's
///   per-pixel `sdf` cannot divide by zero.
///
/// The coordinate space is **world `(x, y)`** on the ground plane `z = 0`; the shader
/// tests the fragment's world position against these slots.
pub fn apply_shadows(
    parameters: &mut ProgramParameters,
    footprints: impl IntoIterator<Item = ShadowFootprint>,
) -> usize {
    parameters.shadow_data = [[0.0; 4]; SHADOW_VECS];
    let mut count = 0;
    for footprint in footprints {
        if count == SHADOW_MAX {
            break;
        }
        let footprint = footprint.sanitized();
        let offset = count * SHADOW_STRIDE;
        let floats = [
            footprint.center[0],
            footprint.center[1],
            footprint.half_extent[0],
            footprint.half_extent[1],
            footprint.blur,
        ];
        for (index, value) in floats.into_iter().enumerate() {
            let flat = offset + index;
            parameters.shadow_data[flat / 4][flat % 4] = value;
        }
        count += 1;
    }
    parameters.shadow_count = count as f32;
    count
}

/// The parameter surface of the fixed program ABI (`docs/developer/workbenches/grid-overlay-plan.md` §3.4).
///
/// A WGSL uniform block has a static layout, so an authored program cannot invent
/// names: the compiler maps the Coin `SoShaderParameter*` nodes onto these fields by
/// name. The renderer writes them into group 2, binding 1 (`Program`).
///
/// The defaults are the grid overlay's FerroCAD look: the neutral grid ink at low
/// alpha, a 1-unit base spacing, and no shadows (an empty block). A scene producer
/// fills the shadow block through [`apply_shadows`]. The grid shader itself is
/// mod-owned: `mods/GridOverlayCoin/Resources/grid_overlay.wgsl`.
///
/// The axis fields are a backward-compatible tail: the grid shader reads only the
/// prefix, while `shaders/axis_overlay.wgsl` reads the whole block. X reuses
/// [`color`](Self::color); `axis_y`/`axis_z` are the other two stem inks in linear
/// light. The defaults match the native `AxisStyle` (`#4ADE80`, `#38BDF8`, 80 px,
/// 1.5 px, 0.65 opacity, 0.10 ghost alpha, origin and labels on).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProgramParameters {
    /// Linear RGB line ink (`Program.grid_color`), also the X axis stem's ink.
    pub color: [f32; 3],
    /// World units of the finest decade (`SoShaderParameter1f "base_spacing"`).
    pub base_spacing: f32,
    /// Coverage of the minor lines (`"minor_alpha"`).
    pub minor_alpha: f32,
    /// Coverage of the major lines (`"major_alpha"`).
    pub major_alpha: f32,
    /// Shadow strength (`"shadow_opacity"`); `0` disables the shadow loop.
    pub shadow_opacity: f32,
    /// Number of populated shadow slots (`"shadow_count"`); written by
    /// [`apply_shadows`]. The shader reads it from `counts.x` and stops its loop there.
    pub shadow_count: f32,
    /// The packed shadow slots (`"shadow_data"`), laid out exactly like the shader's
    /// `Program.shadows`: 64 slots of `[center.x, center.y, half_extent.x,
    /// half_extent.y, blur]` with flat float `o` at `shadow_data[o / 4][o % 4]`.
    ///
    /// Populate it with [`apply_shadows`]; the unused tail is always zeroed.
    pub shadow_data: [[f32; 4]; SHADOW_VECS],
    /// The Y axis stem's linear RGB ink (`"axis_color_y"`).
    pub axis_y: [f32; 3],
    /// The Z axis stem's linear RGB ink (`"axis_color_z"`).
    pub axis_z: [f32; 3],
    /// The stem length in device pixels (`"axis_length"`).
    pub axis_length: f32,
    /// The stem width in device pixels (`"axis_width"`).
    pub axis_width: f32,
    /// The lit stems' opacity (`"axis_opacity"`).
    pub axis_opacity: f32,
    /// The occluded stems' alpha multiplier (`"axis_ghost_alpha"`). Accepted by the
    /// ABI even though the Coin backend's depth-tested visibility pass cannot use it.
    pub axis_ghost_alpha: f32,
    /// Whether the origin node is drawn (`"axis_origin"`); `1` is on.
    pub axis_origin: f32,
    /// Whether the axis labels are drawn (`"axis_labels"`); `1` is on. Accepted by
    /// the ABI; the Coin shader cannot draw text, so the native labels own this.
    pub axis_labels: f32,
}

impl Default for ProgramParameters {
    fn default() -> Self {
        Self {
            // #94A3B8 in linear light, the native grid's ink token.
            color: [0.296, 0.366, 0.480],
            base_spacing: 1.0,
            minor_alpha: 0.15,
            major_alpha: 0.35,
            shadow_opacity: 0.0,
            shadow_count: 0.0,
            shadow_data: [[0.0; 4]; SHADOW_VECS],
            // #4ADE80 and #38BDF8 in linear light, the native axis tokens.
            axis_y: [0.069, 0.730, 0.216],
            axis_z: [0.040, 0.509, 0.939],
            axis_length: 80.0,
            axis_width: 1.5,
            axis_opacity: 0.65,
            axis_ghost_alpha: 0.10,
            axis_origin: 1.0,
            axis_labels: 1.0,
        }
    }
}

/// A resolved shader program: inline WGSL plus the fixed-ABI parameters.
///
/// A program is compared and hashed by its **content fingerprint** (source bytes,
/// parameters and geometry mode), never by the full source or by pointer identity. The
/// fingerprint is a 64-bit FNV-1a digest, which is what a pipeline cache keys on; two
/// scene nodes that build the same program therefore share a cache entry, while a
/// one-byte edit misses. The name is descriptive only and does not participate in
/// equality.
///
/// A **plain** program ignores the packet's geometry and draws the fixed ABI's
/// placeholder quad; a **geometry** program (see [`with_geometry`](Self::with_geometry))
/// draws the packet's own mesh with its position/normal/uv/colour vertex attributes.
/// The mode is part of the fingerprint because the two need different vertex state and
/// therefore different pipelines.
#[derive(Clone, Debug)]
pub struct ShaderProgram {
    name: Arc<str>,
    source: Arc<str>,
    parameters: ProgramParameters,
    /// Whether the program draws the packet's mesh instead of the ABI placeholder.
    geometry: bool,
    hash: u64,
    /// Whether `name` is derived from the fingerprint (and must follow it when the
    /// parameters change) rather than supplied by the caller.
    auto_named: bool,
}

impl ShaderProgram {
    /// A program with an explicit, descriptive `name`.
    ///
    /// It starts as a plain program (the ABI placeholder); chain
    /// [`with_geometry`](Self::with_geometry) to draw the packet's own mesh instead.
    pub fn new(name: impl Into<Arc<str>>, source: impl Into<Arc<str>>) -> Self {
        let source = source.into();
        Self {
            hash: fingerprint(&source, &ProgramParameters::default(), false),
            name: name.into(),
            source,
            parameters: ProgramParameters::default(),
            geometry: false,
            auto_named: false,
        }
    }

    /// A program named after its own fingerprint (`shader-<hex>`).
    ///
    /// Scene extraction uses this, because a Coin `SoShaderProgram` has no name. The
    /// name follows the fingerprint when [`with_parameters`](Self::with_parameters)
    /// changes it.
    pub fn from_source(source: impl Into<Arc<str>>) -> Self {
        let source = source.into();
        let hash = fingerprint(&source, &ProgramParameters::default(), false);
        Self {
            name: format!("shader-{hash:016x}").into(),
            source,
            parameters: ProgramParameters::default(),
            geometry: false,
            hash,
            auto_named: true,
        }
    }

    /// Replace the fixed-ABI parameters, recomputing the fingerprint.
    pub fn with_parameters(mut self, parameters: ProgramParameters) -> Self {
        self.hash = fingerprint(&self.source, &parameters, self.geometry);
        self.parameters = parameters;
        if self.auto_named {
            self.name = format!("shader-{:016x}", self.hash).into();
        }
        self
    }

    /// Draw the packet's own mesh instead of the fixed ABI's placeholder quad.
    ///
    /// A geometry program is declared with the scene attribute layout
    /// (position/normal/uv/colour), so it receives the packet's vertices, normals and
    /// per-vertex colours. The mode participates in the fingerprint: the same source
    /// used both ways is two programs, because the two need different pipelines.
    pub fn with_geometry(mut self) -> Self {
        self.geometry = true;
        self.hash = fingerprint(&self.source, &self.parameters, true);
        if self.auto_named {
            self.name = format!("shader-{:016x}", self.hash).into();
        }
        self
    }

    /// The descriptive name (from [`new`](Self::new), or derived from the fingerprint).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The inline WGSL module.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The fixed-ABI parameters bound to group 2, binding 1.
    pub fn parameters(&self) -> &ProgramParameters {
        &self.parameters
    }

    /// Whether the program draws the packet's own mesh (`true`) or the ABI's
    /// placeholder quad (`false`).
    pub fn geometry(&self) -> bool {
        self.geometry
    }

    /// The stable content fingerprint a pipeline cache keys on.
    pub fn hash(&self) -> u64 {
        self.hash
    }
}

impl PartialEq for ShaderProgram {
    /// Content identity, not pointer identity and not a source string comparison.
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash
    }
}

impl Eq for ShaderProgram {}

impl Hash for ShaderProgram {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.hash.hash(state);
    }
}

/// FNV-1a over the source, the parameter words and the geometry mode.
///
/// Deliberately hand-written rather than [`std::collections::hash_map::DefaultHasher`]:
/// the fingerprint is persisted in nothing today, but a cache key must not change with
/// the Rust toolchain.
///
/// The geometry bit is appended only when set, so a plain program's fingerprint is
/// byte-identical to what it was before the mode existed (no cache churn), while a
/// geometry program differs from the same source drawn plainly.
fn fingerprint(source: &str, parameters: &ProgramParameters, geometry: bool) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = OFFSET;
    let mut write = |bytes: &[u8]| {
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(PRIME);
        }
    };
    write(source.as_bytes());
    // Shadow data only affects the program while at least one slot is live. Hashing it
    // conditionally keeps a default (shadowless) program's fingerprint byte-identical to
    // what it was before the producer existed, so existing cache entries survive.
    let shadow_slots = (parameters.shadow_count.max(0.0) as usize).min(SHADOW_MAX);
    if shadow_slots > 0 {
        for value in parameters.shadow_data.iter().flatten().take(shadow_slots * SHADOW_STRIDE) {
            write(&value.to_bits().to_le_bytes());
        }
    }
    for value in [
        parameters.color[0],
        parameters.color[1],
        parameters.color[2],
        parameters.base_spacing,
        parameters.minor_alpha,
        parameters.major_alpha,
        parameters.shadow_opacity,
        parameters.shadow_count,
        parameters.axis_y[0],
        parameters.axis_y[1],
        parameters.axis_y[2],
        parameters.axis_z[0],
        parameters.axis_z[1],
        parameters.axis_z[2],
        parameters.axis_length,
        parameters.axis_width,
        parameters.axis_opacity,
        parameters.axis_ghost_alpha,
        parameters.axis_origin,
        parameters.axis_labels,
    ] {
        write(&value.to_bits().to_le_bytes());
    }
    if geometry {
        write(&[1]);
    }
    hash
}

/// The depth comparison a pipeline applies (`SoDepthBuffer.function`).
///
/// The names are Coin's (`NEVER` … `ALWAYS`); a backend maps them to its own API. The
/// default, [`Less`](Self::Less), is the comparison the scene pipelines have always
/// used.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DepthCompare {
    /// Never pass (Coin `NEVER`).
    Never,
    /// Pass when the incoming depth is below the stored one (Coin `LESS`).
    #[default]
    Less,
    /// Pass on equality (Coin `EQUAL`).
    Equal,
    /// Pass when below or equal (Coin `LEQUAL`).
    LessEqual,
    /// Pass when above (Coin `GREATER`).
    Greater,
    /// Pass on inequality (Coin `NOTEQUAL`).
    NotEqual,
    /// Pass when above or equal (Coin `GEQUAL`).
    GreaterEqual,
    /// Always pass (Coin `ALWAYS`).
    Always,
}

impl DepthCompare {
    /// The comparison with the depth test disabled, which is `Always`.
    pub fn if_tested(self, test: bool) -> Self {
        if test {
            self
        } else {
            Self::Always
        }
    }
}

/// The depth test/write state a draw binds (`SoDepthBuffer`).
///
/// A backend maps this onto its pipeline's depth-stencil state. `test` is the
/// `SoDepthBuffer.test` field and, when false, forces the comparison to
/// [`Always`](DepthCompare::Always); `write` is `SoDepthBuffer.write` and is honoured
/// independently, because Coin lets a draw compare without writing (the grid overlay's
/// default) and — less usefully — write without comparing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DepthState {
    /// Whether the depth test runs at all.
    pub test: bool,
    /// Whether a passing fragment writes the depth buffer.
    pub write: bool,
    /// The comparison used when the test runs.
    pub compare: DepthCompare,
}

impl Default for DepthState {
    /// Test on, write on, compare `Less` — what the scene pipelines did before
    /// `SoDepthBuffer` was applied.
    fn default() -> Self {
        Self {
            test: true,
            write: true,
            compare: DepthCompare::Less,
        }
    }
}

impl DepthState {
    /// The comparison a pipeline should use: `Always` when the test is disabled.
    pub fn effective_compare(&self) -> DepthCompare {
        self.compare.if_tested(self.test)
    }

    /// Whether a fragment may write depth. The flag is independent of the test: Coin's
    /// `write` field is, and a caller may want to keep the buffer untouched while
    /// comparing.
    pub fn effective_write(&self) -> bool {
        self.write
    }

    /// The state a shader program defaults to: test on, write **off**, compare `Less`.
    ///
    /// A program draws a placeholder covering its object instead of the object's own
    /// geometry, so writing depth from that placeholder would hide everything behind it.
    /// The test stays on so opaque geometry in front of the program still occludes it.
    pub const PROGRAM: Self = Self {
        test: true,
        write: false,
        compare: DepthCompare::Less,
    };
}

/// The resolved state bound to a draw, independent of the node model.
#[derive(Clone, Debug, PartialEq)]
pub struct StateSnapshot {
    /// The world transform to apply to the mesh.
    pub world_transform: Matrix4,
    /// Linear RGBA colour.
    pub color: [f32; 4],
    /// The side/down-facing linear RGBA colour for a two-tone surface.
    ///
    /// Alpha `0` means "single colour": the shader uses [`color`](Self::color) for
    /// every facet. A non-zero alpha makes an up-facing facet use `color` (the
    /// glass-top token) and every other facet use this (the glass-shade token).
    pub shade: [f32; 4],
    /// Depth bias applied before rasterisation.
    pub depth_bias: f32,
    /// The texture to sample, if any (multiplied into the colour).
    pub texture: Option<Arc<Texture2>>,
    /// Selection identity, written to the picking buffer.
    pub id: PickId,
    /// The depth test/write state the pipeline binds (`SoDepthBuffer`).
    pub depth: DepthState,
    /// Whether the pick paths may hit this draw (`SoPickStyle.UNPICKABLE`).
    ///
    /// Both pick paths (the CPU ray cast and the wgpu G-buffer pass) skip a packet whose
    /// `pickable` is false. A program packet is additionally skipped unconditionally, so
    /// its placeholder quad can never shadow the scene.
    pub pickable: bool,
    /// Whether the draw is a screen-space **ribbon** that should use the unlit,
    /// anti-aliased fragment stage (`fs_ribbon`).
    ///
    /// A ribbon's `uv.x` carries the lateral position (`0` at one edge, `1` at the
    /// other), so the fragment shader feathers the silhouette with `smoothstep` instead
    /// of relying on MSAA coverage alone. Normal surfaces leave this false and keep
    /// `fs_main`.
    pub ribbon: bool,
    /// The shader program that owns shading for this draw, if any.
    ///
    /// Scene-graph producers set this from an active `SoShaderProgram`; a renderer
    /// selects its program pipeline instead of the state-driven one. Compared by the
    /// program's content fingerprint (see [`ShaderProgram`]).
    pub program: Option<Arc<ShaderProgram>>,
}

impl Default for StateSnapshot {
    fn default() -> Self {
        Self {
            world_transform: Matrix4::identity(),
            color: [1.0, 1.0, 1.0, 1.0],
            shade: [0.0; 4],
            depth_bias: 0.0,
            texture: None,
            id: PickId::default(),
            depth: DepthState::default(),
            pickable: true,
            ribbon: false,
            program: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_identity_is_the_content_fingerprint() {
        let a = ShaderProgram::new("grid", "@fragment fn fs_main() {}");
        let b = ShaderProgram::new("another name", "@fragment fn fs_main() {}");
        assert_eq!(a, b, "the name is descriptive, not identity");
        assert_eq!(a.hash(), b.hash());

        let edited = ShaderProgram::new("grid", "@fragment fn fs_main() { }");
        assert_ne!(a, edited, "a one-byte edit is a different program");

        let params = ProgramParameters {
            base_spacing: 10.0,
            ..Default::default()
        };
        let retuned =
            ShaderProgram::new("grid", "@fragment fn fs_main() {}").with_parameters(params);
        assert_ne!(a, retuned, "the parameter surface is part of identity");
        assert_eq!(retuned.parameters().base_spacing, 10.0);
    }

    #[test]
    fn the_geometry_mode_is_part_of_identity() {
        let plain = ShaderProgram::new("gizmo", "@fragment fn fs_main() {}");
        let geometry = plain.clone().with_geometry();
        assert!(!plain.geometry(), "a new program is plain by default");
        assert!(geometry.geometry());
        assert_ne!(
            plain, geometry,
            "the same source drawn two ways is two programs"
        );
        assert_ne!(
            plain.hash(),
            geometry.hash(),
            "the fingerprint includes the geometry mode"
        );

        // Hash collapses to the same decision as equality.
        use std::collections::hash_map::DefaultHasher;
        let hash_of = |program: &ShaderProgram| {
            let mut hasher = DefaultHasher::new();
            Hash::hash(program, &mut hasher);
            hasher.finish()
        };
        assert_ne!(hash_of(&plain), hash_of(&geometry));

        // Replacing the parameters keeps the mode and folds it into the new digest.
        let retuned = geometry
            .clone()
            .with_parameters(ProgramParameters {
                minor_alpha: 0.5,
                ..Default::default()
            });
        assert!(retuned.geometry());
        assert_ne!(retuned, geometry);
    }

    #[test]
    fn from_source_defaults_to_no_geometry() {
        let program = ShaderProgram::from_source("");
        assert!(!program.geometry());
        assert_eq!(program.hash(), ShaderProgram::new("x", "").hash());

        let geometry = ShaderProgram::from_source("").with_geometry();
        assert!(geometry.geometry());
        assert_eq!(geometry.name(), format!("shader-{:016x}", geometry.hash()));
    }

    #[test]
    fn generated_name_and_hash_round_trip() {
        let program = ShaderProgram::from_source("@compute @workgroup_size(1) fn c() {}");
        assert_eq!(program.name(), format!("shader-{:016x}", program.hash()));
        assert_eq!(program.source(), "@compute @workgroup_size(1) fn c() {}");

        // The auto-generated name follows the fingerprint after a parameter change.
        let retuned = program.with_parameters(ProgramParameters {
            minor_alpha: 0.5,
            ..Default::default()
        });
        assert_eq!(retuned.name(), format!("shader-{:016x}", retuned.hash()));
    }

    #[test]
    fn snapshot_defaults_to_no_program() {
        let snapshot = StateSnapshot::default();
        assert!(snapshot.program.is_none());
        assert_eq!(snapshot.world_transform, Matrix4::identity());
        assert_eq!(snapshot.color, [1.0, 1.0, 1.0, 1.0]);

        let mut with_program = snapshot.clone();
        with_program.program = Some(Arc::new(ShaderProgram::from_source("")));
        assert_ne!(snapshot, with_program);
        assert_eq!(
            with_program,
            with_program.clone(),
            "the Arc compares by program content"
        );
    }

    #[test]
    fn depth_defaults_match_the_scene_pipeline() {
        let snapshot = StateSnapshot::default();
        assert_eq!(snapshot.depth, DepthState::default());
        assert!(snapshot.depth.test && snapshot.depth.write);
        assert_eq!(snapshot.depth.compare, DepthCompare::Less);
        assert!(snapshot.pickable, "a plain draw stays pickable");
    }

    #[test]
    fn a_disabled_depth_test_compares_always() {
        let mut depth = DepthState::default();
        assert_eq!(depth.effective_compare(), DepthCompare::Less);
        assert!(depth.effective_write());

        depth.test = false;
        assert_eq!(depth.effective_compare(), DepthCompare::Always);
        assert!(depth.effective_write(), "writing is independent of the test");

        depth.write = false;
        assert!(!depth.effective_write());
    }

    #[test]
    fn a_program_default_compares_without_writing() {
        assert_eq!(
            DepthState::PROGRAM,
            DepthState {
                test: true,
                write: false,
                compare: DepthCompare::Less,
            }
        );
    }

    /// Flat float `o` of the shader's `Program.shadows` (`shadow_at` in the WGSL).
    fn shadow_float(parameters: &ProgramParameters, o: usize) -> f32 {
        parameters.shadow_data[o / 4][o % 4]
    }

    #[test]
    fn two_separated_boxes_fill_two_shadow_slots() {
        let mut parameters = ProgramParameters::default();
        let count = apply_shadows(
            &mut parameters,
            [
                ShadowFootprint::from_bounds([-5.0, -5.0], [-1.0, -1.0], 0.5),
                ShadowFootprint::from_bounds([2.0, 3.0], [4.0, 7.0], 0.75),
            ],
        );

        assert_eq!(count, 2);
        assert_eq!(parameters.shadow_count, 2.0, "the shader reads counts.x");

        // Slot 0: centre, half-extent, blur in the shader's 5-float order.
        assert_eq!(shadow_float(&parameters, 0), -3.0);
        assert_eq!(shadow_float(&parameters, 1), -3.0);
        assert_eq!(shadow_float(&parameters, 2), 2.0);
        assert_eq!(shadow_float(&parameters, 3), 2.0);
        assert_eq!(shadow_float(&parameters, 4), 0.5);

        // Slot 1 starts at the next stride boundary, so it crosses a vec4.
        assert_eq!(shadow_float(&parameters, 5), 3.0);
        assert_eq!(shadow_float(&parameters, 6), 5.0);
        assert_eq!(shadow_float(&parameters, 7), 1.0);
        assert_eq!(shadow_float(&parameters, 8), 2.0);
        assert_eq!(shadow_float(&parameters, 9), 0.75);

        // The vec4 packing is exactly the shader's flat indexing.
        assert_eq!(parameters.shadow_data[0], [-3.0, -3.0, 2.0, 2.0]);
        assert_eq!(parameters.shadow_data[1][0], 0.5);

        // The unused tail (from flat float 10, after two 5-float slots) stays zeroed.
        assert!(
            parameters
                .shadow_data
                .iter()
                .flatten()
                .skip(2 * SHADOW_STRIDE)
                .all(|value| *value == 0.0)
        );
    }

    #[test]
    fn an_empty_scene_clears_the_shadow_block() {
        let mut parameters = ProgramParameters::default();
        apply_shadows(
            &mut parameters,
            [ShadowFootprint::from_bounds([0.0, 0.0], [2.0, 2.0], 0.5)],
        );
        assert_eq!(parameters.shadow_count, 1.0);

        // An empty scene is a deliberate clear, not a no-op: stale slots must not leak.
        let count = apply_shadows(&mut parameters, []);
        assert_eq!(count, 0);
        assert_eq!(parameters.shadow_count, 0.0);
        assert!(
            parameters.shadow_data.iter().flatten().all(|value| *value == 0.0),
            "the block is zeroed"
        );
    }

    #[test]
    fn the_shadow_producer_clamps_to_the_abi_cap() {
        let mut parameters = ProgramParameters::default();
        let count = apply_shadows(
            &mut parameters,
            (0..SHADOW_MAX + 5).map(|index| {
                let x = index as f32;
                ShadowFootprint::from_bounds([x, x], [x + 1.0, x + 1.0], 0.5)
            }),
        );

        assert_eq!(count, SHADOW_MAX, "never more than the fixed block");
        assert_eq!(parameters.shadow_count, SHADOW_MAX as f32);

        // The last slot is the 64th footprint, not the 69th: the extra five are dropped.
        let last = (SHADOW_MAX - 1) * SHADOW_STRIDE;
        assert_eq!(shadow_float(&parameters, last), 63.5);
        assert_eq!(shadow_float(&parameters, last + 1), 63.5);
        assert_eq!(shadow_float(&parameters, last + 2), 0.5);

        // The block is exactly full: the last float written is the 320th.
        assert_eq!(shadow_float(&parameters, SHADOW_MAX * SHADOW_STRIDE - 1), 0.5);
        assert_eq!(parameters.shadow_data.len(), SHADOW_VECS);
    }

    #[test]
    fn shadow_footprints_are_sanitised_for_the_shader() {
        let mut parameters = ProgramParameters::default();
        apply_shadows(
            &mut parameters,
            [ShadowFootprint {
                center: [f32::NAN, 1.0],
                half_extent: [-2.0, 0.0],
                blur: -1.0,
            }],
        );

        assert_eq!(parameters.shadow_count, 1.0);
        assert_eq!(shadow_float(&parameters, 0), 0.0, "a NaN centre is zeroed");
        assert_eq!(shadow_float(&parameters, 2), 2.0, "an extent is an absolute value");
        assert_eq!(
            shadow_float(&parameters, 3),
            SHADOW_MIN_EXTENT,
            "a degenerate extent keeps the sdf finite"
        );
        assert_eq!(shadow_float(&parameters, 4), 1e-6, "blur is positive");
    }

    #[test]
    fn the_shadow_block_is_part_of_program_identity_only_when_live() {
        let source = "@fragment fn fs_main() {}";
        let plain = ShaderProgram::new("grid", source);

        // A cleared block (count 0) must not change the fingerprint: existing programs
        // keep their cache identity.
        let cleared = plain.clone().with_parameters(ProgramParameters::default());
        assert_eq!(plain.hash(), cleared.hash());

        let mut parameters = ProgramParameters::default();
        apply_shadows(
            &mut parameters,
            [ShadowFootprint::from_bounds([0.0, 0.0], [2.0, 2.0], 0.5)],
        );
        let shaded = plain.clone().with_parameters(parameters);
        assert_ne!(plain.hash(), shaded.hash(), "live slots are part of identity");
    }
}
