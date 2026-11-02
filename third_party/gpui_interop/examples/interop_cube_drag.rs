//! `interop_cube_drag`: a foreign 3D viewport between GPUI's layers, the cube turned by dragging.
//!
//! ```sh
//! cargo run -p gpui_interop --example interop_cube_drag --features example
//! ```
//!
//! The window is a 3D viewport — a cube a producer renders with its own Vulkan pipeline and hands over
//! as a dma-buf, no CPU copy — with GPUI drawn **both under and over** it. Beneath the viewport sits a
//! row of button-shaped tiles, which the transparent frame lets through and the cube clips; over it sit
//! a panel of text and a bar of real buttons. Drag the viewport to turn the cube, scroll to zoom it,
//! or press a button.
//!
//! # The producer sees no GPUI
//!
//! The canvas is wrapped in [`surface_input`], the input gateway, and the handler takes [`SurfaceInput`]:
//! each event's position is in **physical pixels from the viewport's top-left corner**, with the scale
//! factor alongside, and its `SurfaceEvent` is the crate's vocabulary rather than GPUI's. A drag starts
//! on the viewport, which founds the gateway's own drag, so the cube keeps turning while the pointer is
//! off past the edge — and because the gateway refreshes the window after each event, the producer only
//! has to change its own state. Not every event takes that road: the wheel, which zooms the cube, is the
//! demo's own `on_scroll_wheel`, because the gateway's vocabulary carries no scroll yet and the zoom is
//! the view's rather than the producer's.
//!
//! # Three layers, and why the frame is transparent
//!
//! The viewport area stacks three things. At the bottom, ordinary GPUI elements — a row of button-shaped
//! tiles — which the surface composites *over*: the producer clears its frame to transparent
//! (`cube::TRANSPARENT`), so only the cube's own pixels occlude them and the tiles show through
//! everywhere the cube is not. Above them the surface, and above that the chrome, drawn over the whole
//! window. One window therefore shows both directions at once — GPUI under the surface, GPUI over it —
//! with the cube between the two. It is what a transparent frame buys and an opaque one cannot:
//! `interop_cube` clears to `cube::OPAQUE` and any tiles under it would be hidden.
//!
//! # How the chrome and the viewport stay out of each other's way
//!
//! Which element an event belongs to is hit-testing's to decide, and a GPUI element in front takes it
//! over its own rectangle:
//!
//! - **A button keeps its own events.** The bar's buttons sit over the viewport and `occlude`, so a
//!   pointer-down on one hit-tests to the button and never to the viewport: a drag that starts on a
//!   button turns nothing, and a click on it is a click.
//! - **The text panel claims nothing.** A plain `div` of text inserts no hitbox, so the viewport under
//!   it is still what the pointer is over — a drag across the panel turns the cube as if the panel were
//!   not there. A rectangle is not the same as wanting the event.
//! - **A drag that starts on the viewport keeps arriving.** The gateway founds an engine drag on it, so
//!   the cube keeps turning even when the drag passes *under* the panel or the bar, and the buttons
//!   under it do not fire. The handler pairs the down and the up itself, so a gesture that merely
//!   passes over the viewport — a drag begun on a button — does not turn it.
//!
//! `interop_cube` is the same cube turned on a clock instead; see the crate's `input` module for what
//! the gateway adapts and what it deliberately leaves to the engine.

#[cfg(target_os = "linux")]
fn main() {
    demo::run();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!(
        "interop_cube_drag is a Linux example: it renders into a shareable Vulkan image and exports it \
         as a dma-buf for the window's renderer. Run it on Linux."
    );
}

#[cfg(target_os = "linux")]
#[path = "common/cube.rs"]
mod cube;

#[cfg(target_os = "linux")]
mod demo {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use gpui::{
        AnyElement, App, Bounds, Context, IntoElement, Render, ScrollDelta, Window, WindowBounds,
        WindowOptions, div, gpu_canvas, prelude::*, px, rgb, rgba, size,
    };
    use gpui_interop::{SurfaceEvent, SurfaceInput, SurfacePoint, surface_input};

