//! A radial marking menu: a dielectric torus split into wedge targets around a
//! hollow cancel deadzone.
//!
//! [`wheel`] is **presentation and geometry only**: it paints the torus, the wedge
//! highlights and the inner semantic arc, and it places the labels. [`slot_at`] is
//! pure, so any caller can hit-test a pointer position without the element's bounds.
//!
//! [`WheelMenu`] is the **stateful sidecar** built on it. It owns the gesture — the
//! armed/latched phases, hover, the tap-vs-flick decision, dismissal, and its own
//! anchored overlay — and reports [`WheelEvent`]. A host decides only *when* to
//! open it (with which slots) and what a selected id means; the trigger itself
//! stays with the host, which knows where its pointer and canvas are.
//!
//! Tokens come from [`ferrocad_theme::ui`]: the glass torus, the `#38BDF8`
//! hover tint, and the per-tone inner arc (`#48E5FF` structural, `#F59E0B`
//! additive, `#EF4444` destructive).

use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::time::{Duration, Instant};

use gpui::{
    Anchor, Bounds, Context, EventEmitter, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, SharedString, Window, anchored, canvas,
    deferred, div, point, prelude::*, px, rgb, rgba,
};

use ferrocad_theme::ui::{
    CHROME_LINE, FG, HIGHLIGHT_TINT, HOVER_RIM, MUTED, PUSH_PULL_COLOR, SUBTRACT_COLOR, WHEEL_GLASS,
};

/// The inner radius of the active ring; a release inside it cancels.
pub const INNER_RADIUS: f32 = 24.0;
/// The outer radius of the active ring.
pub const OUTER_RADIUS: f32 = 76.0;
/// The label ring's radius.
pub const LABEL_RADIUS: f32 = 50.0;
/// The wheel element's side, including a little air around the torus.
pub const SIZE: f32 = 2.0 * OUTER_RADIUS + 16.0;
/// The label half-width, in logical px.
const LABEL_HALF_WIDTH: f32 = 44.0;

/// The semantic class of a slot, which selects its inner-arc ink.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelTone {
    /// A neutral transform (`Move`, `Set WP`, `Morph`): `#48E5FF`.
    Structural,
    /// An additive operation (`Extrude`, `Push/Pull`): `#F59E0B`.
    Additive,
    /// A destructive operation (`Delete`, `Pocket`): `#EF4444`.
    Destructive,
}

impl WheelTone {
    /// The inner-arc ink for the tone.
    pub fn accent(self) -> u32 {
        match self {
            Self::Structural => HOVER_RIM,
            Self::Additive => PUSH_PULL_COLOR,
            Self::Destructive => SUBTRACT_COLOR,
        }
    }
}

/// One wedge target.
#[derive(Clone, Debug)]
pub struct WheelSlot {
    /// An opaque id the caller maps back to a command; never interpreted here.
    pub id: SharedString,
    pub label: SharedString,
    pub tone: WheelTone,
    pub enabled: bool,
}

/// What is under a pointer position, relative to the wheel's centre.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelHit {
    /// Inside the hollow centre: releasing here cancels.
    Deadzone,
    /// Outside the outer ring: releasing here cancels.
    Outside,
    /// A wedge, by index into the slot list.
    Slot(usize),
}

/// The angle of a slot's centre, in radians, measured from +x (screen y down).
///
/// Slot 0 is north (up) and the slots run clockwise, so a four-slot wheel is
/// `N, E, S, W` and the destructive slot is conventionally last (south).
pub fn slot_angle(index: usize, count: usize) -> f32 {
    debug_assert!(count > 0);
    -FRAC_PI_2 + index as f32 * TAU / count as f32
}

/// The boundary angle before slot 0. The wedge around north is split evenly, so
/// slot 0 is **centred** on north rather than starting there.
fn start_angle(count: usize) -> f32 {
    -FRAC_PI_2 - PI / count as f32
}

