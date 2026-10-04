//! A scrollable text area with a read-only prefix and an editable tail.
//!
//! This is the console's element. The buffer ([`TextBuffer`]) holds the whole
//! transcript; everything before `read_only_len` is drawn muted and cannot be
//! edited, and the tail after it is the live input. `Enter` emits a
//! [`SubmitEvent`] with that tail; the owner runs it, appends the result and a
//! fresh prompt with [`TextAreaState::set_transcript`], and the caret returns to
//! the new editable region.
//!
//! The editable tail carries no newlines (paste is sanitised), so it always
//! lives on the last line. That keeps the rendering and hit-testing simple: every
//! line but the last is read-only, and the last line is a muted prefix run plus a
//! normal editable run.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, Font, GlobalElementId,
    IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PaintQuad, Pixels, Point, Render, ScrollHandle, ShapedLine, SharedString, Style, TextAlign,
    TextRun, UTF16Selection, UnderlineStyle, Window, div, fill, point, prelude::*, px, relative,
    rgb, rgba, size,
};

use crate::text::{SubmitEvent, TextBuffer};

const AREA_BG: u32 = 0x0d0f12;
const READ_ONLY: u32 = 0xb6bcc6;
const EDITABLE: u32 = 0xe8ebef;
const CARET: u32 = 0x7aa2f7;
const SELECTION: u32 = 0x3311ff30;

/// Monospace-ish line box; a constant so layout, painting and hit-testing agree.
const LINE_HEIGHT: Pixels = px(18.);
const PAD_Y: Pixels = px(4.);
const PAD_X: Pixels = px(8.);

/// The backing state of the console text area.
pub struct TextAreaState {
    buffer: TextBuffer,
    focus_handle: FocusHandle,
    scroll: ScrollHandle,
    // Geometry captured during the last paint, used for hit-testing.
    last_bounds: Option<Bounds<Pixels>>,
    last_layout: Option<ShapedLine>,
    last_line_start: usize,
    is_selecting: bool,
}

impl EventEmitter<SubmitEvent> for TextAreaState {}