    use crate::cube::{Cube, TRANSPARENT};

    /// How far the eye moves per notch of the wheel, as a factor: below one brings the cube nearer and
    /// makes it larger. Geometric, so a notch feels the same wherever the zoom is.
    const ZOOM_PER_NOTCH: f32 = 0.9;

    /// What a line of wheel notches is taken to be, for a mouse that reports pixels instead.
    const PIXELS_PER_LINE: f32 = 40.0;

    /// The edge the producer renders at. The viewport is a square as tall as the window, so at a typical
    /// window height this is within a whisker of one texel a pixel.
    const TILE: u32 = 640;

    /// How far the cube turns per physical pixel dragged — about a third of a turn across the viewport.
    const SENSITIVITY: f32 = 0.006;

    pub fn run() {
        gpui::application().run(|cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(900.), px(640.)), cx);
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
        /// The pointer's last surface-local position, so a move becomes the delta a drag turns by. Held
        /// across frames, since the element tree is rebuilt every render.
        last: Rc<Cell<Option<SurfacePoint>>>,
        /// Whether the viewport's own gesture is in progress — the down it saw and has not seen the up
        /// of. A gesture that only passes over the viewport has no down, so it does not turn the cube.
        grab: Rc<Cell<bool>>,
        /// Whether the cube turns on its own, toggled by the bar's button. Shared with the paint
        /// callback, which is what turns it.
        spinning: Rc<Cell<bool>>,
        /// The tile of the layer under the cube last clicked — proof that a click on a transparent
        /// pixel reaches what the surface is drawn over.
        picked: Rc<Cell<Option<&'static str>>>,
        /// How many frames have been requested: a couple to build the cube (or give up), and one per
        /// delivered input, which the gateway asks for itself.
        requested: u32,
    }

    impl Showcase {
        fn new() -> Self {
            Self {
                cube: Rc::new(RefCell::new(None)),
                error: Rc::new(RefCell::new(None)),
                last: Rc::new(Cell::new(None)),
                grab: Rc::new(Cell::new(false)),
                spinning: Rc::new(Cell::new(false)),
                picked: Rc::new(Cell::new(None)),
                requested: 0,
            }
        }
    }

    impl Render for Showcase {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            // The viewport turns the cube only when the pointer or the Spin button turns it, and the
            // gateway asks for the frame each of those implies — but a spinning cube needs a frame
            // every tick, and a couple of frames up front build it (or give up).
            let built = self.cube.borrow().is_some();
            let spinning = self.spinning.get();
            if spinning || (!built && self.requested < 2) {
                if !built {
                    self.requested += 1;
                }
                window.request_animation_frame();
            }

            let status = match (self.cube.borrow().as_ref(), self.error.borrow().as_ref()) {
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
                .relative()
                .size_full()
                .overflow_hidden()
                .bg(rgb(0x0b0b0f))
                // The viewport: a square as large as the window's shorter side allows, so the
                // producer's square buffer is not stretched, and holding the two layers the
                // transparency is here to show — GPUI's own elements, and the surface over them. The
                // chrome floats over the whole window, not only over the square.
                .child(
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .h_full()
                                .aspect_square()
                                .child(under(self.picked.clone()))
                                .child(canvas(
                                    self.cube.clone(),
                                    self.error.clone(),
                                    self.last.clone(),
                                    self.grab.clone(),
                                    self.spinning.clone(),
                                )),
                        ),
                )
                .child(panel(status, self.picked.get()))
                .child(bar(cx, self.spinning.clone()))
        }
    }

    /// The viewport: the surface, wrapped in the input gateway. The first paint builds the cube, and
    /// every pointer event over the viewport turns it by the pointer's delta.
    fn canvas(
        cube: Rc<RefCell<Option<Cube>>>,
        error: Rc<RefCell<Option<String>>>,
        last: Rc<Cell<Option<SurfacePoint>>>,
        grab: Rc<Cell<bool>>,
        spinning: Rc<Cell<bool>>,
    ) -> AnyElement {
        let drawing = cube.clone();
        let zooming = cube.clone();
        let covering = cube.clone();
        let surface = gpu_canvas(move |gpu| {
            let mut cube = drawing.borrow_mut();
            if cube.is_none() {
                match Cube::build(gpu, TILE, TRANSPARENT) {
                    Ok(built) => *cube = Some(built),
                    Err(build) => {
                        log::error!("interop_cube_drag: cannot build the cube: {build:#}");
                        *error.borrow_mut() = Some(format!("{build}"));
                        return;
                    }
                }
            }
            let cube = cube.as_mut().expect("the cube was just built");
            if spinning.get() {
                cube.turn(0.013, 0.0078);
            }
            if let Some(handle) = cube.frame() {
                gpu.paint_surface(handle);
            }
        })
        // The canvas claims the pointer only where the cube is, so the tiles under it stay live: the
        // hover and the click they get is the engine's hit-testing reaching past the surface. The
        // engine and the producer ask the same predicate in the same frame — the canvas's own `0..1`,
        // which is the frame the composited buffer is indexed by.
        .occlude_where(move |uv| {
            covering
                .borrow()
                .as_ref()
                .is_some_and(|cube| cube.covers(uv))
        })
        // The wheel zooms the *cube*, so it is the demo's own gpui handler and not the gateway's: the
        // gateway's vocabulary carries no scroll yet, and the zoom is the view's rather than the
        // producer's. One notch moves the eye by a fixed proportion, so zooming out to see the layer
        // behind the cube and zooming back in feel the same at either end.
        .on_scroll_wheel(move |event, window, _cx| {
            let notches = match event.delta {
                ScrollDelta::Lines(lines) => lines.y,
                ScrollDelta::Pixels(pixels) => pixels.y.as_f32() / PIXELS_PER_LINE,
            };
            let mut cube = zooming.borrow_mut();
            let Some(cube) = cube.as_mut() else {
                // Nothing to zoom before the first paint has built the cube.
                return;
            };
            cube.zoom(ZOOM_PER_NOTCH.powf(notches));
            window.refresh();
        })
        // No background of its own: the surface is composited over the layer beneath it, and where
        // the producer left the frame transparent that layer shows through.
        .absolute()
        .size_full();

        let handler = move |input: SurfaceInput| {
            let mut cube = cube.borrow_mut();
            let Some(cube) = cube.as_mut() else {
                return;
            };
            match input.event {
                // The gesture is the viewport's from here until the up, whether or not it has yet
                // become a drag — the engine starts the drag only past its own threshold.
                SurfaceEvent::PointerDown { position, .. } => {
                    grab.set(true);
                    last.set(Some(position));
                }
                // A held move *only* turns the cube when this viewport founded the gesture; a drag that
                // merely passes over it has no down here, so it is reported and ignored.
                SurfaceEvent::PointerMove { position, .. } if grab.get() => {
                    if let Some(previous) = last.replace(Some(position)) {
                        cube.turn(
                            (position.x - previous.x) * SENSITIVITY,
                            (position.y - previous.y) * SENSITIVITY,
                        );
                    }
                }
                SurfaceEvent::PointerMove { .. } => {}
                SurfaceEvent::PointerUp { .. } => {
                    grab.set(false);
                    last.set(None);
                }
            }
        };

        surface_input(surface, handler).into_any_element()
    }

    /// The layer the buffer is transparent over: ordinary GPUI controls the cube is seen to sit in
    /// front of. Each carries an id and a listener, so each inserts a hitbox and takes the pointer —
    /// but only where the canvas declines it, which is to say where the cube is not. Over the cube the
    /// canvas blocks, so a tile is neither hovered nor clicked; in the gaps it is, and the layer behind
    /// the surface is live again.
    fn under(picked: Rc<Cell<Option<&'static str>>>) -> AnyElement {
        div()
            .absolute()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .gap_6()
            .child(tile("Cut", picked.clone()))
            .child(tile("Copy", picked.clone()))
            .child(tile("Paste", picked.clone()))
            .child(tile("Undo", picked.clone()))
            .child(tile("Redo", picked))
            .into_any_element()
    }

    /// A button-shaped tile of the layer under the surface. Its id and its listener are what make it a
    /// control: a hitbox the canvas's decline lets hit-testing through to.
    fn tile(label: &'static str, picked: Rc<Cell<Option<&'static str>>>) -> AnyElement {
        div()
            .id(label)
            .px_5()
            .py_3()
            .rounded_md()
            .bg(rgba(0xffffff2e))
            .border_1()
            .border_color(rgba(0xffffff42))
            .text_sm()
            .text_color(rgb(0xe7eaf0))
            .cursor_pointer()
            .hover(|style| style.bg(rgba(0xffffff52)).border_color(rgb(0x8fb7ff)))
            .on_click(move |_, window, _| {
                picked.set(Some(label));
                window.refresh();
            })
            .child(label)
            .into_any_element()
    }

    /// A translucent panel of text, top left. It is not interactive, so a drag over it still reaches the
    /// viewport underneath — only a control should take the pointer.
    fn panel(status: String, picked: Option<&'static str>) -> AnyElement {
        div()
            .absolute()
            .top(px(16.))
            .left(px(16.))
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .rounded_lg()
            .bg(rgba(0x0a0a0fdd))
            .border_1()
            .border_color(rgba(0xffffff14))
            .child(
                div()
                    .text_lg()
                    .text_color(rgb(0xffffff))
                    .child("Foreign 3D viewport"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0xa8a8b2))
                    .max_w(px(240.))
                    .child(
                        "Drag to turn the cube; scroll to zoom. Where the cube is not, the transparent \
                         frame hands clicks to the tiles behind.",
                    ),
            )
            .child(div().text_xs().text_color(rgb(0x6f8fbf)).child(status))
            .child(div().text_xs().text_color(rgb(0x8fb7ff)).child(match picked {
                Some(label) => format!("clicked through: {label}"),
                None => "click a tile in the gap".to_owned(),
            }))
            .into_any_element()
    }

    /// The bar of controls, bottom centre. Each button is a GPUI element in front of the viewport, so a
    /// click over one never reaches the cube.
    fn bar(cx: &mut Context<Showcase>, spinning: Rc<Cell<bool>>) -> AnyElement {
        let reset = div()
            .id("cube-reset")
            .occlude()
            .px_3()
            .py_1()
            .rounded_md()
            .bg(rgba(0xffffff1a))
            .text_sm()
            .text_color(rgb(0xececf2))
            .cursor_pointer()
            .hover(|style| style.bg(rgba(0xffffff2e)))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(cube) = this.cube.borrow_mut().as_mut() {
                    cube.reset();
                }
                cx.notify();
            }))
            .child("Reset");

        let label = if spinning.get() { "Still" } else { "Spin" };
        let spin = div()
            .id("cube-spin")
            .occlude()
            .px_3()
            .py_1()
            .rounded_md()
            .bg(rgba(0xffffff1a))
            .text_sm()
            .text_color(rgb(0xececf2))
            .cursor_pointer()
            .hover(|style| style.bg(rgba(0xffffff2e)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.spinning.set(!this.spinning.get());
                cx.notify();
            }))
            .child(label);

        div()
            .absolute()
            .bottom(px(16.))
            .left_0()
            .right_0()
            .flex()
            .flex_row()
            .justify_center()
            .gap_2()
            .child(reset)
            .child(spin)
            .into_any_element()
    }
}
