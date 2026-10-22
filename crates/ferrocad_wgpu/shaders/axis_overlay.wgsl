// FerroCAD axis overlay: the world axis cross drawn as a screen-space overlay.
//
// The cross is an orientation aid, independent of the ground grid. A full-screen
// quad is projected from four placeholder corners (only their sign matters, as in
// `grid_overlay.wgsl`); every pixel then tests itself against three screen-space
// capsules that run from the projected world origin toward the projected unit
// axes. The quads are placed at the origin by the mod and the overlay is
// depth-tested with writes off, so model geometry in front still occludes it.
//
// This is the **Coin backend's visibility pass only**:
//
//  * `axis_ghost_alpha` is part of the fixed ABI but unused here. The
//    `SoDepthBuffer` node (`test = true`, `write = false`, `function = LESS`)
//    admits one depth comparison, so a second `Greater` pass for occluded stems is
//    not possible in this shader. The FerroCAD-native backend owns the ghost pass.
//  * `axis_labels` is accepted but unused: WGSL cannot draw text, and the native
//    gpui labels are a separate layer. Do not read the Coin backend as drawing
//    the X/Y/Z tags.
//  * `Program` carries the grid prefix (`grid_color`, `values`, `counts`,
//    `shadows`) so the block stays backward-compatible. `grid_color.rgb` is the X
//    stem's ink; the grid-only slots are unused here.
//
// Deviations from the grid shader are deliberate: there is no ray/plane
// intersection and no derivative use, because the cross is constructed in pixel
// space from the current `@builtin(position)`. Discards therefore cannot corrupt a
// derivative.

struct Frame {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // x = 1 for a perspective projection (an explicit host flag), yz = target
    // resolution in physical pixels, w reserved.
    flags: vec4<f32>,
};

struct Program {
    // The grid prefix (1,328 B). Only `grid_color` is used here; it is the X stem.
    grid_color: vec4<f32>,
    values: vec4<f32>,
    counts: vec4<f32>,
    shadows: array<vec4<f32>, 80>,
    // The axis extension (64 B).
    axis_y: vec4<f32>,
    axis_z: vec4<f32>,
    // (length_px, width_px, opacity, ghost_alpha)
    axis_values: vec4<f32>,
    // (origin_node, labels, 0, 0)
    axis_flags: vec4<f32>,
};

@group(2) @binding(0) var<uniform> frame: Frame;
@group(2) @binding(1) var<uniform> program: Program;

struct VertexInput {
    @location(0) position: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
};

struct FragmentInput {
    @builtin(position) position: vec4<f32>,
};

struct FragmentOutput {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    // The four corners become the four screen corners; the fragment stage does the
    // work. NDC z = 0 is the near plane in WebGPU; the fragment overrides depth.
    output.clip_position = vec4<f32>(sign(input.position.xy), 0.0, 1.0);
    return output;
}

// A projected point and whether it is in front of the eye.
struct Projected {
    pixel: vec2<f32>,
    valid: bool,
};

// Project a world point to top-left-origin pixel coordinates.
//
// A point at or behind the eye has no pixel position; `valid` is false and the
// caller skips that stem (it points at the eye).
fn project(point: vec3<f32>, size: vec2<f32>) -> Projected {
    let clip = frame.view_proj * vec4<f32>(point, 1.0);
    if (clip.w <= 0.0) {
        return Projected(vec2<f32>(0.0), false);
    }
    let ndc = clip.xy / clip.w;
    return Projected(
        vec2<f32>(
            (ndc.x * 0.5 + 0.5) * size.x,
            (1.0 - (ndc.y * 0.5 + 0.5)) * size.y,
        ),
        true,
    );
}

// Antialiased coverage of a capsule of half-width `half` around segment a→b.
fn capsule(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, half: f32) -> f32 {
    let ab = b - a;
    let denom = max(dot(ab, ab), 1.0e-12);
    let t = clamp(dot(p - a, ab) / denom, 0.0, 1.0);
    let closest = a + ab * t;
    let dist = length(p - closest);
    return 1.0 - smoothstep(half - 0.5, half + 0.5, dist);
}

@fragment
fn fs_main(input: FragmentInput) -> FragmentOutput {
    let size = frame.flags.yz;
    let origin_clip = frame.view_proj * vec4<f32>(0.0, 0.0, 0.0, 1.0);
    // The cross is anchored at the world origin: a projected origin behind the eye
    // has no pixel position, so nothing can be drawn.
    if (origin_clip.w <= 0.0) {
        discard;
    }
    let origin_ndc = origin_clip.xy / origin_clip.w;
    let origin_px = vec2<f32>(
        (origin_ndc.x * 0.5 + 0.5) * size.x,
        (1.0 - (origin_ndc.y * 0.5 + 0.5)) * size.y,
    );

    let pixel = input.position.xy;
    let length_px = max(program.axis_values.x, 0.0);
    let half = max(program.axis_values.y * 0.5, 0.5);
    let opacity = program.axis_values.z;

    // The projected direction of each unit axis. A direction shorter than ~1e-4 px
    // means the stem points at the eye; there is no screen-space line to draw.
    let axis_x = project(vec3<f32>(1.0, 0.0, 0.0), size);
    let axis_y = project(vec3<f32>(0.0, 1.0, 0.0), size);
    let axis_z = project(vec3<f32>(0.0, 0.0, 1.0), size);

    var coverage = 0.0;
    var ink = program.grid_color.rgb;

    if (axis_x.valid) {
        let dir = axis_x.pixel - origin_px;
        let len = length(dir);
        if (len >= 1.0e-4) {
            let tip = origin_px + dir / len * length_px;
            let c = capsule(pixel, origin_px, tip, half);
            if (c > coverage) {
                coverage = c;
                ink = program.grid_color.rgb;
            }
        }
    }

    if (axis_y.valid) {
        let dir = axis_y.pixel - origin_px;
        let len = length(dir);
        if (len >= 1.0e-4) {
            let tip = origin_px + dir / len * length_px;
            let c = capsule(pixel, origin_px, tip, half);
            // The three stems have distinct hues; the strongest coverage owns the pixel.
            if (c > coverage) {
                coverage = c;
                ink = program.axis_y.rgb;
            }
        }
    }

    if (axis_z.valid) {
        let dir = axis_z.pixel - origin_px;
        let len = length(dir);
        if (len >= 1.0e-4) {
            let tip = origin_px + dir / len * length_px;
            let c = capsule(pixel, origin_px, tip, half);
            if (c > coverage) {
                coverage = c;
                ink = program.axis_z.rgb;
            }
        }
    }

    // The origin node is a small Z-coloured dot, like the native cross's centre.
    if (program.axis_flags.x > 0.5) {
        let node = 1.0 - smoothstep(1.5, 2.5, length(pixel - origin_px));
        if (node > coverage) {
            coverage = node;
            ink = program.axis_z.rgb;
        }
    }

    let alpha = coverage * opacity;
    if (alpha < 0.003) {
        discard;
    }

    var output: FragmentOutput;
    output.color = vec4<f32>(ink, alpha);
    // The depth of the world origin, so model geometry in front occludes the
    // overlay. WebGPU NDC z is already [0, 1]; no GL remap is needed.
    output.depth = origin_clip.z / origin_clip.w;
    return output;
}
