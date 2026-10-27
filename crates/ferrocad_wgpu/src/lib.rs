//! # FerroCAD wgpu backend (`ferrocad_wgpu`)
//!
//! One backend behind the renderer-agnostic batch boundary defined by
//! [`ferrocad_graphics`]. It consumes the [`DrawBatch`] trait — never the node
//! model — so a D3D or Metal backend can sit in its place without the compiler or
//! scene graph changing.
//!
//! [`WgpuRenderer`] implements [`InventorView`], so a host can present a frame in
//! either of the two variants the trait exposes: borrow the offscreen
//! [`wgpu::Texture`](WgpuRenderer::surface) for zero-copy compositing, or call
//! [`image`](InventorView::image) for a readback [`ImageData`] (the MVP path while
//! `bite-gpui` has no wgpu surface primitive).
//!
//! - [`WgpuRenderer`] — encodes a frame: uploads [`mesh_id`]/[`generation`] cache
//!   misses, binds each packet's [`StateSnapshot`] and draws it.
//! - [`context`] — the [`GpuContext`]: the device/queue the renderer owns.
//! - [`adapter`] — which adapter the renderer uses and what the machine offers.
//! - [`batch`] — uploading fresh geometry into GPU meshes.
//! - [`pipeline`] — the WGSL render pipelines (opaque, transparent, lines, points).
//! - [`program`] — drawing an authored shader program: group 2 (`Frame`/`Program`), the
//!   pipeline cache keyed by program content and depth state, and the placeholder quad.
//! - [`uniforms`] — the dynamic-uniform buffer (per-frame globals + per-packet
//!   state in one allocation).
//! - [`pick`] — the g-buffer picking pass that maps a pixel back to a pick id.
//!
//! The grid overlay's WGSL (the port of the add-on's GLSL; see
//! `docs/workbenches/grid-overlay-plan.md` §3.4) is **mod-owned**: it ships in
//! `mods/GridOverlayCoin/Resources/grid_overlay.wgsl` and reaches the renderer as a
//! program packet's module (G4). A packet carrying a
//! [`ShaderProgram`](ferrocad_graphics::ShaderProgram) replaces the scene pipeline
//! with a pipeline built from that source. This crate exports no copy: its naga tests
//! read and validate the mod's file, and skip those checks when the workspace's `mods/`
//! tree is absent (so an unpacked crate still builds and tests).
//!
//! [`AXIS_OVERLAY_WGSL`] is the axis-cross sibling and is still crate-owned: the same
//! fixed `Frame`/`Program` binding, with the backward-compatible axis tail appended to
//! the `Program` block.
//!
//! The gizmo skin is mod-owned on the same terms (`mods/GizmoOverlayShader/`); this
//! crate exports no skin constant either.
//!
//! [`DrawBatch`]: ferrocad_graphics::DrawBatch
//! [`InventorView`]: ferrocad_inventor::InventorView
//! [`ImageData`]: ferrocad_inventor::ImageData
//! [`mesh_id`]: ferrocad_graphics::MeshId
//! [`generation`]: ferrocad_graphics::Generation
//! [`StateSnapshot`]: ferrocad_graphics::StateSnapshot

pub mod adapter;
pub mod batch;
pub mod context;
pub mod pick;
pub mod pipeline;
pub mod program;
pub mod renderer;
pub mod snapshot;
pub mod textures;
pub mod trace;
pub mod uniforms;

pub use adapter::{AdapterChoice, AdapterSummary};
pub use context::GpuContext;
pub use renderer::{RenderError, WgpuRenderer};
pub use snapshot::{
    SnapshotBox, SnapshotCamera, SnapshotDrawMode, SnapshotError, SnapshotGeometry, SnapshotMesh,
    SnapshotShape, SnapshotSpec,
};
pub use trace::Tone;

/// The axis-cross overlay's WGSL module. It declares the extended `Program`
/// block (the grid prefix plus four axis `vec4`s) and the same entry points.
pub const AXIS_OVERLAY_WGSL: &str = include_str!("../shaders/axis_overlay.wgsl");

/// The mod-owned grid overlay source, when the workspace `mods/` tree is present.
///
/// `mods/GridOverlayCoin/Resources/grid_overlay.wgsl` is the single home for this
/// shader. The crate carries no copy, so its tests read the workspace tree and skip
/// the shader-specific checks when it is absent (for example when the crate is
/// unpacked from a `.crate` archive outside the workspace).
#[cfg(test)]
pub(crate) fn grid_overlay_source() -> Option<String> {
    mod_source("mods/GridOverlayCoin/Resources/grid_overlay.wgsl")
}

