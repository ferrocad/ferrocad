// FerroCAD grid overlay: the WGSL port of the addon's `grid.vert.glsl` +
// `grid.frag.glsl` (GLSL 1.20, compatibility profile).
//
// A full-screen quad. The four vertices come from the shared scene vertex buffer
// (location 0 only; the other attributes are unused) and only their sign matters:
// each corner picks a screen corner. The vertex stage emits the world-space near/far
// points of that corner's view ray; the fragment stage intersects the ray with the
// z = 0 plane, per pixel, and derives the decade LOD from screen-space derivatives.
//
// Deviations from the reference, all deliberate (`docs/grid-overlay-plan.md` §3.4):
//
//  * WebGPU NDC z is [0, 1]; GL's is [-1, 1]. The unprojection uses near z = 0 and
//    far z = 1 (the GLSL uses -1 / +1), and the fragment writes `clip.z / clip.w`
//    directly instead of `clip.z / clip.w * 0.5 + 0.5`.
//  * `fwidth()` does not exist in WGSL: `fwidth` below is `abs(dpdx) + abs(dpdy)`,
//    which is the GLSL definition.
//  * The program's uniforms live in group 2, not the fixed-function built-ins. The
//    layout is the fixed ABI: group 2 binding 0 is `Frame` (per frame), binding 1 is
//    `Program` (per program). Groups 0 and 1 are layout-only; this module declares
//    no bindings there, so the shared pipeline layout keeps working.
//  * Our tokens replace the addon's hard-coded colours: the axis hues are the
//    linear-token values of #F87171 / #4ADE80, and the ink arrives as `grid_color`.
//  * Shadows are deferred. The block and its loop are present, compiled and
//    validated, but `shadow_count` is 0 in this slice, so the loop never runs.
//    Document bounds are not wired yet (§3.5); a later stage producer fills
//    `shadows` without restructuring this file.
//
// Derivatives are computed before every `discard` (WGSL inherits the GLSL rule).

struct Frame {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // x = 1 for a perspective projection (an explicit host flag; do NOT infer it
    // from the matrix), yz = target resolution in physical pixels, w reserved.
    flags: vec4<f32>,
};

struct Program {
    grid_color: vec4<f32>,
    // (base_spacing, minor_alpha, major_alpha, shadow_opacity)
    values: vec4<f32>,
    // (shadow_count, ...) — the rest reserved.
    counts: vec4<f32>,
    // 64 slots x 5 floats packed as 80 vec4s: uniform array elements are 16-byte
    // aligned, so a float array cannot have a 4-byte stride. Flat float `o` lives at
    // `shadows[o / 4u][o % 4u]`; slot i starts at `o = i * SHADOW_STRIDE`.
    shadows: array<vec4<f32>, 80>,
};

@group(2) @binding(0) var<uniform> frame: Frame;
@group(2) @binding(1) var<uniform> program: Program;

const MIN_PX: f32 = 8.0; // finest visible line spacing in pixels (approx.)
const DEPTH_BIAS: f32 = 1e-6; // push the grid slightly back so coplanar faces win
const X_AXIS: vec3<f32> = vec3<f32>(0.939, 0.165, 0.165); // #F87171, linear
const Y_AXIS: vec3<f32> = vec3<f32>(0.069, 0.730, 0.216); // #4ADE80, linear
const SHADOW_MAX: u32 = 64u;
const SHADOW_STRIDE: u32 = 5u;

struct VertexInput {
    @location(0) position: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) near_point: vec3<f32>,
    @location(1) far_point: vec3<f32>,
};

