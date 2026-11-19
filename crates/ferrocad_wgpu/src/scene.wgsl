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
    // The view's size in device pixels: `[width, height, 0, 0]`. A point sprite is
    // sized in pixels, so it needs this; nothing else reads it.
    viewport: vec4<f32>,
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
    return place(input);
}

/// The world → clip transform every vertex stage shares (a WGSL entry point cannot call
/// another, so the transform lives here).
fn place(input: VertexInput) -> VertexOutput {
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
    // A three-point rig, authored in the Z-up world frame. The key separates the four
    // side facets; the fill comes from the opposite side so the faces the key leaves
    // dark keep some form; the rim comes from behind to pick the silhouette out of the
    // background. The weights sum to one, so `ambient + (1 - ambient) * diffuse` still
    // maps to [0, 1].
    let normal = normalize(input.normal);
    // Two-sided: culling is off for the POC, so accept either face orientation.
    let key = abs(dot(normal, normalize(vec3<f32>(0.40, 0.60, 0.80))));
    let fill = abs(dot(normal, normalize(vec3<f32>(-0.70, 0.20, 0.35))));
    let rim = abs(dot(normal, normalize(vec3<f32>(-0.15, -0.85, 0.50))));
    let diffuse = 0.72 * key + 0.20 * fill + 0.08 * rim;
    let ambient = 0.18;
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

// A screen-space **point sprite**: the four vertices of one mark share a position and
// carry the quad's corner in `uv`, so the mark is expanded here — after projection, in
// pixels — instead of being sized in the world.
//
// This is what keeps a dot field cheap: a world-sized dot has to be rebuilt whenever the
// camera moves (its width comes from the projection) and foreshortens with depth, while
// this one is the same mark at every depth and the mesh never changes. Two device pixels
// is the Coin shader's floor (`dot_cov` never goes below a one-pixel radius), so the two
// backends draw the same mark.
const SPRITE_PX: f32 = 2.0;

@vertex
fn vs_sprite(input: VertexInput) -> VertexOutput {
    var output = place(input);
    // NDC spans 2 across the viewport, so one pixel is `2 / viewport` in NDC and the
    // quad's half-extent is `SPRITE_PX / viewport`. Multiplying by `w` cancels the
    // perspective divide, which is what makes the mark the same size near and far (and it
    // is a no-op for an orthographic projection, whose `w` is 1).
    let corner = input.uv * 2.0 - 1.0;
    let offset = corner * (SPRITE_PX / globals.viewport.xy) * output.clip_position.w;
    output.clip_position = vec4<f32>(
        output.clip_position.xy + offset,
        output.clip_position.z,
        output.clip_position.w,
    );
    return output;
}

// The sprite's ink is its per-vertex colour, unlit: a dot is a mark on the drawing, not a
// surface. The quad is `SPRITE_PX` across and the MSAA target antialiases its edge, so
// there is no per-pixel coverage term to compute.
@fragment
fn fs_sprite(input: VertexOutput) -> @location(0) vec4<f32> {
    return globals.color * input.color;
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
