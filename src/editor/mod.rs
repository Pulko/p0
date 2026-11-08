#![allow(dead_code, unused_imports)]

mod clipboard;
mod coords;
mod document;
mod ops;

pub use clipboard::Clipboard;
pub use coords::{
    Position, Range, byte_to_position, char_col_at_display, char_to_position, clamp_position,
    display_col, line_display_width, line_text, line_visible_chars, position_to_byte,
    position_to_char, position_to_utf16, snap_to_grapheme, utf16_to_column,
};
pub use document::{Document, DocumentChange, DocumentId, LanguageId, LineEnding};
pub use ops::{
    EditorView, Movement, Selection, Viewport, delete_backward, delete_forward, delete_selection,
    indent_for_newline, insert_text, move_cursor, position_at_display, replace_range, select_line,
    select_word_at, selected_text, set_cursor, word_bounds,
};
