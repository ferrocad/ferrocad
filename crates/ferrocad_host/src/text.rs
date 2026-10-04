//! The shared text-editing model behind the input and text-area components.
//!
//! [`TextBuffer`] holds the text, the selection, the IME marked range and, for a
//! console buffer, the length of a **read-only prefix** that edits cannot cross.
//! It is free of any `Window` or `Context`, so the editing rules are ordinary
//! Rust and can be unit-tested directly. The components (`input`, `textarea`)
//! wrap it, implement `EntityInputHandler` on top of it, and draw it.
//!
//! All offsets are UTF-8 byte offsets. The platform speaks UTF-16; conversion
//! happens at the component boundary through [`TextBuffer::range_to_utf16`] and
//! [`TextBuffer::range_from_utf16`].
//!
//! WIP: the components still carry their own editing code; the next slice moves
//! `input` and the new `textarea` onto this buffer. Until then it is unused
//! outside its tests.
#![allow(dead_code)]

use std::ops::Range;

use gpui::SharedString;
use unicode_segmentation::UnicodeSegmentation;

/// Text, selection and read-only boundary for one editable region.
#[derive(Clone, Debug)]
pub struct TextBuffer {
    content: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    read_only_len: usize,
}

impl Default for TextBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl TextBuffer {
    pub fn new() -> Self {
        Self {
            content: SharedString::default(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            read_only_len: 0,
        }
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    /// The byte offset where the editable region begins. Everything before it is
    /// read-only (console output). Zero for a plain field.
    pub fn read_only_len(&self) -> usize {
        self.read_only_len
    }

    /// The editable text: the tail after the read-only prefix.
    pub fn editable(&self) -> &str {
        &self.content[self.read_only_len..]
    }

    pub fn selected_range(&self) -> &Range<usize> {
        &self.selected_range
    }

    pub fn selection_reversed(&self) -> bool {
        self.selection_reversed
    }

    pub fn marked_range(&self) -> Option<&Range<usize>> {
        self.marked_range.as_ref()
    }

    pub fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    /// Clamp an offset into the editable region.
    fn clamp(&self, offset: usize) -> usize {
        offset.clamp(self.read_only_len, self.content.len())
    }

    pub fn move_to(&mut self, offset: usize) {
        let offset = self.clamp(offset);
        self.selected_range = offset..offset;
        self.selection_reversed = false;
    }

    pub fn select_to(&mut self, offset: usize) {
        let offset = self.clamp(offset);
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
    }

    pub fn select_all(&mut self) {
        self.move_to(self.read_only_len);
        self.select_to(self.content.len());
    }

    pub fn set_selection(&mut self, range: Range<usize>, reversed: bool) {
        let start = self.clamp(range.start);
        let end = self.clamp(range.end).max(start);
        self.selected_range = start..end;
        self.selection_reversed = reversed;
    }

    pub fn set_marked_range(&mut self, range: Option<Range<usize>>) {
        self.marked_range = range;
    }

    pub fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    pub fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    /// Byte range of the line containing `offset` (excluding the newline).
    pub fn line_range(&self, offset: usize) -> Range<usize> {
        let offset = self.clamp(offset);
        let start = self.content[..offset].rfind('\n').map_or(0, |i| i + 1);
        let end = self.content[offset..]
            .find('\n')
            .map_or(self.content.len(), |i| offset + i);
        start..end
    }

    /// Replace `range` with `text`, clamped to the editable region, and put the
    /// caret after the inserted text.
    pub fn replace_range(&mut self, range: Range<usize>, text: &str) {
        let start = self.clamp(range.start);
        let end = self.clamp(range.end).max(start);
        self.content =
            (self.content[0..start].to_owned() + text + &self.content[end..]).into();
        let caret = start + text.len();
        self.selected_range = caret..caret;
        self.selection_reversed = false;
        self.marked_range = None;
    }

    /// Insert `text` at the selection.
    pub fn insert(&mut self, text: &str) {
        let range = self.selected_range.clone();
        self.replace_range(range, text);
    }

    /// Delete the selection, or the grapheme before the caret. Returns whether
    /// anything changed (false at the read-only boundary, where the caller may
    /// play the system bell).
    pub fn backspace(&mut self) -> bool {
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            if prev >= self.cursor_offset() {
                return false;
            }
            self.select_to(prev);
            if self.selected_range.is_empty() {
                // The grapheme lies entirely in the read-only prefix.
                return false;
            }
        }
        let range = self.selected_range.clone();
        self.replace_range(range, "");
        true
    }

    /// Delete the selection, or the grapheme after the caret.
    pub fn delete(&mut self) -> bool {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if next <= self.cursor_offset() {
                return false;
            }
            self.select_to(next);
            if self.selected_range.is_empty() {
                return false;
            }
        }
        let range = self.selected_range.clone();
        self.replace_range(range, "");
        true
    }

