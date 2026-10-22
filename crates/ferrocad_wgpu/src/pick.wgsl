// The G-buffer picking pass: render each draw with its pick id, sub-element index and
// surface normal into offscreen integer/float targets. A 1x1 readback then identifies
// what is under the cursor (O(1), unlike a CPU ray cast).

struct Globals {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    color: vec4<f32>,
    // Declared to match the scene shader's uniform layout; picking ignores both.
    shade: vec4<f32>,
    pick: u32,
    topology: u32,
    depth_bias: f32,
    two_tone: u32,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // An integer vertex output cannot be interpolated: it must be `flat`.
    @location(3) part_idx: u32,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) @interpolate(flat) part_idx: u32,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    let world = globals.model * vec4<f32>(input.position, 1.0);
    // Uniform scale is assumed, so the model matrix doubles as the normal matrix.
    output.world_normal = (globals.model * vec4<f32>(input.normal, 0.0)).xyz;
    output.part_idx = input.part_idx;
    var position = globals.view_proj * world;
    // Match the scene pass: a highlighted overlay is also the thing under the cursor.
    position.z -= globals.depth_bias * position.w;
    output.position = position;
    return output;
}

struct FragmentOutput {
    // Target 0 (Rg32Uint): [entity_id, part_idx].
    @location(0) id_and_part: vec2<u32>,
    // Target 1 (Rgba32Float): [normal.xyz, topology].
    @location(1) normal_and_topology: vec4<f32>,
    // Target 2 (R32Float): the depth, written from the fragment's `@builtin(position)`.
    // A depth *attachment* cannot be read back a texel at a time on every backend, so the
    // pass also writes it to a colour target the host can copy.
    @location(2) depth: f32,
};

@fragment
fn fs_main(input: VertexOutput) -> FragmentOutput {
    var output: FragmentOutput;
    // `pick + 1` so the cleared target (0) means "empty" while a real object that happens
    // to carry `PickId(0)` is still a hit.
    output.id_and_part = vec2<u32>(globals.pick + 1u, input.part_idx);
    var normal = input.world_normal;
    let length = length(normal);
    normal = select(vec3<f32>(0.0, 0.0, 1.0), normal / length, length > 1e-6);
    output.normal_and_topology = vec4<f32>(normal, f32(globals.topology));
    // `position.z` is the fragment depth (the same value the depth buffer holds).
    output.depth = input.position.z;
    return output;
}
