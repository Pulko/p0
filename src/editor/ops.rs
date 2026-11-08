use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;

use super::coords::{
    Position, Range, char_col_at_display, clamp_position, display_col, line_display_width,
    line_text, line_visible_chars, snap_to_grapheme,
};
use super::document::{Document, DocumentChange, indent_of_line, text_in_range};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Viewport {
    pub top: usize,
    pub left: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Position,
    pub cursor: Position,
}

impl Selection {
    pub fn range(self) -> Range {
        Range::new(self.anchor, self.cursor)
    }

    pub fn is_empty(self) -> bool {
        self.anchor == self.cursor
    }
}

#[derive(Clone, Debug)]
pub struct EditorView {
    pub document_id: super::document::DocumentId,
    pub cursor: Position,
    pub selection: Option<Selection>,
    pub viewport: Viewport,
    pub preferred_column: Option<usize>,
}

impl EditorView {
    pub fn new(document_id: super::document::DocumentId) -> Self {
        Self {
            document_id,
            cursor: Position::default(),
            selection: None,
            viewport: Viewport::default(),
            preferred_column: None,
        }
    }

    pub fn active_range(&self) -> Range {
        self.selection
            .map(|s| s.range())
            .unwrap_or(Range::new(self.cursor, self.cursor))
    }

