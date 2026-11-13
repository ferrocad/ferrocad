// Axis Cross (JS): the world axis cross as a JS workbench's program overlay.
//
// A JS workbench has no `pivy.coin` and no filesystem. What it does have is
// `window.ferrocad.overlay(spec)`: the host extracts the JSON scene, registers it with
// the same pull bridge the Coin backends use, and the viewport compiles the WGSL.
//
// This backend is a **renderer, not a settings store**. The axis policy — the shared
// `FerroCAD.Axis` switch, inks, length and flags — is the feature's one registry; the
// shell pushes it as `onViewState`, and this module renders exactly those values. It
// advertises no schema and no command, so `Settings/Axis` is one form whichever backend
// is loaded, and turning the axis off hides this overlay too.
//
// The cross is a screen-space overlay: a four-corner placeholder quad, and a fragment
// shader that projects the world origin and the three unit axes into pixels. It is
// depth-tested, so model geometry in front occludes it. `overlay` is the one place the
// shader source crosses into Rust; `view` carries the native-hide claim, exactly as the
// Coin backend's `axis.overlay` does.
//
// What this backend does not draw: the occluded "ghost" stems (a single depth pass
// cannot also compare `Greater`) and the X/Y/Z tags (WGSL has no text). The policy still
// carries those values for the native backend.

const NAME = "axis-overlay-js";
const FEATURE = "axis";
const BACKEND = "js";
const LABEL = "Axis Cross (JS)";

// The shared axis policy, mirrored from `onViewState`. The linear tokens are the
// fallbacks for the moment before the first state arrives.
let axis = {
  enabled: true,
  colors: [
    [0.939, 0.165, 0.165], // #F87171
    [0.069, 0.730, 0.216], // #4ADE80
    [0.036, 0.474, 0.930], // #38BDF8
  ],
  opacity: 0.65,
  length: 80,
  width: 1.5,
  origin: true,
  labels: true,
  ghost_alpha: 0.1,
};
// Whether the chain's slot has this backend loaded, and whether the host accepted the
// overlay as visible. The backend never selects itself: `setProvider` loads it.
let loaded = false;
let shown = false;

