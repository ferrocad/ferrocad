# A minimal wgpu/WebGL host for the wasm kernel

Status: **research note (2026-11-02)**, followed by the first implementation
(`crates/ferrocad_web`, §11). The research itself changed no crate, manifest or
lockfile; the implementation is a separate commit.

What this note **ran** (host: Rust 1.95.0; container: Rust 1.97.0, clang 22 /
wasi-sdk 33, image digest `sha256:4dc88df…`):

- `cargo check -p ferrocad_wgpu --target wasm32-unknown-unknown` — passes.
- A scratch crate adding `wgpu = { version = "29", features = ["webgl"] }` — passes.
- The documented Docker repro, debug and release, plus import inspection under Node 22.
- A WASI call probe (13 stubs, call counting) and a **link-time WASI shim** that
  removes the imports entirely; both drive `occt_init()` + `filleted_box_triangles()`.
- `wasm-opt -Oz` (binaryen 120) on the release artifacts, re-run under Node.
- `wasm-bindgen` 0.2.129 (prebuilt CLI) on the OCCT module: glue generated, and the
  glue + optimized wasm run under Node and return the same 40 triangles.
- `cargo check -p ferrocad_gpui` and `-p ferrocad_viewport` for wasm (both fail; §2).
- The image's tool inventory (`wasm-bindgen`, `wasm-opt`, `wasm-pack` absent).

What this note **read** (not run): wgpu 29.0.4's surface, web and GLES backends;
the WebGPU Candidate Recommendation (2026-09-15); the WebGL 1.0 editor's draft;
`bite-gp-web 1.24.402`; the wasm-bindgen 0.2.129 release assets; the crate sources
cited by `file:line`.

There is **no browser and no GPU** in this environment, so every statement about
what a browser does with the frame is from the specs and the wgpu source, not from a
rendered canvas. §10 lists exactly what that leaves unverified.

---

## 0. TL;DR

1. **Host the example in a new `crates/ferrocad_web`** (`cdylib` + `wasm-bindgen`).
   Depend on `ferrocad_occt`, `ferrocad_stage`, `ferrocad_graphics`,
   `ferrocad_types`, `ferrocad_geom` and `ferrocad_wgpu` directly. Do not depend on
   `ferrocad_occt_wasm` (it is a `cdylib` smoke test, and a `cdylib` cannot be
   linked as a dependency).
2. **`ferrocad_wgpu` is wasm-clean today** — zero compile errors. Its two browser
   hazards are runtime, not compile-time: `block_on` (a spin loop) and the
   `image()` readback. A browser host must create the device asynchronously and
   must not call `image()`.
3. **It already renders into a caller-provided `wgpu::TextureView`**; MSAA resolves
   into it. The single hard blocker is the hard-coded `Rgba8UnormSrgb` target
   format: WebGPU canvases do not support sRGB formats. Either parameterize the
   renderer's format (≈20 lines, recommended) or use the canvas
   `format: rgba8unorm` + `viewFormats: [rgba8unorm-srgb]` trick (§4).