    pub fn ensure_visible(&mut self, rope: &Rope, _height: usize, width: usize, tab_width: usize) {
        let line = self.cursor.line;
        let text = line_text(rope, line);
        let col = display_col(&text, self.cursor.column, tab_width);
        if col < self.viewport.left {
            self.viewport.left = col;
        } else if width > 0 && col >= self.viewport.left + width {
            self.viewport.left = col + 1 - width;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Movement {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    LineStart,
    LineEnd,
    PageUp,
    PageDown,
    DocumentStart,
    DocumentEnd,
}

pub fn set_cursor(view: &mut EditorView, rope: &Rope, pos: Position, selecting: bool) {
    let pos = clamp_position(rope, pos);
    let text = line_text(rope, pos.line);
    let pos = Position::new(pos.line, snap_to_grapheme(&text, pos.column));
    if selecting {
        let anchor = view.selection.map(|s| s.anchor).unwrap_or(view.cursor);
        view.cursor = pos;
        view.selection = Some(Selection {
            anchor,
            cursor: pos,
        });
        if view.selection.map(|s| s.is_empty()).unwrap_or(false) {
            view.selection = None;
        }
    } else {
        view.cursor = pos;
        view.selection = None;
    }
    view.preferred_column = None;
}

pub fn move_cursor(
    view: &mut EditorView,
    rope: &Rope,
    movement: Movement,
    selecting: bool,
    page: usize,
) {
    let next = target_position(view, rope, movement, page);
    if matches!(
        movement,
        Movement::Up | Movement::Down | Movement::PageUp | Movement::PageDown
    ) {
        let preferred = view.preferred_column.unwrap_or(view.cursor.column);
        set_cursor(view, rope, next, selecting);
        view.preferred_column = Some(preferred);
    } else {
        set_cursor(view, rope, next, selecting);
    }
}

fn target_position(view: &EditorView, rope: &Rope, movement: Movement, page: usize) -> Position {
    let pos = view.cursor;
    match movement {
        Movement::Left => {
            if pos.column > 0 {
                let text = line_text(rope, pos.line);
                let mut col = pos.column;
                let mut idx = 0;
                let mut prev = 0;
                for g in text.graphemes(true) {
                    let next = idx + g.chars().count();
                    if next >= pos.column {
                        col = prev;
                        break;
                    }
                    prev = next;
                    idx = next;
                }
                Position::new(pos.line, col)
            } else if pos.line > 0 {
                Position::new(pos.line - 1, line_visible_chars(rope, pos.line - 1))
            } else {
                pos
            }
        }
        Movement::Right => {
            let vis = line_visible_chars(rope, pos.line);
            if pos.column < vis {
                let text = line_text(rope, pos.line);
                let mut idx = 0;
                for g in text.graphemes(true) {
                    let next = idx + g.chars().count();
                    if idx == pos.column {
                        return Position::new(pos.line, next);
                    }
                    idx = next;
                }
                Position::new(pos.line, vis)
            } else if pos.line + 1 < rope.len_lines() {
                Position::new(pos.line + 1, 0)
            } else {
                pos
            }
        }
        Movement::Up => {
            if pos.line == 0 {
                pos
            } else {
                vertical(
                    rope,
                    pos.line - 1,
                    view.preferred_column.unwrap_or(pos.column),
                )
            }
        }
        Movement::Down => {
            if pos.line + 1 >= rope.len_lines() {
                pos
            } else {
                vertical(
                    rope,
                    pos.line + 1,
                    view.preferred_column.unwrap_or(pos.column),
                )
            }
        }
        Movement::WordLeft => word_left(rope, pos),
        Movement::WordRight => word_right(rope, pos),
        Movement::LineStart => Position::new(pos.line, 0),
        Movement::LineEnd => Position::new(pos.line, line_visible_chars(rope, pos.line)),
        Movement::PageUp => {
            let line = pos.line.saturating_sub(page.max(1));
            vertical(rope, line, view.preferred_column.unwrap_or(pos.column))
        }
        Movement::PageDown => {
            let line = (pos.line + page.max(1)).min(rope.len_lines().saturating_sub(1));
            vertical(rope, line, view.preferred_column.unwrap_or(pos.column))
        }
        Movement::DocumentStart => Position::default(),
        Movement::DocumentEnd => {
            let line = rope.len_lines().saturating_sub(1);
            Position::new(line, line_visible_chars(rope, line))
        }
    }
}

fn vertical(rope: &Rope, line: usize, column: usize) -> Position {
    let vis = line_visible_chars(rope, line);
    Position::new(line, column.min(vis))
}

fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn word_left(rope: &Rope, pos: Position) -> Position {
    let mut line = pos.line;
    let mut col = pos.column;
    if col == 0 {
        if line == 0 {
            return pos;
        }
        line -= 1;
        col = line_visible_chars(rope, line);
    }
    let chars: Vec<char> = line_text(rope, line).chars().collect();
    col = col.min(chars.len());
    while col > 0 && chars[col - 1].is_whitespace() {
        col -= 1;
    }
    if col > 0 {
        let word = is_word(chars[col - 1]);
        while col > 0 && is_word(chars[col - 1]) == word && !chars[col - 1].is_whitespace() {
            col -= 1;
        }
    }
    Position::new(line, col)
}

fn word_right(rope: &Rope, pos: Position) -> Position {
    let chars: Vec<char> = line_text(rope, pos.line).chars().collect();
    let mut col = pos.column.min(chars.len());
    if col >= chars.len() {
        if pos.line + 1 >= rope.len_lines() {
            return pos;
        }
        return Position::new(pos.line + 1, 0);
    }
    let word = is_word(chars[col]);
    while col < chars.len() && is_word(chars[col]) == word && !chars[col].is_whitespace() {
        col += 1;
    }
    while col < chars.len() && chars[col].is_whitespace() {
        col += 1;
    }
    Position::new(pos.line, col)
}

pub fn word_bounds(rope: &Rope, pos: Position) -> Range {
    let chars: Vec<char> = line_text(rope, pos.line).chars().collect();
    if chars.is_empty() {
        return Range::new(Position::new(pos.line, 0), Position::new(pos.line, 0));
    }
    let mut idx = pos.column.min(chars.len().saturating_sub(1));
    if idx < chars.len() && !is_word(chars[idx]) && idx > 0 && is_word(chars[idx - 1]) {
        idx -= 1;
    }
    if idx >= chars.len() || !is_word(chars[idx]) {
        let start = pos.column.min(chars.len());
        return Range::new(
            Position::new(pos.line, start),
            Position::new(pos.line, start),
        );
    }
    let mut start = idx;
    let mut end = idx + 1;
    while start > 0 && is_word(chars[start - 1]) {
        start -= 1;
    }
    while end < chars.len() && is_word(chars[end]) {
        end += 1;
    }
    Range::new(Position::new(pos.line, start), Position::new(pos.line, end))
}

pub fn select_word_at(view: &mut EditorView, rope: &Rope, pos: Position) {
    let range = word_bounds(rope, pos);
    view.cursor = range.end;
    view.selection = Some(Selection {
        anchor: range.start,
        cursor: range.end,
    });
}

pub fn select_line(view: &mut EditorView, rope: &Rope, line: usize) {
    let end_line = line;
    let end_col = line_visible_chars(rope, end_line);
    view.selection = Some(Selection {
        anchor: Position::new(line, 0),
        cursor: Position::new(end_line, end_col),
    });
    view.cursor = Position::new(end_line, end_col);
}

pub fn position_at_display(
    rope: &Rope,
    line: usize,
    display_col: usize,
    tab_width: usize,
) -> Position {
    let line = line.min(rope.len_lines().saturating_sub(1));
    let text = line_text(rope, line);
    let col = char_col_at_display(&text, display_col, tab_width);
    Position::new(line, snap_to_grapheme(&text, col))
}

pub fn insert_text(
    doc: &mut Document,
    view: &mut EditorView,
    text: &str,
    group: bool,
) -> Vec<DocumentChange> {
    let mut changes = Vec::new();
    if view.selection.is_some() {
        if let Some(change) = delete_selection(doc, view) {
            changes.push(change);
        }
    }
    let change = doc.apply_replace(Range::new(view.cursor, view.cursor), text, group);
    view.cursor = change.new_end_position;
    view.selection = None;
    view.preferred_column = None;
    changes.push(change);
    changes
}

pub fn indent_for_newline(doc: &Document, view: &EditorView) -> String {
    let mut indent = indent_of_line(&doc.rope, view.cursor.line);
    let line = line_text(&doc.rope, view.cursor.line);
    let before: String = line.chars().take(view.cursor.column).collect();
    if before.trim_end().ends_with('{')
        || before.trim_end().ends_with('(')
        || before.trim_end().ends_with('[')
    {
        indent.push_str("    ");
    }
    indent
}

pub fn delete_selection(doc: &mut Document, view: &mut EditorView) -> Option<DocumentChange> {
    let sel = view.selection?;
    let range = sel.range();
    if range.is_empty() {
        view.selection = None;
        return None;
    }
    let change = doc.apply_replace(range, "", false);
    view.cursor = range.start;
    view.selection = None;
    Some(change)
}

pub fn delete_backward(doc: &mut Document, view: &mut EditorView) -> Vec<DocumentChange> {
    if view.selection.is_some() {
        return delete_selection(doc, view).into_iter().collect();
    }
    if view.cursor == Position::default() {
        return Vec::new();
    }
    let mut tmp = view.clone();
    move_cursor(&mut tmp, &doc.rope, Movement::Left, false, 1);
    let change = doc.apply_replace(Range::new(tmp.cursor, view.cursor), "", true);
    view.cursor = tmp.cursor;
    vec![change]
}

pub fn delete_forward(doc: &mut Document, view: &mut EditorView) -> Vec<DocumentChange> {
    if view.selection.is_some() {
        return delete_selection(doc, view).into_iter().collect();
    }
    let mut tmp = view.clone();
    move_cursor(&mut tmp, &doc.rope, Movement::Right, false, 1);
    if tmp.cursor == view.cursor {
        return Vec::new();
    }
    let change = doc.apply_replace(Range::new(view.cursor, tmp.cursor), "", true);
    vec![change]
}

pub fn replace_range(
    doc: &mut Document,
    view: &mut EditorView,
    range: Range,
    text: &str,
) -> DocumentChange {
    let change = doc.apply_replace(range, text, false);
    view.cursor = change.new_end_position;
    view.selection = None;
    change
}

pub fn selected_text(doc: &Document, view: &EditorView) -> String {
    text_in_range(&doc.rope, view.active_range())
}

pub fn max_line_display_width(rope: &Rope, tab_width: usize) -> usize {
    (0..rope.len_lines())
        .map(|i| line_display_width(&line_text(rope, i), tab_width))
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::document::DocumentId;

    #[test]
    fn insert_and_move() {
        let mut doc = Document::new(DocumentId(1), None, String::new());
        let mut view = EditorView::new(doc.id);
        insert_text(&mut doc, &mut view, "ab", false);
        move_cursor(&mut view, &doc.rope, Movement::Left, false, 1);
        insert_text(&mut doc, &mut view, "X", false);
        assert_eq!(doc.text(), "aXb");
        assert_eq!(view.cursor, Position::new(0, 2));
    }

    #[test]
    fn mouse_click_wide_char() {
        let doc = Document::new(DocumentId(1), None, "あa".into());
        let pos = position_at_display(&doc.rope, 0, 2, 4);
        assert_eq!(pos, Position::new(0, 1));
        let start = position_at_display(&doc.rope, 0, 0, 4);
        assert_eq!(start, Position::new(0, 0));
    }

    #[test]
    fn word_selection() {
        let doc = Document::new(DocumentId(1), None, "hello world".into());
        let range = word_bounds(&doc.rope, Position::new(0, 2));
        assert_eq!(range.start.column, 0);
        assert_eq!(range.end.column, 5);
    }
}
