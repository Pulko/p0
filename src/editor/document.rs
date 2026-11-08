use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ropey::Rope;

use super::coords::{
    Position, Range, char_to_position, clamp_position, line_text, position_to_byte,
    position_to_char,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocumentId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LanguageId {
    TypeScript,
    Tsx,
    JavaScript,
    Json,
    Yaml,
    Toml,
    Html,
    Css,
    PlainText,
}

impl LanguageId {
    pub fn from_path(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "ts" => Self::TypeScript,
            "tsx" => Self::Tsx,
            "js" | "mjs" | "cjs" | "jsx" => Self::JavaScript,
            "json" | "jsonc" => Self::Json,
            "yaml" | "yml" => Self::Yaml,
            "toml" => Self::Toml,
            "html" | "htm" => Self::Html,
            "css" => Self::Css,
            _ => Self::PlainText,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::TypeScript => "TypeScript",
            Self::Tsx => "TSX",
            Self::JavaScript => "JavaScript",
            Self::Json => "JSON",
            Self::Yaml => "YAML",
            Self::Toml => "TOML",
            Self::Html => "HTML",
            Self::Css => "CSS",
            Self::PlainText => "Plain Text",
        }
    }

    pub fn lsp_language_id(self) -> &'static str {
        match self {
            Self::TypeScript => "typescript",
            Self::Tsx => "typescriptreact",
            Self::JavaScript => "javascript",
            Self::Json => "json",
            Self::Yaml => "yaml",
            Self::Toml => "toml",
            Self::Html => "html",
            Self::Css => "css",
            Self::PlainText => "plaintext",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
        }
    }

    pub fn detect(text: &str) -> Self {
        if text.contains("\r\n") {
            Self::Crlf
        } else {
            Self::Lf
        }
    }
}

#[derive(Clone, Debug)]
pub struct DocumentChange {
    pub document_id: DocumentId,
    pub revision: u64,
    pub start_byte: usize,
    pub old_end_byte: usize,
    pub new_end_byte: usize,
    pub start_char: usize,
    pub old_end_char: usize,
    pub new_end_char: usize,
    pub start_position: Position,
    pub old_end_position: Position,
    pub new_end_position: Position,
    pub start_row: usize,
    pub start_byte_column: usize,
    pub old_end_row: usize,
    pub old_end_byte_column: usize,
    pub new_end_row: usize,
    pub new_end_byte_column: usize,
    pub inserted_text: String,
    pub deleted_text: String,
}

#[derive(Clone, Debug)]
struct UndoOp {
    start: Position,
    deleted: String,
    inserted: String,
}

#[derive(Clone, Debug, Default)]
struct UndoHistory {
    past: Vec<Vec<UndoOp>>,
    future: Vec<Vec<UndoOp>>,
    open: Option<Vec<UndoOp>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GroupKind {
    InsertWord,
    InsertSpace,
    Delete,
    Other,
}

pub struct Document {
    pub id: DocumentId,
    pub path: Option<PathBuf>,
    pub language: LanguageId,
    pub rope: Rope,
    pub revision: u64,
    pub saved_revision: u64,
    pub line_ending: LineEnding,
    pub mtime: Option<SystemTime>,
    undo: UndoHistory,
    last_group: Option<GroupKind>,
}

impl Document {
    pub fn new(id: DocumentId, path: Option<PathBuf>, text: String) -> Self {
        let language = path
            .as_ref()
            .map(|p| LanguageId::from_path(p))
            .unwrap_or(LanguageId::PlainText);
        let line_ending = LineEnding::detect(&text);
        Self {
            id,
            path,
            language,
            rope: Rope::from_str(&text),
            revision: 0,
            saved_revision: 0,
            line_ending,
            mtime: None,
            undo: UndoHistory::default(),
            last_group: None,
        }
    }