4. **The WASI shim should not be a JS package.** The 13 imports are never called on
   the box/fillet/tessellate path, and **13 same-named Rust stubs remove the WASI
   imports from the kernel module entirely** — measured: `WebAssembly.Module.imports()
   == []` and the module still returns 40. That is compatible with wasm-bindgen's
   glue, which has no hook for extra import modules. (The full example adds only
   wasm-bindgen's own `wbg` imports, which the glue supplies.)
5. **wasm-bindgen must match the lock exactly**: web-sys 0.3.106 pins
   `wasm-bindgen =0.2.129`; the 0.2.127 CLI rejects a 0.2.129 module (measured).
   Install the prebuilt 0.2.129 binary, not "latest".
6. **Size is dominated by OCCT**: release 14,791,735 B (LTO), `wasm-opt -Oz`
   10,402,652 B; 96.8 % of the code section is `_Z`-mangled C/C++ (OCCT, libc++ and
   the cxx bridge), 0.33 % is Rust. The browser-shaped artifact (shim + bindgen +
   `-Oz`) is 10,402,437 B.

Top three risks: (a) the canvas path is not browser-tested, and the WebGL branch
(MSAA resolve into the default framebuffer, `Depth32Float`, sRGB canvas) is
unverified; (b) the exact wasm-bindgen/CLI version match, which CI must derive from
the lock; (c) the WASI stubs are sound only for the linked OCCT subset — any new
kernel entry point that adds libc file/clock calls must extend or re-verify the stub
set.

---

## 1. The hypotheses, tested

| Hypothesis | Verdict | Evidence |
| --- | --- | --- |
| `gpui_interop` is native-only (dma-buf), so the GPUI shell cannot host a browser example. | **True for this module**, but not because GPUI lacks a web backend. | `crates/ferrocad_gpui/src/runtime/interop.rs:60-62` says it is compiled only where gpui's wgpu renderer exists, "not macOS or wasm"; `crates/ferrocad_gpui/src/lib.rs:37-42` and `:73-89` gate it with `not(target_family = "wasm")`. The dma-buf path is Linux-only (`interop.rs:70-78`). |
| GPUI itself has no wasm/browser backend. | **False** for the pinned fork. | `bite-gpui 1.24.402` depends on `bite-gp-web` on wasm (`Cargo.toml:407-410`) and `src/platform_entry.rs:16-20, 66-80, 111-121` constructs `gpui_web::WebPlatform`; `bite-gp-web-1.24.402/src/platform.rs:240-292` picks WebGPU then WebGL2 and keeps a `wgpu::Surface<'static>` on the canvas. **But** FerroCAD's shell still cannot build for wasm: `ferrocad_gpui` depends on PyO3 `auto-initialize` (`Cargo.toml:52`) and `pyo3-ffi` fails on wasm32 (measured, §2). |
| gpui's `image()` workaround is GPU→CPU→GPU. | **True**, and it is worse on the web. | `renderer.rs:1081-1162` copies the target to a `MAP_READ` buffer and maps it; the viewport's `refresh` wraps that in gpui's `RenderImage` (the path quoted in `docs/project/research/gpu-interop-assessment.md:52-60`). In a browser `Device::poll` is a no-op ("Device is polled automatically", `wgpu-29.0.4/src/backend/webgpu.rs:2560-2563`), so `get_mapped_range()` runs before the JS `mapAsync` promise settles and panics. |
| "Zero copy" in a browser means rendering into the canvas swapchain. | **True, with a format caveat.** | WebGPU: `getCurrentTexture()` exposes a writable view of the canvas `[[drawingBuffer]]` (`https://www.w3.org/TR/webgpu/#dom-gpucanvascontext-getcurrenttexture`), presented by the compositor; wgpu's web surface `present()` is a no-op ("Swapchain is presented automatically on the web", `backend/webgpu.rs:3946-3949`). WebGL: draws to the drawing buffer, presented to the compositor (`https://registry.khronos.org/webgl/specs/latest/1.0/#2.2`). The caveat is §4: the canvas texture cannot be `Rgba8UnormSrgb` on WebGPU. |

The dma-buf work is therefore **irrelevant** to the browser (§8): it exists to hand a
texture across a device/process boundary that the page cannot see.

---

## 2. Which crate hosts the example (Q1)

### Decision table

| Candidate | What it is | Direct dependency weight | wasm status (measured or read) | Verdict |
| --- | --- | --- | --- | --- |
| **new `crates/ferrocad_web`** | `cdylib` + `wasm-bindgen`; owns the canvas, the rAF loop and the WASI stubs | 6 internal crates + `wgpu` + `wasm-bindgen` + `web-sys` | New code; its internal deps all pass `cargo check --target wasm32-unknown-unknown` (the OCCT-backed ones build in the cross image) | **Recommended** |
| `ferrocad_wgpu` | Renderer library; surface-free by design (`trace.rs:275`, no `wgpu::Surface` anywhere in the crate) | graphics, inventor, types, stage, `wgpu`, `image` | `cargo check -p ferrocad_wgpu --target wasm32-unknown-unknown` **passes** (50.14 s, no warnings); also passes with `wgpu`'s `webgl` feature (44.09 s) | Keep as the **renderer**. Do **not** turn it into a `cdylib`: native hosts (`ferrocad_gpui`, `ferrocad_viewport`) link it as a library, and a `cdylib` is not linkable as a dependency. |
| `ferrocad_viewport` | Interaction/chrome layer (`InventorViewport`, navicube, presenter seam); depends on `bite-gpui` and `image` | whole GPUI stack | `cargo check -p ferrocad_viewport --target wasm32-unknown-unknown` **fails**: `wasm_thread 0.3.3` uses `#![feature(stdarch_wasm_atomic_wait)]` → `error[E0554]` on stable, because `bite-gpui`'s `platform` feature pulls `gpui_web` with its default `multithreaded` feature | No. A viewport is not needed to prove the kernel→canvas path, and it drags GPUI/stable-nightly friction in. |
| `ferrocad_graphics` | Pure data: `Camera`, `Projection`, `Viewport`, `RenderQueue`, `DrawPacket` | `ferrocad_types`, `ferrocad_theme` | compiles for wasm (built as a dependency of the `ferrocad_wgpu` check) | Dependency only; cannot host a binary. |
| `ferrocad_stage` | Pure data: `Stage`, `ShapeItem`, `DrawMode` | graphics, geom, types | compiles for wasm (same check) | Dependency only; this is the bridge from `Tessellation` to draw packets (`crates/ferrocad_stage/src/scene/shape.rs:360`, `scene/stage.rs:70-81`). |
| `ferrocad_toy` / `ferrocad` | Native edition shell: `ferrocad_gpui` + `ferrocad` + PyO3 | CPython, GPUI, widgets | fails transitively at `pyo3-ffi` (see below) | No. |
| `ferrocad_gpui` | Full shell: widgets, dialogs, Python facade, `gpui_interop` | PyO3 `auto-initialize`, `ferrocad_inventor_py`, `ferrocad_stage_py`, `gpui_interop`, `gpui_wgpu` | `cargo check -p ferrocad_gpui --target wasm32-unknown-unknown` **fails** in `pyo3-ffi 0.25.1`: `cannot find type intptr_t/ssize_t/size_t in crate ::libc` (12 errors) plus "The output binary will link to libpython" | No. The shell is a native app; it is not a browser host even though GPUI has a web backend. |
| `ferrocad_occt_wasm` | `cdylib` smoke test (`occt_init`, `filleted_box_triangles`) | `ferrocad_geom`, `ferrocad_occt` | links and runs (this is the existing proof) | **A `cdylib` cannot be a dependency** (its `[lib] crate-type = ["cdylib"]`, `Cargo.toml:16`). Depend on `ferrocad_occt` instead and call the kernel API directly. |

### Recommended `ferrocad_web` dependency set

```toml
[package]
name = "ferrocad_web"
version.workspace = true
edition.workspace = true
license.workspace = true
publish = false

[lib]
crate-type = ["cdylib"]

[dependencies]
# Geometry and the kernel.
ferrocad_geom.workspace = true
ferrocad_occt.workspace = true
# Draw contract and the declarative producer.
ferrocad_graphics.workspace = true
ferrocad_types.workspace = true
ferrocad_stage.workspace = true
# The renderer (surface-free; it draws into a TextureView).
ferrocad_wgpu.workspace = true
# WebGPU in the browser; `webgl` enables the GL/WebGL2 fallback.
wgpu = { version = "29", features = ["webgl"] }
wasm-bindgen = "0.2"
wasm-bindgen-futures = "0.4"
js-sys = "0.3"                 # clock_time_get in the WASI shim
web-sys = { version = "0.3", features = ["Document", "Element", "HtmlCanvasElement", "Window", "console"] }
console_error_panic_hook = "0.1"
```

Notes:

- The workspace's `wgpu = "29"` already enables `webgpu` and `wgsl` by default
  (`wgpu-29.0.4/Cargo.toml:52-61`); `webgl` is the only feature that has to be added.
  `webgl` also pulls `wgpu-hal`/`glow` (`Cargo.toml:114-119`).
- `wasm-bindgen`, `js-sys`, `web-sys` and `wasm-bindgen-futures` are already in
  `Cargo.lock` (0.2.129 / 0.3.106 / 0.4.79), because `wgpu`'s `web`/`webgpu` features
  pull them. Adding the direct dependencies does not add a download.
- Add the crate to `members` but not `default-members` (it links the kernel and, on
  wasm, needs the cross image). This mirrors `ferrocad_occt_wasm`.
- Do not put the WASI stubs in `ferrocad_occt`: a native build must not define
  `__imported_wasi_snapshot_preview1_*` symbols. Keep them in a `#[cfg(target_arch =
  "wasm32")] mod wasi;` inside `ferrocad_web` (§5).

---

## 3. Is `ferrocad_wgpu` wasm-compilable? (Q2)

### Measurement

```
CARGO_TARGET_DIR=/tmp/wasmcheck cargo check -p ferrocad_wgpu \
  --target wasm32-unknown-unknown --offline
```

Result: **exit 0, no failures, no warnings from `ferrocad_wgpu`**, finished in
50.14 s. The compiles include `wgpu 29.0.4`, `wgpu-core`, `image 0.25.10` (with its
default AVIF/WebP/TIFF/EXR codecs), `wasm-bindgen 0.2.129`, `web-sys 0.3.106` and
`wasm-bindgen-futures 0.4.79`.

A second scratch check added `wgpu = { version = "29", features = ["webgl"] }` and
also passed (44.09 s), compiling `wgpu-hal`, `glow 0.17` and
`wgpu-core-deps-wasm 29.0.4`. So the answer to "does it compile?" is **yes, both
backends**; there is no `#[cfg]` work to do in the crate.

### Features and dependencies

| Need | Where it comes from |
| --- | --- |
| `webgpu` backend | `wgpu` default feature (`wgpu-29.0.4/Cargo.toml:52-61`); `SurfaceTarget::Canvas(web_sys::HtmlCanvasElement)` is available only with `web` (`src/api/surface.rs:290-300`). |
| `webgl` fallback | opt-in feature; adds `wgpu-hal` + `glow` (`Cargo.toml:114-119`). Without it `Backends::GL` has no wasm implementation. |
| WGSL | `wgsl` default feature; `naga` stays out of the wasm binary for the WebGPU path because shader modules are passed through as WGSL. |
| Canvas type | the host needs `web-sys` (`HtmlCanvasElement`); `wgpu` only names it behind `web`. |
| Async init | `wasm-bindgen-futures` (already a `wgpu` dependency) for `request_adapter`/`request_device`. |

The exact minimums declared by wgpu 29 are `wasm-bindgen ^0.2.108`, `js-sys`/
`web-sys ^0.3.85`, `wasm-bindgen-futures ^0.4.58` (`Cargo.toml:208-229`). The lock
resolves higher because web-sys 0.3.106 **pins** `wasm-bindgen =0.2.129`
(`web-sys-0.3.106/Cargo.toml:2826-2828`) and `js-sys =0.3.106`. That exact pin is
the toolchain trap in §6.

### Native-only code inventory

Compilation is clean; these are the *runtime* issues a browser host must route
around, or the places where wasm is fine by accident:

| Item | Location | Wasm behaviour |
| --- | --- | --- |
| `std::thread::yield_now` in `block_on` | `renderer.rs:1358-1370` (loop at `:1367`) | On `wasm32-unknown-unknown` `yield_now` resolves to the "unsupported" impl whose body is `// do nothing` (`library/std/src/sys/thread/unsupported.rs:36-38`, <https://github.com/rust-lang/rust/blob/1.95.0/library/std/src/sys/thread/unsupported.rs#L36-L38>). `block_on` becomes a tight loop that **never returns to the JS event loop**, so a JS promise can never settle: the tab hangs. |
| `block_on` around adapter/device | `adapter.rs:102, 115, 123, 130`; `renderer.rs:247` (`request_device`) | `new_default`/`new_with_choice`/`adapters()` are unusable in a browser. Construct `WgpuRenderer::new(GpuContext::new(device, queue), size)` instead — `new`/`with_sample_count` are synchronous (`renderer.rs:167-224`). |
| `image()` readback | `renderer.rs:1081-1162` | `Device::poll` is a no-op on web (`backend/webgpu.rs:2560-2563`), so `get_mapped_range()` panics before the map completes. Never call `image()` from the browser path. |
| `std::time::Instant` | `trace.rs:31, 48-56` | `Instant::now()` panics on `wasm32-unknown-unknown` ("time not implemented on this platform"). It is only reached when tracing is enabled. |
| `std::env::var_os("FERROCAD_TRACE")` | `trace.rs:59-69` | Returns `None` on wasm, so tracing stays off. (Do not force `--trace` in the browser.) |
| `image` (PNG/AVIF/… encoders) | `snapshot.rs:27-28, 476-493`; `Cargo.toml:34` | Compiles. `encode_png` is unreachable from the example, so `wasm-ld --gc-sections` drops it; the **compile time** is still paid. A `snapshot` cargo feature (or `image` with `default-features = false, features = ["png"]`) is the clean fix; neither is required for correctness. |
| `std::fs` | `lib.rs:87-94` | `#[cfg(test)]` only; not compiled. |
| `wgpu::Surface` | none in the crate | The renderer is surface-free by design (`trace.rs:275`). The browser host owns the only surface. |
| threads/atomics | none in the renderer | No `wasm-bindgen-rayon`/shared-memory requirement. `OnceLock` + atomics (`trace.rs:29-30`) are fine in non-atomics wasm. |

One more wasm-specific compile comment: building a wasm artifact from the host with
this repository's cargo fails at link time, because `.cargo/config.toml:10-13` adds
`-C link-arg=-Wl,-rpath,…` to every build and `wasm-ld` rejects the ELF-style option.
The cross image sets `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS`, which takes
precedence over `build.rustflags`, so the container build is unaffected. A host-side
build would need its own `RUSTFLAGS`/target rustflags — one more reason the repro
runs inside the image.

---

## 4. Rendering into the canvas (Q3)

### It already accepts a caller-provided view

`WgpuRenderer` exposes exactly the seam a browser needs:

- `render(&mut self, target: &wgpu::TextureView, camera: &Camera, batches: &[B])`
  (`renderer.rs:311-318`), and `render_with_workplane` (`:326-332`).
- `render_views` / `encode_views` take the same `&wgpu::TextureView`
  (`renderer.rs:580-601`); `encode_views` returns the `CommandBuffer` for a host
  that submits itself.
- With MSAA on, the pass draws into the multisampled target and uses the caller's
  view as `resolve_target` (`renderer.rs:390-397, 678-681`); with MSAA off it draws
  into the caller's view directly.

So **no readback is required to present**. The internal `targets.color` texture is
still allocated; in direct-to-canvas mode it is only used by `image()` (which is
stale/blank, and panics on web anyway). A host must call `resize([canvas.width,
canvas.height])` so the MSAA/depth attachments match the canvas; on the web backend
`Surface::configure` sets `canvas.width/height` from `config.width/height`
(`wgpu-29.0.4/src/backend/webgpu.rs:3878-3886`), so configure first, then resize the
renderer to the same size.

### The one hard blocker: the colour format

`TARGET_FORMAT` is a private const, `wgpu::TextureFormat::Rgba8UnormSrgb`
(`renderer.rs:87`). It is used for the offscreen colour target (`renderer.rs:1298`),
the MSAA target (`:1325`), and both pipeline constructors (`renderer.rs:194, 200`).
The pipelines are built with that format (`pipeline.rs:146-153`,
`program.rs:411-417`), and a render pipeline's colour-target format must equal the
attachment view's format.

WebGPU forbids that format on a canvas. The spec's *supported context formats* are
exactly « `bgra8unorm`, `rgba8unorm`, `rgba16float` » and it says explicitly: "Canvas
configuration cannot use `srgb` formats like `bgra8unorm-srgb`. Instead, use the
non-srgb equivalent, specify the `srgb` format in the `viewFormats`, and use
`createView()` to create a view with an `srgb` format."
(`https://www.w3.org/TR/webgpu/#canvas-configuration`). wgpu 29's web backend
likewise reports only `Rgba8Unorm`, `Bgra8Unorm`, `Rgba16Float`, with the preferred
format moved to the front — no sRGB (`backend/webgpu.rs:3846-3873`).

On WebGL, wgpu's GLES surface capabilities **do** include `Rgba8UnormSrgb` when the
context supports sRGB (`wgpu-hal-29.0.4/src/gles/adapter.rs:1241-1275`), but they
list `Rgba8Unorm` first, so `get_default_config` would still pick the wrong format.

Two ways out:

**Option A — zero renderer changes (spike-quality).**

- WebGPU: configure the canvas as `Rgba8Unorm` with `view_formats:
  [Rgba8UnormSrgb]`, then create the surface-texture view with
  `format: Some(Rgba8UnormSrgb)`. wgpu 29's web backend forwards the view format to
  JS without local validation (`backend/webgpu.rs:2841-2874`) and forwards
  `viewFormats` on configure (`:3901-3913`); WebGPU's texture-view-format rule
  allows the sRGB/non-sRGB pair (`#texture-view-format-compatible`). The MSAA
  resolve target then also has format `Rgba8UnormSrgb`, matching the MSAA texture.
- WebGL: configure with `Rgba8UnormSrgb` directly (no view-format trick; the GLES
  default framebuffer's view is special-cased, so do not rely on reinterpreting it).

This works, but it depends on surface-view details that are easy to break when wgpu
is bumped.

**Option B — the smallest change, and the one to implement.** Replace the const with
a field and a constructor:

```rust
// renderer.rs — the only change the browser host needs
pub fn new_with_format(
    context: GpuContext,
    size: [u32; 2],
    sample_count: u32,
    color_format: wgpu::TextureFormat,
) -> Self { /* Targets::new(..., color_format); pipelines with color_format */ }
```

Concretely: `Targets::new(device, size, sample_count, color_format)`,
`create_color`/`create_msaa_color` take the format, and the two `*_new` calls pass
it instead of `TARGET_FORMAT` (`renderer.rs:176, 178, 194, 200`). Keep
`TARGET_FORMAT` as the default for `new`/`with_sample_count`, so native behaviour is
byte-identical. That is ~20 lines and no public API break. (If a host wants no
offscreen colour texture at all in direct-to-canvas mode, `Targets` would need an
`Option<color>` mode; that is an optimisation, not a requirement.)

`pick_at` (`renderer.rs:800+`) renders its own G-buffer with its own formats, so it
is unaffected; picking is out of scope for the example.

---

## 5. The WASI shim (Q4)

### The 13 imports

Measured on the rebuilt smoke artifact
(`target/wasm-smoke/wasm32-unknown-unknown/debug/ferrocad_occt_wasm.wasm`,
39,299,928 B) with `WebAssembly.Module.imports`:

```
wasi_snapshot_preview1.environ_get          wasi_snapshot_preview1.fd_prestat_dir_name
wasi_snapshot_preview1.environ_sizes_get    wasi_snapshot_preview1.fd_read
wasi_snapshot_preview1.clock_time_get       wasi_snapshot_preview1.fd_seek
wasi_snapshot_preview1.fd_close             wasi_snapshot_preview1.fd_write
wasi_snapshot_preview1.fd_fdstat_get        wasi_snapshot_preview1.path_open
wasi_snapshot_preview1.fd_fdstat_set_flags  wasi_snapshot_preview1.proc_exit
wasi_snapshot_preview1.fd_prestat_get
```

They come from `libcadrum_c.a`, the merged wasi-libc archive built for
`wasm32-wasip1`: its undefined symbols are the linker's
`__imported_wasi_snapshot_preview1_*` names, and `--gc-sections` keeps only the 13
that the linked code reaches. The tarball is built with wasi-sdk 33,
`--target=wasm32-wasip1`, `-fwasm-exceptions`
(`docs/project/research/occt-8-upgrade.md:115-128`;
`ferrocad_occt_cxx/crates/opencascade-sys/build.rs:259-270`).

### Are they called at all?

No, for this path. I instantiated the module with a hand-written import object that
counts calls (and implements `fd_write` to stderr, `clock_time_get` from
`process.hrtime`, `environ_*` as empty, and `EBADF`/`ENOENT` for the rest), then ran
`occt_init()` + `filleted_box_triangles()`:

```
filleted_box_triangles = 40
import calls: [Object: null prototype] {}
```

So the box → fillet → tessellate path executes **zero** WASI calls. This is an
observation about this path only, not a licence to stub everything blindly (§10).

### The better shim: define the import symbols in Rust

Because the unwanted symbols are ordinary undefined symbols with the linker's
`__imported_…` spelling, defining them in the same module makes the linker resolve
them and drop the imports. I built a scratch `cdylib` with those 13 functions
(`#[unsafe(export_name = "__imported_wasi_snapshot_preview1_fd_write")]`, …) plus the
same box/fillet/tessellate code:

```
$ node -e '… new WebAssembly.Module(bytes) …'
bytes 14805959 imports []
$ node -e 'WebAssembly.instantiate(bytes, {}).then(… occt_init(); filleted_box_triangles())'
triangles 40
```

The final module has **no imports at all** and still runs with an empty import
object. After `wasm-opt -Oz --strip-debug`: 10,387,989 B, still zero imports,
still 40. Adding `#[wasm_bindgen] pub fn triangles()` and running wasm-bindgen
0.2.129 produced a 13,468,796 B `_bg.wasm` whose only imports are wasm-bindgen's own
`./…_bg.js` functions; `initSync({ module })` + `triangles()` in Node returns 40.
That is the whole chain except the GPU.

Recommendation: **ship the 13-symbol shim, not a JS WASI package.** The alternatives
are all worse for this shape:

- `@wasmer/wasi`, `@wasm-tool/wasi`, `@jsquash/wasi`: implement a real WASI
  filesystem/clock and require injecting an import object. wasm-bindgen's
  `--target web` glue builds its import object itself (`__wbg_get_imports()` in the
  generated JS) and exposes no hook for an extra module, so using one means
  hand-patching or wrapping the glue.
- `wasi-preview1-component-adapter`: that is for the component model / preview1
  adapters, not for a core module that already imports
  `wasi_snapshot_preview1` and is being fed to wasm-bindgen.
- Trying to build OCCT without WASI: not practical. wasi-sdk's sysroot *is*
  wasi-libc/libc++ built against `wasm32-wasip1`; removing the imports means
  rebuilding the whole kernel against a different libc (custom `-nostdlib` libc, or
  Emscripten), which is a kernel-build project and would invalidate the prebuilt
  tarball. The link-time stubs achieve the same observable result (no WASI imports
  in the shipped module) at the cost of stubbing the behaviour.

Caveats for the shim:

- Return `EBADF`/`ENOENT` for file APIs, empty environment, zero for `fd_seek`.
  `clock_time_get` can read `performance.now()` through `js-sys` (that adds a
  wasm-bindgen import, which the glue already supplies) or return 0.
- `proc_exit` must not silently return: libc calls it from `abort`/`_Exit`. Make it
  trap (`core::arch::wasm32::unreachable()` or a panic) so a kernel abort is
  visible.
- The stub set is **per linked subset**. The 13 names are what this cdylib's
  `--gc-sections` kept; a different OCCT entry point can pull in more libc wrappers
  (`fd_filestat_get`, `path_filestat_get`, …). CI should inspect
  `WebAssembly.Module.imports()` on the final artifact and fail on any
  `wasi_snapshot_preview1.*` that the shim does not define.
- `__imported_wasi_snapshot_preview1_*` is a linker spelling, not a stable API.
  The cross image digest is already pinned (`.github/workflows/ci.yml:311`), which is
  what makes this reproducible.

---

## 6. Toolchain for a browser build (Q5)

### Versions

| Component | Required | Why |
| --- | --- | --- |
| `wgpu` | 29.0.4 (workspace) | the renderer's version |
| `wasm-bindgen` crate | **exactly 0.2.129** | `web-sys 0.3.106` declares `wasm-bindgen = "=0.2.129"` (`web-sys-0.3.106/Cargo.toml:2826-2828`); `js-sys` pins the same pair |
| `wasm-bindgen` CLI | **exactly 0.2.129** | the CLI's bindgen schema must match the crate's; mismatch is a hard error |
| `wasm-bindgen-futures` | 0.4.79 in the lock | async adapter/device |
| `web-sys` / `js-sys` | 0.3.106 in the lock | canvas/`Document`/`Window` |

Measured mismatch, using the 0.2.127 CLI already on the host against a module built
with `wasm-bindgen = "=0.2.129"`:

```
rust Wasm file schema version: 0.2.129
   this binary schema version: 0.2.127
Currently the bindgen format is unstable enough that these two schema versions
must exactly match.
```

The 0.2.129 release notes also mention this error now names both files
(`https://github.com/wasm-bindgen/wasm-bindgen/releases/tag/0.2.129`).

### Installing the CLI

The cross image has **none** of `wasm-bindgen`, `wasm-pack`, `wasm-opt`,
`wasm-objdump`; it has `llvm-objdump` from wasi-sdk (`/opt/wasi-sdk/bin`), `rustc`
1.97.0 and clang 22 (`docker run … sh -c 'command -v …'`). So the bindgen step runs
**after** the container build, on the host/runner, against the mounted artifact —
the linked wasm is ordinary portable wasm.

Preferred (fast, no toolchain build):

```
curl -sSLO https://github.com/wasm-bindgen/wasm-bindgen/releases/download/0.2.129/wasm-bindgen-0.2.129-x86_64-unknown-linux-musl.tar.gz
# verify against the published .sha256sum, then:
tar xzf wasm-bindgen-0.2.129-x86_64-unknown-linux-musl.tar.gz
./wasm-bindgen-0.2.129-x86_64-unknown-linux-musl/wasm-bindgen --version   # 0.2.129
```

(The asset exists; the release also publishes
`…-x86_64-unknown-linux-musl.tar.gz.sha256sum`.) Fallback:
`cargo install wasm-bindgen-cli --version 0.2.129 --locked` (MSRV 1.81 per the
release notes, so the 1.85+ workspace toolchain is fine).

**Do not hard-code 0.2.129 in CI without a check.** A `cargo update`, or a wgpu/web-sys
bump, moves web-sys and wasm-bindgen together. Derive the version from the lock and
assert the CLI matches, e.g.:

```
cargo tree -p wasm-bindgen --depth 0 | head -1   # wasm-bindgen v0.2.129
wasm-bindgen --version                            # wasm-bindgen 0.2.129
```

`wasm-opt` is optional. If used, it must be installed separately (binaryen 120 on
this host handled the legacy-EH OCCT module, and `-Oz --strip-debug` preserved
executability — measured with both the raw and the bindgen-processed module).

### Build sequence

1. In the pinned cross image (network + `OCCT_WASM_ROOT`), build the `cdylib` for
   `wasm32-unknown-unknown` with a persistent `CARGO_TARGET_DIR` outside the image
   (the recipe is in §Appendix A).
2. On the runner, run `wasm-bindgen --target web --out-dir pkg <artifact>.wasm`.
3. Optionally `wasm-opt -Oz --strip-debug` the `pkg/*_bg.wasm`.
4. Serve `pkg/` + `index.html` as static files.

`--target web` produces an ES module plus the wasm; no bundler is needed (the
`bundler` target would require one). Do not run `wasm-bindgen` inside the image:
the CLI is not there, and the artifact does not need the image's toolchain.

Version traps beyond the CLI: `wasm-bindgen` 0.2.129's walrus **does** accept the
legacy wasm-EH `try`/`tag` sections that OCCT uses — verified by generating the
glue and running it. That was the biggest unknown and it passed.

---

## 7. Minimal example design (Q6)

### Layout

```
crates/ferrocad_web/
  Cargo.toml
  src/
    lib.rs        # start(), App, frame loop
    # the 13 link-time stubs live in `crates/ferrocad_wasm`, shared with the GPUI
    # probe and retained by `ferrocad_wasm::install()`
  www/
    index.html    # the whole page
    pkg/          # wasm-bindgen output (generated, not committed)
```

`crates/ferrocad_web/www/index.html` (≈12 lines):

```html
<!doctype html>
<meta charset="utf-8">
<title>FerroCAD wasm kernel</title>
<style>
  html, body { margin: 0; height: 100%; background: #0f1218 }
  canvas { display: block; width: 100%; height: 100% }
</style>
<canvas id="cad" width="960" height="600"></canvas>
<script type="module">
  import init from "./pkg/ferrocad_web.js";
  init().catch((error) => {
    document.body.textContent = String(error);
    console.error(error);
  });
</script>
```

`#[wasm_bindgen(start)]` runs during `init()`, finds `#cad`, starts the async init
and schedules the first frame. Nothing else is needed in JS; the canvas is sized by
`Surface::configure` on the web backend (set `width`/`height` in the config; do not
fight the CSS size).

### `src/lib.rs` sketch

```rust
use std::{cell::RefCell, rc::Rc};

use ferrocad_graphics::{Camera, Projection, Viewport};
use ferrocad_geom::{ElementKind, GeometryBackend, TessellationOptions};
use ferrocad_occt::OcctBackend;
use ferrocad_stage::{DrawMode, ShapeItem, Stage};
use ferrocad_types::{Placement, Rotation, Vector3};
use ferrocad_wgpu::{GpuContext, WgpuRenderer};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

#[cfg(target_arch = "wasm32")]
mod wasi;

unsafe extern "C" {
    fn __wasm_call_ctors();
}

#[wasm_bindgen(start)]
pub fn main() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    let canvas = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("cad"))
        .ok_or_else(|| JsValue::from_str("no #cad canvas"))?
        .dyn_into::<web_sys::HtmlCanvasElement>()?;
    wasm_bindgen_futures::spawn_local(async move {
        match App::new(canvas).await {
            Ok(app) => request_frame(Rc::new(RefCell::new(app))),
            Err(error) => web_sys::console::error_1(&error),
        }
    });
    Ok(())
}

struct App {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    view_format: wgpu::TextureFormat,
    renderer: WgpuRenderer,
    camera: Camera,
    queue: ferrocad_graphics::RenderQueue,
}

impl App {
    async fn new(canvas: web_sys::HtmlCanvasElement) -> Result<Self, JsValue> {
        // OCCT's C++ constructors must run before any kernel call.
        unsafe { __wasm_call_ctors() };

        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL;
        let instance = wgpu::util::new_instance_with_webgpu_detection(desc).await;

        let surface = instance.create_surface(canvas.clone())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("ferrocad web"),
                ..Default::default()
            })
            .await
            .map_err(|error| JsValue::from_str(&error.to_string()))?;

        let caps = surface.get_capabilities(&adapter);
        // WebGPU forbids sRGB canvas formats; WebGL lists the sRGB variant.
        let srgb = wgpu::TextureFormat::Rgba8UnormSrgb;
        let (format, view_formats) = if caps.formats.contains(&srgb) {
            (srgb, Vec::new())
        } else {
            (wgpu::TextureFormat::Rgba8Unorm, vec![srgb])
        };
        let size = [canvas.width().max(1), canvas.height().max(1)];
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size[0],
            height: size[1],
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats,
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // Synchronous constructor: the device was obtained above, async.
        let renderer = WgpuRenderer::new(GpuContext::new(device, queue), size);
        let camera = Camera {
            view: Placement::new(
                Vector3::new(0.0, 0.0, -4.0),
                Rotation::from_axis_angle(&Vector3::new(1.0, 1.0, 0.0), 0.6),
            )
            .to_matrix(),
            projection: Projection::Perspective {
                fov_y: 45.0_f64.to_radians(),
                near: 0.1,
                far: 100.0,
            },
            viewport: Viewport::new(size, 1.0),
        };
        // One-shot geometry: the renderer uploads it on the first frame and caches
        // it by (mesh_id, generation) for every later frame.
        let queue = ferrocad_stage::Stage::new()
            .with_item(
                ShapeItem::from_tessellation(1, &filleted_box())
                    .with_draw_mode(DrawMode::ShadedWithEdges)
                    .with_surface_color([0.62, 0.66, 0.72, 1.0]),
            )
            .to_queue();
        Ok(Self { surface, config, view_format: srgb, renderer, camera, queue })
    }

    fn frame(&mut self) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            _ => return,
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.view_format),
            ..Default::default()
        });
        if self.renderer.render(&view, &self.camera, &self.queue.packets).is_ok() {
            frame.present();
        }
    }

    /// Call from a `ResizeObserver`; the renderer's attachments and the camera's
    /// aspect must match the canvas.
    fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.renderer.context().device, &self.config);
        self.renderer.resize([self.config.width, self.config.height]);
        self.camera.viewport = Viewport::new([self.config.width, self.config.height], 1.0);
    }
}

fn request_frame(app: Rc<RefCell<App>>) {
    let next = app.clone();
    let callback = Closure::once_into_js(move || {
        app.borrow_mut().frame();
        request_frame(next);
    });
    web_sys::window()
        .expect("window")
        .request_animation_frame(callback.unchecked_ref())
        .expect("requestAnimationFrame");
}

fn filleted_box() -> ferrocad_geom::Tessellation {
    let backend = OcctBackend::new();
    let shape = backend.make_box(10.0, 10.0, 10.0).expect("box");
    let edges = backend.elements(&shape, ElementKind::Edge);
    let filleted = backend
        .fillet(&shape, &edges[..1], 1.0)
        .expect("fillet");
    backend
        .tessellate(&filleted.shape, &TessellationOptions::absolute(0.2))
        .expect("tessellate")
}
```

Notes on the sketch:

- `WgpuRenderer::render` takes `&[B]`; `&self.queue.packets` (`Vec<DrawPacket>`)
  coerces to a slice. `ferrocad_graphics::RenderQueue`'s `packets` is public
  (`batch.rs:100-103`).
- `CurrentSurfaceTexture` is wgpu 29's API; `SurfaceTexture::present()` must be
  called after the submit (`api/surface.rs:120-175`, `api/surface_texture.rs:5-30`).
  Handle `Outdated`/`Lost` by reconfiguring/recreating the surface; the sketch
  simply skips the frame.
- Use `wgpu::util::new_instance_with_webgpu_detection`, not bare
  `Instance::new`: WebGPU support must be decided at instance creation and it can
  only be probed asynchronously (`wgpu-29.0.4/src/util/init.rs:98-126`).
- `BROWSER_WEBGPU | GL` is the intended combination; the helper removes
  `BROWSER_WEBGPU` when `navigator.gpu` exists but no adapter can be created.
- With Option A (§4) the format selection above is what lets the unchanged
  `Rgba8UnormSrgb` pipelines work. With Option B, keep the canvas configuration
  (`Rgba8Unorm` + `view_formats: [Rgba8UnormSrgb]` on WebGPU, `Rgba8UnormSrgb` on
  WebGL), create the view as `Rgba8UnormSrgb`, and pass `Rgba8UnormSrgb` as the
  renderer's target format.
- The 13-symbol shim from §5 is `crates/ferrocad_wasm` (shared with the GPUI
  probe); `ferrocad_wasm::install()` is what keeps it in the link.

### What the example proves, and what it does not

Proves: OCCT 8.0.1 initialises in a browser wasm module; the Rust `ferrocad_occt`
API builds a fillet and tessellates it; `ferrocad_stage` turns the tessellation into
draw packets; `ferrocad_wgpu` encodes and submits them; the frame goes into the
canvas texture with no CPU readback; and the OCCT module needs no JS WASI package.

Deliberately does not prove: picking (`pick_at`), text/labels, the navigation cube,
the GPUI shell, document/undo, import/export beyond the linked subset (the wasm
tarball has no IGES, STL or OCAF toolkits; STEP is present), performance under a
real GPU, or the WebGL path.

---

## 8. What "zero copy" means here (Q7)

### WebGPU path

1. `Instance::create_surface(canvas)` wraps the canvas's `GPUCanvasContext`.
2. `surface.configure(device, config)` calls the JS `GPUCanvasContext.configure`
   with the format, usage, alpha mode and `viewFormats`
   (`wgpu-29.0.4/src/backend/webgpu.rs:3901-3913`).
3. Per frame, `surface.get_current_texture()` calls `getCurrentTexture()` and wraps
   the returned `GPUTexture`; that texture is a writable view of the canvas
   `[[drawingBuffer]]` (`https://www.w3.org/TR/webgpu/#dom-gpucanvascontext-getcurrenttexture`).
4. `WgpuRenderer::render(view, …)` encodes the scene pass straight into that view
   (or resolves MSAA into it) and submits.
5. `frame.present()` is a no-op on the web backend: "Swapchain is presented
   automatically on the web" (`backend/webgpu.rs:3946-3949`). The browser presents
   the drawing buffer when it updates the canvas rendering
   (`https://www.w3.org/TR/webgpu/#updating-the-rendering-of-a-webgpu-canvas`).

No application-visible GPU→CPU copy exists on this path.

### WebGL path

`surface.get_current_texture()` returns the canvas's default framebuffer (the
drawing buffer); `render` binds it as the render target (with a multisampled
renderbuffer + resolve when MSAA is on). WebGL "presents its drawing buffer to the
HTML page compositor immediately before a compositing operation" and requires the
implementation to flush rendering first
(`https://registry.khronos.org/webgl/specs/latest/1.0/#2.2`). No readback.
`preserveDrawingBuffer: false` means the buffer is cleared after presentation, so a
host must not try to read it later.

### What is still copied

| Copy | When | Where |
| --- | --- | --- |
| Vertex + index data: wasm linear memory → GPU buffer | On a mesh cache miss only (`renderer.rs:346-358` uploads fresh geometry; `batch.rs` creates the buffers). The renderer keys meshes by `(view, mesh_id)` and replaces on a newer `Generation`, so an orbit does not re-upload. | `wgpu::Queue::write_buffer` / `create_buffer_init`; on the web this is the JS `writeBuffer`/mapped-at-creation path. The WebGPU spec calls out that this "may first need to cross the process boundary… sometimes requires a copy into driver-allocated staging memory" (`https://www.w3.org/TR/webgpu/#memory-model`). |
| Uniform data (per-view matrices + per-packet state) | Every frame, one small dynamic-uniform write (`renderer.rs:362-375`) | `Queue::write_buffer`, a few KiB per frame. |
| MSAA resolve | Every frame, when `sample_count > 1` | GPU-side resolve into the canvas texture (`renderer.rs:394-397`); not a CPU copy. |
| Compositor present | Every frame | Browser/OS; not observable or controllable. |

Contrast with the `image()` path: `copy_texture_to_buffer` + `map_async` copies the
whole frame GPU→CPU, then `ferrocad_viewport`/gpui uploads it back CPU→GPU as a
`RenderImage`. That is the path the browser must not use (and cannot use: `poll` is
a no-op, `get_mapped_range` panics).

The dma-buf interop is irrelevant because it solves a different problem: exporting
a texture to another device/process on Linux so a *second* compositor can sample it
without a copy (`crates/ferrocad_gpui/src/runtime/interop.rs:3-17`). A canvas has no
such boundary; the page cannot export GPU memory and does not need to.

---

## 9. Size and performance (Q8)

### Measured

| Artifact | Bytes | MiB | Notes |
| --- | ---: | ---: | --- |
| debug `cdylib` (`ferrocad_occt_wasm`, workspace profile) | 39,299,928 | 37.48 | ~21 MB of `.debug_*` DWARF (`.debug_str` 12.1 MB, `.debug_info` 8.2 MB) |
| release `cdylib` (`ferrocad_occt_wasm`, workspace `lto = true`) | 14,791,735 | 14.11 | 3m50s in the container |
| release + `wasm-opt -Oz --strip-debug` | 10,402,652 | 9.92 | −29.7 %; still returns 40 |
| shim `cdylib` (13 WASI stubs, scratch default release, no LTO) | 14,805,959 | 14.12 | zero imports; returns 40 |
| shim + `wasm-opt -Oz` | 10,387,989 | 9.91 | zero imports; returns 40 |
| shim + wasm-bindgen 0.2.129 (`--target web`) | 13,468,796 | 12.85 | `_bg.wasm`; only `wbg` imports |
| shim + bindgen + `wasm-opt -Oz` | 10,402,437 | 9.92 | `initSync` + `triangles()` = 40 |

Build times (container, `-j 2`): debug 4m00s, release 3m50s, shim release 3m41s.
All from a warm `CARGO_TARGET_DIR`; a cold build pays the 61-translation-unit cxx
bridge again.

### What dominates

Code-section attribution of the release module (11,738,535 B of code, 16,828
functions):

| Symbol class | Bytes | Share | Functions |
| --- | ---: | ---: | ---: |
| Itanium C++ (`_Z…`; OCCT, libc++/libc++abi, cxx bridge) | 11,368,588 | **96.8 %** | 15,870 |
| Rust (`_R…` / legacy `…17h<16 hex>E`) | 39,049 | **0.33 %** | 131 |
| other (C exports, cxxbridge glue, libc-in-C) | 330,898 | 2.8 % | 827 |

Plus 1,637,269 B of data and a 1,342,255 B `name` section (release keeps names;
`wasm-opt --strip-debug` removes them). So the kernel, not the Rust host, sets the
size: the tarball's static archives total 118 MB (19 `TK*` libs plus libc++ and
the cadrum runtime) before `--gc-sections`.

The browser crate will add wgpu's web backend, `web-sys` bindings and the example
(~a few hundred KiB, not measured) plus `image`'s compiled code (mostly GC'd, §3).
Expect **≈10.5 MB** for the final optimized wasm with the example; measure it once
`ferrocad_web` exists.

