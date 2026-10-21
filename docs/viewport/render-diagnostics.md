# Render diagnostics

`ferrocad_wgpu` has opt-in diagnostics for the GPU path (the "black screen" class
of bug). They are compiled in, but every entry point is an atomic load and a
branch when disabled: no allocation, no formatting, no logging.

## Enable

Either knob works; `--trace` is checked by `ferrocad_gpui::run_with` and by
`check_init`, so an edition's binary does not need to parse it.

```sh
# environment variable
FERROCAD_TRACE=1 ./target/debug/ferrocad_toy

# command-line flag (the shell reads it before booting Python)
./target/debug/ferrocad_toy --trace
```

`FERROCAD_TRACE=0`, `false`, `off` and an empty value all mean off.
The flag is a development/diagnosis tool: **do not ship it on**.

## What is logged

Every line starts with `ferrocad trace:` and a monotonic `t=<ms>` timestamp, so
an intermittent event has a trail even when it is not caught live.

| Line | When | Contains |
| --- | --- | --- |
| `gpu: adapter=… backend=… device_type=… driver=…` | renderer creation | adapter identity |
| `gpu: target format=… samples=… min_uniform_buffer_offset_alignment=…` | renderer creation | the selected target format and sample count, plus the limits that shape the ABI |
| `gpu: limits={…}` / `gpu: features={…}` | renderer creation | the full wgpu limits/features |
| `surface: none (offscreen target; …)` | renderer creation | this crate has no swapchain (see below) |
| `callbacks: installed …` | renderer creation | the error hooks are live |
| `frame N: render/encode/views=… packets=… programs=…` | every frame | frame counter and the drawn batch shape |
| `frame N: view I eye=(…) projection=…` | every view | the camera, so a repro script can tell whether its orbit/zoom reached the viewport |
| `frame N: group2 …` | every group-2 bind | program hash or `default`, frame slot, uniform slot, dynamic offset |
| `program pipeline: hash=… depth_compare=… depth_write=… samples=… format=…` | pipeline cache miss | the exact state the pipeline was built for |
| `program buffer: hash=… size=1328 bytes` | program group creation | the fixed ABI block |
| `frame N: encoded views=…` / `frame N: submitted … index=…` | encode/submit | the command buffer reached the queue, and its submission index |
| `presenter: ring depth=… target=… size=… release_capable=…` | presenter creation and every resize | the ring was (re)allocated at one size, and whether the slots carry a consumer-release descriptor |
| `frame N: presenter slot=… depth=… reuse_distance=… via_release=… submission=…` | every zero-copy frame | which ring slot was written, how many submissions since it was last used, and whether the consumer's release chose it |
| `presenter: repainted last frame slot=… age=… (reason)` | a paint with no new frame | the blank-frame guard repeated the last complete frame instead of painting an empty canvas |
| `presenter: skipped a frame (reason); depth=…` | a draw found no reusable slot | all slots may still be in flight; the last complete frame is repainted |
| `presenter: nothing to present (reason)` | a paint with no new frame and no valid previous frame | the canvas genuinely has nothing (startup, or every presented slot failed) |
| `presenter: disabled after present (reason); …` | presenter failure | the last good frame was shown, then the image path takes over |
| `resize: AxB -> CxD samples=…` | target resize | the attachments were recreated |
| `frame N: wgpu validation/out-of-memory/internal: …` | any uncaptured error | the full wgpu message and its detail |
| `frame N: device lost: reason=… message=…` | device loss | why the device went away |
| `frame N: black (r=…,g=…,b=…) size=…` | readback probe | an all-but-black frame (see below) |
| `frame N: recovered (no longer black)` | readback probe | the first lit frame after a black one |

The frame number is the one in flight when an error arrives, which is why the
counter is incremented before encoding: a validation error from frame N cannot be
mistaken for frame N+1.

### The black-frame probe

`image()` (the readback fallback path) classifies the frame from a 5 x 5 sample
grid that excludes the border. A frame is black when every sampled pixel's
brightest channel is below 8/255. The sample is cheap and the classification is
only run while tracing is enabled.

