// The canvas background: a full-screen triangle with the vertical void gradient.

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    // 0 at the bottom edge, 1 at the top edge.
    @location(0) t: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    // One triangle covers the viewport: (-1,-1), (3,-1), (-1,3).
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let p = positions[index];
    var out: VertexOut;
    out.position = vec4<f32>(p, 0.0, 1.0);
    out.t = p.y * 0.5 + 0.5;
    return out;
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    // Linear `#0F1218` at the top and `#161B22` at the bottom
    // (`ferrocad_graphics::theme::CANVAS_VOID` / `CANVAS_FAR`).
    let void = vec3<f32>(0.004777, 0.006049, 0.009134);
    let far = vec3<f32>(0.008023, 0.010960, 0.015996);
    return vec4<f32>(mix(far, void, clamp(input.t, 0.0, 1.0)), 1.0);
}
