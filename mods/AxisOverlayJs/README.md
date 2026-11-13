# Axis Cross (JS)

A **JavaScript** backend of FerroCAD's world axis cross. It is the JS counterpart of
`mods/AxisOverlayCoin`: the same screen-space shader idea, but the workbench is
`InitGui.js` running on QuickJS (desktop) or in the browser page, and the shader is a
string in that file rather than a Coin tree and a `Resources/` read. It registers as
the `axis`/`js` provider beside the native `axis`/`ferrocad` entry, so the `Mods_Settings`
dialog and the axis popover can switch back and forth.

## Compatibility

This mod uses FerroCAD's JS kernel (`window.ferrocad.overlay` / `.view`) and the
host's `onViewState` notification, so it is **FerroCAD-only**. A host with no JS
script half never loads it; a JS page whose host does not install the overlay channel
reports `axis-js: no overlay kernel on this host` instead of crashing.

## What it contributes

| item | what it does |
| --- | --- |
| provider `axis`/`js` | the backend `Mods_Settings`, Dev Tools and the axis popover list; the chain loads it with `setProvider` and unloads it with `releaseProvider` |

There is **no command and no settings schema**. The axis policy — the shared
`FerroCAD.Axis` switch, inks, length, width and flags — is the feature's one registry;
the shell pushes it to the page as `onViewState` and this module renders exactly those
values. `Settings/Axis` therefore stays one form whichever backend is loaded, and the
one `App_ToggleAxisCross` switch hides this overlay exactly as it hides the native one.
A mod that does need provider-private rows can advertise them as a schema and the chain
will show that form instead; this backend has none.

The overlay is one JSON scene: a four-corner placeholder quad, a WGSL module with the
`@vertex` and `@fragment` entry points, and the fixed `Program` parameter block
(`grid_color`, `axis_color_y`, `axis_color_z`, `axis_length`, `axis_width`,
`axis_opacity`, `axis_ghost_alpha`, `axis_origin`, `axis_labels`). The fragment shader
projects the world origin and the three unit axes into pixels and draws the cross in
the style of a CAD orientation aid: each **positive** stem is solid, the **negative**
side is the same axis backwards, dashed and dimmer, and a round badge on the positive
end carries the axis letter in white on a solid ink disc.

The host measures the stage (`Program.bounds_min`/`bounds_max`, an additive tail on the
fixed ABI) and the badges end **just outside the part's extents** — a small pixel gap
before the body — and are held inside the viewport by a margin, so a label never
touches the part or clips at the window edge. Without a stage a badge marks its stem's
end, so it always reads as that axis's tip rather than floating in the sky. The stems
**end at the grid's radius**: the shader projects `±r` along the axis and stops the
screen-space stem there, and the last stretch dissolves over the projection of
`0.55 r … r`, the same stretch the grid fades over (`Frame.workplane_origin.w`).
Measuring both the end and the fade in **screen space** is what keeps the cross stable
at a grazing angle: the world distance along the axis comes from the closest point of
the pixel's ray, and with the ray nearly parallel to the axis that distance drifts
across the stem, so the fade used to land in the wrong place (or not at all). A stem
also never reaches past the axis's own **vanishing point**, where its image converges:
an axis that runs past the camera inside its own patch projects its near end
arbitrarily far out, which is how one stem came to cover the whole window at full
strength while its badge sat on the edge. The stems soften over the last pixels before
the window edge as well. The badges are annotations, so they draw on top; the stems
write the real world depth of the axis at each pixel (the camera ray intersected with
the axis line), so the part occludes them exactly as it occludes the native triad.
`axis_length` is the minimum stem length, not a fixed tip. The packet is depth-tested
with writes off.

## The JS channels

The host installs these JSON-in/JSON-out functions before the workbench loads:

| function | what it does |
| --- | --- |
| `window.ferrocad.call(request)` | the document API (not used by this mod) |
| `window.ferrocad.overlay(spec)` | register, replace or remove a named program overlay |
| `window.ferrocad.view(request)` | queue a `{"view": …}` request for the command outcome |
| `window.__ferrocadJs.onViewState(state)` | the shell's presentation state (the shared axis policy) |

An overlay spec is `{"name", "visible", "shader", "parameters", "points"}`. `visible:
false` removes the named overlay; `parameters` maps the ABI's slot names onto a number,
a three-number array (`vec3`) or a longer array (`array1f`). The host validates the
program with the same compiler a Coin overlay uses, so a misspelled parameter is
reported to the mod rather than silently drawing nothing.

`onViewState` carries the linear stem inks, the shared `enabled` switch, and the stem
length/width/opacity/flags the viewport is drawing. The module re-registers or removes
the overlay from that state; it calls `window.ferrocad.view({"axis": {"overlay": …}})`
only when the chain loads or refreshes it, so the claim follows the slot, not the state
notification.

## Intent that differs from the Coin backend

- **No Coin tree, no resource file.** The WGSL lives in `InitGui.js`; a JS mod ships one
  file and the browser needs no pack resource.
- **A renderer, not a store.** The shared axis policy is the one setting registry; this
  module owns no schema, no dialog and no switch. That is what makes `Settings/Axis` one
  form and `App_ToggleAxisCross` one switch across every backend.
- **The provider registry is in the page.** A JS workbench cannot call the kernel at
  import time (`window.ferrocad` is installed after `install()`), so the mod merges
  `providers`/`setProvider`/`releaseProvider`/`refreshProvider`/`providerSummary` into
  `window.__ferrocadJs`; `JsScriptHost` forwards the `ScriptHost` provider verbs to those
  hooks. The page lists **only** the `js` backend it implements; the native and Coin
  entries come from the Python host and `HostChain` merges the two catalogs.
- **No ghost pass.** A single depth pass cannot also draw occluded stems, so
  `ghost_alpha` is carried by the policy and the ABI but unused here; the native backend
  owns the ghost pass. The labels are **not** missing: WGSL cannot draw text, but the
  letters X/Y/Z are rasterised as antialiased strokes inside their badges, so
  `axis_labels` controls a real part of this backend rather than being a no-op.
- **Session state, shared settings.** The mirrored policy is per session (the shell
  pushes it); the values themselves live in `FerroCAD.Axis`, so switching backends keeps
  them. The chain's slot survives for the session; the JS page re-registers on boot.
- **Listed beside the Python backends.** A composed `cpython,js` desktop boot (the
  default) or a `pywasm,js` page **merges** the two catalogs: `HostChain` lists one entry
  per feature, each backend once, and one loaded backend. The JS backend therefore
  appears in `Mods_Settings`, the header popover and Dev Tools beside the native and Coin
  backends. A switch is routed to the half that lists it, and the summaries follow the
  loaded backend; the other hosts are released, so two overlays cannot draw together.

## Running it

A desktop build ships the JS half by default (`js` is a default feature and the
composition is `cpython,js`), and `--script-host=js` runs it alone. Either way the
shell discovers `mods/AxisOverlayJs/InitGui.js` like every other JS workbench. The
browser's `tools/mods_pack.py` copies the same file to `www/mods/AxisOverlayJs.js` and
names it in the pack index; a page running the JS half (`?js=1` / `?runtime=js`) imports
it before the module boots. Select **Axis Cross (JS)** in the `Settings/Axis` provider
row (or `Mods_Settings` / Dev Tools); the shared axis rows are the only form.

Headless coverage is in `crates/ferrocad_shell_js/tests/axis_js.rs`: it boots the real
`InitGui.js` on QuickJS, loads the provider, pushes a `ViewState`, and asserts the
registered overlay compiles to one program packet with the shared policy's values — and
that the shared `enabled` switch removes and restores it. Drawing itself needs a GPU and
a manual run; no readback is part of this slice.