### Not measured

Transfer size after gzip/brotli (not available offline), browser compile/instantiate
time, GPU frame time, and the cost of `ferrocad_wgpu`'s per-frame uniform write on a
real device. The spec permits a copy for `writeBuffer`; whether a given browser can
avoid it is implementation-defined (`#memory-model`, note after the staging
paragraph).

---

## 10. Risks and what is unverified

1. **The canvas path has never been rendered.** There is no browser or GPU here. The
   format reasoning is from the WebGPU spec and wgpu's caps; the zero-copy claim is
   from the spec's canvas model, not a frame capture. The first browser run must
   check: `getCapabilities().formats`, a validation-error-free `configure`, a
   non-black frame, and no `uncapturederror`.
2. **WebGL is doubly unverified.** `ferrocad_wgpu` requests 4× MSAA and
   `Depth32Float` (`renderer.rs:90`, `pipeline.rs:27-31`). WebGL2 should support both
   for an RGBA8 default framebuffer, but wgpu-hal's GLES resolve path, the sRGB
   canvas attribute, and `Rgba8UnormSrgb` on the default framebuffer were only
   read, not run. If WebGL is fragile, ship WebGPU-only first (drop the `webgl`
   feature and request only `BROWSER_WEBGPU`).
3. **The WASI stubs are path-specific.** Zero calls were observed for
   box+fillet+tessellate, and the 13-symbol shim is exactly the set `--gc-sections`
   left. A new kernel entry point may add imports (or call a stubbed file API and
   get `EBADF`). CI must assert the final module's import list, and the stubs must
   trap on `proc_exit`.
