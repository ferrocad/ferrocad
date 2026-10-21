# Upstream dma-buf race — filed issue and residual symptom

## The filed issue

- Repository: [`bite-gpui/gpui_interop`](https://github.com/bite-gpui/gpui_interop)
- Issue **#1**: <https://github.com/bite-gpui/gpui_interop/issues/1>
- Title: **Single exportable dma-buf is overwritten while the consumer may still
  sample it (intermittent black frames for a continuously rendering producer)**
- Filed 2026-10-21. The repository has issues enabled and no issue templates, so
  the report uses the generic shape: environment, symptom, evidence, the
  consumer-side workaround, four questions, and a minimal reproducer sketch.

The report asks `gpui_interop` to:

1. confirm whether `DmaBufHandle::release` is signalled once per **sampling**
   submission of a handle, and whether it can fire while the renderer still
   intends to sample that buffer;
2. say whether `WgpuSharedBuffer::submit` can expose consumer completion rather
   than only the producer's acquire fence;
3. state the recommended ring depth / reuse rule for a producer that renders
   continuously (3, 4, or waiting on a fence), and whether
   `reuse_distance >= RING_DEPTH` is actually sufficient;
4. say whether the same-device `WgpuSurface` path can expose a release
   descriptor too.

Environment recorded there: FerroCAD on Linux, Intel UHD Graphics 630 (CFL
GT2), Vulkan, Mesa 26.0.3, `gpui_interop` 0.1.0, `bite-gpui` 1.23.305,
`bite-gp-wgpu` 1.23.305, `wgpu` 29.0.4, `Rgba8UnormSrgb`, 4x MSAA.

## Residual symptom

The three-slot ring in `crates/ferrocad_gpui/src/interop.rs` removes the
unconditional reuse that caused the original race, but it does not eliminate
the black frames:

- **Black frames still occur** in the zero-copy viewport. They are intermittent
  and not deterministic; the ring does not make them disappear.
- **Only with the WGSL/Coin grid overlay** (`View → Grid Overlay (Coin)`). The
  native grid does not reproduce them. The overlay's full-screen fragment
  shader lengthens every frame, widening the window in which the consumer has
  not sampled the previous content.
- The ring **mitigates but does not eliminate** the symptom: residual black
  buffers are still observed after the fix, still only with the WGSL grid.
- It is not a renderer fault: a CPU/offscreen render of the same scene is
  correct, and traced runs show frames and submissions but no wgpu
  validation/out-of-memory error and no device loss.
- It is not the presenter failure path either: in the traced runs,
  `ferrocad: dma-buf submit failed` and `ferrocad: imported viewport texture
  failed` were absent.

Until the upstream questions are answered, treat this as an open residual
defect, not a regression of the presenter.

### Capturing one

```sh
FERROCAD_TRACE=1 timeout 60 ./target/debug/ferrocad_toy \
    > target/trace/dmabuf-black.log 2>&1

# exercise the window with View → Grid Overlay (Coin) on; resize and orbit
grep -n "frame .*: black\|recovered\|presenter slot=\|presenter: skipped\|presenter: repainted\|wgpu \|device lost" \
    target/trace/dmabuf-black.log
```

What to look for:

- `frame N: black (r=…,g=…,b=…) size=…` is the readback probe. It only runs on
  the image path — the zero-copy target is `RENDER_ATTACHMENT` only — so in
  zero-copy mode it appears only after the presenter has been disabled.
- In zero-copy mode the trail is the presenter lines: which
  `presenter slot=…` was written, its `reuse_distance` and `via_release`, and
  whether a `presenter: skipped a frame` or `presenter: repainted last frame`
  sits next to the black canvas. A slot written with `reuse_distance` close to
  3 and `via_release=false` is the signature to collect.
- `frame N: encoded` / `frame N: submitted index=…` prove the frame was
  produced; the absence of `wgpu`/`device lost` lines around it keeps the
  diagnosis in presentation.

The trace knobs and the full line list are in
[`render-diagnostics.md`](render-diagnostics.md).

## Usage guidance

Keep the presenter's current rules until upstream provides a stronger signal:

- **Keep `RING_DEPTH` at 3.** Two is the minimum for a producer that never
  waits; three covers a frame still queued behind gpui's own scene submission,
  which is the window the Coin grid's full-screen shader widens.
- **Prefer the release signal.** Use `DmaBufHandle::release` (written from
  gpui's `on_submitted_work_done`) to choose a slot whenever it has fired.
- **Never reuse a slot below depth 3** when no release was observed. The
  conservative rule is the safety net for the same-device path, a driver that
  does not signal, and a slow consumer; the ring tests assert this invariant.
- **Repaint the last good frame on skip.** A paint with nothing new to show
  repeats the newest valid handle instead of painting an empty canvas, and a
  genuine "nothing to present" is reported as such.
- **Reallocate all slots on resize.** The ring is allocated and replaced as a
  unit, so no old-size target can be sampled as if it were the new one.

## What would close this

Either of these, from `gpui_interop`:

- a **consumer-completion API** — `submit` (or the returned handle) reporting
  when the consumer has finished sampling, with the guarantee spelled out per
  sampling submission; then a producer can recycle a slot exactly instead of
  using a distance heuristic; or
- a **documented fence recipe** for a continuously rendering producer: the
  ring depth, when to wait on the release descriptor, and what the same-device
  `WgpuSurface` path should do with no release descriptor at all.

Context and the original diagnosis: [`gpu-interop-assessment.md` §10
("The presenting ring")](gpu-interop-assessment.md#10-the-presenting-ring-2026-10-21)
and [`render-diagnostics.md`](render-diagnostics.md).