struct FragmentOutput {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

fn unproject(ndc: vec2<f32>, z: f32) -> vec3<f32> {
    let point = frame.inv_view_proj * vec4<f32>(ndc, z, 1.0);
    return point.xyz / point.w;
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    let ndc = sign(input.position.xy);
    // WebGPU NDC z is [0, 1]: the near plane is 0, the far plane is 1 (GL: -1/+1).
    output.near_point = unproject(ndc, 0.0);
    output.far_point = unproject(ndc, 1.0);
    output.clip_position = vec4<f32>(ndc, 0.0, 1.0);
    return output;
}

// GLSL's fwidth: the sum of the absolute screen-space derivatives.
fn fwidth(v: f32) -> f32 {
    return abs(dpdx(v)) + abs(dpdy(v));
}

// Coverage of a ~1px line at integer values of c. d = cells per pixel.
fn line_cov(c: f32, d: f32) -> f32 {
    let cells = max(d, 1e-12);
    let dist_px = abs(fract(c - 0.5) - 0.5) / cells;
    let cov = 1.0 - clamp(dist_px, 0.0, 1.0);
    // Fade out when cells get too dense to resolve (kills moire).
    return cov * (1.0 - smoothstep(0.25, 0.5, cells));
}

// Lines perpendicular to one world axis, with its own decade LOD.
// w = coordinate in base units, dw = its screen-space derivative.
fn axis_grid(w: f32, dw: f32) -> f32 {
    let d = max(dw, 1e-12);
    let l = log(d * MIN_PX) * 0.4342944819; // log10
    let e = floor(l);
    let f = l - e; // 0..1 blend between decades
    let s0 = pow(10.0, e + 1.0);
    let s1 = s0 * 10.0;
    let s2 = s1 * 10.0;

    let minor = program.values.y;
    let major = program.values.z;
    let a0 = (1.0 - f) * minor * line_cov(w / s0, d / s0);
    let a1 = mix(major, minor, f) * line_cov(w / s1, d / s1);
    let a2 = f * major * line_cov(w / s2, d / s2);
    return max(a0, max(a1, a2));
}

fn axis_line(w: f32, dw: f32) -> f32 {
    let dist_px = abs(w) / max(dw, 1e-12);
    return clamp(1.5 - dist_px, 0.0, 1.0);
}

// Flat float access into the 16-byte-strided shadow block.
fn shadow_at(index: u32) -> f32 {
    return program.shadows[index / 4u][index % 4u];
}

@fragment
fn fs_main(input: VertexOutput) -> FragmentOutput {
    // Ray through this pixel. Perspective rays start at the camera, so the grid is
    // NOT limited to the renderer's near/far range; orthographic rays are parallel,
    // so any t along them is valid.
    let dir = normalize(input.far_point - input.near_point);
    let persp = frame.flags.x > 0.5;
    let origin = select(input.near_point, frame.eye.xyz, persp);
    var dz = dir.z;
    if (abs(dz) < 1e-8) {
        dz = 1e-8;
    }
    let t = -origin.z / dz;
    let p = origin + t * dir;

    // All derivatives first, so the discards below cannot corrupt them.
    let w = p.xy / program.values.x;
    let dw = vec2<f32>(fwidth(w.x), fwidth(w.y));
    let gx = axis_grid(w.x, dw.x);
    let gy = axis_grid(w.y, dw.y);
    let ax = axis_line(p.y, fwidth(p.y)); // x axis (y == 0)
    let ay = axis_line(p.x, fwidth(p.x)); // y axis (x == 0)

    // Fade toward the horizon where the plane goes edge-on.
    let grazing = smoothstep(0.0, 0.08, abs(dir.z));

    let clip = frame.view_proj * vec4<f32>(p, 1.0);
    // Perspective: t < 0 means the plane is behind the camera (sky).
    if ((persp && t < 0.0) || clip.w <= 0.0) {
        discard;
    }

    var a = max(gx, gy);
    var rgb = program.grid_color.rgb;
    rgb = mix(rgb, X_AXIS, ax);
    a = max(a, ax * program.values.z);
    rgb = mix(rgb, Y_AXIS, ay);
    a = max(a, ay * program.values.z);
    a = a * grazing;

    // Soft per-object ground shadows, composited under the lines in the same pass
    // and depth test as the grid. Deferred: `shadow_count` is 0 in this slice, so
    // the loop is compiled and validated but never entered (plan §3.5).
    var sa = 0.0;
    if (program.values.w > 0.0 && program.counts.x > 0.5) {
        for (var i: u32 = 0u; i < SHADOW_MAX; i = i + 1u) {
            if (f32(i) >= program.counts.x) {
                break;
            }
            let o = i * SHADOW_STRIDE;
            let c = vec2<f32>(shadow_at(o), shadow_at(o + 1u));
            let h = vec2<f32>(shadow_at(o + 2u), shadow_at(o + 3u));
            let blur = max(shadow_at(o + 4u), 1e-6);
            let q = p.xy - c;
            // Approximate signed distance to an axis-aligned ellipse.
            let k0 = length(q / h);
            let k1 = length(q / (h * h));
            let sdf = k0 * (k0 - 1.0) / max(k1, 1e-9);
            if (sdf > blur * 1.6) {
                continue;
            }
            let k = max(sdf, 0.0) / blur; // 0 at/inside the footprint
            sa = max(sa, exp(-3.8 * k * k));
        }
        sa = sa * program.values.w * grazing;
    }

    let oa = a + sa * (1.0 - a);
    if (oa < 0.003) {
        discard;
    }
    let orgb = (rgb * a + vec3<f32>(0.03) * sa * (1.0 - a)) / oa;

    var output: FragmentOutput;
    output.color = vec4<f32>(orgb, oa);
    // WebGPU NDC z is [0, 1] already, so there is no `* 0.5 + 0.5` remap here (the
    // GLSL needs it because GL's clip z is [-1, 1]). Clamp instead of clip: a grid
    // outside the near/far range still draws, pinned to the depth limits so model
    // geometry keeps occluding it.
    output.depth = clamp(clip.z / clip.w + DEPTH_BIAS, 0.0, 0.999999);
    return output;
}