/// The centre of a slot's label, relative to the wheel's centre.
pub fn slot_offset(index: usize, count: usize) -> Point<Pixels> {
    let angle = slot_angle(index, count);
    point(
        px(LABEL_RADIUS * angle.cos()),
        px(LABEL_RADIUS * angle.sin()),
    )
}

/// Hit-test `position` against a wheel centred at `centre`.
pub fn slot_at(centre: Point<Pixels>, position: Point<Pixels>, count: usize) -> WheelHit {
    if count == 0 {
        return WheelHit::Outside;
    }
    let dx = position.x.0 - centre.x.0;
    let dy = position.y.0 - centre.y.0;
    let radius = (dx * dx + dy * dy).sqrt();
    if radius < INNER_RADIUS {
        return WheelHit::Deadzone;
    }
    if radius > OUTER_RADIUS {
        return WheelHit::Outside;
    }
    let step = TAU / count as f32;
    let relative = (dy.atan2(dx) - start_angle(count)).rem_euclid(TAU);
    let index = (relative / step).floor() as usize;
    WheelHit::Slot(index.min(count - 1))
}

/// A radial menu of `slots`, with `hovered` highlighted.
///
/// The element is [`SIZE`] square and centred on its position; place it so its
/// centre lands on the gesture anchor. The caller handles pointer events.
pub fn wheel(key: &'static str, slots: &[WheelSlot], hovered: Option<usize>) -> impl IntoElement {
    let slots = slots.to_vec();
    let paint_slots = slots.clone();
    let outer_key: &'static str = key;
    let mut root = div()
        .id(SharedString::from(format!("{key}-wheel")))
        .debug_selector(move || format!("{outer_key}-wheel"))
        .relative()
        .w(px(SIZE))
        .h(px(SIZE));

    root = root.child(
        canvas(
            move |_bounds, _window, _cx| {},
            move |bounds: Bounds<Pixels>, _state, window: &mut Window, _cx| {
                paint(bounds, &paint_slots, hovered, window);
            },
        )
        .absolute()
        .size_full(),
    );

    let centre = px(SIZE / 2.0);
    for (index, slot) in slots.iter().enumerate() {
        let offset = slot_offset(index, slots.len());
        let ink = if !slot.enabled {
            MUTED
        } else if hovered == Some(index) {
            match slot.tone {
                WheelTone::Destructive => SUBTRACT_COLOR,
                _ => HIGHLIGHT_TEXT,
            }
        } else {
            FG
        };
        let label_id = SharedString::from(format!("{key}-label-{}", slot.id));
        root = root.child(
            div()
                .id(label_id.clone())
                .debug_selector({
                    let id = slot.id.clone();
                    move || format!("{key}-label-{id}")
                })
                .absolute()
                .left(px(centre.0 + offset.x.0 - LABEL_HALF_WIDTH))
                .top(px(centre.0 + offset.y.0 - 8.0))
                .w(px(LABEL_HALF_WIDTH * 2.0))
                .text_xs()
                .text_center()
                .text_color(rgb(ink))
                .child(slot.label.clone()),
        );
    }
    root
}

/// The hovered label ink for structural/additive slots: the bright rim.
const HIGHLIGHT_TEXT: u32 = HOVER_RIM;

