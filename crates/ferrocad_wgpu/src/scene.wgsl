// The scene pipeline: one vertex/fragment pair drawing flat-shaded meshes with a
// per-draw, dynamic-offset uniform (view-projection, model, colour).

// The scene pass and the pick pass share one uniform layout, so the last two words
// (read only by pick.wgsl) are declared here too; the scene shader ignores them.
struct Globals {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    color: vec4<f32>,
    // The side/down-facing ink; alpha 0 means single-colour.
    shade: vec4<f32>,
    pick: u32,
    topology: u32,
    depth_bias: f32,
    two_tone: u32,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var shape_texture: texture_2d<f32>;
@group(1) @binding(1) var shape_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(4) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    let world = globals.model * vec4<f32>(input.position, 1.0);
    // Uniform scale is assumed, so the model matrix doubles as the normal matrix.
    output.normal = (globals.model * vec4<f32>(input.normal, 0.0)).xyz;
    output.uv = input.uv;
    output.color = input.color;
    var clip = globals.view_proj * world;
    // A positive bias moves the fragment towards the camera (smaller depth), so a
    // coplanar selection highlight wins the depth test against the surface it marks.
    clip.z -= globals.depth_bias * clip.w;
    output.clip_position = clip;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    // The key light is authored in the Z-up world frame: mostly overhead, offset in
    // +X/+Y so the four side facets still separate from each other.
    let light_dir = normalize(vec3<f32>(0.4, 0.6, 0.8));
    let normal = normalize(input.normal);
    // Two-sided: culling is off for the POC, so accept either face orientation.
    let diffuse = abs(dot(normal, light_dir));
    let ambient = 0.25;
    // The texture (the default is opaque white) and the per-vertex colour both
    // multiply into the packet colour.
    let albedo = globals.color * input.color * textureSample(shape_texture, shape_sampler, input.uv);
    // V2b: an up-facing facet uses the glass-top ink (`color`); every other facet
    // uses the glass-shade ink (`shade`). `shade.a == 0` keeps a single colour.
    // The world is Z-up (the camera's `world_up` and the NaviCube agree), so "up"
    // is +Z, not +Y.
    let up = dot(normal, vec3<f32>(0.0, 0.0, 1.0)) > 0.5;
    let base = select(globals.shade.rgb, albedo.rgb, up || globals.two_tone == 0u);
    let lit = base * (ambient + (1.0 - ambient) * diffuse);
    return vec4<f32>(lit, albedo.a);
}

// A screen-space control ribbon: unlit, and feathered over its last screen pixel.
//
// `uv.x` is the lateral coordinate across the quad (0 at one edge, 1 at the other), so
// `fwidth(uv.x)` is exactly one pixel in uv units whatever the ribbon's width. That
// turns the hard triangle-quad staircase into a smooth vector edge without a texture
// or a per-draw width uniform. A vertex that leaves `uv.x` at 0.5 (the filled
// arrowhead) gets full coverage everywhere.
@fragment
fn fs_ribbon(input: VertexOutput) -> @location(0) vec4<f32> {
    // `fwidth(uv.x)` is one screen pixel in uv units whatever the ribbon's width, so
    // convert both the half-width and the fragment's distance to pixels and feather a
    // single pixel around the geometric edge. Full coverage is kept for any fragment
    // whose centre is at least half a pixel inside the edge.
    let fw = max(fwidth(input.uv.x), 1e-6);
    let half_width = 0.5 / fw;
    let distance = abs(input.uv.x - 0.5) / fw;
    let coverage = 1.0 - smoothstep(half_width - 0.5, half_width + 0.5, distance);
    let ink = globals.color * input.color;
    return vec4<f32>(ink.rgb, ink.a * coverage);
}