impl Focusable for TextAreaState {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TextAreaState {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            buffer: TextBuffer::new(),
            focus_handle: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            last_bounds: None,
            last_layout: None,
            last_line_start: 0,
            is_selecting: false,
        }
    }

    /// Replace the whole transcript. Everything is read-only; the editable tail
    /// starts empty at the end and the view scrolls to it.
    pub fn set_transcript(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.buffer.set_content(text);
        let end = self.buffer.content().len();
        self.buffer.set_read_only_len(end);
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        let range = self.buffer.selected_range().clone();
        if !range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.buffer.content()[range].to_string(),
            ));
        }
    }

    fn cut(&mut self, cx: &mut Context<Self>) {
        let range = self.buffer.selected_range().clone();
        if !range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.buffer.content()[range.clone()].to_string(),
            ));
            self.buffer.replace_range(range, "");
            cx.notify();
        }
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            // Keep the editable tail on one line.
            self.buffer.insert(&text.replace('\n', " "));
            cx.notify();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let secondary = keystroke.modifiers.platform || keystroke.modifiers.control;
        let handled = match keystroke.key.as_str() {
            "enter" | "return" => {
                cx.emit(SubmitEvent {
                    text: self.buffer.editable().to_string(),
                });
                true
            }
            "backspace" => {
                if !self.buffer.backspace() {
                    window.play_system_bell();
                }
                cx.notify();
                true
            }
            "delete" => {
                if !self.buffer.delete() {
                    window.play_system_bell();
                }
                cx.notify();
                true
            }
            "left" => {
                if self.buffer.selected_range().is_empty() {
                    let cursor = self.buffer.cursor_offset();
                    let prev = self.buffer.previous_boundary(cursor);
                    self.buffer.move_to(prev);
                } else {
                    let start = self.buffer.selected_range().start;
                    self.buffer.move_to(start);
                }
                cx.notify();
                true
            }
            "right" => {
                if self.buffer.selected_range().is_empty() {
                    let cursor = self.buffer.cursor_offset();
                    let next = self.buffer.next_boundary(cursor);
                    self.buffer.move_to(next);
                } else {
                    let end = self.buffer.selected_range().end;
                    self.buffer.move_to(end);
                }
                cx.notify();
                true
            }
            "home" => {
                // The start of the editable region, not the start of the line:
                // the prefix is read-only.
                let start = self.buffer.read_only_len();
                self.buffer.move_to(start);
                cx.notify();
                true
            }
            "end" => {
                let end = self.buffer.content().len();
                self.buffer.move_to(end);
                cx.notify();
                true
            }
            "a" if secondary => {
                self.buffer.select_all();
                cx.notify();
                true
            }
            "c" if secondary => {
                self.copy(cx);
                true
            }
            "x" if secondary => {
                self.cut(cx);
                true
            }
            "v" if secondary => {
                self.paste(cx);
                true
            }
            _ => false,
        };

        if handled {
            cx.stop_propagation();
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        self.is_selecting = true;
        let offset = self.index_for_mouse_position(event.position);
        if event.modifiers.shift {
            self.buffer.select_to(offset);
        } else {
            self.buffer.move_to(offset);
        }
        cx.notify();
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _cx: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            let offset = self.index_for_mouse_position(event.position);
            self.buffer.select_to(offset);
            cx.notify();
        }
    }

    /// Map a window point to a byte offset. Clicks on read-only lines put the
    /// caret at the start of the editable region.
    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let read_only_len = self.buffer.read_only_len();
        let Some(bounds) = self.last_bounds else {
            return read_only_len;
        };
        if position.y < bounds.top() {
            return read_only_len;
        }
        let y = position.y - bounds.top() - PAD_Y;
        let line = (y / LINE_HEIGHT).floor().max(0.0) as usize;
        let line_count = self.buffer.content().split('\n').count().max(1);
        if line + 1 < line_count {
            return read_only_len;
        }
        let Some(layout) = self.last_layout.as_ref() else {
            return read_only_len;
        };
        let x = position.x - bounds.left() - PAD_X;
        self.last_line_start + layout.closest_index_for_x(x)
    }
}

impl EntityInputHandler for TextAreaState {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.buffer.range_from_utf16(&range_utf16);
        actual_range.replace(self.buffer.range_to_utf16(&range));
        Some(self.buffer.content()[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.buffer.range_to_utf16(self.buffer.selected_range()),
            reversed: self.buffer.selection_reversed(),
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.buffer
            .marked_range()
            .map(|range| self.buffer.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.buffer.set_marked_range(None);
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.buffer.range_from_utf16(range_utf16))
            .or_else(|| self.buffer.marked_range().cloned())
            .unwrap_or_else(|| self.buffer.selected_range().clone());
        self.buffer.replace_range(range, new_text);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.buffer.range_from_utf16(range_utf16))
            .or_else(|| self.buffer.marked_range().cloned())
            .unwrap_or_else(|| self.buffer.selected_range().clone());
        let selection = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.buffer.range_from_utf16(range_utf16));
        self.buffer.replace_and_mark(range, new_text, selection);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.last_layout.as_ref()?;
        let range = self.buffer.range_from_utf16(&range_utf16);
        let start = range.start.saturating_sub(self.last_line_start);
        let end = range.end.saturating_sub(self.last_line_start);
        Some(Bounds::from_corners(
            point(
                bounds.left() + PAD_X + layout.x_for_index(start),
                bounds.top(),
            ),
            point(
                bounds.left() + PAD_X + layout.x_for_index(end),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let layout = self.last_layout.as_ref()?;
        let x = point.x - bounds.left() - PAD_X;
        let idx = layout.index_for_x(x)?;
        Some(self.buffer.offset_to_utf16(self.last_line_start + idx))
    }
}

impl Render for TextAreaState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("console-textarea")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .bg(rgb(AREA_BG))
            .text_size(px(13.))
            .line_height(LINE_HEIGHT)
            .cursor(CursorStyle::IBeam)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(TextAreaElement { state: cx.entity() })
    }
}