/// Read a workspace file (`CARGO_MANIFEST_DIR/../..`), if it is present.
#[cfg(test)]
fn mod_source(relative: &str) -> Option<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative);
    std::fs::read_to_string(&path)
        .inspect_err(|error| eprintln!("workspace file {} is unavailable: {error}", path.display()))
        .ok()
}

/// A minimal, valid geometry program for tests that only need *a* pipeline.
///
/// It deliberately does not model the grid look: the tests that assert pixels read
/// the mod's real source and skip when the workspace `mods/` tree is absent.
#[cfg(test)]
pub(crate) const FIXTURE_PROGRAM_WGSL: &str = r#"@vertex
fn vs_main(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(position, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 0.0, 1.0, 1.0);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse and validate the grid overlay module without a GPU.
    fn validated_grid_overlay() -> Option<naga::Module> {
        let Some(source) = grid_overlay_source() else {
            eprintln!("the workspace mods/ tree is absent; skipping the grid-shader checks");
            return None;
        };
        let module = naga::front::wgsl::parse_str(&source)
            .expect("the mod's grid_overlay.wgsl should parse as WGSL");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("the mod's grid_overlay.wgsl should validate");
        Some(module)
    }

    fn struct_by_name<'a>(module: &'a naga::Module, name: &str) -> naga::Handle<naga::Type> {
        module
            .types
            .iter()
            .find_map(|(handle, ty)| {
                (ty.name.as_deref() == Some(name)
                    && matches!(ty.inner, naga::TypeInner::Struct { .. }))
                .then_some(handle)
            })
            .unwrap_or_else(|| panic!("no struct {name} in the module"))
    }

    #[test]
    fn grid_overlay_has_the_expected_entry_points() {
        let Some(module) = validated_grid_overlay() else {
            return;
        };
        let vertex = module
            .entry_points
            .iter()
            .find(|entry| entry.name == "vs_main")
            .expect("vs_main");
        let fragment = module
            .entry_points
            .iter()
            .find(|entry| entry.name == "fs_main")
            .expect("fs_main");
        assert_eq!(vertex.stage, naga::ShaderStage::Vertex);
        assert_eq!(fragment.stage, naga::ShaderStage::Fragment);

        let result = fragment
            .function
            .result
            .as_ref()
            .expect("fs_main returns a fragment output struct");
        let naga::TypeInner::Struct { members, .. } = &module.types[result.ty].inner else {
            panic!("fs_main's result should be a struct");
        };
        assert!(
            members.iter().any(|member| {
                member.binding == Some(naga::Binding::BuiltIn(naga::BuiltIn::FragDepth))
            }),
            "fs_main must write @builtin(frag_depth)"
        );
    }

    #[test]
    fn grid_overlay_binds_only_the_program_group() {
        let Some(module) = validated_grid_overlay() else {
            return;
        };
        let mut bindings: Vec<(u32, u32)> = module
            .global_variables
            .iter()
            .filter_map(|(_, variable)| {
                variable
                    .binding
                    .as_ref()
                    .map(|binding| (binding.group, binding.binding))
            })
            .collect();
        bindings.sort_unstable();
        // Groups 0 (Globals) and 1 (texture) are layout-only: the shared pipeline
        // layout keeps them, and the program shader does not touch them.
        assert_eq!(bindings, vec![(2, 0), (2, 1)]);

        let frame = module
            .global_variables
            .iter()
            .find(|(_, variable)| {
                variable
                    .binding
                    .as_ref()
                    .is_some_and(|binding| binding.group == 2 && binding.binding == 0)
            })
            .map(|(_, variable)| variable)
            .expect("group 2 binding 0");
        let program = module
            .global_variables
            .iter()
            .find(|(_, variable)| {
                variable
                    .binding
                    .as_ref()
                    .is_some_and(|binding| binding.group == 2 && binding.binding == 1)
            })
            .map(|(_, variable)| variable)
            .expect("group 2 binding 1");
        assert_eq!(frame.space, naga::AddressSpace::Uniform);
        assert_eq!(program.space, naga::AddressSpace::Uniform);
        assert!(matches!(
            module.types[frame.ty].inner,
            naga::TypeInner::Struct { .. }
        ));
    }

    #[test]
    fn grid_overlay_uniform_blocks_match_the_fixed_abi() {
        let Some(module) = validated_grid_overlay() else {
            return;
        };
        let mut layouter = naga::proc::Layouter::default();
        layouter
            .update(module.to_ctx())
            .expect("WGSL uniform layouts");

        // Frame: view_proj + inv_view_proj (2 x 64) + eye + flags (2 x 16).
        let frame = struct_by_name(&module, "Frame");
        assert_eq!(layouter[frame].size, 160);
        // Program: 3 x vec4 plus 80 shadow vec4s (64 slots x 5 floats, re-packed).
        let program = struct_by_name(&module, "Program");
        assert_eq!(layouter[program].size, 1_328);
        assert_eq!(layouter[program].alignment.round_up(1_328), 1_328);
    }

    /// Parse and validate the axis overlay module without a GPU.
    fn validated_axis_overlay() -> naga::Module {
        let module = naga::front::wgsl::parse_str(AXIS_OVERLAY_WGSL)
            .expect("axis_overlay.wgsl should parse as WGSL");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("axis_overlay.wgsl should validate");
        module
    }

    #[test]
    fn axis_overlay_has_the_expected_entry_points() {
        let module = validated_axis_overlay();
        assert!(module.entry_points.iter().any(|entry| {
            entry.name == "vs_main" && entry.stage == naga::ShaderStage::Vertex
        }));
        let fragment = module
            .entry_points
            .iter()
            .find(|entry| entry.name == "fs_main" && entry.stage == naga::ShaderStage::Fragment)
            .expect("fs_main as a fragment entry point");
        let result = fragment
            .function
            .result
            .as_ref()
            .expect("fs_main returns a fragment output struct");
        let naga::TypeInner::Struct { members, .. } = &module.types[result.ty].inner else {
            panic!("fs_main's result should be a struct");
        };
        assert!(
            members.iter().any(|member| {
                member.binding == Some(naga::Binding::BuiltIn(naga::BuiltIn::FragDepth))
            }),
            "fs_main must write @builtin(frag_depth)"
        );
    }

    #[test]
    fn axis_overlay_binds_only_the_program_group() {
        let module = validated_axis_overlay();
        let mut bindings: Vec<(u32, u32)> = module
            .global_variables
            .iter()
            .filter_map(|(_, variable)| {
                variable
                    .binding
                    .as_ref()
                    .map(|binding| (binding.group, binding.binding))
            })
            .collect();
        bindings.sort_unstable();
        assert_eq!(bindings, vec![(2, 0), (2, 1)]);
    }

    #[test]
    fn axis_overlay_uniform_blocks_match_the_extended_abi() {
        let module = validated_axis_overlay();
        let mut layouter = naga::proc::Layouter::default();
        layouter
            .update(module.to_ctx())
            .expect("WGSL uniform layouts");

        let frame = struct_by_name(&module, "Frame");
        assert_eq!(layouter[frame].size, 160);
        // Program: the 1,328-byte grid prefix plus four axis vec4s.
        let program = struct_by_name(&module, "Program");
        assert_eq!(layouter[program].size, 1_392);
        assert_eq!(layouter[program].alignment.round_up(1_392), 1_392);
    }

    /// The mod-owned gizmo skin source.
    ///
    /// Like the grid overlay, the gizmo skin is **mod-owned**: the shader ships with
    /// `GizmoOverlayShader` and reaches the shell through the provider registry. These
    /// tests validate the mod's copy, so the workspace's `mods/` tree is required to run
    /// them.
    fn gizmo_skin_source() -> Option<String> {
        mod_source("mods/GizmoOverlayShader/Resources/gizmo_skin.wgsl")
    }

    /// Parse and validate the gizmo skin module without a GPU.
    fn validated_gizmo_skin() -> Option<naga::Module> {
        let Some(source) = gizmo_skin_source() else {
            eprintln!("the workspace mods/ tree is absent; skipping the gizmo-skin checks");
            return None;
        };
        let module = naga::front::wgsl::parse_str(&source)
            .expect("the mod's gizmo_skin.wgsl should parse as WGSL");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("the mod's gizmo_skin.wgsl should validate");
        Some(module)
    }

    /// The skin must parse before it can be handed to `create_shader_module`.
    #[test]
    fn gizmo_skin_parses() {
        let Some(source) = gizmo_skin_source() else {
            return;
        };
        naga::front::wgsl::parse_str(&source)
            .expect("the mod's gizmo_skin.wgsl should parse as WGSL");
    }

    #[test]
    fn gizmo_skin_is_a_valid_geometry_program() {
        let Some(module) = validated_gizmo_skin() else {
            return;
        };
        assert!(module.entry_points.iter().any(|entry| {
            entry.name == "vs_main" && entry.stage == naga::ShaderStage::Vertex
        }));
        let fragment = module
            .entry_points
            .iter()
            .find(|entry| entry.name == "fs_main" && entry.stage == naga::ShaderStage::Fragment)
            .expect("fs_main as a fragment entry point");
        let result = fragment
            .function
            .result
            .as_ref()
            .expect("fs_main returns its target colour");
        assert!(
            matches!(
                module.types[result.ty].inner,
                naga::TypeInner::Vector {
                    size: naga::VectorSize::Quad,
                    ..
                }
            ),
            "fs_main returns a vec4 colour"
        );

        // Group 0 is the scene Globals; group 2 binding 0 is the fixed ABI's Frame
        // (binding 1 is not needed by a geometry program). Group 1 stays layout-only.
        let mut bindings: Vec<(u32, u32)> = module
            .global_variables
            .iter()
            .filter_map(|(_, variable)| {
                variable
                    .binding
                    .as_ref()
                    .map(|binding| (binding.group, binding.binding))
            })
            .collect();
        bindings.sort_unstable();
        assert_eq!(bindings, vec![(0, 0), (2, 0)]);

        let mut layouter = naga::proc::Layouter::default();
        layouter
            .update(module.to_ctx())
            .expect("WGSL uniform layouts");
        let frame = struct_by_name(&module, "Frame");
        assert_eq!(layouter[frame].size, 160);
        let globals = struct_by_name(&module, "Globals");
        assert_eq!(layouter[globals].size, 176);
    }

    /// The scene-extraction contract: an active `SoShaderProgram` binds to the sibling
    /// shapes that follow it, draws nothing itself, and its WGSL round-trips into the
    /// packet.
    #[test]
    fn shader_scene_carries_the_program_on_the_following_shape() {
        use ferrocad_graphics::DrawBatch;
        use ferrocad_inventor::{
            Node, NodeKind, Scene, ShaderParameter, ShaderStage, ShaderStageKind, compile,
        };

        let leaf = |kind: NodeKind| Node {
            kind,
            children: Vec::new(),
        };
        let vertex = leaf(NodeKind::ShaderStage(ShaderStage {
            kind: ShaderStageKind::Vertex,
            // A single-module WGSL file: the vertex stage holds it, the fragment
            // stage stays empty (WGSL has no per-stage modules).
            source: Some(FIXTURE_PROGRAM_WGSL.to_owned()),
            source_program: None,
        }));
        let fragment = Node {
            kind: NodeKind::ShaderStage(ShaderStage::fragment()),
            children: vec![
                leaf(NodeKind::ShaderParameter(ShaderParameter::Vec3 {
                    name: "grid_color".to_owned(),
                    value: [1.0, 0.5, 0.25],
                })),
                leaf(NodeKind::ShaderParameter(ShaderParameter::Float {
                    name: "base_spacing".to_owned(),
                    value: 10.0,
                })),
            ],
        };
        let program = Node {
            kind: NodeKind::ShaderProgram,
            // Order-independent: the stages are resolved by kind.
            children: vec![fragment, vertex],
        };
        let quad = leaf(NodeKind::Mesh {
            indices: vec![0, 1, 2, 0, 2, 3],
            normal_indices: Vec::new(),
            topology: ferrocad_graphics::Topology::Triangles,
        });
        let scene = Scene {
            root: Node {
                kind: NodeKind::Separator,
                children: vec![
                    leaf(NodeKind::Coordinate3(vec![
                        [-1.0, -1.0, 0.0],
                        [1.0, -1.0, 0.0],
                        [1.0, 1.0, 0.0],
                        [-1.0, 1.0, 0.0],
                    ])),
                    program,
                    quad,
                ],
            },
        };

        let (queue, _) = compile(&scene);
        assert_eq!(queue.len(), 1, "the program node must not emit a packet");
        let snapshot = queue.packets[0].state();
        let program = snapshot
            .program
            .as_ref()
            .expect("the packet carries a program");
        assert_eq!(
            program.source(),
            FIXTURE_PROGRAM_WGSL,
            "the WGSL round-trips"
        );
        assert_eq!(program.parameters().color, [1.0, 0.5, 0.25]);
        assert_eq!(program.parameters().base_spacing, 10.0);
        // What the packet carries is itself valid WGSL.
        naga::front::wgsl::parse_str(program.source())
            .expect("the extracted source parses as WGSL");
    }
}