fn paint(bounds: Bounds<Pixels>, slots: &[WheelSlot], hovered: Option<usize>, window: &mut Window) {
    let centre = bounds.center();
    let count = slots.len();
    if count == 0 {
        return;
    }
    let step = TAU / count as f32;
    let start = start_angle(count);

    // The dielectric torus: outer disc with the deadzone cut out (opposite
    // winding), so the model stays visible through the hole.
    let mut glass = PathBuilder::fill();
    glass.add_polygon(&circle(centre, OUTER_RADIUS, 128), true);
    let mut hole = circle(centre, INNER_RADIUS, 96);
    hole.reverse();
    glass.add_polygon(&hole, true);
    paint_fill(window, glass, WHEEL_GLASS);

    // The hovered wedge is a tint, never an opaque button.
    if let Some(index) = hovered.filter(|index| *index < count) {
        let from = start + index as f32 * step;
        let wedge = sector(centre, INNER_RADIUS, OUTER_RADIUS, from, from + step, 24);
        let mut path = PathBuilder::fill();
        path.add_polygon(&wedge, true);
        paint_fill(window, path, HIGHLIGHT_TINT);
    }

    // Radial hairlines between the wedges.
    for index in 0..count {
        let angle = start + index as f32 * step;
        let mut path = PathBuilder::stroke(px(1.0));
        path.add_polygon(
            &[
                polar(centre, INNER_RADIUS, angle),
                polar(centre, OUTER_RADIUS, angle),
            ],
            false,
        );
        paint_stroke(window, path, CHROME_LINE);
    }

    // The inner arc: the semantic tone. A destructive slot keeps a dim red
    // marker even at rest, so its position is legible before hover.
    for (index, slot) in slots.iter().enumerate() {
        let from = start + index as f32 * step + 0.06;
        let to = start + (index as f32 + 1.0) * step - 0.06;
        let (alpha, hovered_now) = match (hovered == Some(index), slot.tone) {
            (true, _) => (1.0, true),
            (false, WheelTone::Destructive) => (0.45, false),
            _ => continue,
        };
        let mut path = PathBuilder::stroke(px(if hovered_now { 2.0 } else { 1.5 }));
        path.add_polygon(&arc(centre, INNER_RADIUS + 2.0, from, to, 24), false);
        paint_stroke_alpha(window, path, slot.tone.accent(), alpha);
    }

    // The deadzone boundary and the outer rim: 1 px dielectric rings.
    for radius in [INNER_RADIUS, OUTER_RADIUS] {
        let mut path = PathBuilder::stroke(px(1.0));
        path.add_polygon(&circle(centre, radius, 96), true);
        paint_stroke(window, path, CHROME_LINE);
    }
}

fn paint_fill(window: &mut Window, builder: PathBuilder, color: u32) {
    if let Ok(path) = builder.build() {
        window.paint_path(path, rgba(color));
    }
}

fn paint_stroke(window: &mut Window, builder: PathBuilder, color: u32) {
    if let Ok(path) = builder.build() {
        window.paint_path(path, rgba(color));
    }
}

fn paint_stroke_alpha(window: &mut Window, builder: PathBuilder, color: u32, alpha: f32) {
    if let Ok(path) = builder.build() {
        // The tone tokens are opaque; the dim marker is the same ink at 45%.
        let a = (alpha * 255.0).round() as u32;
        window.paint_path(path, rgba((color << 8) | a));
    }
}

fn polar(centre: Point<Pixels>, radius: f32, angle: f32) -> Point<Pixels> {
    point(
        px(centre.x.0 + radius * angle.cos()),
        px(centre.y.0 + radius * angle.sin()),
    )
}

fn circle(centre: Point<Pixels>, radius: f32, segments: usize) -> Vec<Point<Pixels>> {
    (0..segments)
        .map(|index| polar(centre, radius, TAU * index as f32 / segments as f32))
        .collect()
}

fn arc(
    centre: Point<Pixels>,
    radius: f32,
    from: f32,
    to: f32,
    segments: usize,
) -> Vec<Point<Pixels>> {
    (0..=segments)
        .map(|index| {
            let angle = from + (to - from) * index as f32 / segments as f32;
            polar(centre, radius, angle)
        })
        .collect()
}

fn sector(
    centre: Point<Pixels>,
    inner: f32,
    outer: f32,
    from: f32,
    to: f32,
    segments: usize,
) -> Vec<Point<Pixels>> {
    let mut points = arc(centre, inner, from, to, segments);
    points.extend(arc(centre, outer, to, from, segments));
    points
}

// ---------------------------------------------------------------------------
// The stateful sidecar
// ---------------------------------------------------------------------------

/// What the wheel reports to its host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WheelEvent {
    /// The user chose a wedge; the host runs the command with this id.
    Selected(SharedString),
    /// The wheel closed without a choice (tap-cancel, deadzone click, Esc).
    Closed,
}

