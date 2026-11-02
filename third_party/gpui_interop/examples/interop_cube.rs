//! `interop_cube`: a foreign 3D viewport, its cube turned on a clock.
//!
//! ```sh
//! cargo run -p gpui_interop --example interop_cube --features example
//! ```
//!
//! The window shows a spinning cube, rendered by a producer that owns its device and handed over as a
//! dma-buf — no CPU copy. `examples/common/cube.rs` is that producer — the Vulkan pipeline, the ring and
//! the export — and this file is only the window and the view: it asks for a frame every animation tick
//! and turns the cube a little between them.
//!
//! `interop_cube_drag` turns the same cube with the pointer instead, through the input gateway; this
//! one spins so that the zero-copy path can be seen without touching anything.

#[cfg(target_os = "linux")]
fn main() {
    demo::run();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!(
        "interop_cube is a Linux example: it renders into a shareable Vulkan image and exports it as \
         a dma-buf for the window's renderer. Run it on Linux."
    );
}

#[cfg(target_os = "linux")]
#[path = "common/cube.rs"]
mod cube;

#[cfg(target_os = "linux")]
mod demo {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{
        AnyElement, App, Bounds, Context, IntoElement, Render, Window, WindowBounds, WindowOptions,
        div, gpu_canvas, prelude::*, px, rgb, size,
    };

    use crate::cube::{Cube, OPAQUE};

    /// The edge of the produced surface, in pixels, and of the canvas it lands in.
    const TILE: u32 = 320;

    pub fn run() {
        gpui::application().run(|cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(560.), px(520.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| cx.new(|_| Showcase::new()),
            )
            .expect("a window to composite into");
            cx.activate(true);
        });
    }

    /// The window's view. The cube is built lazily at paint time — the renderer's device exists only
    /// then — so the view holds a shared slot the canvas fills on the first frame.
    struct Showcase {
        cube: Rc<RefCell<Option<Cube>>>,
        /// Why the cube could not be built, when it could not: shown in place of the device line.
        error: Rc<RefCell<Option<String>>>,
        /// How many frames have been requested. The window keeps asking for frames while a cube exists;
        /// before it does, a couple of frames are enough to build one (or give up), so a window with no
        /// dma-buf device does not spin on its own.
        requested: u32,
    }

    impl Showcase {
        fn new() -> Self {
            Self {
                cube: Rc::new(RefCell::new(None)),
                error: Rc::new(RefCell::new(None)),
                requested: 0,
            }
        }
    }

    impl Render for Showcase {
        fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let built = self.cube.borrow().is_some();
            if built || self.requested < 2 {
                self.requested += 1;
                window.request_animation_frame();
            }

            let line = match (self.cube.borrow().as_ref(), self.error.borrow().as_ref()) {
                (Some(cube), _) => format!(
                    "{} · {} frames rendered, {} dropped",
                    cube.gpu(),
                    cube.rendered(),
                    cube.dropped()
                ),
                (None, Some(error)) => error.clone(),
                (None, None) => "negotiating with the window's renderer…".to_owned(),
            };

            div()
                .size_full()
                .bg(rgb(0x101014))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_6()
                .p_8()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .text_2xl()
                                .text_color(rgb(0xffffff))
                                .child("Foreign 3D viewport"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(0x9a9aa2))
                                .text_center()
                                .max_w(px(460.))
                                .child(
                                    "gpui_interop · Linux · a cube a producer renders with its own \
                                     Vulkan pipeline, exported as a dma-buf and sampled by the window's \
                                     renderer — no CPU copy",
                                ),
                        )
                        .child(div().text_sm().text_color(rgb(0x6f8fbf)).child(line)),
                )
                .child(canvas(self.cube.clone(), self.error.clone()))
        }
    }

    /// The one canvas: it builds the cube on the first paint, turns it a step, and renders a frame on
    /// every paint after that.
    fn canvas(cube: Rc<RefCell<Option<Cube>>>, error: Rc<RefCell<Option<String>>>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .items_center()
            .w(px(TILE as f32))
            .child(
                div().w(px(TILE as f32)).h(px(TILE as f32)).child(
                    gpu_canvas(move |gpu| {
                        let mut cube = cube.borrow_mut();
                        if cube.is_none() {
                            match Cube::build(gpu, TILE, OPAQUE) {
                                Ok(built) => *cube = Some(built),
                                Err(build) => {
                                    log::error!("interop_cube: cannot build the cube: {build:#}");
                                    *error.borrow_mut() = Some(format!("{build}"));
                                    return;
                                }
                            }
                        }
                        let cube = cube.as_mut().expect("the cube was just built");
                        // The clock this example turns the cube with.
                        cube.turn(0.013, 0.013 * 0.6);
                        if let Some(handle) = cube.frame() {
                            gpu.paint_surface(handle);
                        }
                    })
                    .bg(rgb(0x23232c))
                    .size_full(),
                ),
            )
            .into_any_element()
    }
}