struct LineLayout {
    line: ShapedLine,
    start: usize,
}

struct PrepaintState {
    lines: Vec<LineLayout>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    last_layout: Option<ShapedLine>,
    last_line_start: usize,
}

struct TextAreaElement {
    state: Entity<TextAreaState>,
}

impl IntoElement for TextAreaElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextAreaElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let lines = self.state.read(cx).buffer.content().split('\n').count().max(1);
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = (LINE_HEIGHT * lines as f32 + PAD_Y * 2.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let state = self.state.read(cx);
        let content = state.buffer.content().to_string();
        let read_only_len = state.buffer.read_only_len();
        let selected = state.buffer.selected_range().clone();
        let cursor = state.buffer.cursor_offset();
        let marked = state.buffer.marked_range().cloned();
        let style = window.text_style();
        let font = style.font();
        let font_size = style.font_size.to_pixels(window.rem_size());

        let part_count = content.split('\n').count();
        let mut lines = Vec::with_capacity(part_count);
        let mut offset = 0usize;
        for (i, part) in content.split('\n').enumerate() {
            let is_last = i + 1 == part_count;
            let start = offset;
            let runs = line_runs(part, read_only_len - start, marked.as_ref(), start, is_last, &font);
            let line = window
                .text_system()
                .shape_line(part.to_string().into(), font_size, &runs, None);
            lines.push(LineLayout { line, start });
            offset = start + part.len() + 1;
        }

        let last = lines.last().expect("at least one line");
        let last_layout = last.line.clone();
        let last_line_start = last.start;
        let last_top = bounds.top() + PAD_Y + (part_count as f32 - 1.) * LINE_HEIGHT;
        let last_bottom = last_top + LINE_HEIGHT;

        let caret_in_line = cursor.saturating_sub(last_line_start);
        let cursor_quad = fill(
            Bounds::new(
                point(
                    bounds.left() + PAD_X + last.line.x_for_index(caret_in_line),
                    last_top,
                ),
                size(px(2.), LINE_HEIGHT),
            ),
            rgb(CARET),
        );

        let selection_quad = if selected.is_empty() {
            None
        } else {
            let s = selected.start.saturating_sub(last_line_start);
            let e = selected.end.saturating_sub(last_line_start);
            Some(fill(
                Bounds::from_corners(
                    point(bounds.left() + PAD_X + last.line.x_for_index(s), last_top),
                    point(bounds.left() + PAD_X + last.line.x_for_index(e), last_bottom),
                ),
                rgba(SELECTION),
            ))
        };

        PrepaintState {
            lines,
            cursor: Some(cursor_quad),
            selection: selection_quad,
            last_layout: Some(last_layout),
            last_line_start,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.state.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.state.clone()),
            cx,
        );

        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }

        for (i, layout) in prepaint.lines.iter().enumerate() {
            let origin = point(
                bounds.left() + PAD_X,
                bounds.top() + PAD_Y + i as f32 * LINE_HEIGHT,
            );
            layout
                .line
                .paint(origin, LINE_HEIGHT, TextAlign::Left, None, window, cx)
                .unwrap();
        }

        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        self.state.update(cx, |state, _cx| {
            state.last_layout = prepaint.last_layout.take();
            state.last_line_start = prepaint.last_line_start;
            state.last_bounds = Some(bounds);
        });
    }
}