**The zero-copy presenter path cannot be probed this way.** On Linux the viewport
renders into a `WgpuSharedBuffer` (`gpui_interop`), or a `WgpuSurface` where
export is unavailable, and hands it to gpui without a readback. Those textures
are created with `RENDER_ATTACHMENT` only, so there is no `COPY_SRC`/sampling
path the renderer could use to inspect them. The trace still names every frame
that was encoded; a black presenter frame has to be inferred from the
`present`/import failures the presenter already prints (`ferrocad: dma-buf
submit failed`, `ferrocad: imported viewport texture failed`) plus the absence
of `wgpu` errors here. The presenter's own lines (`presenter: ring`, `presenter
slot=…`, `presenter: repainted`, `presenter: skipped`, `presenter: disabled`)
show whether a frame was overwritten early, skipped, or replaced by the
fallback; `docs/viewport/gpu-interop-assessment.md` §10 explains the rule.

### Surfaces and `SurfaceError`

This application has no swapchain in `ferrocad_wgpu`: frames go to an offscreen
texture (image path) or the presenter's exportable texture (zero-copy path).
wgpu 29 therefore never surfaces an acquisition error here; the window surface is
gpui's. `trace::surface_status` maps wgpu 29's `CurrentSurfaceTexture` outcomes
(`Success`, `Suboptimal`, `Timeout`, `Occluded`, `Outdated`, `Lost`,
`Validation`) for a host that does own a surface, and `OutOfMemory` is reported
through the device error callback.

### Callback ownership

A wgpu device has exactly one `on_uncaptured_error` handler and one device-lost
callback. gpui's window renderer installs its own (it records errors silently in
`bite-gp-wgpu`'s `DeviceErrorState`). **Trace mode replaces those handlers on the
device the renderer wraps**, so the log can see a validation error that gpui would
otherwise swallow. That is deliberate; it is also why trace mode is not for
shipment. Without trace mode the handlers are untouched.

## Reproduce

The toy edition is the reproduction host. Cap the build jobs (the host lost a
GNOME session to memory pressure on 2026-10-21) and never leave more than one
GUI process running.

```sh
cd /home/john/projects/ferrocad/ferrocad    # the workspace root

# 1. Build once.
CARGO_BUILD_JOBS=2 cargo build -p ferrocad_toy

# 2. One bounded run, one log.
mkdir -p target/trace
FERROCAD_TRACE=1 timeout 60 ./target/debug/ferrocad_toy \
    > target/trace/toy.log 2>&1
echo "exit=$?"          # 124 means the timeout stopped it, as intended

# 3. Exercise the window while that run is up: resize the window (drag or
#    Alt+F10 twice), scroll to zoom, drag to orbit, and open/apply
#    View → Grid Overlay (Coin) from the commands bar (it has no default
#    accelerator; Ctrl+Shift+S is Toy_MakeSlab).

# 4. Read the trail around the last picture.
grep -n "wgpu \|device lost\|black (\|recovered\|resize:" target/trace/toy.log
tail -80 target/trace/toy.log
```

A non-interactive smoke run (no resizing) is still useful and is what CI can do:

```sh
FERROCAD_TRACE=1 timeout 20 ./target/debug/ferrocad_toy \
    > target/trace/smoke.log 2>&1
grep -c "frame .*: encode" target/trace/smoke.log   # frames rendered
grep -n "wgpu \|device lost" target/trace/smoke.log  # must be empty
```

The headless example exercises the program pipeline without a window (and does
run the black-frame probe, because it reads back):

```sh
PYTHONPATH=python cargo run -p ferrocad_gpui --example inventor_roundtrip \
    -- --coin-overlay target/trace/coin.png
# (rebuild the example first if the workspace changed)
```

Tests:

```sh
CARGO_BUILD_JOBS=2 cargo test -p ferrocad_wgpu -p ferrocad_gpui --offline \
    -- --test-threads=1
