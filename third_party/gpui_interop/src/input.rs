//! The surface input gateway: GPUI's events, adapted for a producer.
//!
//! GPUI owns the window and its input. It hit-tests the scene front to back and dispatches to the
//! element under the pointer — a `gpu_canvas` is an ordinary element in that tree, so the events over
//! its bounds are the producer's. This module is how they reach a producer **without the producer
//! naming `gpui`**: the element side (which lives here, in the crate that already depends on the
//! facade) subscribes to GPUI's handlers and hands the producer a [`SurfaceInput`].
//!
//! ```text
//! surface_input(gpu_canvas(..), |input| { .. })   // the element, with a producer's handler
//!     SurfaceInput { event: PointerMove { position: .., .. }, scale }   // what the handler gets
//! ```
//!
//! What the gateway adapts:
//!
//! - **Coordinates.** GPUI's events carry a window-relative logical `Point<Pixels>`. A [`SurfaceEvent`]
//!   carries a [`SurfacePoint`] in **physical pixels from the surface's top-left corner**, and the
//!   [`SurfaceInput`] carries the `scale` it was computed with (physical pixels per logical pixel — the
//!   window's device-pixel ratio). A producer that wants logical surface-local coordinates divides the
//!   position by `scale`.
//! - **Vocabulary.** [`SurfaceEvent`] and its parts are the crate's, mapped from GPUI's, so a producer
//!   never names `MouseMoveEvent`.
//!
//! What it deliberately does **not** do:
//!
//! - **Route.** Hit-testing and dispatch are the engine's. The gateway only sees what GPUI already
//!   routed to the surface's element.
//! - **Decide pass-through.** Whether a producer wants an *event it can see* but not *handle* — so a
//!   GPUI element under the surface's rectangle still receives it — is the producer's to answer, and
//!   the engine has no "declines" signal on a hit target for the gateway to relay yet.
//!
//! # The gesture is the engine's drag, not a capture of our own
//!
//! A gesture that starts on the surface has to keep arriving once the pointer has left its bounds —
//! otherwise a drag turns only while the cursor is over the viewport, which is not what a drag is.
//! GPUI already carries such a gesture: a *drag*, started by [`StatefulInteractiveElement::on_drag`]
//! once a pointer-down is followed by a move past the engine's threshold, held on the `App` rather
//! than in any element so it survives the redraws it causes, and delivered to every
//! [`InteractiveElement::on_drag_move`] whose type it matches. The gateway uses it rather than
//! capturing the pointer itself, and so keeps no gesture state of its own across a frame:
//!
//! - It registers an `on_drag` on the wrapped element whose value is the surface's own, with an empty
//!   preview — the producer paints the surface, so there is nothing to draw under the cursor.
//! - It registers an `on_drag_move` that forwards each move, **wherever the pointer is**, as a
//!   [`SurfaceEvent::PointerMove`]. Every surface's `on_drag_move` sees every gateway drag, so the
//!   value carries the *origin surface's id* and each surface forwards only the drag it started.
//! - The move that crossed the threshold is consumed by the drag's own activation, so the constructor
//!   reports it; without that the producer would never see the first off-bounds position.
//! - The pointer-**up** is read from that same event: the engine releases the drag on it, so the
//!   surface's own drag reports the up wherever the pointer is (`on_mouse_up_out`), and an ordinary
//!   gesture reports it from the hovered handler as before.
//!
//! # The one authoring consequence: an id
//!
//! The drag model keys an element's state by an `ElementId`, so the surface element must have one.
//! [`surface_input`] names an element the caller left unnamed, so a lone surface needs nothing — but
//! two children of one parent may not share an id, so a caller with more than one surface under a
//! parent names them (`gpu_canvas(..).id("pane-3")`), and the name is what picks that surface's drag
//! out of the ones passing over it.
//!
//! # Pairing down and up is the producer's
//!
//! The gateway reports what GPUI routes to the surface's bounds: a move *over* the surface arrives
//! even while another element holds the gesture, with `pressed` set. A producer that wants only its
//! own gesture therefore tracks whether it saw the [`SurfaceEvent::PointerDown`] — the gateway does
//! not, because it holds no state across a frame. The one asymmetry is the up of a gesture that began
//! elsewhere and ended over the surface, which arrives without a matching down.