4. **wasm-bindgen's exact version.** The lock pins `wasm-bindgen 0.2.129` through
   web-sys; using the wrong CLI is a hard error (measured). Derive it in CI; do not
   float it.
5. **Host-link rustflags.** `.cargo/config.toml`'s `build.rustflags` breaks wasm
   links outside the cross image (§3). Any host-side wasm build needs its own
   `RUSTFLAGS`/target rustflags; the documented Docker path is unaffected.
6. **`image` in the renderer.** It compiles and is GC'd, but it makes the wasm
   compile pay for codecs the browser example never uses. A `snapshot` feature is
   the clean follow-up.
7. **Not verified at all:** a `ferrocad_web` containing both wgpu *and* OCCT
   (the two halves were built separately); the combined size; wasm-bindgen +
   wgpu glue under a browser; `request_animation_frame` scheduling; device loss and
   canvas resize; and any CI job.

---

## Appendix A — commands whose results are cited

Host (`/home/john/projects/ferrocad/ferrocad`):

```sh
# 1. The renderer compiles for wasm (exit 0, 50.14 s, no warnings).
CARGO_TARGET_DIR=/tmp/wasmcheck cargo check -p ferrocad_wgpu \
  --target wasm32-unknown-unknown --offline

# 2. A scratch crate (target/wgpu-webgl-check) with
#    ferrocad_wgpu = { path = "…/crates/ferrocad_wgpu" }
#    wgpu = { version = "29", features = ["webgl"] }
CARGO_TARGET_DIR=/home/john/projects/ferrocad/ferrocad/target/wasmcheck cargo check \
  --target wasm32-unknown-unknown --offline

# 3. The two shell candidates fail for wasm (stable toolchain).
CARGO_TARGET_DIR=/home/john/projects/ferrocad/ferrocad/target/wasmcheck cargo check -p ferrocad_gpui \
  --target wasm32-unknown-unknown --offline          # pyo3-ffi: intptr_t/ssize_t/size_t missing
CARGO_TARGET_DIR=/home/john/projects/ferrocad/ferrocad/target/wasmcheck cargo check -p ferrocad_viewport \
  --target wasm32-unknown-unknown --offline          # wasm_thread: error[E0554]
```