    /// Replace the whole content. The read-only boundary is clamped to the new
    /// length and the caret moves to the end.
    pub fn set_content(&mut self, text: impl Into<SharedString>) {
        self.content = text.into();
        self.read_only_len = self.read_only_len.min(self.content.len());
        let caret = self.content.len();
        self.selected_range = caret..caret;
        self.selection_reversed = false;
        self.marked_range = None;
    }

    pub fn set_read_only_len(&mut self, len: usize) {
        self.read_only_len = len.min(self.content.len());
        let caret = self.cursor_offset().max(self.read_only_len);
        self.move_to(caret);
    }

    /// Append text to the read-only prefix (console output) and move the caret
    /// to the end of the now-empty editable region.
    pub fn push_read_only(&mut self, text: &str) {
        self.content = (self.content.to_string() + text).into();
        self.read_only_len = self.content.len();
        self.move_to(self.read_only_len);
    }

    /// Remove and return the editable text, leaving the read-only prefix.
    pub fn take_editable(&mut self) -> String {
        let text = self.content[self.read_only_len..].to_string();
        self.content = self.content[..self.read_only_len].to_owned().into();
        self.move_to(self.read_only_len);
        text
    }

    pub fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    pub fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    pub fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    pub fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_moves_the_caret_and_inserts() {
        let mut b = TextBuffer::new();
        b.set_content("hello");
        b.insert("!");
        assert_eq!(b.content(), "hello!");
        assert_eq!(b.cursor_offset(), 6);

        b.move_to(0);
        b.insert(">");
        assert_eq!(b.content(), ">hello!");
        assert_eq!(b.cursor_offset(), 1);
    }

    #[test]
    fn backspace_and_delete_respect_graphemes() {
        let mut b = TextBuffer::new();
        b.set_content("a👨‍👩‍👧b");
        b.move_to(b.content().len());
        assert!(b.backspace());
        assert_eq!(b.content(), "a👨‍👩‍👧");
        b.move_to(1);
        assert!(b.delete());
        assert_eq!(b.content(), "a");
    }

    #[test]
    fn read_only_prefix_cannot_be_edited() {
        let mut b = TextBuffer::new();
        b.push_read_only(">>> ");
        assert_eq!(b.read_only_len(), 4);
        // The caret cannot move into the prefix.
        b.move_to(0);
        assert_eq!(b.cursor_offset(), 4);
        // Backspace at the boundary is a no-op.
        assert!(!b.backspace());
        assert_eq!(b.content(), ">>> ");
        // Typing appends to the editable tail.
        b.insert("1 + 1");
        assert_eq!(b.content(), ">>> 1 + 1");
        assert_eq!(b.editable(), "1 + 1");
        // Select-all only reaches the editable region.
        b.select_all();
        assert_eq!(b.selected_range(), &(4..9));
    }

    #[test]
    fn take_editable_commits_and_resets() {
        let mut b = TextBuffer::new();
        b.push_read_only(">>> ");
        b.insert("doc.recompute()");
        let line = b.take_editable();
        assert_eq!(line, "doc.recompute()");
        assert_eq!(b.content(), ">>> ");
        assert_eq!(b.editable(), "");
        assert_eq!(b.cursor_offset(), 4);
    }

    #[test]
    fn utf16_round_trip() {
        let mut b = TextBuffer::new();
        b.set_content("héllo");
        // "hé" is 2 UTF-16 units and 3 UTF-8 bytes.
        let utf16 = b.range_to_utf16(&(0..3));
        assert_eq!(utf16, 0..2);
        assert_eq!(b.range_from_utf16(&utf16), 0..3);
    }
}