use std::cell::Cell;
use std::panic::Location;
use std::rc::Rc;

use gpui::{
    App, AppContext as _, Bounds, DragMoveEvent, Element, ElementId, Empty, Global, GlobalElementId,
    Hitbox, InteractiveElement, IntoElement, LayoutId, Modifiers, MouseButton, Pixels, Point,
    ScrollDelta, Stateful, StatefulInteractiveElement, Window,
};

/// The id a surface is named with when its element carried none.
///
/// Two surfaces under one parent may not share an id, so this name is enough for the common case and
/// a caller with several surfaces names each of them — see the module docs.
const DEFAULT_ID: &str = "gpui_interop::surface";

/// Where a pointer is on the surface, in physical pixels from its top-left corner.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SurfacePoint {
    /// Pixels to the right of the surface's left edge.
    pub x: f32,
    /// Pixels below the surface's top edge.
    pub y: f32,
}

/// A pointer button, as a producer sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceButton {
    /// The primary button.
    Left,
    /// The secondary button.
    Right,
    /// The middle button.
    Middle,
    /// A navigation button (back/forward), or one the gateway does not distinguish.
    Other,
}

/// A scroll delta, as the engine reports it.
///
/// Deliberately not normalised: a wheel sends notches and a trackpad sends precise pixels, and a
/// consumer that re-derives a rate (every zoom does) needs to know which it got. The sign is the
/// engine's — a positive `y` is "up", which is already the opposite of the browser's `deltaY`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SurfaceScroll {
    /// Precise scrolling, in the engine's pixels.
    Pixels { x: f32, y: f32 },
    /// Discrete notches.
    Lines { x: f32, y: f32 },
}

/// The modifier keys held when an event arrived.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SurfaceModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    /// The platform key — Command on macOS, the Super/Windows key elsewhere.
    pub platform: bool,
}

/// A producer-facing input event over a surface, in surface-local physical pixels.
///
/// The event's own position is the pointer's; only a [`Scroll`](Self::Scroll) event, or a gesture a
/// producer reads from absolute positions, needs it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SurfaceEvent {
    /// A pointer button went down over the surface.
    PointerDown {
        position: SurfacePoint,
        button: SurfaceButton,
        modifiers: SurfaceModifiers,
        /// How many clicks this press is part of: 2 or more is a double-click.
        click_count: usize,
    },
    /// The pointer moved over the surface — while a button is held (`pressed`) or merely hovering.
    /// Once the surface's own gesture has begun the position may be *outside* its bounds, since the
    /// engine's drag keeps delivering it wherever the pointer goes.
    PointerMove {
        position: SurfacePoint,
        pressed: Option<SurfaceButton>,
        modifiers: SurfaceModifiers,
    },
    /// A pointer button went up.
    PointerUp {
        position: SurfacePoint,
        button: SurfaceButton,
        modifiers: SurfaceModifiers,
    },
    /// The pointer entered the surface's bounds. There is no position: this says that the pointer
    /// came, not where it is.
    PointerEnter { modifiers: SurfaceModifiers },
    /// The pointer left the surface's bounds, including by leaving the window.
    PointerLeave { modifiers: SurfaceModifiers },
    /// The wheel or trackpad scrolled over the surface.
    Scroll {
        /// Where the pointer was, so a zoom can stay anchored under it.
        position: SurfacePoint,
        delta: SurfaceScroll,
        modifiers: SurfaceModifiers,
    },
}

/// A surface input, adapted from GPUI's, ready for a producer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceInput {
    /// What happened.
    pub event: SurfaceEvent,
    /// Physical pixels per logical pixel — the window's device-pixel ratio. Every
    /// [`SurfaceEvent`]'s position is already in these pixels; this is the factor they were computed
    /// with, for a producer that wants logical coordinates instead.
    pub scale: f32,
}