Container (from `/home/john/projects/ferrocad`), the two builds as run:

```sh
docker run --rm -v "$PWD":/work -w /work/ferrocad \
  -e OCCT_WASM_ROOT=/work/ferrocad_occt_cxx/.occt-wasm/occt-8_0_1_rev3-wasm32_unknown_unknown \
  -e CARGO_TARGET_DIR=/work/ferrocad/target/wasm-smoke \
  -e HTTP_PROXY -e HTTPS_PROXY -e ALL_PROXY \
  ghcr.io/lzpel/cross-wasm32-unknown-unknown \
  cargo build -j 2 -p ferrocad_occt_wasm --target wasm32-unknown-unknown --locked

# same with: cargo build --release -j 2 -p ferrocad_occt_wasm …
```

The link-time shim was a scratch crate at `target/wasi-shim-check` (relative path
dependencies, a `[patch.crates-io]` for `ferrocad_occt_cxx`, `wasm-bindgen =
"=0.2.129"`), built in the same image with `cargo build --release -j 2 --target
wasm32-unknown-unknown`.

Node / binaryen / bindgen measurements (artifacts since deleted):

```sh
# 13 imports; zero calls; 40 triangles.
node target/wasi-probe.mjs target/wasm-smoke/wasm32-unknown-unknown/debug/ferrocad_occt_wasm.wasm

# Optimized module still works.
wasm-opt -Oz --strip-debug -o /tmp/occt_oz.wasm \
  target/wasm-smoke/wasm32-unknown-unknown/release/ferrocad_occt_wasm.wasm
node target/wasi-probe.mjs /tmp/occt_oz.wasm

# The link-time shim: zero imports, still 40.
node -e 'const b=require("fs").readFileSync("…/wasi_shim_check.wasm");
  WebAssembly.instantiate(b,{}).then(({instance})=>{ instance.exports.occt_init();
  console.log(instance.exports.filleted_box_triangles()) })'

# Full browser-shaped chain: bindgen 0.2.129 + initSync under Node → 40.
/tmp/wasm-bindgen-0.2.129-x86_64-unknown-linux-musl/wasm-bindgen \
  --target web --out-dir /tmp/wb-pkg …/wasi_shim_check.wasm
node --input-type=module -e 'import fs from "node:fs";
  import { initSync, triangles } from "/tmp/wb-pkg/wasi_shim_check.js";
  initSync({ module: fs.readFileSync("/tmp/wb-pkg/wasi_shim_check_bg.wasm") });
  console.log(triangles())'
```