    pub fn load(id: DocumentId, path: PathBuf) -> io::Result<Self> {
        let bytes = fs::read(&path)?;
        let text = String::from_utf8(bytes).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("file is not valid UTF-8: {e}"),
            )
        })?;
        let mtime = fs::metadata(&path).and_then(|m| m.modified()).ok();
        let mut doc = Self::new(id, Some(path), text);
        doc.mtime = mtime;
        Ok(doc)
    }

    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn save(&mut self) -> io::Result<()> {
        let Some(path) = self.path.clone() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "document has no path",
            ));
        };
        self.save_to(&path)?;
        self.path = Some(path);
        Ok(())
    }

    pub fn save_to(&mut self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp_name = format!(
            ".{}.p0tmp",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("untitled")
        );
        let tmp_path = path
            .parent()
            .map(|p| p.join(&tmp_name))
            .unwrap_or_else(|| PathBuf::from(&tmp_name));
        {
            let mut file = fs::File::create(&tmp_path)?;
            for chunk in self.rope.chunks() {
                file.write_all(chunk.as_bytes())?;
            }
            file.sync_all()?;
        }
        fs::rename(&tmp_path, path).or_else(|_| {
            fs::copy(&tmp_path, path)?;
            fs::remove_file(&tmp_path)
        })?;
        self.saved_revision = self.revision;
        self.mtime = fs::metadata(path).and_then(|m| m.modified()).ok();
        Ok(())
    }

    pub fn apply_replace(&mut self, range: Range, text: &str, group: bool) -> DocumentChange {
        let range = Range::new(
            clamp_position(&self.rope, range.start),
            clamp_position(&self.rope, range.end),
        );
        let start_char = position_to_char(&self.rope, range.start);
        let end_char = position_to_char(&self.rope, range.end);
        let start_byte = position_to_byte(&self.rope, range.start);
        let old_end_byte = position_to_byte(&self.rope, range.end);
        let (start_row, start_byte_column) = byte_point(&self.rope, start_byte);
        let (old_end_row, old_end_byte_column) = byte_point(&self.rope, old_end_byte);
        let deleted = if start_char == end_char {
            String::new()
        } else {
            self.rope.slice(start_char..end_char).to_string()
        };
        let old_end_position = range.end;
        if start_char != end_char {
            self.rope.remove(start_char..end_char);
        }
        if !text.is_empty() {
            self.rope.insert(start_char, text);
        }
        let new_end_char = start_char + text.chars().count();
        let new_end_byte = start_byte + text.len();
        let new_end_position = char_to_position(&self.rope, new_end_char);
        let (new_end_row, new_end_byte_column) = byte_point(&self.rope, new_end_byte);
        self.revision += 1;

        let kind = classify_group(text, &deleted);
        if !group || !self.try_merge_undo(kind, range.start, &deleted, text) {
            self.push_undo(UndoOp {
                start: range.start,
                deleted: deleted.clone(),
                inserted: text.to_string(),
            });
            self.last_group = Some(kind);
        }

        DocumentChange {
            document_id: self.id,
            revision: self.revision,
            start_byte,
            old_end_byte,
            new_end_byte,
            start_char,
            old_end_char: end_char,
            new_end_char,
            start_position: range.start,
            old_end_position,
            new_end_position,
            start_row,
            start_byte_column,
            old_end_row,
            old_end_byte_column,
            new_end_row,
            new_end_byte_column,
            inserted_text: text.to_string(),
            deleted_text: deleted,
        }
    }

    fn try_merge_undo(
        &mut self,
        kind: GroupKind,
        start: Position,
        deleted: &str,
        inserted: &str,
    ) -> bool {
        if kind == GroupKind::Other {
            return false;
        }
        if self.last_group != Some(kind) {
            return false;
        }
        let Some(open) = self.undo.open.as_mut() else {
            return false;
        };
        let Some(last) = open.last_mut() else {
            return false;
        };
        match kind {
            GroupKind::InsertWord | GroupKind::InsertSpace => {
                if !deleted.is_empty() {
                    return false;
                }
                let end = position_after(&self.rope, last.start, &last.inserted);
                if end != start {
                    return false;
                }
                last.inserted.push_str(inserted);
                true
            }
            GroupKind::Delete => {
                if !inserted.is_empty() {
                    return false;
                }
                if start == last.start {
                    last.deleted.push_str(deleted);
                    true
                } else if position_after(&self.rope, start, deleted) == last.start {
                    let mut combined = deleted.to_string();
                    combined.push_str(&last.deleted);
                    last.start = start;
                    last.deleted = combined;
                    true
                } else {
                    false
                }
            }
            GroupKind::Other => false,
        }
    }

    fn push_undo(&mut self, op: UndoOp) {
        if let Some(open) = self.undo.open.take() {
            self.undo.past.push(open);
        }
        self.undo.open = Some(vec![op]);
        self.undo.future.clear();
    }

    pub fn close_undo_group(&mut self) {
        if let Some(open) = self.undo.open.take() {
            self.undo.past.push(open);
        }
        self.last_group = None;
    }

    pub fn undo(&mut self) -> Option<Vec<DocumentChange>> {
        self.close_undo_group();
        let ops = self.undo.past.pop()?;
        let mut inverse = Vec::new();
        let mut changes = Vec::new();
        for op in ops.iter().rev() {
            let start = op.start;
            let end = position_after(&self.rope, start, &op.inserted);
            let change = self.apply_raw(Range::new(start, end), &op.deleted);
            inverse.push(UndoOp {
                start,
                deleted: op.inserted.clone(),
                inserted: op.deleted.clone(),
            });
            changes.push(change);
        }
        self.undo.future.push(ops);
        Some(changes)
    }

    pub fn redo(&mut self) -> Option<Vec<DocumentChange>> {
        self.close_undo_group();
        let ops = self.undo.future.pop()?;
        let mut changes = Vec::new();
        for op in &ops {
            let start = op.start;
            let end = position_after(&self.rope, start, &op.deleted);
            let change = self.apply_raw(Range::new(start, end), &op.inserted);
            changes.push(change);
        }
        self.undo.past.push(ops);
        Some(changes)
    }

    fn apply_raw(&mut self, range: Range, text: &str) -> DocumentChange {
        let start_char = position_to_char(&self.rope, range.start);
        let end_char = position_to_char(&self.rope, range.end);
        let start_byte = position_to_byte(&self.rope, range.start);
        let old_end_byte = position_to_byte(&self.rope, range.end);
        let (start_row, start_byte_column) = byte_point(&self.rope, start_byte);
        let (old_end_row, old_end_byte_column) = byte_point(&self.rope, old_end_byte);
        let deleted = if start_char == end_char {
            String::new()
        } else {
            self.rope.slice(start_char..end_char).to_string()
        };
        let old_end_position = range.end;
        if start_char != end_char {
            self.rope.remove(start_char..end_char);
        }
        if !text.is_empty() {
            self.rope.insert(start_char, text);
        }
        let new_end_char = start_char + text.chars().count();
        let new_end_byte = start_byte + text.len();
        let new_end_position = char_to_position(&self.rope, new_end_char);
        let (new_end_row, new_end_byte_column) = byte_point(&self.rope, new_end_byte);
        self.revision += 1;
        DocumentChange {
            document_id: self.id,
            revision: self.revision,
            start_byte,
            old_end_byte,
            new_end_byte,
            start_char,
            old_end_char: end_char,
            new_end_char,
            start_position: range.start,
            old_end_position,
            new_end_position,
            start_row,
            start_byte_column,
            old_end_row,
            old_end_byte_column,
            new_end_row,
            new_end_byte_column,
            inserted_text: text.to_string(),
            deleted_text: deleted,
        }
    }
}