/// Wrap `element` so a producer receives its input through [`SurfaceInput`].
///
/// `element` is the surface's own element — a `gpu_canvas` — and `handler` is called with every pointer
/// event routed to it, in surface-local physical pixels. The window is marked for redraw after each
/// event, since a surface that handled input has almost certainly changed what it draws.
///
/// The element keeps the sizing, styling and painting it had; the gateway adds the input
/// subscription, reads the element's bounds from its own prepaint so the producer's coordinates are
/// surface-local without the caller plumbing them, and names the element if the caller did not (the
/// drag model needs an id — see the module docs).
pub fn surface_input<E>(element: E, handler: impl Fn(SurfaceInput) + 'static) -> impl IntoElement
where
    E: InteractiveElement + Element<PrepaintState = Option<Hitbox>> + 'static,
{
    // Kept if the caller named the element, so a caller with several surfaces under one parent can
    // pick each one's drag; given a default otherwise.
    let id = Element::id(&element).unwrap_or_else(|| ElementId::from(DEFAULT_ID));
    let surface = Rc::new(Surface {
        bounds: Cell::new(None),
        handler: Rc::new(handler),
    });
    let inner = attach(element.id(id.clone()), &surface, id);
    SurfaceInputElement { inner, surface }
}

/// The state the listeners and the element share.
///
/// It lives for one paint, not across frames: the gesture's own continuity is the engine's drag's,
/// held on the `App`, so nothing here has to survive a redraw.
struct Surface {
    /// The element's bounds from the last prepaint — the origin a window position is made
    /// surface-local against, and (for a drag) evidence of where the surface was.
    bounds: Cell<Option<Bounds<Pixels>>>,
    handler: Rc<dyn Fn(SurfaceInput)>,
}

/// The value a surface's drag carries.
///
/// `on_drag_move` matches a drag by its value's type alone, so the type has to be the gateway's own
/// and has to say *which* surface started it — every surface sees every gateway drag, and the id is
/// what tells them apart.
struct SurfaceDrag {
    origin: ElementId,
}

/// The gateway drag in flight, if one is.
fn drag(cx: &App) -> Option<&SurfaceDrag> {
    cx.active_drag_value::<SurfaceDrag>()
}

/// The surface whose gesture is in progress, if any.
///
/// Held on the `App`, like the engine's drag, because the element-side state lives for one paint
/// while a press outlives it. It is what lets an up that never founded a drag — a right or middle
/// press released off the surface — still reach the producer: the engine's `on_drag` only starts
/// for the left button, so relying on the drag alone loses that up entirely.
#[derive(Default)]
struct PressedSurface(Option<ElementId>);

impl Global for PressedSurface {}

fn pressed_origin(cx: &App) -> Option<&ElementId> {
    cx.try_global::<PressedSurface>()
        .and_then(|pressed| pressed.0.as_ref())
}

fn set_pressed_origin(cx: &mut App, origin: Option<ElementId>) {
    cx.set_global(PressedSurface(origin));
}

/// Subscribe GPUI's pointer handlers on `element`, translating each to a [`SurfaceInput`] through
/// `surface`. `id` is the element's, which the surface's own drag carries so each surface forwards
/// only its own.
fn attach<E>(element: Stateful<E>, surface: &Rc<Surface>, id: ElementId) -> Stateful<E>
where
    E: InteractiveElement + Element<PrepaintState = Option<Hitbox>> + 'static,
{
    let down = surface.clone();
    let pressed = id.clone();
    let element = element.on_any_mouse_down(move |event, window, cx| {
        let Some(bounds) = down.bounds.get() else {
            return;
        };
        set_pressed_origin(cx, Some(pressed.clone()));
        let button = button(event.button);
        let modifiers = modifiers(event.modifiers);
        let click_count = event.click_count;
        let position = local(window.scale_factor(), bounds.origin, event.position);
        deliver(&down, window, position, |position| {
            SurfaceEvent::PointerDown {
                position,
                button,
                modifiers,
                click_count,
            }
        });
    });

    // The plain move: hovering, or the sub-threshold part of a gesture. While this surface's own drag
    // is in flight its moves arrive through `on_drag_move` — which fires wherever the pointer is —
    // so reporting them here too would double each one.
    let hover = surface.clone();
    let own = id.clone();
    let element = element.on_mouse_move(move |event, window, cx| {
        if drag(cx).is_some_and(|drag| drag.origin == own) {
            return;
        }
        let Some(bounds) = hover.bounds.get() else {
            return;
        };
        let pressed = event.pressed_button.map(button);
        let modifiers = modifiers(event.modifiers);
        let position = local(window.scale_factor(), bounds.origin, event.position);
        deliver(&hover, window, position, |position| {
            SurfaceEvent::PointerMove {
                position,
                pressed,
                modifiers,
            }
        });
    });

    // The wheel and the trackpad. The signals are the engine's, so a positive `y` is "up"; a
    // consumer re-derives its own notch rate, as every zoom does.
    let scrolled = surface.clone();
    let element = element.on_scroll_wheel(move |event, window, _cx| {
        let Some(bounds) = scrolled.bounds.get() else {
            return;
        };
        let position = local(window.scale_factor(), bounds.origin, event.position);
        let delta = scroll(event.delta);
        let modifiers = modifiers(event.modifiers);
        deliver(&scrolled, window, position, |position| {
            SurfaceEvent::Scroll {
                position,
                delta,
                modifiers,
            }
        });
    });

    // Coming and going. The engine has no pointer-enter/leave event of its own; a hover listener is
    // the transition, and a producer needs it to clear hover state (a gizmo, a cursor hint).
    let hovered = surface.clone();
    let element = element.on_hover(move |is_hovered: &bool, window, _cx| {
        let event = if *is_hovered {
            SurfaceEvent::PointerEnter {
                modifiers: modifiers(window.modifiers()),
            }
        } else {
            SurfaceEvent::PointerLeave {
                modifiers: modifiers(window.modifiers()),
            }
        };
        (hovered.handler)(SurfaceInput {
            event,
            scale: window.scale_factor(),
        });
        window.refresh();
    });

    // The surface's drag once the pointer has left its bounds, and while it is still over them.
    let carried = surface.clone();
    let own = id.clone();
    let element = element.on_drag_move::<SurfaceDrag>(
        move |event: &DragMoveEvent<SurfaceDrag>, window, cx| {
            // Fires for every surface's drag; only the one that started it reports.
            if event.drag(cx).origin != own {
                return;
            }
            let pressed = event.event.pressed_button.map(button);
            let modifiers = modifiers(event.event.modifiers);
            let position = local(
                window.scale_factor(),
                event.bounds.origin,
                event.event.position,
            );
            deliver(&carried, window, position, |position| {
                SurfaceEvent::PointerMove {
                    position,
                    pressed,
                    modifiers,
                }
            });
        },
    );

    // The drag's own founding move — the one past the threshold, which the activation consumes.
    let started = surface.clone();
    let element = element.on_drag(
        SurfaceDrag { origin: id.clone() },
        move |_value, offset, window, cx| {
            let position = local(window.scale_factor(), Point::default(), offset);
            let modifiers = modifiers(window.modifiers());
            deliver(&started, window, position, |position| {
                SurfaceEvent::PointerMove {
                    position,
                    pressed: Some(SurfaceButton::Left),
                    modifiers,
                }
            });
            // Nothing is drawn under the cursor: a surface paints itself.
            cx.new(|_| Empty)
        },
    );

    // The up over the surface: the ordinary case, and a drag released where it began. Under capture
    // phase and hovered, so it is the one that reports a plain click.
    let up = surface.clone();
    let up_origin = id.clone();
    let element = element.capture_any_mouse_up(move |event, window, cx| {
        let Some(bounds) = up.bounds.get() else {
            return;
        };
        if pressed_origin(cx) == Some(&up_origin) {
            set_pressed_origin(cx, None);
        }
        let button = button(event.button);
        let modifiers = modifiers(event.modifiers);
        let position = local(window.scale_factor(), bounds.origin, event.position);
        deliver(&up, window, position, |position| SurfaceEvent::PointerUp {
            position,
            button,
            modifiers,
        });
    });

    // The up of this surface's own drag, wherever the pointer is — the engine still holds the drag
    // during the up's dispatch, so a surface whose drag is in flight recognizes its own.
    let mut element = element;
    for released_button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
        let released = surface.clone();
        let own = id.clone();
        element = element.on_mouse_up_out(released_button, move |event, window, cx| {
            // Either the engine's drag is carrying this surface's gesture (left button, past the
            // threshold), or a press that never became a drag is ending out here — which is the
            // only way a right or middle press released off the surface reports an up at all.
            let carried = drag(cx).is_some_and(|drag| drag.origin == own);
            if !carried && pressed_origin(cx) != Some(&own) {
                return;
            }
            set_pressed_origin(cx, None);
            let Some(bounds) = released.bounds.get() else {
                return;
            };
            let button = button(event.button);
            let modifiers = modifiers(event.modifiers);
            let position = local(window.scale_factor(), bounds.origin, event.position);
            deliver(&released, window, position, |position| {
                SurfaceEvent::PointerUp {
                    position,
                    button,
                    modifiers,
                }
            });
        });
    }

    element
}