The exact image tool inventory:

```sh
docker run --rm ghcr.io/lzpel/cross-wasm32-unknown-unknown sh -c \
  'command -v wasm-opt; command -v wasm-bindgen; command -v wasm-pack; \
   command -v wasm-objdump; command -v llvm-objdump; rustc --version'
# → only /opt/wasi-sdk/bin/llvm-objdump, rustc 1.97.0
```

## Appendix B — the 13 shim symbols

`__imported_wasi_snapshot_preview1_` + `environ_get`, `environ_sizes_get`,
`clock_time_get`, `fd_close`, `fd_fdstat_get`, `fd_fdstat_set_flags`,
`fd_prestat_get`, `fd_prestat_dir_name`, `fd_read`, `fd_seek`, `fd_write`,
`path_open`, `proc_exit`.

Signatures used in the measured build (wasm32: pointers and sizes are `i32`, WASI
`u64` arguments are `i64`): `fd_write/fd_read(fd, iovs, iovs_len, nwritten/nread)`,
`fd_close(fd)`, `fd_seek(fd, offset: i64, whence, newoffset)`,
`fd_fdstat_get(fd, buf)`, `fd_fdstat_set_flags(fd, flags)`,
`fd_prestat_get(fd, buf)`, `fd_prestat_dir_name(fd, path, len)`,
`environ_get(environ, buf)`, `environ_sizes_get(count, size)`,
`clock_time_get(id, precision: i64, time)`,
`path_open(fd, dirflags, path, path_len, oflags, rights_base: i64,
rights_inheriting: i64, fdflags, opened_fd)`, `proc_exit(code)`.

