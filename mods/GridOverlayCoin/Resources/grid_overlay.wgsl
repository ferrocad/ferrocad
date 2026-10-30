// FerroCAD grid overlay: the WGSL port of the addon's `grid.vert.glsl` +
// `grid.frag.glsl` (GLSL 1.20, compatibility profile).
//
// A full-screen quad. The four vertices come from the shared scene vertex buffer
// (location 0 only; the other attributes are unused) and only their sign matters:
// each corner picks a screen corner. The vertex stage emits the world-space near/far
// points of that corner's view ray; the fragment stage intersects the ray with the
// active plane (the host's workplane tail of `Frame`, default world `XY`), per pixel,
// and draws the host's two steps in that plane: dots at the minor step, lines at the
// major one.
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
//  * The grid is built in the **active construction plane's** `u`/`v` frame, not world
//    `XY`. That plane is host state (`Frame.workplane_*`), never a mod parameter: a mod
//    cannot claim a plane the viewport is not on. The default frame is world `XY` at
//    the origin with radius 0, so an unset workplane is exactly the old ground.
//  * The host sends the native grid's **extent** (a face patch, or the camera-following
//    ground), so the plane is bounded before it reaches its horizon. A radius of 0 still
//    means "unbounded", but the viewport no longer sends it.
//  * The edge-on fade uses the **camera's** angle to the plane, not the per-pixel ray
//    angle: the ray varies across the view, and its zero-crossing is the plane's own
//    horizon line, which draws as a soft band through the model.
//  * A non-zero patch radius fades the grid out radially around the plane origin,
//    using the native grid's `smoothstep(0.55 r, r)` law.
//  * Ground shadows are **plane-relative**: the host projects each item onto the
//    active plane and sends the `(u, v)` AABB, so the shader reads `(pu, pv)`. A
//    world-`(x, y)` footprint would land off to the side on a vertical workplane.
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
    // The active construction plane, supplied by the host (not by a parameter node):
    // xyz = the plane's world origin, w = the patch radius (0 = no patch, the
    // unbounded ground).
    workplane_origin: vec4<f32>,
    // The in-plane unit axes: u is the grid's x, v its y. w is reserved.
    workplane_u: vec4<f32>,
    workplane_v: vec4<f32>,
    // The unit plane normal (u x v). w is reserved. The default is world XY:
    // origin 0, u = +X, v = +Y, n = +Z, radius 0.
    workplane_n: vec4<f32>,
    // xy = the plane's minor and major steps in **world units**, as the host resolved
    // them for this camera (the native grid's effective steps). zw = the two levels'
    // **base ink** (the native token alphas times the user's opacity); 0 means the host
    // does not claim it and `program.values.yz` is used.
    workplane_steps: vec4<f32>,
    // The host's stroke: x = 1 when the minor level is a line lattice (0 = intersection
    // dots), y/z = the major/minor stroke widths in **world units** (0 = the 1 px
    // hairline below). The native generator paints the same in-plane strokes, so the two
    // backends show the same line weight.
    grid_stroke: vec4<f32>,
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

const DEPTH_BIAS: f32 = 1e-6; // push the grid slightly back so coplanar faces win
// The fraction of a patch's radius that stays at full alpha; beyond it the grid fades
// to nothing at the radius. Matches `ferrocad_stage::grid::FADE_START`.
const PATCH_FADE_START: f32 = 0.55;
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

// The host's base ink for a level, or the program's own strength when the host does not
// claim it (a pre-ink `Frame` tail). Using the host's alphas is what makes the two
// backends' lines the same weight: a shader drawing 4x the ink reads as a thicker line
// at the same width.
fn ink_for(host: f32, fallback: f32) -> f32 {
    return select(fallback, host, host > 0.0);
}

// Coverage of a line at integer values of c. `d` is cells per pixel for this axis, `step`
// the level's world step and `width` the stroke's width in world units (0 = 1 px).
fn line_cov(c: f32, d: f32, step: f32, width: f32) -> f32 {
    let cells = max(d, 1e-12);
    let dist_px = abs(fract(c - 0.5) - 0.5) / cells;
    // The stroke is measured in the plane's units, exactly like the native quads, then
    // converted to pixels on this axis so its edge can be feathered over one pixel.
    let half_px = max(0.5, 0.5 * width / (cells * step));
    let cov = 1.0 - clamp(dist_px - (half_px - 0.5), 0.0, 1.0);
    // Fade out when cells get too dense to resolve (kills moire).
    return cov * (1.0 - smoothstep(0.25, 0.5, cells));
}

