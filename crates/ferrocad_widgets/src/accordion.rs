//! A vertical accordion: titled sections whose bodies give way to headers when the
//! stack runs out of height.
//!
//! The widget owns presentation and the *policy* of when a body is painted; the host
//! owns the open section and every body's content. Three layouts come out of
//! [`resolve_layout`]:
//!
//! * [`AccordionLayout::All`] — every body fits at its preferred height, so every
//!   body is painted. This is the default until the user clicks a header.
//! * [`AccordionLayout::Expanded(Some(i))`] — the stack is too short: section `i`
//!   absorbs the leftover height and every other body is hidden under its header.
//! * [`AccordionLayout::Expanded(None)`] — the user collapsed the open section, so
//!   only headers are painted.
//!
//! The first click on a header switches the state to **manual**: from then on the
//! open section is the only body, even in a window tall enough for all of them. That
//! keeps the control predictable — an automatic re-expansion that undid a deliberate
//! collapse would be worse than one extra click.
//!
//! The collapsed header is the only affordance left, so it carries the section title,
//! a chevron and (optionally) a one-line summary the host supplies.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, IntoElement, Pixels, SharedString, Window, div, prelude::*, px,
    rgb, rgba,
};

use ferrocad_theme::ui::{ACTIVE_ACCENT, FG, MUTED, PANEL, PANEL_ALT, ROW_HOVER};

/// The height of a header row, including its padding.
pub const HEADER_HEIGHT: Pixels = px(26.);

/// One titled section.
#[derive(Clone, Debug, PartialEq)]
pub struct AccordionSection {
    /// A stable id used in the debug selectors (`{key}-header-{id}`). It is not
    /// interpreted by the widget.
    pub id: SharedString,
    pub title: SharedString,
    /// A short value shown right-aligned on the header (a count, a state word).
    pub summary: Option<SharedString>,
    /// The height this body wants when every section is expanded. It is a *floor*:
    /// in [`AccordionLayout::All`] the bodies share any extra height.
    pub preferred_height: Pixels,
}

impl AccordionSection {
    /// A section with no summary.
    pub fn new(
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        preferred_height: Pixels,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            summary: None,
            preferred_height,
        }
    }

    /// Attach the right-aligned header value.
    pub fn with_summary(mut self, summary: impl Into<SharedString>) -> Self {
        self.summary = Some(summary.into());
        self
    }
}

/// Which section is expanded, and whether the user has taken the choice over.
///
/// `open` is the section expanded when the stack is too short. It is also the one a
/// click opens; the default (`Default::default()`) has every body collapsed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AccordionState {
    open: Option<usize>,
    manual: bool,
}

impl AccordionState {
    /// A state whose section `open` expands as soon as the stack is too short.
    pub fn new(open: usize) -> Self {
        Self {
            open: Some(open),
            manual: false,
        }
    }

    /// The section that is (or would be) expanded.
    pub fn open(&self) -> Option<usize> {
        self.open
    }

    /// Whether the user has clicked a header, which stops the auto-expansion.
    pub fn is_manual(&self) -> bool {
        self.manual
    }

    /// Expand `index`, taking the choice over from the auto-expansion.
    pub fn open_section(&mut self, index: usize) {
        self.open = Some(index);
        self.manual = true;
    }

    /// Click a header: expand that section, or collapse it when it is already open.
    pub fn toggle(&mut self, index: usize) {
        self.open = if self.open == Some(index) {
            None
        } else {
            Some(index)
        };
        self.manual = true;
    }
}

/// The layout [`resolve_layout`] picks for a stack in `available` height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccordionLayout {
    /// Every body fits at its preferred height.
    All,
    /// One body absorbs the remaining height; `None` paints headers only.
    Expanded(Option<usize>),
}

/// The height the stack needs when every body is at its preferred size.
pub fn preferred_height(sections: &[AccordionSection]) -> Pixels {
    sections.iter().fold(px(0.), |total, section| {
        total + HEADER_HEIGHT + section.preferred_height
    })
}

