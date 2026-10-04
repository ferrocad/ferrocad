//! A scrollable text area with a read-only prefix and an editable tail.
//!
//! This is the console's element. The buffer ([`TextBuffer`]) holds the whole
//! transcript; everything before `read_only_len` is drawn muted and cannot be
//! edited, and the tail after it is the live input. The tail may contain
//! newlines: `Shift+Enter` inserts one and a pasted snippet keeps its own. `Enter`
//! emits a [`SubmitEvent`] with the whole tail; the owner runs it and rebuilds the
//! transcript with [`TextAreaState::set_transcript`].
//!
//! Rendering walks the content line by line. A line is split into a muted
//! read-only run and a normal editable run at `read_only_len`, and the caret and
//! the selection are drawn only inside the editable region.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, Font, GlobalElementId,
    IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PaintQuad, Pixels, Point, Render, ScrollHandle, ShapedLine, SharedString, Style, TextAlign,
    TextRun, UTF16Selection, UnderlineStyle, Window, div, fill, point, prelude::*, px, relative,
    rgb, rgba, size,
};

use crate::events::SubmitEvent;
use crate::text_buffer::TextBuffer;

const AREA_BG: u32 = 0x0d0f12;
const READ_ONLY: u32 = 0xb6bcc6;
const EDITABLE: u32 = 0xe8ebef;
const CARET: u32 = 0x7aa2f7;
const SELECTION: u32 = 0x3311ff30;

/// Monospace-ish line box; a constant so layout, painting and hit-testing agree.
const LINE_HEIGHT: Pixels = px(18.);
const PAD_Y: Pixels = px(4.);
const PAD_X: Pixels = px(8.);

struct LineLayout {
    start: usize,
    line: ShapedLine,
}