// The grid's coverage in the plane's frame: the minor level (intersection **dots**, or a
// full lattice with `grid_stroke.x`) and **lines** at the major step — the native
// `grid::ground` look.
//
// The reference shader drew a line lattice at every level it picked. With the host's
// steps that means a line mesh at the minor step, which on a small face patch is ~10x
// the native grid's fragments (and reads as a dense cage). Drawing the minor level as
// dots keeps the two backends identical: the product of both axes' coverage is a dot.
fn grid_alpha(pu: f32, pv: f32, dwu: f32, dwv: f32) -> f32 {
    var minor_step = frame.workplane_steps.x;
    var major_step = frame.workplane_steps.y;
    if (minor_step <= 0.0 || major_step <= 0.0) {
        // A host that predates the step tail: fall back to the parameter's base spacing.
        let base = max(program.values.x, 1e-6);
        minor_step = base;
        major_step = base * 10.0;
    }
    let minor_width = max(frame.grid_stroke.z, 0.0);
    let major_width = max(frame.grid_stroke.y, 0.0);
    let minor_ink = ink_for(frame.workplane_steps.z, program.values.y);
    let major_ink = ink_for(frame.workplane_steps.w, program.values.z);
    let minor_u = line_cov(pu / minor_step, dwu / minor_step, minor_step, minor_width);
    let minor_v = line_cov(pv / minor_step, dwv / minor_step, minor_step, minor_width);
    // Dots at the minor intersections (both axes on a line at once), or the lattice the
    // user asked for (`minor_style: lines`).
    var minor = minor_u * minor_v;
    if (frame.grid_stroke.x > 0.5) {
        minor = max(minor_u, minor_v);
    }
    let dots = minor * minor_ink;
    // Lines at the major step, along either axis.
    let major_u = line_cov(pu / major_step, dwu / major_step, major_step, major_width);
    let major_v = line_cov(pv / major_step, dwv / major_step, major_step, major_width);
    let lines = max(major_u, major_v) * major_ink;
    return max(dots, lines);
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
    // The active construction plane. The host writes unit axes; the default frame is
    // world XY, which reproduces the pre-workplane ground exactly.
    let plane_origin = frame.workplane_origin.xyz;
    let plane_u = frame.workplane_u.xyz;
    let plane_v = frame.workplane_v.xyz;
    let plane_n = frame.workplane_n.xyz;
    let patch_radius = frame.workplane_origin.w;

    // Ray through this pixel. Perspective rays start at the camera, so the grid is
    // NOT limited to the renderer's near/far range; orthographic rays are parallel,
    // so any t along them is valid. Intersect the ray with the plane, not z = 0.
    let dir = normalize(input.far_point - input.near_point);
    let persp = frame.flags.x > 0.5;
    let origin = select(input.near_point, frame.eye.xyz, persp);
    let nd = dot(dir, plane_n);
    var denom = nd;
    if (abs(denom) < 1e-8) {
        denom = 1e-8;
    }
    let t = dot(plane_origin - origin, plane_n) / denom;
    let p = origin + t * dir;
    // The world point in the plane's own frame (world units around its origin).
    let rel = p - plane_origin;
    let pu = dot(rel, plane_u);
    let pv = dot(rel, plane_v);

    // All derivatives first, so the discards below cannot corrupt them.
    let g = grid_alpha(pu, pv, fwidth(pu), fwidth(pv));
    let ax = axis_line(pv, fwidth(pv)); // u axis (v == 0)
    let ay = axis_line(pu, fwidth(pu)); // v axis (u == 0)

    // Fade the **whole plane** as the camera goes edge-on to it. This is a property of
    // the camera, not of the pixel: `dir` varies per fragment, so `abs(dot(dir, n))`
    // reaches zero along the plane's own horizon line and paints a soft band across the
    // view — a vertical one on a `YZ` plane, straight through the model. The centre ray
    // is the same for every fragment, so the fade is uniform.
    let centre = vec2<f32>(0.0);
    let centre_dir = select(
        normalize(unproject(centre, 1.0) - unproject(centre, 0.0)),
        normalize(unproject(centre, 1.0) - frame.eye.xyz),
        persp,
    );
    let grazing = smoothstep(0.0, 0.08, abs(dot(centre_dir, plane_n)));

    let clip = frame.view_proj * vec4<f32>(p, 1.0);
    // Perspective: t < 0 means the plane is behind the camera (sky).
    if ((persp && t < 0.0) || clip.w <= 0.0) {
        discard;
    }

    var a = g;
    let major_ink = ink_for(frame.workplane_steps.w, program.values.z);
    // The grid dims edge-on; the two axis lines keep their ink, because they are what
    // still says where the plane is when the view runs along it.
    a = a * grazing;
    // A face-bound plane is a finite patch: fade it out radially around the plane
    // origin (the native grid's law). Radius 0 is the unbounded ground.
    if (patch_radius > 0.0) {
        let distance = length(vec2<f32>(pu, pv));
        a = a * (1.0 - smoothstep(PATCH_FADE_START * patch_radius, patch_radius, distance));
    }
    var rgb = program.grid_color.rgb;
    rgb = mix(rgb, X_AXIS, ax);
    a = max(a, ax * major_ink);
    rgb = mix(rgb, Y_AXIS, ay);
    a = max(a, ay * major_ink);

    // Soft per-object shadows, composited under the lines in the same pass and depth
    // test as the grid. The footprints are in the active plane's `(u, v)` frame, so a
    // bound workplane is shaded too.
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
            // A footprint is an `(u, v)` rectangle in the active plane's frame (the host
            // projects each item onto that plane), so `q` is its own coordinate there.
            let q = vec2<f32>(pu, pv) - c;
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