/// The gesture phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WheelMode {
    /// Opened by a button press; a flick commits on release, a tap latches.
    Armed,
    /// Open without a button held: move to highlight, click (or a host key) to commit.
    Latched,
}

/// A release at or below this age (and near the press) is a trackpad **tap**, not a
/// flick: the wheel latches instead of flashing closed.
const TAP_MS: u64 = 200;
/// A tap's maximum pointer travel, in logical px.
const TAP_PX: f32 = 12.0;

/// The radial menu's stateful sidecar.
///
/// It owns the gesture (armed/latched, hover, tap-vs-flick, dismissal) and its own
/// anchored overlay; the host only decides **when** to open it (and with which
/// slots) and **what** a selected id means. The trigger itself stays with the host:
/// it knows its canvas and its pointer.
///
/// ```text
/// host: open_armed(anchor, slots) ──► Armed ──flick──► emit Selected(id), close
///                                       │
///                                       └─tap──► Latched ──click──► Selected/Closed
/// ```
pub struct WheelMenu {
    slots: Vec<WheelSlot>,
    anchor: Point<Pixels>,
    hovered: Option<usize>,
    mode: Option<WheelMode>,
    pressed_at: Option<Instant>,
    press_position: Point<Pixels>,
}

impl EventEmitter<WheelEvent> for WheelMenu {}