/// Hand the producer an event at `position`, then ask for the redraw handled input implies.
fn deliver(
    surface: &Rc<Surface>,
    window: &mut Window,
    position: SurfacePoint,
    event: impl FnOnce(SurfacePoint) -> SurfaceEvent,
) {
    let scale = window.scale_factor();
    (surface.handler)(SurfaceInput {
        event: event(position),
        scale,
    });
    window.refresh();
}

/// A window position, as physical pixels from a surface-local `origin`.
fn local(scale: f32, origin: Point<Pixels>, position: Point<Pixels>) -> SurfacePoint {
    let local = position - origin;
    SurfacePoint {
        x: local.x.as_f32() * scale,
        y: local.y.as_f32() * scale,
    }
}

/// The guest of a GPUI button, as a producer-facing one.
fn button(button: MouseButton) -> SurfaceButton {
    match button {
        MouseButton::Left => SurfaceButton::Left,
        MouseButton::Right => SurfaceButton::Right,
        MouseButton::Middle => SurfaceButton::Middle,
        MouseButton::Navigate(_) => SurfaceButton::Other,
    }
}

/// The engine's scroll delta, as a producer-facing one.
fn scroll(delta: ScrollDelta) -> SurfaceScroll {
    match delta {
        ScrollDelta::Pixels(point) => SurfaceScroll::Pixels {
            x: point.x.as_f32(),
            y: point.y.as_f32(),
        },
        ScrollDelta::Lines(point) => SurfaceScroll::Lines {
            x: point.x,
            y: point.y,
        },
    }
}

/// The guest of GPUI's modifiers, as a producer-facing set.
fn modifiers(modifiers: Modifiers) -> SurfaceModifiers {
    SurfaceModifiers {
        shift: modifiers.shift,
        control: modifiers.control,
        alt: modifiers.alt,
        platform: modifiers.platform,
    }
}

/// The element [`surface_input`] returns: it delegates everything to the surface's own element, and
/// only records the hitbox its prepaint yields.
struct SurfaceInputElement<E> {
    inner: E,
    surface: Rc<Surface>,
}

impl<E> Element for SurfaceInputElement<E>
where
    E: Element<PrepaintState = Option<Hitbox>>,
{
    type RequestLayoutState = E::RequestLayoutState;
    type PrepaintState = E::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        self.inner.id()
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        self.inner.source_location()
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.inner.request_layout(id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let prepaint = self.inner.prepaint(id, bounds, request_layout, window, cx);
        if let Some(hitbox) = &prepaint {
            self.surface.bounds.set(Some(hitbox.bounds));
        }
        prepaint
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner
            .paint(id, bounds, request_layout, prepaint, window, cx);
    }
}

impl<E: Element<PrepaintState = Option<Hitbox>>> IntoElement for SurfaceInputElement<E> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}