// The whole WGSL module. It declares the fixed `Frame`/`Program` blocks the compiler
// binds (`ferrocad_graphics::state`), the placeholder vertex stage, and the
// pixel-space cross. It is deliberately self-contained: a JS mod ships one file.
//
// The look: each positive stem is solid and runs to the plane's patch edge (the same
// radius the grid fades at), so it stops at the horizon rather than crossing into the
// sky; on an unbounded ground it runs to the edge of the visible scene. The negative
// side is the same stem the other way, dashed and dimmer. A round badge on the positive
// end carries the axis letter in white on a solid ink disc and sits at the **part's
// extent** (the host's stage AABB), so it follows the model rather than the window;
// without a stage it marks the stem's end. The stems write their real world depth, so
// the part occludes them, while the badges are annotations drawn on top. `axis_length`
// is the minimum stem length.
//
// Both the stems and their fades are **screen space**: a stem ends where its world end
// projects, and dissolves over the projected '0.55 r ... r' the grid fades at, so the
// two can never disagree. The world distance along the axis is only used for depth.
const SHADER = `
struct Frame {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    flags: vec4<f32>,
    // The host's active-plane tail: xyz = plane origin, w = grid radius. The stems
    // fade at the same radius the grid does, so they never leave the plane's horizon.
    workplane_origin: vec4<f32>,
    workplane_u: vec4<f32>,
    workplane_v: vec4<f32>,
    workplane_n: vec4<f32>,
    workplane_steps: vec4<f32>,
    grid_stroke: vec4<f32>,
};

struct Program {
    grid_color: vec4<f32>,
    values: vec4<f32>,
    counts: vec4<f32>,
    shadows: array<vec4<f32>, 80>,
    axis_y: vec4<f32>,
    axis_z: vec4<f32>,
    axis_values: vec4<f32>,
    axis_flags: vec4<f32>,
    // The host's stage AABB. A w of 1 means claimed; the badges end at these extents.
    bounds_min: vec4<f32>,
    bounds_max: vec4<f32>,
};

@group(2) @binding(0) var<uniform> frame: Frame;
@group(2) @binding(1) var<uniform> program: Program;

struct VertexInput {
    @location(0) position: vec3<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> @builtin(position) vec4<f32> {
    // The four corners become the four screen corners; only their signs matter.
    return vec4<f32>(sign(input.position.xy), 0.0, 1.0);
}

struct Projected {
    pixel: vec2<f32>,
    valid: bool,
};

struct FragmentOutput {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

// One stem's coverage, its badge disc, and the letter on the disc. The three are
// separate so the badge can cover a stem and the letter can cover the badge.
struct Stroke {
    line: f32,
    badge: f32,
    glyph: f32,
};

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

// Antialiased coverage of a segment of half-width 'half' and 'aa' pixels of edge.
fn segment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, half: f32, aa: f32) -> f32 {
    let ab = b - a;
    let denom = max(dot(ab, ab), 1.0e-12);
    let t = clamp(dot(p - a, ab) / denom, 0.0, 1.0);
    let closest = a + ab * t;
    let dist = length(p - closest);
    return 1.0 - smoothstep(half - aa * 0.5, half + aa * 0.5, dist);
}

fn capsule(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, half: f32) -> f32 {
    return segment(p, a, b, half, 1.0);
}

// Unproject an NDC point at depth 'z' (0 near, 1 far) back into world space.
fn unproject(ndc: vec2<f32>, z: f32) -> vec3<f32> {
    let p = frame.inv_view_proj * vec4<f32>(ndc, z, 1.0);
    return p.xyz / p.w;
}

// The clip depth of a world point, or 'fallback' when it is behind the eye.
fn world_depth(world: vec3<f32>, fallback: f32) -> f32 {
    let clip = frame.view_proj * vec4<f32>(world, 1.0);
    if (clip.w <= 0.0) {
        return fallback;
    }
    return clip.z / clip.w;
}

// The clip depth of the point of the world axis through the origin that projects to
// 'ndc', or 'fallback' when the pixel's ray does not meet the axis.
//
// The cross is a screen-space overlay, so without this every stem would write the
// origin's depth and draw over the part. Intersecting the pixel's camera ray with the
// world axis gives the stem's real depth, so the model occludes it exactly like the
// native triad.
//
// 'span' is the axis's patch radius (0 for the unbounded ground). The crossing is taken
// on [-span, span], because at a grazing angle the ray is nearly parallel to the axis and
// the solve can put the crossing almost anywhere, which made a grazing stem flicker in
// and out behind the part. A stem is only ever drawn inside that span, so clamping
// cannot move a depth that was already right. A crossing that is then behind the eye
// falls back to the origin's depth.
fn axis_depth(ndc: vec2<f32>, axis: vec3<f32>, span: f32, fallback: f32) -> f32 {
    // The near-plane point is the ray origin for both projections: a perspective ray
    // starts at the eye, an orthographic one at its own point on the near plane.
    let near = unproject(ndc, 0.0);
    let far = unproject(ndc, 1.0);
    let direction = far - near;
    let length_sq = dot(direction, direction);
    if (length_sq < 1.0e-12) {
        return fallback;
    }
    let d = direction / sqrt(length_sq);
    let b = dot(d, axis);
    let denom = 1.0 - b * b;
    if (abs(denom) < 1.0e-6) {
        // The ray runs along the axis: there is no single crossing to take.
        return fallback;
    }
    var s = (dot(axis, near) - b * dot(d, near)) / denom;
    if (span > 0.0) {
        s = clamp(s, -span, span);
    }
    return world_depth(axis * s, fallback);
}

// A badge clears the part's boundary by this gap, and the viewport edge by this inset.
// Both are physical pixels.
const BADGE_GAP: f32 = 8.0;
const EDGE_MARGIN: f32 = 14.0;
// The stems dissolve over the projected stretch where the grid's own radial fade runs
// ('0.55 r .. r'), and over the last pixels before the viewport edge.
const AXIS_FADE_START: f32 = 0.55;
const EDGE_FADE: f32 = 48.0;

// How far 'p' travels along 'd' before leaving the viewport inset by 'margin'.
fn edge_reach(p: vec2<f32>, d: vec2<f32>, size: vec2<f32>, margin: f32) -> f32 {
    var t = 1.0e9;
    if (abs(d.x) > 1.0e-6) {
        let bound = select(size.x - margin, margin, d.x < 0.0);
        t = min(t, (bound - p.x) / d.x);
    }
    if (abs(d.y) > 1.0e-6) {
        let bound = select(size.y - margin, margin, d.y < 0.0);
        t = min(t, (bound - p.y) / d.y);
    }
    return max(t, 0.0);
}

// A short dash along the negative ray: half on, half off, one pixel of edge.
fn dash(along: f32) -> f32 {
    let period = 9.0;
    let phase = fract(along / period);
    let aa = 1.0 / period;
    return smoothstep(0.0, aa, phase) * (1.0 - smoothstep(0.5, 0.5 + aa, phase));
}

// One 'X', 'Y' or 'Z' in the unit box, with the stroke width and edge in that box.
fn glyph(p: vec2<f32>, letter: f32, half: f32, aa: f32) -> f32 {
    if (letter < 0.5) {
        return max(
            segment(p, vec2<f32>(-0.72, -0.72), vec2<f32>(0.72, 0.72), half, aa),
            segment(p, vec2<f32>(-0.72, 0.72), vec2<f32>(0.72, -0.72), half, aa),
        );
    }
    if (letter < 1.5) {
        return max(
            max(
                segment(p, vec2<f32>(-0.72, -0.72), vec2<f32>(0.0, 0.0), half, aa),
                segment(p, vec2<f32>(0.72, -0.72), vec2<f32>(0.0, 0.0), half, aa),
            ),
            segment(p, vec2<f32>(0.0, 0.0), vec2<f32>(0.0, 0.72), half, aa),
        );
    }
    return max(
        max(
            segment(p, vec2<f32>(-0.72, -0.72), vec2<f32>(0.72, -0.72), half, aa),
            segment(p, vec2<f32>(0.72, -0.72), vec2<f32>(-0.72, 0.72), half, aa),
        ),
        segment(p, vec2<f32>(-0.72, 0.72), vec2<f32>(0.72, 0.72), half, aa),
    );
}

// One half of a stem, in screen pixels from the origin: where it ends, and where its fade
// begins.
struct Stem {
    end: f32,
    fade: f32,
};

// The screen distance of 'point' from 'origin' along 'dir', or -1 when it projects behind
// the eye: the stem cannot end at a point that is not on the screen.
fn along_screen(point: vec3<f32>, origin: vec2<f32>, dir: vec2<f32>, size: vec2<f32>) -> f32 {
    let projected = project(point, size);
    if (!projected.valid) {
        return -1.0;
    }
    return max(dot(projected.pixel - origin, dir), 0.0);
}

// The screen distance of the axis's own vanishing point from the origin.
//
// A cross cannot usefully reach past it: beyond that point the axis's image is only the
// far half coming back, and near it every world distance projects into the same pixel.
// This is also what bounds a stem whose axis runs past the camera inside its own patch -
// its near end then projects arbitrarily far out, which is how one axis came to cover the
// whole window at full strength while its badge sat on the edge. A very large value means
// there is no such point (an orthographic view, where parallel lines never converge).
fn vanishing_reach(axis: vec3<f32>, origin: vec2<f32>, size: vec2<f32>) -> f32 {
    let clip = frame.view_proj * vec4<f32>(axis, 0.0);
    if (abs(clip.w) < 1.0e-6) {
        return 1.0e9;
    }
    let ndc = clip.xy / clip.w;
    let pixel = vec2<f32>(
        (ndc.x * 0.5 + 0.5) * size.x,
        (1.0 - (ndc.y * 0.5 + 0.5)) * size.y,
    );
    return length(pixel - origin);
}

// One half of the axis on screen: the stem ends where its world end projects, and fades
// over the projected '0.55 r .. r' the grid fades at.
//
// A screen-space fade is what keeps this stable. The world distance along the axis (the
// closest point of the pixel's ray) says nothing at a grazing angle: the ray is then
// nearly parallel to the axis, so the distance it reports drifts across the stem, the
// fade lands in the wrong place and the stem can stay bright to its very end. Measuring
// both the end and the fade in the same space also means they always agree, and the
// badge - placed from the same end - stays on its stem.
//
// A radius of 0 is the unbounded ground: the stem runs to the viewport edge and softens
// there.
fn stem_half(
    axis: vec3<f32>,
    origin: vec2<f32>,
    dir: vec2<f32>,
    size: vec2<f32>,
    patch_radius: f32,
) -> Stem {
    // The window crops the stem, and its own vanishing point bounds it.
    let edge = edge_reach(origin, dir, size, 0.0);
    let vanish = vanishing_reach(axis, origin, size);
    let cap = min(edge, vanish);
    // The window softens its crop; a stem that stops at its vanishing point dissolves over
    // the outer stretch it has instead.
    let soften = select(
        edge - EDGE_FADE,
        min(edge - EDGE_FADE, cap * AXIS_FADE_START),
        vanish < edge,
    );
    if (patch_radius <= 0.0) {
        return Stem(cap, clamp(soften, 0.0, cap));
    }
    let end = along_screen(axis * patch_radius, origin, dir, size);
    let shrink = along_screen(axis * (AXIS_FADE_START * patch_radius), origin, dir, size);
    // 'end' is negative when the patch edge projects behind the eye, which happens when
    // the axis passes the camera inside its own patch: the vanishing point is then the
    // only end there is.
    let tip = select(cap, min(end, cap), end >= 0.0);
    let fade = min(soften, select(cap * AXIS_FADE_START, shrink, shrink >= 0.0));
    return Stem(tip, clamp(fade, 0.0, tip));
}

// The grid's radial fade as a factor on a screen distance along the stem.
fn stem_fade(along: f32, stem: Stem) -> f32 {
    let span = max(stem.end - stem.fade, 0.5);
    return 1.0 - smoothstep(0.0, 1.0, clamp((along - stem.fade) / span, 0.0, 1.0));
}

// One axis: a solid positive stem, a dashed negative stem, and a lettered badge.
//
// The stems run to the plane's patch edge when the host supplies a radius (the same
// 'r' the radial fade ends at), and to the visible edge when the ground is unbounded.
// When the host has measured the stage, the badge ends just outside the part's extent
// along this axis and the stems write their real world depth, so the part occludes
// them; without a stage the badge marks the stem's end. Either way the badge is held
// inside the stem's own end and the viewport inset, so it never floats past its stem or
// clips at the edge.
fn axis_stroke(
    pixel: vec2<f32>,
    origin: vec2<f32>,
    toward: vec2<f32>,
    size: vec2<f32>,
    min_len: f32,
    half: f32,
    radius: f32,
    letter: f32,
    labels: bool,
    axis: vec3<f32>,
    position_extent: f32,
    claimed: bool,
    patch_radius: f32,
) -> Stroke {
    var out = Stroke(0.0, 0.0, 0.0);
    let len = length(toward);
    if (len < 1.0e-4) {
        return out;
    }
    let dir = toward / len;

    // Positive: solid, out to the plane's edge, dissolving over the grid's own fade.
    let pos = stem_half(axis, origin, dir, size, patch_radius);
    let reach = max(pos.end, min_len);
    let tip = origin + dir * reach;
    let pos_along = dot(pixel - origin, dir);
    out.line = capsule(pixel, origin, tip, half) * stem_fade(pos_along, pos);

    // Negative: the same axis backwards, dashed and dimmer.
    let neg = stem_half(-axis, origin, -dir, size, patch_radius);
    let back = max(neg.end, min_len);
    let behind = origin - dir * back;
    let neg_along = dot(origin - pixel, dir);
    let dashes = dash(neg_along);
    out.line = max(
        out.line,
        capsule(pixel, origin, behind, half) * dashes * 0.55 * stem_fade(neg_along, neg),
    );

    if (labels) {
        // The badge sits just outside the part's extent (a clean gap before the body) or,
        // without a stage, at the stem's end. 'limit' is that end minus the disc and its
        // gap, and never past the viewport inset; the disc's rim then keeps BADGE_GAP
        // from the stem's end and EDGE_MARGIN from the window.
        let limit = max(
            min(
                reach - radius - BADGE_GAP,
                edge_reach(origin, dir, size, EDGE_MARGIN + radius),
            ),
            0.0,
        );
        var along = limit;
        if (claimed && position_extent > 0.0) {
            let projected = project(axis * position_extent, size);
            if (projected.valid) {
                along = clamp(
                    dot(projected.pixel - origin, dir) + radius + BADGE_GAP,
                    0.0,
                    limit,
                );
            }
        }
        let center = origin + dir * along;
        let d = pixel - center;
        out.badge = 1.0 - smoothstep(radius - 0.5, radius + 0.5, length(d));
        if (out.badge > 0.003) {
            // The glyph is evaluated in badge-local pixels so its edge stays 1 px.
            let scale = radius * 0.62;
            let width = max(0.9, radius * 0.11) / scale;
            out.glyph = glyph(d / scale, letter, width, 1.0 / scale) * out.badge;
        }
    }
    return out;
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> FragmentOutput {
    let size = frame.flags.yz;
    let origin_clip = frame.view_proj * vec4<f32>(0.0, 0.0, 0.0, 1.0);
    if (origin_clip.w <= 0.0) {
        discard;
    }
    let origin_ndc = origin_clip.xy / origin_clip.w;
    let origin = vec2<f32>(
        (origin_ndc.x * 0.5 + 0.5) * size.x,
        (1.0 - (origin_ndc.y * 0.5 + 0.5)) * size.y,
    );

    let pixel = position.xy;
    let min_len = max(program.axis_values.x, 0.0);
    let half = max(program.axis_values.y * 0.5, 0.5);
    let opacity = program.axis_values.z;
    let labels = program.axis_flags.y > 0.5;
    let radius = clamp(7.0 + program.axis_values.y * 4.0, 10.0, 18.0);
    let origin_depth = origin_clip.z / origin_clip.w;
    let ndc = vec2<f32>(
        (pixel.x / size.x) * 2.0 - 1.0,
        1.0 - (pixel.y / size.y) * 2.0,
    );

    // The host measures the stage, so the badges mark the part's extents. Without a
    // stage the claim is off and they mark their stems' ends instead.
    let claimed = program.bounds_max.w > 0.5;
    let extent_x = max(program.bounds_max.x, 0.0);
    let extent_y = max(program.bounds_max.y, 0.0);
    let extent_z = max(program.bounds_max.z, 0.0);

    let axis_x = project(vec3<f32>(1.0, 0.0, 0.0), size);
    let axis_y = project(vec3<f32>(0.0, 1.0, 0.0), size);
    let axis_z = project(vec3<f32>(0.0, 0.0, 1.0), size);

    let sx = axis_stroke(
        pixel, origin,
        select(vec2<f32>(0.0), axis_x.pixel - origin, axis_x.valid),
        size, min_len, half, radius, 0.0, labels,
        vec3<f32>(1.0, 0.0, 0.0), extent_x, claimed, frame.workplane_origin.w,
    );
    let sy = axis_stroke(
        pixel, origin,
        select(vec2<f32>(0.0), axis_y.pixel - origin, axis_y.valid),
        size, min_len, half, radius, 1.0, labels,
        vec3<f32>(0.0, 1.0, 0.0), extent_y, claimed, frame.workplane_origin.w,
    );
    let sz = axis_stroke(
        pixel, origin,
        select(vec2<f32>(0.0), axis_z.pixel - origin, axis_z.valid),
        size, min_len, half, radius, 2.0, labels,
        vec3<f32>(0.0, 0.0, 1.0), extent_z, claimed, frame.workplane_origin.w,
    );

    var coverage = 0.0;
    var ink = program.grid_color.rgb;
    var alpha_scale = opacity;
    var depth = origin_depth;

    // Stems first; the strongest coverage owns the pixel. Each stem has already faded at
    // the grid's radius (its own half carries the screen-space fade) and writes the real
    // depth of the world axis at this pixel, so the part occludes it.
    let span = frame.workplane_origin.w;
    if (sx.line > coverage) {
        coverage = sx.line;
        ink = program.grid_color.rgb;
        alpha_scale = opacity;
        depth = axis_depth(ndc, vec3<f32>(1.0, 0.0, 0.0), span, origin_depth);
    }
    if (sy.line > coverage) {
        coverage = sy.line;
        ink = program.axis_y.rgb;
        alpha_scale = opacity;
        depth = axis_depth(ndc, vec3<f32>(0.0, 1.0, 0.0), span, origin_depth);
    }
    if (sz.line > coverage) {
        coverage = sz.line;
        ink = program.axis_z.rgb;
        alpha_scale = opacity;
        depth = axis_depth(ndc, vec3<f32>(0.0, 0.0, 1.0), span, origin_depth);
    }

    // The origin node is a small Z-coloured dot, like the native cross's centre.
    if (program.axis_flags.x > 0.5) {
        let node = 1.0 - smoothstep(1.5, 2.5, length(pixel - origin));
        if (node > coverage) {
            coverage = node;
            ink = program.axis_z.rgb;
            alpha_scale = opacity;
            depth = origin_depth;
        }
    }

    // The badges are solid annotations, not geometry: draw them on top so a boundary that
    // falls behind the part still labels the axis (the reference's badges are always
    // visible). The stems keep their real depth.
    if (sx.badge > coverage) {
        coverage = sx.badge;
        ink = program.grid_color.rgb;
        alpha_scale = 1.0;
        depth = 0.0;
    }
    if (sy.badge > coverage) {
        coverage = sy.badge;
        ink = program.axis_y.rgb;
        alpha_scale = 1.0;
        depth = 0.0;
    }
    if (sz.badge > coverage) {
        coverage = sz.badge;
        ink = program.axis_z.rgb;
        alpha_scale = 1.0;
        depth = 0.0;
    }

    // The letters are white and always sit on top of their own disc.
    if (sx.glyph > 0.003) {
        coverage = sx.glyph;
        ink = vec3<f32>(1.0);
        alpha_scale = 1.0;
        depth = 0.0;
    } else if (sy.glyph > 0.003) {
        coverage = sy.glyph;
        ink = vec3<f32>(1.0);
        alpha_scale = 1.0;
        depth = 0.0;
    } else if (sz.glyph > 0.003) {
        coverage = sz.glyph;
        ink = vec3<f32>(1.0);
        alpha_scale = 1.0;
        depth = 0.0;
    }

    let alpha = coverage * alpha_scale;
    if (alpha < 0.003) {
        discard;
    }
    var output: FragmentOutput;
    output.color = vec4<f32>(ink, alpha);
    // WebGPU NDC z is already [0, 1]; no GL remap is needed. The small bias keeps a
    // badge that lies exactly on a surface from losing the Less test.
    output.depth = clamp(depth - 1.0e-5, 0.0, 1.0);
    return output;
}
`;

