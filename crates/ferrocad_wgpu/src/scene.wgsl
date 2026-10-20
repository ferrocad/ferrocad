// The scene pipeline: one vertex/fragment pair drawing flat-shaded meshes with a
// per-draw, dynamic-offset uniform (view-projection, model, colour).

// The scene pass and the pick pass share one uniform layout, so the last two words
// (read only by pick.wgsl) are declared here too; the scene shader ignores them.
struct Globals {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    color: vec4<f32>,
    pick: u32,
    topology: u32,
    depth_bias: f32,
    _padding: u32,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var shape_texture: texture_2d<f32>;
@group(1) @binding(1) var shape_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    let world = globals.model * vec4<f32>(input.position, 1.0);
    // Uniform scale is assumed, so the model matrix doubles as the normal matrix.
    output.normal = (globals.model * vec4<f32>(input.normal, 0.0)).xyz;
    output.uv = input.uv;
    var clip = globals.view_proj * world;
    // A positive bias moves the fragment towards the camera (smaller depth), so a
    // coplanar selection highlight wins the depth test against the surface it marks.
    clip.z -= globals.depth_bias * clip.w;
    output.clip_position = clip;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let light_dir = normalize(vec3<f32>(0.4, 0.8, 0.6));
    let normal = normalize(input.normal);
    // Two-sided: culling is off for the POC, so accept either face orientation.
    let diffuse = abs(dot(normal, light_dir));
    let ambient = 0.25;
    // The texture (the default is opaque white) multiplies into the colour.
    let albedo = globals.color * textureSample(shape_texture, shape_sampler, input.uv);
    let lit = albedo.rgb * (ambient + (1.0 - ambient) * diffuse);
    return vec4<f32>(lit, albedo.a);
}