fn byte_point(rope: &Rope, byte: usize) -> (usize, usize) {
    let byte = byte.min(rope.len_bytes());
    let row = rope.byte_to_line(byte);
    let line_start = rope.line_to_byte(row);
    (row, byte - line_start)
}

fn classify_group(inserted: &str, deleted: &str) -> GroupKind {
    if inserted.chars().count() == 1 && deleted.is_empty() {
        let ch = inserted.chars().next().unwrap();
        if ch.is_whitespace() || ch == '\n' {
            return GroupKind::InsertSpace;
        }
        if ch.is_alphanumeric() || ch == '_' {
            return GroupKind::InsertWord;
        }
        return GroupKind::Other;
    }
    if inserted.is_empty() && !deleted.is_empty() && deleted.chars().count() == 1 {
        return GroupKind::Delete;
    }
    GroupKind::Other
}

fn position_after(rope: &ropey::Rope, start: Position, text: &str) -> Position {
    if text.is_empty() {
        return start;
    }
    let idx = position_to_char(rope, start) + text.chars().count();
    char_to_position(rope, idx)
}

pub fn text_in_range(rope: &Rope, range: Range) -> String {
    let start = position_to_char(rope, range.start);
    let end = position_to_char(rope, range.end);
    if start >= end {
        String::new()
    } else {
        rope.slice(start..end).to_string()
    }
}

pub fn indent_of_line(rope: &Rope, line: usize) -> String {
    let text = line_text(rope, line);
    text.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile_dir::temp_path;

    mod tempfile_dir {
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicU64, Ordering};

        static N: AtomicU64 = AtomicU64::new(0);

        pub fn temp_path(name: &str) -> PathBuf {
            let id = N.fetch_add(1, Ordering::Relaxed);
            std::env::temp_dir().join(format!("p0-test-{id}-{name}"))
        }
    }

    #[test]
    fn insert_delete_roundtrip() {
        let mut doc = Document::new(DocumentId(1), None, String::new());
        doc.apply_replace(
            Range::new(Position::default(), Position::default()),
            "hello",
            false,
        );
        assert_eq!(doc.text(), "hello");
        doc.apply_replace(
            Range::new(Position::new(0, 0), Position::new(0, 5)),
            "",
            false,
        );
        assert_eq!(doc.text(), "");
    }

    #[test]
    fn undo_groups_words() {
        let mut doc = Document::new(DocumentId(1), None, String::new());
        for ch in ["h", "e", "l", "l", "o"] {
            doc.apply_replace(
                Range::new(
                    Position::new(0, doc.rope.len_chars()),
                    Position::new(0, doc.rope.len_chars()),
                ),
                ch,
                true,
            );
        }
        doc.undo();
        assert_eq!(doc.text(), "");
        doc.redo();
        assert_eq!(doc.text(), "hello");
    }

    #[test]
    fn save_and_reload() {
        let path = temp_path("file.ts");
        let mut doc = Document::new(DocumentId(1), Some(path.clone()), "const x = 1;\n".into());
        doc.save().unwrap();
        let loaded = Document::load(DocumentId(2), path.clone()).unwrap();
        assert_eq!(loaded.text(), "const x = 1;\n");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn undo_redo_inverse_with_replacement() {
        let mut doc = Document::new(DocumentId(1), None, "abc".into());
        doc.apply_replace(
            Range::new(Position::new(0, 1), Position::new(0, 2)),
            "XYZ",
            false,
        );
        assert_eq!(doc.text(), "aXYZc");
        doc.undo();
        assert_eq!(doc.text(), "abc");
        doc.redo();
        assert_eq!(doc.text(), "aXYZc");
    }
}