/** The host kernel, or `null` in a build with no JS page. */
function kernel() {
  return window.ferrocad ?? null;
}

/** Call one kernel sink; both hosts answer `""` on success or the reason. */
function call(sink, payload) {
  const error = sink(payload);
  if (error) {
    throw new Error(error);
  }
}

/** The overlay description the host extracts, from the shared axis policy. */
function description() {
  return {
    name: NAME,
    visible: loaded && axis.enabled,
    shader: SHADER,
    parameters: {
      grid_color: axis.colors[0],
      axis_color_y: axis.colors[1],
      axis_color_z: axis.colors[2],
      axis_length: Number(axis.length),
      axis_width: Number(axis.width),
      axis_opacity: Number(axis.opacity),
      axis_ghost_alpha: Number(axis.ghost_alpha),
      axis_origin: axis.origin ? 1.0 : 0.0,
      axis_labels: axis.labels ? 1.0 : 0.0,
    },
  };
}

/** Register or remove the overlay. The claim is only stated on a switch/refresh. */
function showOverlay() {
  const host = kernel();
  if (!host || !host.overlay) {
    throw new Error("no overlay kernel on this host");
  }
  call(host.overlay, JSON.stringify(description()));
  shown = loaded && axis.enabled;
}

/** Register (or remove) the overlay and claim the native axis. Throws on refusal. */
function publish() {
  const host = kernel();
  if (!host || !host.view) {
    throw new Error("no view kernel on this host");
  }
  showOverlay();
  // The claim follows the shared switch: a disabled overlay has nothing to hide.
  call(host.view, JSON.stringify({ axis: { overlay: axis.enabled } }));
}

