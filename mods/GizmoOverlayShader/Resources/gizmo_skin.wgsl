// The manipulator gizmo's skin (slice G2): a **geometry program**.
//
// Unlike the grid/axis overlays, this module draws the packet's own mesh — the gizmo's
// ribbons, glass planes and ghost wire are native geometry, and only their material
// lives here. The hit test, the drag and every commit stay in the native gizmo.
//
// It preserves the packet's alpha exactly (`globals.color.a`): the glass planes arrive
// at 10 %/28 %, the ribbons at 1.0, and the occluded ghost pass at 35 %, so the ghost
// and glass passes keep their meaning. It draws no readout text and no labels; those
// are native overlays.
//
// The skin is deliberately not a PBR clone: a half-Lambert headlight gives broad,
// readable shading, and a strong grazing-angle rim brightens the silhouette so the
// handles stay legible against the canvas. Line packets (plane rims, ghost wire) carry
// no usable normal, so they take the flat fallback instead of producing NaN.

// Group 0, binding 0 — the scene `Globals` block, declared exactly as `scene.wgsl` does
// so the shared pipeline layout and the dynamic offset line up. Only `view_proj`,
// `model`, `color` and `depth_bias` are read here.
struct Globals {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    color: vec4<f32>,
    shade: vec4<f32>,
    pick: u32,
    topology: u32,
    depth_bias: f32,
    two_tone: u32,
};

// Group 2, binding 0 — the fixed ABI's per-view frame. Group 1 (texture) stays
// undeclared; the pipeline layout still binds it, but the gizmo is untextured.
struct Frame {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    flags: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(2) @binding(0) var<uniform> frame: Frame;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(4) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    let world = globals.model * vec4<f32>(input.position, 1.0);
    output.world_position = world.xyz;
    // Uniform scale is assumed (as in the scene shader), so the model matrix doubles as
    // the normal matrix. A zero normal stays zero and is handled in the fragment stage.
    output.normal = (globals.model * vec4<f32>(input.normal, 0.0)).xyz;
    output.color = input.color;
    var clip = globals.view_proj * world;
    clip.z -= globals.depth_bias * clip.w;
    output.clip_position = clip;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    // The packet ink is the per-handle colour (active/inactive is already baked in);
    // the vertex colour is a per-vertex multiplier, as in the scene pipeline.
    let ink = globals.color.rgb * input.color.rgb;

    let to_eye = frame.eye.xyz - input.world_position;
    let eye_distance = length(to_eye);
    // An orthographic frame has no meaningful eye direction for a fragment at the eye;
    // fall back to straight-on rather than normalizing zero.
    let view_dir = select(
        vec3<f32>(0.0, 0.0, 1.0),
        to_eye / max(eye_distance, 0.00001),
        eye_distance > 0.00001,
    );

    let normal_length = length(input.normal);
    var light = 1.0;
    var rim = 1.0;
    if normal_length > 0.00001 {
        let n = input.normal / normal_length;
        // Half-Lambert, lit from the eye: broad, never-black shading on either face.
        let headlight = clamp(dot(n, view_dir) * 0.5 + 0.5, 0.0, 1.0);
        light = headlight * headlight;
        // A crisp silhouette term: the more grazing the view, the brighter.
        rim = pow(1.0 - clamp(abs(dot(n, view_dir)), 0.0, 1.0), 3.0);
    }

    let lit = ink * (0.30 + 0.70 * light);
    let rgb = clamp(lit * (1.0 + 0.45 * rim), vec3<f32>(0.0), vec3<f32>(1.0));
    // Alpha is the packet's, untouched: glass stays glass and the ghost stays ghost.
    return vec4<f32>(rgb, globals.color.a);
}
