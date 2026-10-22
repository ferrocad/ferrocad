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

/// The parameter surface of the fixed program ABI (`docs/workbenches/grid-overlay-plan.md` §3.4).
///
/// A WGSL uniform block has a static layout, so an authored program cannot invent
/// names: the compiler maps the Coin `SoShaderParameter*` nodes onto these fields by
/// name. The renderer writes them into group 2, binding 1 (`Program`).
///
/// The defaults are the grid overlay's FerroCAD look: the neutral grid ink at low
/// alpha, a 1-unit base spacing, and shadows off (the shadow block is deferred; see
/// `shaders/grid_overlay.wgsl`).
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
    /// Number of populated shadow slots (`"shadow_count"`); `0` in this slice.
    pub shadow_count: f32,
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
/// A program is compared and hashed by its **content fingerprint** (source bytes and
/// parameters), never by the full source or by pointer identity. The fingerprint is a
/// 64-bit FNV-1a digest, which is what a pipeline cache keys on; two scene nodes that
/// build the same program therefore share a cache entry, while a one-byte edit misses.
/// The name is descriptive only and does not participate in equality.
#[derive(Clone, Debug)]
pub struct ShaderProgram {
    name: Arc<str>,
    source: Arc<str>,
    parameters: ProgramParameters,
    hash: u64,
    /// Whether `name` is derived from the fingerprint (and must follow it when the
    /// parameters change) rather than supplied by the caller.
    auto_named: bool,
}

impl ShaderProgram {
    /// A program with an explicit, descriptive `name`.
    pub fn new(name: impl Into<Arc<str>>, source: impl Into<Arc<str>>) -> Self {
        let source = source.into();
        Self {
            hash: fingerprint(&source, &ProgramParameters::default()),
            name: name.into(),
            source,
            parameters: ProgramParameters::default(),
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
        let hash = fingerprint(&source, &ProgramParameters::default());
        Self {
            name: format!("shader-{hash:016x}").into(),
            source,
            parameters: ProgramParameters::default(),
            hash,
            auto_named: true,
        }
    }

    /// Replace the fixed-ABI parameters, recomputing the fingerprint.
    pub fn with_parameters(mut self, parameters: ProgramParameters) -> Self {
        self.hash = fingerprint(&self.source, &parameters);
        self.parameters = parameters;
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

/// FNV-1a over the source and the parameter words.
///
/// Deliberately hand-written rather than [`std::collections::hash_map::DefaultHasher`]:
/// the fingerprint is persisted in nothing today, but a cache key must not change with
/// the Rust toolchain.
fn fingerprint(source: &str, parameters: &ProgramParameters) -> u64 {
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
}
