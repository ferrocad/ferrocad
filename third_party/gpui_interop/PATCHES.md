# Vendored `gpui_interop` 0.2.0 — surface-input gaps

This directory is a copy of the crates.io source for `gpui_interop` 0.2.0
(<https://github.com/bite-gpui/gpui_interop>). Changed from that copy:

| File | Change |
| --- | --- |
| `src/input.rs` | `SurfaceEvent::Scroll` + `SurfaceScroll`; `PointerDown.click_count`; `PointerEnter`/`PointerLeave`; an `App`-held pressed origin so an off-surface up is always delivered |
| `src/gpui_interop.rs` | re-export `SurfaceScroll` |
| `examples/interop_cube_drag.rs` | handle `SurfaceEvent::Scroll` and the hover edges in its `match`, instead of binding its own `on_scroll_wheel` |

Everything else (`Cargo.toml.orig`, license files, `README.md`, the other
examples, the crate's own tests) is unchanged.

The two patched files are byte-identical between 0.1.3 and 0.2.0, so this patch
ports without changes; 0.2.0's only reason to move was the `bite-gpui 1.25.203`
/ wgpu 30 bump, and it does **not** include any of the four fixes below.

It is wired in through the workspace root `Cargo.toml`:

```toml
[patch.crates-io]
gpui_interop = { path = "third_party/gpui_interop" }
```

## Why

`surface_input` is the input half of FerroCAD's browser viewport: the probe
(`crates/ferrocad_web_gpui`) drives the camera entirely through it. Four gaps
mattered, and each is filed upstream:

- **No scroll.** `SurfaceEvent` is down/move/up only — while the enum's own doc
  links to a `Scroll` variant that does not exist, and the example says "the
  gateway's vocabulary carries no scroll yet". Everything needed is present
  (the platform delivers `ScrollWheelEvent`, and a listener is one line).
  <https://github.com/bite-gpui/gpui_interop/issues/2>
- **Lost `PointerUp`.** The up outside the surface is only reported while the
  engine's drag is in flight, and that drag is founded only by a left press past
  a 2 px threshold. A right or middle press released off-canvas therefore
  produces no up, the producer's pressed state never clears, and the next hover
  move keeps orbiting.
  <https://github.com/bite-gpui/gpui_interop/issues/3>
- **No `click_count`.** `MouseDownEvent` has it (and the browser computes it);
  the gateway dropped it, so a producer cannot tell a click from a double-click.
  <https://github.com/bite-gpui/gpui_interop/issues/4>
- **No enter/leave.** A hover highlight cannot be cleared.
  <https://github.com/bite-gpui/gpui_interop/issues/5>

A fifth gap is the engine's, not the gateway's: `on_drag` starts only for the
left button (`bite-gp-authoring/src/elements/div.rs:2978-2980`), so a
middle/right gesture cannot be captured past the element. Filed against
`bite-gpui`: <https://github.com/bite-gpui/bite-gpui/issues/20>, and fixed in
1.25.203 with `InteractiveElementExt::on_drag_with(button, ...)`. The gateway
still calls `on_drag` (left) until it grows a button parameter, so a middle/
right gesture through `surface_input` is still left-button-only; that is a
follow-up, not part of this patch.

## Changes

1. **`SurfaceScroll`** (`Pixels { x, y }` / `Lines { x, y }`) and
   `SurfaceEvent::Scroll { position, delta, modifiers }`, delivered from a new
   `on_scroll_wheel` listener in `attach`. Deliberately unnormalised: a consumer
   re-derives its own notch rate, and the sign is the engine's (positive is
   "up", already the opposite of the browser's `deltaY`).
2. **`PointerDown { .., click_count: usize }`**, copied from the event.
3. **`PointerEnter { modifiers }` / `PointerLeave { modifiers }`**, delivered
   from an `on_hover` listener. No position: the event says the pointer came or
   went, not where it is.
4. **A `PressedSurface(Option<ElementId>)` global** set on any down and cleared
   on the up paths. `on_mouse_up_out` now reports the up when *either* the
   engine drag is this surface's *or* this surface owns the press, which is what
   makes a right/middle release off-canvas visible. It lives on the `App` (next
   to the engine's drag) because the element-side state only exists for one
   paint.

## Using it

- The probe handles `SurfaceEvent::Scroll` instead of binding
  `on_scroll_wheel` itself, and uses `click_count >= 2` to recentre the camera.
- Producers still pair their own down with their up (the gateway holds no
  gesture state), but they can now also treat `PointerMove { pressed: None }`
  while grabbed as an implicit release; the probe does both, so a dropped up
  cannot wedge the camera even if this patch is reverted.

## Removing this

Drop the `[patch.crates-io]` entry once the four issues above are fixed
upstream. (This workspace is on the 1.25.203 family and wgpu 30, which is what
0.2.0 targets; the vendored copy stays until the input fixes land, not
until the version wall moves.)

## Verified

- The probe builds in the cross image and runs in headless Chrome/WebGL2
  (ANGLE + SwiftShader). Four captures of the same document:
  - no input vs `?drag=1`: **22 027** pixels differ (orbit);
  - no input vs `?wheel=1`: **26 127** pixels differ (zoom, now through
    `SurfaceEvent::Scroll`);
  - no input vs `?dblclick=1` (drag, then two quick clicks): **0** pixels
    differ — the double-click recentred to the demo pose, which is also the
    proof that `click_count` arrives;
  - `?drag=1` vs `?dblclick=1`: 22 027.
- The vendored crate's examples compile on their own, which the workspace build
  never checks (they need the `example` feature):
  `cargo check --manifest-path third_party/gpui_interop/Cargo.toml --features example --examples --locked`.
  This matters because the patch adds enum variants: an exhaustive `match` in an
  example stops compiling. The `headless` CI job runs the same command.
- **Not verified:** the crate's own `input-test` suite was not run in this
  fork. Adding enter/leave puts two more events in the recorded stream, so some
  of its exact-sequence assertions will need updating; the integration test here
  is the probe.
