//! A single-line editable field.
//!
//! Two pieces, per the `View` pattern (see `docs/input-components.md`):
//!
//! * [`TextInputState`] is the backing entity. It owns the [`TextBuffer`], the
//!   focus handle and the editing methods, and implements [`EntityInputHandler`].
//! * [`Edit`] is the ephemeral view. It carries the per-call-site props (the
//!   placeholder, and later a width) and implements [`View`], returning the
//!   state's id from `entity_id()` so the two share reactive identity.
//!
//! The shell's property editor draws one [`Edit`] per editable property; the
//! console is a text area (`crate::textarea`) instead.

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityId, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId,
    IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PaintQuad, Pixels, Point, ShapedLine, SharedString, Style, TextAlign, TextRun, UTF16Selection,
    UnderlineStyle, View, Window, div, fill, point, prelude::*, px, relative, rgb, rgba, size,
};

use crate::text::{SubmitEvent, TextBuffer};

const INPUT_BG: u32 = 0x171a1f;
const INPUT_FG: u32 = 0xd8dbe0;
const INPUT_PLACEHOLDER: u32 = 0x6b7280;
const CARET: u32 = 0x7aa2f7;
const SELECTION: u32 = 0x3311ff30;

/// The backing state of a single-line field.
pub struct TextInputState {
    buffer: TextBuffer,
    focus_handle: FocusHandle,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
}

impl EventEmitter<SubmitEvent> for TextInputState {}