Return `0` from a successful call, `8` (`EBADF`) / `44` (`ENOENT`) from the file
calls. The measured build's `proc_exit` returned; a shipped shim should trap there
(§5).

---

## 11. What landed (2026-11-02)

Implemented after this note:

- `ferrocad_wgpu` gained `with_sample_count_and_format` (Option B, §4): `Targets`
  and both colour attachments take the format, while `new`/`with_sample_count`
  keep `TARGET_FORMAT`, so native rendering is unchanged. The 64 renderer tests
  pass.
- `crates/ferrocad_web` is a `cdylib` with `#[wasm_bindgen(start)]`, a canvas
  surface (`BROWSER_WEBGPU | GL`, so WebGL2 is the required path), one sample
  (MSAA deferred until the WebGL path is browser-tested), `pick_formats` (an sRGB
  view on WebGPU, an sRGB surface on WebGL), and a filleted box from
  `ferrocad_occt`.
- `ferrocad_wasm` defines the 13 stubs from Appendix B; the shipped module has no
  `wasi_snapshot_preview1` imports.
- One extra gotcha: wasm-bindgen 0.2.129's glue reads the `__instance_terminated`
  export. The static is `#[no_mangle]` in `wasm_bindgen::__rt`, but
  `--gc-sections` drops it unless something references it; `main` does a volatile
  read so the export survives.