impl WheelMenu {
    /// A closed wheel. Mount it once as a child of the canvas; it renders nothing
    /// until opened.
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            slots: Vec::new(),
            anchor: Point::default(),
            hovered: None,
            mode: None,
            pressed_at: None,
            press_position: Point::default(),
        }
    }

    /// Open for a **button press**: a flick commits on release, a quick tap latches.
    /// Returns whether it opened (no enabled slot means no).
    pub fn open_armed(
        &mut self,
        anchor: Point<Pixels>,
        slots: Vec<WheelSlot>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.open(anchor, slots, WheelMode::Armed, cx)
    }

    /// Open **latched** (the keyboard path): no button is held, so a click or a host
    /// key commits.
    pub fn open_latched(
        &mut self,
        anchor: Point<Pixels>,
        slots: Vec<WheelSlot>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.open(anchor, slots, WheelMode::Latched, cx)
    }

    fn open(
        &mut self,
        anchor: Point<Pixels>,
        slots: Vec<WheelSlot>,
        mode: WheelMode,
        cx: &mut Context<Self>,
    ) -> bool {
        if !slots.iter().any(|slot| slot.enabled) {
            return false;
        }
        self.anchor = anchor;
        self.slots = slots;
        self.hovered = None;
        self.mode = Some(mode);
        self.pressed_at = (mode == WheelMode::Armed).then(Instant::now);
        self.press_position = anchor;
        cx.notify();
        true
    }

    /// Close without a choice; emits [`WheelEvent::Closed`] when it was open.
    pub fn close(&mut self, cx: &mut Context<Self>) -> bool {
        let was_open = self.mode.is_some();
        if was_open {
            self.mode = None;
            self.pressed_at = None;
            self.hovered = None;
            cx.emit(WheelEvent::Closed);
            cx.notify();
        }
        was_open
    }

    pub fn is_open(&self) -> bool {
        self.mode.is_some()
    }

    pub fn is_latched(&self) -> bool {
        self.mode == Some(WheelMode::Latched)
    }

    pub fn anchor(&self) -> Point<Pixels> {
        self.anchor
    }

    pub fn slots(&self) -> &[WheelSlot] {
        &self.slots
    }

    pub fn hovered(&self) -> Option<usize> {
        self.hovered
    }

    /// Update the hovered wedge. Safe in either phase; a move outside the ring (or
    /// into the deadzone) clears it.
    pub fn pointer_move(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.mode.is_none() {
            return;
        }
        let hovered = match slot_at(self.anchor, position, self.slots.len()) {
            WheelHit::Slot(index) => self
                .slots
                .get(index)
                .filter(|slot| slot.enabled)
                .map(|_| index),
            _ => None,
        };
        if hovered != self.hovered {
            self.hovered = hovered;
            cx.notify();
        }
    }

    /// The arming button was released: commit a flicked wedge, latch a tap, or close.
    pub fn pointer_up(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.mode != Some(WheelMode::Armed) {
            return;
        }
        if let Some(id) = self.slot_id_at(position) {
            self.finish(Some(id), cx);
            return;
        }
        let elapsed = self
            .pressed_at
            .map(|pressed| pressed.elapsed())
            .unwrap_or_default();
        let travelled = distance(self.press_position, position);
        if elapsed < Duration::from_millis(TAP_MS) && travelled < TAP_PX {
            // A trackpad tap: latch instead of flashing the wheel closed.
            self.mode = Some(WheelMode::Latched);
            self.pressed_at = None;
            self.pointer_move(position, cx);
            cx.notify();
            return;
        }
        // A long hold released in the deadzone/outside cancels.
        self.finish(None, cx);
    }

    /// A primary click: commit in the latched phase, ignore an armed flick.
    pub fn click(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.mode != Some(WheelMode::Latched) {
            return;
        }
        match self.slot_id_at(position) {
            Some(id) => self.finish(Some(id), cx),
            None => self.finish(None, cx),
        }
    }

    /// A host key (Space) while hovered: commit the wedge. Returns whether it did;
    /// the host closes an open wheel when this is `false`.
    pub fn commit_hovered(&mut self, cx: &mut Context<Self>) -> bool {
        if self.mode != Some(WheelMode::Latched) {
            return false;
        }
        let Some(index) = self.hovered else {
            return false;
        };
        let Some(id) = self
            .slots
            .get(index)
            .filter(|slot| slot.enabled)
            .map(|slot| slot.id.clone())
        else {
            return false;
        };
        self.finish(Some(id), cx);
        true
    }

    fn slot_id_at(&self, position: Point<Pixels>) -> Option<SharedString> {
        match slot_at(self.anchor, position, self.slots.len()) {
            WheelHit::Slot(index) => self
                .slots
                .get(index)
                .filter(|slot| slot.enabled)
                .map(|slot| slot.id.clone()),
            _ => None,
        }
    }

    fn finish(&mut self, id: Option<SharedString>, cx: &mut Context<Self>) {
        self.mode = None;
        self.pressed_at = None;
        self.hovered = None;
        match id {
            Some(id) => cx.emit(WheelEvent::Selected(id)),
            None => cx.emit(WheelEvent::Closed),
        }
        cx.notify();
    }
}

impl Render for WheelMenu {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.mode.is_none() {
            return div().into_any_element();
        }
        let anchor = self.anchor;
        let slots = self.slots.clone();
        let hovered = self.hovered;
        let view = wheel("radial", &slots, hovered);
        deferred(
            anchored()
                .anchor(Anchor::TopLeft)
                .position(point(anchor.x - px(SIZE / 2.0), anchor.y - px(SIZE / 2.0)))
                // No window snapping: `slot_at` hit-tests against the anchor, so the
                // painted centre must stay exactly there.
                .child(
                    div()
                        // Claim the pointer: a latched click must not also select the
                        // scene underneath. The wheel has no hitboxes of its own, so
                        // the overlay is also its hover surface.
                        .occlude()
                        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                            this.pointer_move(event.position, cx)
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                                this.click(event.position, cx);
                                cx.stop_propagation();
                            }),
                        )
                        .on_mouse_up(
                            MouseButton::Right,
                            cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                                this.pointer_up(event.position, cx);
                                cx.stop_propagation();
                            }),
                        )
                        .child(view),
                ),
        )
        .with_priority(2)
        .into_any_element()
    }
}