```

The GPU tests skip (rather than fail) when no adapter exists. The resize/program
test is `renderer::tests::a_program_view_survives_resize_between_frames`; the
black-frame classifier has CPU tests in `ferrocad_wgpu::trace::tests`.

## The 2026-10-21 run on this machine

Adapter: `Intel(R) UHD Graphics 630 (CFL GT2)`, Vulkan, Mesa 26.0.3, 4x MSAA,
`Rgba8UnormSrgb`, `min_uniform_buffer_offset_alignment = 64`.

- A 15 s traced windowed run presented through `dma-buf + sync_file`, drew a
  4-view / 49-packet frame with the Coin grid program (`programs=1`), resized
  twice at startup (640x480 -> 1176x940 -> 1176x938), and logged **no** wgpu
  error, device loss, presenter failure or black readback.
- The renderer re-encoded continuously (~13 fps) while the app was idle: frames
  1..187 over 14.3 s, all `views=4 packets=49 programs=1`. At the time the
  presenter owned a **single** exportable buffer, which the `gpui_interop`
  documentation explicitly says must not be written again until the consumer has
  sampled it ("keep a ring of buffers for a producer that renders
  continuously", `gpui_interop-0.1.0/src/wgpu_dmabuf.rs:246-249`). That was the
  leading hypothesis for an intermittent black/torn frame; the grid's
  full-screen fragment shader makes each frame longer, widening the window in
  which the previous frame has not been sampled. It is now fixed by the ring
  described in §10 of the assessment.
- The renderer-level path is ruled out for the tested cases: the multi-view
  program frame draws correctly at 4x MSAA before, after and between resizes.
- A mouse-only gesture was injected through a temporary `/dev/uinput` device
  (move to the viewport, scroll, drag). The camera trace never changed and the
  repaint loop stopped when the gesture was delivered, so the events did not
  reach the viewport in this session. No wgpu error was produced. The
  `view I eye=… projection=…` lines are what make this checkable in a live
  session; interactive resize/orbit/dialog coverage still needs a human at the
  window (or a session that permits screenshot/input automation).

Ranked hypotheses when a traced run shows no wgpu error:

1. **Single dma-buf reused while gpui still samples it** (presentation, not
   wgpu). Evidence: continuous encode at a steady rate, one output buffer, the
   upstream warning. **Addressed 2026-10-21**: `interop.rs` now allocates a
   three-deep ring, prefers the handle's `release` completion descriptor, and
   falls back to a `reuse_distance >= RING_DEPTH` rule; see
   `gpu-interop-assessment.md` §10 and the rerun below.
2. **Presenter failure masked as a black canvas.** `present` consumes the
   encoded commands; if a paint happens after `draw` returned `false` (or after
   `rebuild` cleared `commands`), the canvas paints nothing. **Addressed
   2026-10-21**: such a paint repaints the last complete frame, and a draw
   failure retires the presenter after that repaint rather than before it. The
   existing `eprintln!` lines in `interop.rs` remain the tell for a real failure.
3. **Device loss/OOM** under memory pressure. The trace names it directly; none
   was seen.
4. **Depth/MSAA invalidation across resize.** Ruled out at the renderer level by
   the resize test; the pipeline key includes sample count and format.

### The ring fix: traced rerun (2026-10-21)

Same command, after the ring landed:

```sh
FERROCAD_TRACE=1 timeout 30 ./target/debug/ferrocad_toy \
    > target/trace/toy-ring-final.log 2>&1
# exit=124 (the timeout stopped it)
```

Excerpt:

```text
ferrocad: viewport presenting through dma-buf + sync_file (3 buffers)
ferrocad trace: t=154ms presenter: ring depth=3 target=dma-buf + sync_file size=640x480 release_capable=true
ferrocad trace: t=182ms presenter: ring depth=3 target=dma-buf + sync_file size=1176x940 release_capable=true
ferrocad trace: t=305ms presenter: ring depth=3 target=dma-buf + sync_file size=1176x938 release_capable=true
ferrocad trace: t=664ms frame 5: presenter slot=2 depth=3 first_use=true submission=3
ferrocad trace: t=2367ms frame 6: presenter slot=1 depth=3 reuse_distance=2 via_release=true submission=4
ferrocad trace: t=3291ms frame 8: presenter slot=0 depth=3 reuse_distance=5 via_release=false submission=6
```

36 frames encoded over 23 s while the window was active (the idle app then
stopped requesting paint), 35 slot selections: four first uses, 30 chosen by the
consumer's release signal at a reuse distance of 2, and one conservative reuse at
distance 5 (`via_release=false`) after a lull when no release had been observed.
No reuse was below the ring depth. The two startup resizes reallocated all three
slots. No `wgpu` error, device loss, encode/submit/import failure or leftover
process. The run is still non-interactive: it does not prove the Coin-grid black
screen is gone, only that the overwrite race is no longer the unconditional
behaviour.