/** Remove the overlay, keeping the shared policy untouched. */
function release() {
  const host = kernel();
  if (!host || !host.overlay) {
    throw new Error("no overlay kernel on this host");
  }
  call(host.overlay, JSON.stringify({ name: NAME, visible: false }));
  shown = false;
}

/** The chain loaded this backend: draw the shared policy. */
function activateJs() {
  loaded = true;
  publish();
  return {
    status: axis.enabled ? "Axis Cross (JS) active" : "Axis Cross (JS) off",
    view: { axis: { overlay: axis.enabled } },
  };
}

/** The chain unloaded this backend: its overlay steps aside. */
function deactivateJs() {
  loaded = false;
  release();
  return {
    status: "Axis Cross (JS) stopped",
    view: { axis: { overlay: false } },
  };
}

/** Shared state changed: redraw from the policy. */
function refreshJs() {
  // Being asked to refresh means the chain's slot holds this backend: a single-host
  // JS build has no other provider to route the boot refresh to.
  loaded = true;
  publish();
  return {
    status: axis.enabled ? "Axis Cross (JS) refreshed" : "Axis Cross (JS) off",
    view: { axis: { overlay: axis.enabled } },
  };
}

/** The shared policy changed (the shell pushed a view state). */
function applyViewState(text) {
  let state;
  try {
    state = JSON.parse(text);
  } catch (error) {
    return;
  }
  const next = state && state.axis;
  if (!next) {
    return;
  }
  if (typeof next.enabled === "boolean") axis.enabled = next.enabled;
  if (Array.isArray(next.colors) && next.colors.length === 3) {
    axis.colors = next.colors.map((color) =>
      [0, 1, 2].map((index) => Number(color[index]) || 0),
    );
  }
  const number = (value, fallback) => {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : fallback;
  };
  axis.opacity = number(next.opacity, axis.opacity);
  axis.length = number(next.length, axis.length);
  axis.width = number(next.width, axis.width);
  axis.ghost_alpha = number(next.ghost_alpha, axis.ghost_alpha);
  if (typeof next.origin === "boolean") axis.origin = next.origin;
  if (typeof next.labels === "boolean") axis.labels = next.labels;
  if (loaded) {
    try {
      showOverlay();
    } catch (error) {
      console.error("axis-js:", error);
    }
  }
}