/// Build the text runs for one line. The last line splits into a muted read-only
/// prefix and a normal editable tail; the IME marked range is underlined.
fn line_runs(
    part: &str,
    prefix_len: usize,
    marked: Option<&Range<usize>>,
    start: usize,
    is_last: bool,
    font: &Font,
) -> Vec<TextRun> {
    let run = |len: usize, color: u32, underline: bool| TextRun {
        len,
        font: font.clone(),
        color: rgb(color).into(),
        background_color: None,
        underline: underline.then_some(UnderlineStyle {
            color: Some(rgb(color).into()),
            thickness: px(1.0),
            wavy: false,
        }),
        strikethrough: None,
    };

    if !is_last {
        return vec![run(part.len(), READ_ONLY, false)];
    }

    let prefix_len = prefix_len.min(part.len());
    let mut bounds = vec![0usize, prefix_len, part.len()];
    if let Some(marked) = marked {
        let ms = marked.start.saturating_sub(start).min(part.len());
        let me = marked.end.saturating_sub(start).min(part.len());
        bounds.push(ms);
        bounds.push(me);
    }
    bounds.sort_unstable();
    bounds.dedup();

    let mut runs = Vec::new();
    for window in bounds.windows(2) {
        let (a, b) = (window[0], window[1]);
        if a >= b {
            continue;
        }
        let color = if b <= prefix_len { READ_ONLY } else { EDITABLE };
        let underlined = marked.is_some_and(|marked| {
            let ms = marked.start.saturating_sub(start);
            let me = marked.end.saturating_sub(start);
            a >= ms && b <= me
        });
        runs.push(run(b - a, color, underlined));
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Render, TestAppContext, VisualTestContext};

    /// A host that shows only the text area, for the component tests.
    struct AreaHost {
        area: Entity<TextAreaState>,
    }

    impl Render for AreaHost {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().flex().flex_col().child(self.area.clone())
        }
    }

    fn open(cx: &mut TestAppContext) -> (VisualTestContext, Entity<TextAreaState>) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                let area = cx.new(|cx| {
                    let mut area = TextAreaState::new(cx);
                    area.set_transcript("welcome\n>>> ", cx);
                    area
                });
                window.focus(&area.read(cx).focus_handle(cx), cx);
                cx.new(|_| AreaHost { area })
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let host: Entity<AreaHost> = window.root(&mut cx).unwrap();
        let area = host.read_with(&cx, |host, _| host.area.clone());
        (cx, area)
    }

    fn content(area: &Entity<TextAreaState>, cx: &VisualTestContext) -> String {
        area.read_with(cx, |area, _| area.buffer.content().to_string())
    }

    #[gpui::test]
    fn typing_appends_after_the_read_only_prompt(cx: &mut TestAppContext) {
        let (mut cx, area) = open(cx);

        cx.simulate_input("1 + 1");
        assert_eq!(content(&area, &cx), "welcome\n>>> 1 + 1");

        // Backspace past the prompt cannot delete it.
        cx.simulate_keystrokes("home");
        cx.simulate_keystrokes("backspace backspace");
        assert_eq!(content(&area, &cx), "welcome\n>>> 1 + 1");
    }

    #[gpui::test]
    fn enter_emits_the_editable_tail(cx: &mut TestAppContext) {
        use std::cell::RefCell;
        use std::rc::Rc;

        let (mut cx, area) = open(cx);

        let seen: Rc<RefCell<Vec<String>>> = Rc::default();
        let sink = seen.clone();
        let _subscription = cx.update(|_window, cx| {
            cx.subscribe(&area, move |_emitter, event: &SubmitEvent, _cx| {
                sink.borrow_mut().push(event.text.clone());
            })
        });

        cx.simulate_input("len(doc.Objects)");
        cx.simulate_keystrokes("enter");

        assert_eq!(seen.borrow().as_slice(), ["len(doc.Objects)"]);
        // The Enter did not insert a newline.
        assert_eq!(content(&area, &cx), "welcome\n>>> len(doc.Objects)");
    }

    #[gpui::test]
    fn set_transcript_clears_the_editable_tail(cx: &mut TestAppContext) {
        let (mut cx, area) = open(cx);
        cx.update(|_window, cx| {
            area.update(cx, |area, cx| area.set_transcript("welcome\n>>> 3\n>>> ", cx));
        });
        assert_eq!(
            content(&area, &cx),
            "welcome\n>>> 3\n>>> "
        );
    }
}