impl Focusable for TextInputState {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TextInputState {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            buffer: TextBuffer::new(),
            focus_handle: cx.focus_handle(),
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
        }
    }

    /// Replace the text and move the caret to the end.
    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.buffer.set_content(text);
        cx.notify();
    }

    fn backspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.buffer.backspace() {
            window.play_system_bell();
        }
        cx.notify();
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.buffer.delete() {
            window.play_system_bell();
        }
        cx.notify();
    }

    fn paste(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            // A single-line field keeps a pasted newline from splitting it.
            self.buffer.insert(&text.replace('\n', " "));
        }
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

    fn cut(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let range = self.buffer.selected_range().clone();
        if !range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.buffer.content()[range.clone()].to_string(),
            ));
            self.buffer.replace_range(range, "");
        }
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
                self.backspace(window, cx);
                true
            }
            "delete" => {
                self.delete(window, cx);
                true
            }
            "left" => {
                let cursor = self.buffer.cursor_offset();
                if self.buffer.selected_range().is_empty() {
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
                let cursor = self.buffer.cursor_offset();
                if self.buffer.selected_range().is_empty() {
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
                self.copy(cx);
                true
            }
            "x" if secondary => {
                self.cut(window, cx);
                true
            }
            "v" if secondary => {
                self.paste(window, cx);
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

impl EntityInputHandler for TextInputState {
    fn text_for_range(
        &mut self,
        range_utf16: std::ops::Range<usize>,
        actual_range: &mut Option<std::ops::Range<usize>>,
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
    ) -> Option<std::ops::Range<usize>> {
        self.buffer
            .marked_range()
            .map(|range| self.buffer.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.buffer.set_marked_range(None);
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<std::ops::Range<usize>>,
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
        range_utf16: Option<std::ops::Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<std::ops::Range<usize>>,
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
        range_utf16: std::ops::Range<usize>,
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

/// The ephemeral view: props plus a handle to the state.
pub struct Edit {
    state: Entity<TextInputState>,
    placeholder: SharedString,
}

impl Edit {
    pub fn new(state: Entity<TextInputState>, placeholder: impl Into<SharedString>) -> Self {
        Self {
            state,
            placeholder: placeholder.into(),
        }
    }
}

impl View for Edit {
    fn entity_id(&self) -> Option<EntityId> {
        Some(self.state.entity_id())
    }

    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let entity = self.state.clone();
        let placeholder = self.placeholder.clone();
        self.state.update(cx, move |this, cx| {
            div()
                .w_full()
                .h(px(26.))
                .px_2()
                .bg(rgb(INPUT_BG))
                .text_size(px(13.))
                .line_height(px(26.))
                .cursor(CursorStyle::IBeam)
                .track_focus(&this.focus_handle)
                .on_key_down(cx.listener(TextInputState::on_key_down))
                .on_mouse_down(MouseButton::Left, cx.listener(TextInputState::on_mouse_down))
                .on_mouse_up(MouseButton::Left, cx.listener(TextInputState::on_mouse_up))
                .on_mouse_up_out(MouseButton::Left, cx.listener(TextInputState::on_mouse_up))
                .on_mouse_move(cx.listener(TextInputState::on_mouse_move))
                .child(TextElement {
                    input: entity.clone(),
                    placeholder,
                })
        })
    }
}

/// The custom element that paints the field and registers the input handler.
struct TextElement {
    input: Entity<TextInputState>,
    placeholder: SharedString,
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
        let state = self.input.read(cx);
        let content = state.buffer.content().to_string();
        let selected_range = state.buffer.selected_range().clone();
        let cursor = state.buffer.cursor_offset();
        let marked_range = state.buffer.marked_range().cloned();
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (self.placeholder.to_string(), rgb(INPUT_PLACEHOLDER).into())
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

        self.input.update(cx, |state, _cx| {
            state.last_layout = Some(line);
            state.last_bounds = Some(bounds);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Render, TestAppContext, ViewElement, VisualTestContext};

    /// The root that shows the field; `Edit` is a view, not a root.
    struct FieldHost {
        state: Entity<TextInputState>,
    }

    impl Render for FieldHost {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            ViewElement::new(Edit::new(self.state.clone(), "type here"))
        }
    }

    /// Open a window holding a single field, focused and ready to type.
    fn open(cx: &mut TestAppContext) -> (VisualTestContext, Entity<TextInputState>) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                let state = cx.new(|cx| TextInputState::new(cx));
                window.focus(&state.read(cx).focus_handle(cx), cx);
                cx.new(|_| FieldHost { state })
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let host: Entity<FieldHost> = window.root(&mut cx).unwrap();
        let state = host.read_with(&cx, |host, _| host.state.clone());
        (cx, state)
    }

    fn content(state: &Entity<TextInputState>, cx: &VisualTestContext) -> String {
        state.read_with(cx, |state, _| state.buffer.content().to_string())
    }

    #[gpui::test]
    fn typing_backspace_and_caret_movement_edit_the_content(cx: &mut TestAppContext) {
        let (mut cx, state) = open(cx);

        cx.simulate_input("hello");
        assert_eq!(content(&state, &cx), "hello");

        cx.simulate_keystrokes("backspace");
        assert_eq!(content(&state, &cx), "hell");

        cx.simulate_keystrokes("left left");
        cx.simulate_input("X");
        assert_eq!(content(&state, &cx), "heXll");
    }

    #[gpui::test]
    fn select_all_then_type_replaces_the_line(cx: &mut TestAppContext) {
        let (mut cx, state) = open(cx);

        cx.simulate_input("remove me");
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input("kept");
        assert_eq!(content(&state, &cx), "kept");
    }

    #[gpui::test]
    fn enter_emits_a_submit_event_without_inserting_a_newline(cx: &mut TestAppContext) {
        use std::cell::RefCell;
        use std::rc::Rc;

        let (mut cx, state) = open(cx);

        let seen: Rc<RefCell<Vec<String>>> = Rc::default();
        let sink = seen.clone();
        let _subscription = cx.update(|_window, cx| {
            cx.subscribe(&state, move |_emitter, event: &SubmitEvent, _cx| {
                sink.borrow_mut().push(event.text.clone());
            })
        });

        cx.simulate_input("1 + 2");
        cx.simulate_keystrokes("enter");

        assert_eq!(seen.borrow().as_slice(), ["1 + 2"]);
        assert_eq!(content(&state, &cx), "1 + 2");
    }
}