/// The backing state of the console text area.
pub struct TextAreaState {
    buffer: TextBuffer,
    focus_handle: FocusHandle,
    scroll: ScrollHandle,
    // Geometry captured during the last paint, used for hit-testing.
    last_bounds: Option<Bounds<Pixels>>,
    last_lines: Vec<LineLayout>,
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
            last_lines: Vec::new(),
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
            // A pasted snippet keeps its newlines; it is code to edit and run.
            self.buffer.insert(&text);
            cx.notify();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let secondary = keystroke.modifiers.platform || keystroke.modifiers.control;
        let handled = match keystroke.key.as_str() {
            "enter" | "return" => {
                if keystroke.modifiers.shift {
                    // Shift+Enter continues the snippet on a new line.
                    self.buffer.insert("\n");
                    cx.notify();
                } else {
                    cx.emit(SubmitEvent {
                        text: self.buffer.editable().to_string(),
                    });
                }
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
            "up" => {
                self.move_line(-1);
                cx.notify();
                true
            }
            "down" => {
                self.move_line(1);
                cx.notify();
                true
            }
            "home" => {
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

    /// Move the caret one visual line up or down, keeping the column if the target
    /// line is long enough.
    fn move_line(&mut self, delta: i32) {
        let cursor = self.buffer.cursor_offset();
        let line_range = self.buffer.line_range(cursor);
        if delta < 0 {
            if line_range.start == self.buffer.read_only_len() {
                return;
            }
            let prev_line = self.buffer.line_range(line_range.start.saturating_sub(1));
            let column = cursor - line_range.start;
            let target = (prev_line.start + column).min(prev_line.end);
            self.buffer.move_to(target);
        } else {
            let end = self.buffer.content().len();
            if line_range.end >= end {
                return;
            }
            let next_line = self.buffer.line_range(line_range.end + 1);
            let column = cursor - line_range.start;
            let target = (next_line.start + column).min(next_line.end);
            self.buffer.move_to(target);
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
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
        if self.last_lines.is_empty() {
            return read_only_len;
        }
        let y = position.y - bounds.top() - PAD_Y;
        let line_index = (y / LINE_HEIGHT).floor().max(0.0) as usize;
        let line_index = line_index.min(self.last_lines.len() - 1);
        let line = &self.last_lines[line_index];
        let x = position.x - bounds.left() - PAD_X;
        line.start + line.line.closest_index_for_x(x)
    }

    /// The last rendered line whose start is at or before `offset`.
    fn line_for_offset(&self, offset: usize) -> Option<(usize, &LineLayout)> {
        let mut found = None;
        for (index, line) in self.last_lines.iter().enumerate() {
            if offset >= line.start {
                found = Some((index, line));
            }
        }
        found
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
        let range = self.buffer.range_from_utf16(&range_utf16);
        let (index, line) = self.line_for_offset(range.start)?;
        let top = bounds.top() + PAD_Y + index as f32 * LINE_HEIGHT;
        let start = range.start.saturating_sub(line.start);
        let end = range.end.saturating_sub(line.start);
        Some(Bounds::from_corners(
            point(
                bounds.left() + PAD_X + line.line.x_for_index(start),
                top,
            ),
            point(bounds.left() + PAD_X + line.line.x_for_index(end), top + LINE_HEIGHT),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        if self.last_lines.is_empty() {
            return None;
        }
        let y = point.y - bounds.top() - PAD_Y;
        let index = ((y / LINE_HEIGHT).floor().max(0.0) as usize).min(self.last_lines.len() - 1);
        let line = &self.last_lines[index];
        let x = point.x - bounds.left() - PAD_X;
        let byte = line.start + line.line.index_for_x(x)?;
        Some(self.buffer.offset_to_utf16(byte))
    }
}

impl Render for TextAreaState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("text-area")
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

struct PrepaintState {
    lines: Vec<LineLayout>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
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

        let mut lines = Vec::new();
        let mut offset = 0usize;
        for part in content.split('\n') {
            let start = offset;
            let runs = line_runs(part, start, read_only_len, marked.as_ref(), &font);
            let line = window
                .text_system()
                .shape_line(part.to_string().into(), font_size, &runs, None);
            lines.push(LineLayout { start, line });
            offset = start + part.len() + 1;
        }

        let line_of = |needle: usize| -> usize {
            let mut found = 0;
            for (index, line) in lines.iter().enumerate() {
                if needle >= line.start {
                    found = index;
                }
            }
            found
        };

        let cursor_line = line_of(cursor);
        let cursor_index = cursor.saturating_sub(lines[cursor_line].start);
        let cursor_top = bounds.top() + PAD_Y + cursor_line as f32 * LINE_HEIGHT;
        let cursor_quad = fill(
            Bounds::new(
                point(
                    bounds.left() + PAD_X + lines[cursor_line].line.x_for_index(cursor_index),
                    cursor_top,
                ),
                size(px(2.), LINE_HEIGHT),
            ),
            rgb(CARET),
        );

        let mut selection = Vec::new();
        if !selected.is_empty() {
            for line in &lines {
                let end = line.start + line.line.text.len();
                let start = selected.start.max(line.start);
                let stop = selected.end.min(end);
                if start >= stop {
                    continue;
                }
                let top = bounds.top() + PAD_Y + line_index(&lines, line.start) as f32 * LINE_HEIGHT;
                selection.push(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + PAD_X + line.line.x_for_index(start - line.start),
                            top,
                        ),
                        point(
                            bounds.left() + PAD_X + line.line.x_for_index(stop - line.start),
                            top + LINE_HEIGHT,
                        ),
                    ),
                    rgba(SELECTION),
                ));
            }
        }

        PrepaintState {
            lines,
            cursor: Some(cursor_quad),
            selection,
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

        for quad in prepaint.selection.drain(..) {
            window.paint_quad(quad);
        }

        for (i, line) in prepaint.lines.iter().enumerate() {
            let origin = point(
                bounds.left() + PAD_X,
                bounds.top() + PAD_Y + i as f32 * LINE_HEIGHT,
            );
            line.line
                .paint(origin, LINE_HEIGHT, TextAlign::Left, None, window, cx)
                .unwrap();
        }

        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        let lines = std::mem::take(&mut prepaint.lines);
        self.state.update(cx, |state, _cx| {
            state.last_bounds = Some(bounds);
            state.last_lines = lines;
        });
    }
}

fn line_index(lines: &[LineLayout], start: usize) -> usize {
    lines.iter().position(|line| line.start == start).unwrap_or(0)
}

/// Build the text runs for one line: a muted read-only part up to `read_only_len`
/// and a normal editable part after it, with the IME marked range underlined.
fn line_runs(
    part: &str,
    start: usize,
    read_only_len: usize,
    marked: Option<&Range<usize>>,
    font: &Font,
) -> Vec<TextRun> {
    let end = start + part.len();
    let rel = |absolute: usize| absolute.clamp(start, end) - start;
    let mut cuts = vec![0usize, part.len(), rel(read_only_len)];
    if let Some(marked) = marked {
        cuts.push(rel(marked.start));
        cuts.push(rel(marked.end));
    }
    cuts.sort_unstable();
    cuts.dedup();

    let mut runs = Vec::new();
    for window in cuts.windows(2) {
        let (a, b) = (window[0], window[1]);
        if a >= b {
            continue;
        }
        let color = if start + a < read_only_len {
            READ_ONLY
        } else {
            EDITABLE
        };
        let underlined = marked.is_some_and(|marked| {
            let ms = marked.start.clamp(start, end);
            let me = marked.end.clamp(start, end);
            start + a >= ms && start + b <= me
        });
        runs.push(TextRun {
            len: b - a,
            font: font.clone(),
            color: rgb(color).into(),
            background_color: None,
            underline: underlined.then_some(UnderlineStyle {
                color: Some(rgb(color).into()),
                thickness: px(1.0),
                wavy: false,
            }),
            strikethrough: None,
        });
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
    fn shift_enter_inserts_a_newline_in_the_snippet(cx: &mut TestAppContext) {
        let (mut cx, area) = open(cx);

        cx.simulate_input("a = 1");
        cx.simulate_keystrokes("shift-enter");
        cx.simulate_input("b = 2");
        assert_eq!(content(&area, &cx), "welcome\n>>> a = 1\nb = 2");
    }

    #[gpui::test]
    fn enter_emits_the_whole_multiline_tail(cx: &mut TestAppContext) {
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

        cx.simulate_input("if True:");
        cx.simulate_keystrokes("shift-enter");
        cx.simulate_input("    print(1)");
        cx.simulate_keystrokes("enter");

        assert_eq!(seen.borrow().as_slice(), ["if True:\n    print(1)"]);
    }

    #[gpui::test]
    fn set_transcript_clears_the_editable_tail(cx: &mut TestAppContext) {
        let (mut cx, area) = open(cx);
        cx.update(|_window, cx| {
            area.update(cx, |area, cx| area.set_transcript("welcome\n>>> 3\n>>> ", cx));
        });
        assert_eq!(content(&area, &cx), "welcome\n>>> 3\n>>> ");
    }
}
