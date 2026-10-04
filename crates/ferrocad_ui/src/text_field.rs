//! A single-line editable field.
//!
//! One entity: it owns the [`TextBuffer`], the focus handle and the editing
//! methods, implements `EntityInputHandler`, and draws itself. It emits
//! [`ChangeEvent`] after every edit (a controlled parent mirrors it) and
//! [`SubmitEvent`] on `Enter` (an uncontrolled parent reads it there).

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, IntoElement,
    KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    Pixels, Point, ShapedLine, SharedString, Style, TextAlign, TextRun, UTF16Selection,
    UnderlineStyle, Window, div, fill, point, prelude::*, px, relative, rgb, rgba, size,
};

use crate::events::{ChangeEvent, SubmitEvent};
use crate::text_buffer::TextBuffer;

const INPUT_BG: u32 = 0x171a1f;
const INPUT_FG: u32 = 0xd8dbe0;
const INPUT_PLACEHOLDER: u32 = 0x6b7280;
const CARET: u32 = 0x7aa2f7;
const SELECTION: u32 = 0x3311ff30;

/// A single-line editable text field.
pub struct TextInput {
    buffer: TextBuffer,
    placeholder: SharedString,
    focus_handle: FocusHandle,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
}

impl EventEmitter<ChangeEvent> for TextInput {}
impl EventEmitter<SubmitEvent> for TextInput {}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TextInput {
    pub fn new(cx: &mut Context<Self>, placeholder: impl Into<SharedString>) -> Self {
        Self {
            buffer: TextBuffer::new(),
            placeholder: placeholder.into(),
            focus_handle: cx.focus_handle(),
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
        }
    }

    pub fn text(&self) -> &str {
        self.buffer.content()
    }

    /// Replace the text and move the caret to the end. Programmatic, so it does
    /// not emit `ChangeEvent` (that would loop with a controlled parent).
    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.buffer.set_content(text);
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.set_text("", cx);
    }

    fn changed(&self, cx: &mut Context<Self>) {
        cx.emit(ChangeEvent {
            text: self.buffer.content().to_string(),
        });
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let secondary = keystroke.modifiers.platform || keystroke.modifiers.control;
        let handled = match keystroke.key.as_str() {
            "enter" | "return" => {
                cx.emit(SubmitEvent {
                    text: self.buffer.content().to_string(),
                });
                true
            }
            "backspace" => {
                if !self.buffer.backspace() {
                    window.play_system_bell();
                }
                self.changed(cx);
                true
            }
            "delete" => {
                if !self.buffer.delete() {
                    window.play_system_bell();
                }
                self.changed(cx);
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
                self.buffer.move_to(0);
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
                let range = self.buffer.selected_range().clone();
                if !range.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        self.buffer.content()[range].to_string(),
                    ));
                }
                true
            }
            "x" if secondary => {
                let range = self.buffer.selected_range().clone();
                if !range.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        self.buffer.content()[range.clone()].to_string(),
                    ));
                    self.buffer.replace_range(range, "");
                    self.changed(cx);
                }
                true
            }
            "v" if secondary => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.buffer.insert(&text.replace('\n', " "));
                    self.changed(cx);
                }
                true
            }
            _ => false,
        };

        if handled {
            cx.stop_propagation();
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

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.buffer.content().len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }
}

impl EntityInputHandler for TextInput {
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
        self.changed(cx);
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
        self.changed(cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.buffer.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(
                bounds.left() + last_layout.x_for_index(range.start),
                bounds.top(),
            ),
            point(
                bounds.left() + last_layout.x_for_index(range.end),
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
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;
        let utf8_index = last_layout.index_for_x(point.x - line_point.x)?;
        Some(self.buffer.offset_to_utf16(utf8_index))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
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
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
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
        let input = self.input.read(cx);
        let content = input.buffer.content().to_string();
        let selected_range = input.buffer.selected_range().clone();
        let cursor = input.buffer.cursor_offset();
        let marked_range = input.buffer.marked_range().cloned();
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.to_string(), rgb(INPUT_PLACEHOLDER).into())
        } else {
            (content, rgb(INPUT_FG).into())
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = marked_range {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text.into(), font_size, &runs, None);

        let cursor_pos = line.x_for_index(cursor);
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_pos, bounds.top()),
                        size(px(2.), bounds.bottom() - bounds.top()),
                    ),
                    rgb(CARET),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(selected_range.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    rgba(SELECTION),
                )),
                None,
            )
        };

        PrepaintState {
            line: Some(line),
            cursor,
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
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );

        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }

        let line = prepaint.line.take().unwrap();
        line.paint(
            bounds.origin,
            window.line_height(),
            TextAlign::Left,
            None,
            window,
            cx,
        )
        .unwrap();

        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .h(px(26.))
            .px_2()
            .bg(rgb(INPUT_BG))
            .text_size(px(13.))
            .line_height(px(26.))
            .cursor(CursorStyle::IBeam)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(TextElement { input: cx.entity() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    #[gpui::test]
    fn typing_reports_changes_and_submit(cx: &mut TestAppContext) {
        use std::cell::RefCell;
        use std::rc::Rc;

        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                let input = cx.new(|cx| TextInput::new(cx, "type here"));
                window.focus(&input.read(cx).focus_handle(cx), cx);
                input
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let input: Entity<TextInput> = window.root(&mut cx).unwrap();

        let changes: Rc<RefCell<Vec<String>>> = Rc::default();
        let submits: Rc<RefCell<Vec<String>>> = Rc::default();
        let change_sink = changes.clone();
        let submit_sink = submits.clone();
        let _subscriptions = cx.update(|_window, cx| {
            (
                cx.subscribe(&input, move |_e, event: &ChangeEvent, _cx| {
                    change_sink.borrow_mut().push(event.text.clone());
                }),
                cx.subscribe(&input, move |_e, event: &SubmitEvent, _cx| {
                    submit_sink.borrow_mut().push(event.text.clone());
                }),
            )
        });

        cx.simulate_input("hi");
        assert_eq!(changes.borrow().last().cloned(), Some("hi".to_string()));

        cx.simulate_keystrokes("enter");
        assert_eq!(submits.borrow().as_slice(), ["hi"]);
    }
}