/// Pick the layout for `available` height.
///
/// The auto-expansion applies only while the user has not touched a header
/// ([`AccordionState::is_manual`]); a manual choice is always honoured, even when the
/// window is tall enough for every body.
pub fn resolve_layout(
    sections: &[AccordionSection],
    available: Pixels,
    state: &AccordionState,
) -> AccordionLayout {
    if !state.is_manual() && preferred_height(sections) <= available {
        return AccordionLayout::All;
    }
    AccordionLayout::Expanded(state.open().filter(|index| *index < sections.len()))
}

/// Render the accordion.
///
/// `key` namespaces the debug selectors: `{key}-accordion`, `{key}-header-{id}` and
/// `{key}-body-{id}`. `bodies` is one element per section, in order; a body is only
/// painted for a section the layout expands. `on_toggle` receives the clicked
/// section's index — the widget does not own the state, so the caller decides what a
/// click means and re-renders.
pub fn accordion(
    key: &'static str,
    sections: Vec<AccordionSection>,
    bodies: Vec<AnyElement>,
    state: AccordionState,
    available: Pixels,
    on_toggle: impl Fn(usize, &ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let on_toggle: Rc<dyn Fn(usize, &ClickEvent, &mut Window, &mut App)> = Rc::new(on_toggle);
    let layout = resolve_layout(&sections, available, &state);
    let mut bodies: Vec<Option<AnyElement>> = bodies.into_iter().map(Some).collect();

    let mut stack = div()
        .id(SharedString::from(format!("{key}-accordion")))
        .debug_selector(move || format!("{key}-accordion"))
        .flex()
        .flex_col()
        .size_full()
        .min_h_0()
        .overflow_hidden()
        .bg(rgb(PANEL));

    for (index, section) in sections.into_iter().enumerate() {
        let open = match layout {
            AccordionLayout::All => true,
            AccordionLayout::Expanded(open) => open == Some(index),
        };

        let header_id = format!("{key}-header-{}", section.id);
        let header_selector = header_id.clone();
        let on_toggle = on_toggle.clone();
        let mut header = div()
            .id(SharedString::from(header_id))
            .debug_selector(move || header_selector)
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .h(HEADER_HEIGHT)
            .flex_shrink_0()
            .px_2()
            .bg(rgb(PANEL_ALT))
            .text_xs()
            .cursor_pointer()
            .hover(|style| style.bg(rgba(ROW_HOVER)))
            .on_click(move |event, window, cx| on_toggle(index, event, window, cx))
            .child(
                div()
                    .text_color(rgb(if open { ACTIVE_ACCENT } else { MUTED }))
                    .child(if open { "\u{25be}" } else { "\u{25b8}" }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(rgb(if open { FG } else { MUTED }))
                    .child(section.title.clone()),
            );
        if let Some(summary) = section.summary.clone() {
            header = header.child(div().text_color(rgb(MUTED)).child(summary));
        }
        stack = stack.child(header);

        if !open {
            continue;
        }
        let Some(body) = bodies.get_mut(index).and_then(Option::take) else {
            continue;
        };
        let body_id = format!("{key}-body-{}", section.id);
        let body_selector = body_id.clone();
        let mut body = div()
            .id(SharedString::from(body_id))
            .debug_selector(move || body_selector)
            .flex()
            .flex_col()
            .min_h_0()
            .overflow_hidden()
            .child(body);
        body = match layout {
            // The preferred height is a floor; the bodies share the leftover space.
            AccordionLayout::All => body.flex_1().min_h(section.preferred_height),
            AccordionLayout::Expanded(_) => body.flex_1(),
        };
        stack = stack.child(body);
    }

    stack
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui::{Context, Render};
    use gpui::{Modifiers, TestAppContext, VisualTestContext, Window, px};

    use super::*;

    fn section(id: &str, preferred: f32) -> AccordionSection {
        AccordionSection::new(id, format!("Section {id}"), px(preferred))
    }

    #[test]
    fn preferred_height_sums_headers_and_bodies() {
        let sections = [section("a", 100.), section("b", 50.)];
        assert_eq!(preferred_height(&sections), px(2. * 26. + 150.));
    }

    #[test]
    fn a_tall_stack_shows_every_body() {
        let sections = [section("a", 100.), section("b", 50.)];
        let state = AccordionState::new(0);
        assert_eq!(
            resolve_layout(&sections, px(300.), &state),
            AccordionLayout::All
        );
    }

    #[test]
    fn a_short_stack_expands_only_the_open_section() {
        let sections = [section("a", 100.), section("b", 50.)];
        let state = AccordionState::new(1);
        assert_eq!(
            resolve_layout(&sections, px(100.), &state),
            AccordionLayout::Expanded(Some(1))
        );
    }

    #[test]
    fn a_collapsed_stack_shows_headers_only() {
        let sections = [section("a", 100.)];
        assert_eq!(
            resolve_layout(&sections, px(10.), &AccordionState::default()),
            AccordionLayout::Expanded(None)
        );
    }

    #[test]
    fn a_click_opens_a_section_and_a_second_click_closes_it() {
        let mut state = AccordionState::default();
        state.toggle(2);
        assert_eq!(state.open(), Some(2));
        assert!(state.is_manual(), "a click takes the choice over");
        state.toggle(2);
        assert_eq!(state.open(), None);
    }

    #[test]
    fn a_manual_choice_survives_a_tall_window() {
        let sections = [section("a", 10.), section("b", 10.)];
        let mut state = AccordionState::default();
        state.toggle(1);
        assert_eq!(
            resolve_layout(&sections, px(1000.), &state),
            AccordionLayout::Expanded(Some(1)),
            "a deliberate collapse is not undone by a resize"
        );
    }

    #[test]
    fn an_out_of_range_open_index_expands_nothing() {
        let sections = [section("a", 10.)];
        let state = AccordionState::new(4);
        assert_eq!(
            resolve_layout(&sections, px(10.), &state),
            AccordionLayout::Expanded(None)
        );
    }

    struct Harness {
        state: AccordionState,
        toggled: Rc<RefCell<Vec<usize>>>,
    }

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let sections = vec![section("first", 100.), section("second", 100.)];
            let bodies = sections
                .iter()
                .map(|section| div().child(section.title.clone()).into_any_element())
                .collect();
            let toggled = self.toggled.clone();
            let state = self.state;
            accordion(
                "accordion-test",
                sections,
                bodies,
                state,
                // Too short for both bodies: only the open one may paint.
                px(80.),
                move |index, _event, _window, _cx| toggled.borrow_mut().push(index),
            )
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    #[gpui::test]
    fn a_short_accordion_paints_its_headers_and_one_body(cx: &mut TestAppContext) {
        let harness = Harness {
            state: AccordionState::new(0),
            toggled: Rc::default(),
        };
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), move |_window, cx| cx.new(|_| harness))
                .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        draw(&mut visual);

        for selector in [
            "accordion-test-accordion",
            "accordion-test-header-first",
            "accordion-test-header-second",
            "accordion-test-body-first",
        ] {
            assert!(
                visual.debug_bounds(selector).is_some(),
                "{selector} is rendered"
            );
        }
        assert!(
            visual.debug_bounds("accordion-test-body-second").is_none(),
            "a collapsed section has no body"
        );
    }

    #[gpui::test]
    fn clicking_a_header_reports_its_index(cx: &mut TestAppContext) {
        let harness = Harness {
            state: AccordionState::new(0),
            toggled: Rc::default(),
        };
        let toggled = harness.toggled.clone();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), move |_window, cx| cx.new(|_| harness))
                .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        draw(&mut visual);

        let bounds = visual
            .debug_bounds("accordion-test-header-second")
            .expect("the second header is rendered");
        visual.simulate_click(bounds.center(), Modifiers::default());
        assert_eq!(&*toggled.borrow(), &[1]);
    }
}