fn distance(a: Point<Pixels>, b: Point<Pixels>) -> f32 {
    let dx = a.x.0 - b.x.0;
    let dy = a.y.0 - b.y.0;
    (dx * dx + dy * dy).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Entity, Render, Subscription, TestAppContext, VisualTestContext};
    use std::cell::RefCell;
    use std::rc::Rc;

    fn at(angle: f32, radius: f32) -> Point<Pixels> {
        point(px(radius * angle.cos()), px(radius * angle.sin()))
    }

    fn slots(count: usize) -> Vec<WheelSlot> {
        (0..count)
            .map(|index| WheelSlot {
                id: format!("slot-{index}").into(),
                label: format!("Slot {index}").into(),
                tone: WheelTone::Structural,
                enabled: true,
            })
            .collect()
    }

    #[test]
    fn the_deadzone_is_in_the_hollow_centre() {
        let centre = point(px(100.), px(100.));
        assert_eq!(
            slot_at(centre, point(px(100.), px(100.)), 4),
            WheelHit::Deadzone
        );
        assert_eq!(
            slot_at(centre, point(px(100.), px(110.)), 4),
            WheelHit::Deadzone,
            "inside the inner radius"
        );
        assert_eq!(
            slot_at(centre, point(px(100.), px(180.)), 4),
            WheelHit::Outside,
            "beyond the outer radius"
        );
    }

    #[test]
    fn four_slots_are_north_east_south_west() {
        let centre = point(px(0.), px(0.));
        let middle = (INNER_RADIUS + OUTER_RADIUS) / 2.0;
        assert_eq!(
            slot_at(centre, at(-FRAC_PI_2, middle), 4),
            WheelHit::Slot(0)
        );
        assert_eq!(slot_at(centre, at(0.0, middle), 4), WheelHit::Slot(1));
        assert_eq!(slot_at(centre, at(FRAC_PI_2, middle), 4), WheelHit::Slot(2));
        assert_eq!(slot_at(centre, at(PI, middle), 4), WheelHit::Slot(3));
    }

    #[test]
    fn two_slots_put_the_second_at_south() {
        let centre = point(px(0.), px(0.));
        let middle = (INNER_RADIUS + OUTER_RADIUS) / 2.0;
        assert_eq!(
            slot_at(centre, at(-FRAC_PI_2, middle), 2),
            WheelHit::Slot(0)
        );
        assert_eq!(slot_at(centre, at(FRAC_PI_2, middle), 2), WheelHit::Slot(1));
    }

    #[test]
    fn the_slot_centres_sit_on_the_label_ring() {
        let north = slot_offset(0, 4);
        assert!(north.x.0.abs() < 1e-4);
        assert!((north.y.0 + LABEL_RADIUS).abs() < 1e-4);
        let south = slot_offset(2, 4);
        assert!(south.x.0.abs() < 1e-4);
        assert!((south.y.0 - LABEL_RADIUS).abs() < 1e-4);
    }

    struct Harness {
        slots: Vec<WheelSlot>,
        hovered: Option<usize>,
    }

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            wheel("wheel-test", &self.slots, self.hovered)
        }
    }

    fn menu_slots() -> Vec<WheelSlot> {
        vec![
            WheelSlot {
                id: "move".into(),
                label: "Move".into(),
                tone: WheelTone::Structural,
                enabled: true,
            },
            WheelSlot {
                id: "delete".into(),
                label: "Delete".into(),
                tone: WheelTone::Destructive,
                enabled: true,
            },
        ]
    }

    fn open_menu(
        cx: &mut TestAppContext,
    ) -> (
        VisualTestContext,
        Entity<WheelMenu>,
        Rc<RefCell<Vec<WheelEvent>>>,
        Subscription,
    ) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_window, cx| cx.new(WheelMenu::new))
                .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        let wheel: Entity<WheelMenu> = window.root(&mut visual).unwrap();
        let events: Rc<RefCell<Vec<WheelEvent>>> = Rc::default();
        let sink = events.clone();
        let subscription = visual.update(|_window, cx| {
            cx.subscribe(&wheel, move |_wheel, event: &WheelEvent, _cx| {
                sink.borrow_mut().push(event.clone());
            })
        });
        (visual, wheel, events, subscription)
    }

    /// The sidecar owns the gesture: a quick release latches, a flick commits, a
    /// hovered wedge commits on click, and a long hold cancels.
    #[gpui::test]
    fn the_sidecar_latches_flicks_and_reports_events(cx: &mut TestAppContext) {
        let center = point(px(200.), px(200.));
        let (mut visual, wheel, events, _subscription) = open_menu(cx);
        let south = {
            let offset = slot_offset(1, 2);
            point(center.x + offset.x, center.y + offset.y)
        };

        // A quick tap in the deadzone latches (the trackpad case).
        wheel.update(&mut visual, |wheel, cx| {
            wheel.open_armed(center, menu_slots(), cx);
            wheel.pointer_up(center, cx);
        });
        assert!(wheel.read_with(&visual, |wheel, _| wheel.is_latched()));
        assert!(events.borrow().is_empty(), "a tap does not choose");

        // Move south and click: the sidecar reports the id.
        wheel.update(&mut visual, |wheel, cx| {
            wheel.pointer_move(south, cx);
        });
        assert_eq!(
            wheel.read_with(&visual, |wheel, _| wheel.hovered()),
            Some(1)
        );
        wheel.update(&mut visual, |wheel, cx| wheel.click(south, cx));
        assert!(!wheel.read_with(&visual, |wheel, _| wheel.is_open()));
        assert_eq!(
            events.borrow().last(),
            Some(&WheelEvent::Selected("delete".into()))
        );

        // A flick commits on release without a second click.
        events.borrow_mut().clear();
        wheel.update(&mut visual, |wheel, cx| {
            wheel.open_armed(center, menu_slots(), cx);
            wheel.pointer_move(south, cx);
            wheel.pointer_up(south, cx);
        });
        assert_eq!(
            events.borrow().last(),
            Some(&WheelEvent::Selected("delete".into()))
        );

        // A long hold released in the deadzone cancels with a Closed event.
        events.borrow_mut().clear();
        wheel.update(&mut visual, |wheel, cx| {
            wheel.open_armed(center, menu_slots(), cx);
        });
        std::thread::sleep(std::time::Duration::from_millis(220));
        wheel.update(&mut visual, |wheel, cx| wheel.pointer_up(center, cx));
        assert_eq!(events.borrow().last(), Some(&WheelEvent::Closed));
    }

    /// The keyboard path opens latched; a host key commits the hovered wedge.
    #[gpui::test]
    fn the_sidecar_commits_a_hovered_wedge(cx: &mut TestAppContext) {
        let center = point(px(200.), px(200.));
        let (mut visual, wheel, events, _subscription) = open_menu(cx);
        let north = {
            let offset = slot_offset(0, 2);
            point(center.x + offset.x, center.y + offset.y)
        };
        wheel.update(&mut visual, |wheel, cx| {
            wheel.open_latched(center, menu_slots(), cx);
            wheel.pointer_move(north, cx);
        });
        assert!(wheel.read_with(&visual, |wheel, _| wheel.is_latched()));
        let committed = wheel.update(&mut visual, |wheel, cx| wheel.commit_hovered(cx));
        assert!(committed);
        assert_eq!(
            events.borrow().last(),
            Some(&WheelEvent::Selected("move".into()))
        );
    }

    #[gpui::test]
    fn the_wheel_renders_its_labels_and_paints(cx: &mut TestAppContext) {
        let harness = Harness {
            slots: slots(4),
            hovered: Some(2),
        };
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), move |_window, cx| cx.new(|_| harness))
                .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });

        assert!(visual.debug_bounds("wheel-test-wheel").is_some());
        for index in 0..4 {
            let selector = match index {
                0 => "wheel-test-label-slot-0",
                1 => "wheel-test-label-slot-1",
                2 => "wheel-test-label-slot-2",
                _ => "wheel-test-label-slot-3",
            };
            assert!(
                visual.debug_bounds(selector).is_some(),
                "{selector} is placed"
            );
        }
    }
}