- `www/smoke.mjs` runs under Node: `initSync({ module })` + `triangles()` returns
  40 and asserts there are no WASI imports. `node www/smoke.mjs`.

Sizes: release `ferrocad_web.wasm` is 20.3 MB, and 17 MB after
`wasm-bindgen --target web` (`_bg.wasm`); the debug build is 181 MB. The CI job
`wasm-web` does the Docker build, installs the lock-derived wasm-bindgen CLI,
generates the glue and runs the smoke test.

Still unverified: an actual browser canvas (there is no browser or GPU here), the
WebGL2 MSAA path, and picking. The canvas draw is the one thing the Node test
cannot cover.

### Follow-up: the first browser run (later on 2026-11-02)

Headless Chrome found three faults the Node test cannot see. WebGL2 was forced by
hiding `navigator.gpu` (so wgpu-core, not the pure-JS WebGPU context, is used):

1. `create_surface()` returned `MissingDisplayHandle`. wgpu-core requires a display
   handle, `InstanceDescriptor::new_without_display_handle()` is the default, and on
   the web the canvas arrives in the *window* handle. The empty `WebDisplayHandle`
   `raw-window-handle` defines for this case satisfies the check; the pure-JS
   context ignores the field.
2. `request_device` then failed with `max_compute_workgroups_per_dimension: 65535
   is better than allowed 0` — a WebGL2 adapter has no compute limits, so a device
   must request `adapter.limits()`, not `Limits::default()`.
3. The framing was wrong: `make_box` builds from the origin corner while the orbit
   camera pivots on the origin, and the eye offset carried a lateral term. The shape
   is now centred with `place()` and the eye offset is `-Z` only.

The surface, device, loop and pointer/wheel navigation moved to
`crates/ferrocad_web_viewport` (the wasm analogue of the GPUI `ferrocad_viewport`),
so `ferrocad_web` is only the demo — scene, camera pose and the smoke-test export.
Orbit and zoom are `Camera::orbit`/`Camera::dolly` in `ferrocad_graphics`, which
`ferrocad_viewport::navicube` now delegates to, so the GPUI and browser viewports
share one implementation instead of drifting.

Verified by screenshot on both paths: the box renders, a synthesized pointer drag
orbits it, and a wheel event dollies the eye.

### The Part layer in the browser (same day)

The demo now drives a real document rather than calling the kernel directly:
`Part::Box` + `Part::Fillet` under `Document::recompute`, with a radius slider on
the page. `ferrocad_part` and `ferrocad_core` are plain Rust and check clean for
`wasm32-unknown-unknown` (8 s), so the whole workbench layer fits in the browser.
Two things surfaced:

- **`ferrocad_part::register(Arc<OcctBackend>)` needs the whole vtable.** Coercing
  to `Arc<dyn GeometryBackend>` retains *every* trait method, including
  `split_shape`, which is `BRepFeat_SplitShape` — and the prebuilt kernel ships no
  `TKFeat`. Static dispatch did not need it, so the earlier demo linked without it.
  The first fix made `OcctBackend::split_shape` a reported error on `wasm32`;
  **resolved** by building that one toolkit into the kernel (`run.sh` sparse-clones
  the 35 translation units and compiles them with the kernel's own wasi-sdk, ~1 min,
  cached), so wasm now links the real implementation and
  `Part.makeSplitShape` works in the browser. The fork
  (`v0.3.0-ferrocad.29`) links `-lTKFeat` only when the archive is present and warns
  with the recipe otherwise; `docs/developer/architecture/occt-bundling.md` §8.1
  lists the kernel's modules and the recipe.
- **A refused fillet is data, not an abort.** `fillet(12.0)` on the 10 mm box now
  returns `OCCT operation \`fillet\` did not complete` with the last good shape
  still in the output property, which the page renders next to the message. This is
  the F1 behaviour of `operation-failure-modelling.md` seen end to end, and it is
  what the original crash report was about.

`www/smoke.mjs` asserts all of it: 40 triangles, a refused radius (reported, last
good shape kept), a valid one (rebuilds), and finally `split_check()` — a 1×2×2 box
face cut by a shifted edge, one direct and one left piece — so the `TKFeat` archive
is exercised by behaviour, not only by linking.

### GPUI in the browser (same day)

`bite-gpui` 1.24.402 already ships a browser platform (`bite-gp-web`: one
document-owned canvas, WebGPU preferred with an automatic WebGL2 fallback), so
"GPUI in wasm" is a repair job, not a port. Five defects stood in the way, all
filed upstream (`bite-gpui/bite-gpui` #15–#19): a default feature that pulls a
nightly-only `wasm_thread`, two wrong `cfg`s in `bite-gp-wgpu`, two imports from
the wrong crate, and — the one that matters here — no `GpuRenderer` impl for the
browser, which is the door `gpui_interop`'s same-device producer knocks on.
`third_party/bite-gp-web` and `third_party/bite-gp-wgpu` carry the patches, each
with a `PATCHES.md` recording the upstream issue and the removal condition.

`crates/ferrocad_web_gpui` is the probe: a GPUI window in Chrome on WebGL2 doing
`try_device::<WgpuRenderer>()` → `gpui_interop::WgpuSurface` →
`ferrocad_wgpu::render(&surface.view(), …)` → `to_imported_handle()` →
`paint_texture`. It draws a real `Part::Box` + `Part::Fillet` document (40
triangles, 2 packets) recomputed by the wasm kernel, with the camera orbiting so a
screenshot proves a live surface. Two things it does not have: text (GPUI panics
when asked to resolve a font and none was added) and CPython (`pyo3` has no
`wasm32-unknown-unknown` story), so the shell proper waits on a `no-python` split.
The OCCT link stubs moved to `crates/ferrocad_wasm`, shared with `ferrocad_web`.

One workspace bug fell out: `.cargo/config.toml` applied the OCCT `-Wl,-rpath`
link args to every target, and `rust-lld` rejects them, so any host
`--target wasm32-unknown-unknown` build failed while linking. They are now scoped
to `cfg(target_os = "linux")`.