/** This page's provider entry: the one backend it implements. */
function providerList() {
  return [{ backend: BACKEND, label: LABEL, active: loaded, command: null }];
}

function providersPayload() {
  return JSON.stringify([{ feature: FEATURE, providers: providerList() }]);
}

/** The chain's load verb. The page never selects itself. */
function setProviderInternal(feature, backend) {
  if (feature !== FEATURE) {
    return { ok: false, error: `no provider for ${feature}` };
  }
  if (backend !== BACKEND) {
    return { ok: false, error: `${backend} is not this page's backend` };
  }
  return Object.assign({ ok: true }, activateJs());
}

/** The chain's unload verb. */
function releaseProviderInternal(feature) {
  if (feature !== FEATURE) {
    return { ok: false, error: `no provider for ${feature}` };
  }
  return Object.assign({ ok: true }, deactivateJs());
}

function refreshProviderInternal(feature) {
  if (feature !== FEATURE) {
    return { ok: false, error: `no provider for ${feature}` };
  }
  return Object.assign({ ok: true }, refreshJs());
}

function numberText(value) {
  return String(Number(Number(value).toFixed(2)));
}

function providerSummary(feature) {
  // The native backend's summary is the document half's: it knows the style the
  // viewport is actually drawing, and it merges this page's provider list itself.
  if (feature !== FEATURE || !loaded) {
    return null;
  }
  return {
    feature: FEATURE,
    backend: BACKEND,
    label: LABEL,
    length: `${numberText(axis.length)} px`,
    width: `${numberText(axis.width)} px`,
    providers: providerList(),
  };
}

export function install() {
  const page = window.__ferrocadJs ?? {};
  window.__ferrocadJs = Object.assign({}, page, {
    active: page.active ?? (() => true),
    commands: page.commands ?? (() => JSON.stringify([])),
    evaluate: page.evaluate ?? ((code) => `js: ${code}`),
    runCommand: page.runCommand ?? (() => false),
    stageMeshes: page.stageMeshes ?? (() => "[]"),
    providers: providersPayload,
    setProvider: (feature, backend) =>
      JSON.stringify(setProviderInternal(feature, backend)),
    releaseProvider: (feature) =>
      JSON.stringify(releaseProviderInternal(feature)),
    refreshProvider: (feature) =>
      JSON.stringify(refreshProviderInternal(feature)),
    providerSummary: (feature) => JSON.stringify(providerSummary(feature)),
    providerSkin: () => JSON.stringify({ source: null }),
    onViewState: applyViewState,
  });
}
