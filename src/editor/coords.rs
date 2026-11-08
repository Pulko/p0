use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

impl Position {
    pub fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

impl Range {
    pub fn new(a: Position, b: Position) -> Self {
        if a <= b {
            Self { start: a, end: b }
        } else {
            Self { start: b, end: a }
        }
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

pub fn line_text(rope: &Rope, line: usize) -> String {
    if line >= rope.len_lines() {
        return String::new();
    }
    let mut text = rope.line(line).to_string();
    if text.ends_with('\n') {
        text.pop();
        if text.ends_with('\r') {
            text.pop();
        }
    } else if text.ends_with('\r') {
        text.pop();
    }
    text
}

pub fn line_visible_chars(rope: &Rope, line: usize) -> usize {
    line_text(rope, line).chars().count()
}

pub fn clamp_position(rope: &Rope, mut pos: Position) -> Position {
    if rope.len_lines() == 0 {
        return Position::default();
    }
    pos.line = pos.line.min(rope.len_lines().saturating_sub(1));
    pos.column = pos.column.min(line_visible_chars(rope, pos.line));
    pos
}

pub fn position_to_char(rope: &Rope, pos: Position) -> usize {
    let pos = clamp_position(rope, pos);
    let line_start = rope.line_to_char(pos.line);
    line_start + pos.column
}

pub fn char_to_position(rope: &Rope, char_idx: usize) -> Position {
    if rope.len_chars() == 0 {
        return Position::default();
    }
    let char_idx = char_idx.min(rope.len_chars());
    let mut line = rope.char_to_line(char_idx);
    let line_start = rope.line_to_char(line);
    let mut column = char_idx - line_start;
    let visible = line_visible_chars(rope, line);
    if column > visible {
        if line + 1 < rope.len_lines() {
            line += 1;
            column = 0;
        } else {
            column = visible;
        }
    }
    Position { line, column }
}

pub fn byte_to_position(rope: &Rope, byte_idx: usize) -> Position {
    let byte_idx = byte_idx.min(rope.len_bytes());
    let char_idx = rope.byte_to_char(byte_idx);
    char_to_position(rope, char_idx)
}

pub fn position_to_byte(rope: &Rope, pos: Position) -> usize {
    rope.char_to_byte(position_to_char(rope, pos))
}

pub fn grapheme_char_lengths(text: &str) -> Vec<usize> {
    text.graphemes(true).map(|g| g.chars().count()).collect()
}

pub fn snap_to_grapheme(text: &str, mut column: usize) -> usize {
    let total = text.chars().count();
    column = column.min(total);
    let mut idx = 0;
    for g in text.graphemes(true) {
        let next = idx + g.chars().count();
        if column == idx || column == next {
            return column;
        }
        if column < next {
            return idx;
        }
        idx = next;
    }
    total
}

pub fn display_width_of(g: &str, col: usize, tab_width: usize) -> usize {
    if g == "\t" {
        let width = tab_width.max(1);
        width - (col % width)
    } else {
        UnicodeWidthStr::width(g).max(if g.chars().all(|c| c.is_control()) {
            0
        } else {
            UnicodeWidthStr::width(g)
        })
    }
}

pub fn display_col(text: &str, char_col: usize, tab_width: usize) -> usize {
    let mut col = 0usize;
    let mut chars = 0usize;
    for g in text.graphemes(true) {
        if chars >= char_col {
            break;
        }
        col += display_width_of(g, col, tab_width);
        chars += g.chars().count();
    }
    col
}

pub fn line_display_width(text: &str, tab_width: usize) -> usize {
    display_col(text, text.chars().count(), tab_width)
}

pub fn char_col_at_display(text: &str, target: usize, tab_width: usize) -> usize {
    let mut col = 0usize;
    let mut chars = 0usize;
    for g in text.graphemes(true) {
        let width = display_width_of(g, col, tab_width);
        let next = col + width;
        if target <= col {
            return chars;
        }
        if target < next {
            if target - col <= next.saturating_sub(target) {
                return chars;
            }
            return chars + g.chars().count();
        }
        col = next;
        chars += g.chars().count();
    }
    chars
}

pub fn position_to_utf16(rope: &Rope, pos: Position) -> u32 {
    let text = line_text(rope, pos.line);
    let mut utf16 = 0u32;
    let mut chars = 0usize;
    for ch in text.chars() {
        if chars >= pos.column {
            break;
        }
        utf16 += ch.len_utf16() as u32;
        chars += 1;
    }
    utf16
}

pub fn utf16_to_column(rope: &Rope, line: usize, utf16: u32) -> usize {
    let text = line_text(rope, line);
    let mut seen = 0u32;
    let mut chars = 0usize;
    for ch in text.chars() {
        if seen >= utf16 {
            break;
        }
        seen += ch.len_utf16() as u32;
        chars += 1;
    }
    chars
}

pub fn range_bytes(rope: &Rope, range: Range) -> (usize, usize) {
    let start = position_to_byte(rope, range.start);
    let end = position_to_byte(rope, range.end);
    (start.min(end), start.max(end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_emoji_roundtrip() {
        let rope = Rope::from_str("a😀b");
        let _pos = Position::new(0, 2);
        assert_eq!(position_to_utf16(&rope, Position::new(0, 1)), 1);
        assert_eq!(position_to_utf16(&rope, Position::new(0, 2)), 3);
        assert_eq!(utf16_to_column(&rope, 0, 3), 2);
        assert_eq!(utf16_to_column(&rope, 0, 1), 1);
    }

    #[test]
    fn wide_cjk_display() {
        let text = "あa";
        assert_eq!(display_col(text, 0, 4), 0);
        assert_eq!(display_col(text, 1, 4), 2);
        assert_eq!(display_col(text, 2, 4), 3);
        assert_eq!(char_col_at_display(text, 0, 4), 0);
        assert_eq!(char_col_at_display(text, 1, 4), 0);
        assert_eq!(char_col_at_display(text, 2, 4), 1);
    }

    #[test]
    fn tabs_display() {
        let text = "\tab";
        assert_eq!(display_col(text, 1, 4), 4);
        assert_eq!(char_col_at_display(text, 1, 4), 0);
        assert_eq!(char_col_at_display(text, 4, 4), 1);
    }

    #[test]
    fn combining_mark_snap() {
        let text = "e\u{0301}x";
        let snapped = snap_to_grapheme(text, 1);
        assert_eq!(snapped, 0);
        assert_eq!(display_col(text, 2, 4), 1);
    }

    #[test]
    fn crlf_positions() {
        let rope = Rope::from_str("ab\r\ncd");
        assert_eq!(line_text(&rope, 0), "ab");
        assert_eq!(line_text(&rope, 1), "cd");
        assert_eq!(
            char_to_position(&rope, position_to_char(&rope, Position::new(1, 1))),
            Position::new(1, 1)
        );
    }
}
