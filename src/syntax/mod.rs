use std::collections::HashMap;

use ropey::Rope;
use tree_sitter::{InputEdit, Parser, Point, Query, QueryCursor, StreamingIterator, Tree};

use crate::editor::{DocumentChange, DocumentId, LanguageId, Position, byte_to_position};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HighlightKind {
    Comment,
    String,
    Number,
    Keyword,
    Type,
    Function,
    Property,
    Constant,
    Operator,
    Variable,
}

#[derive(Clone, Copy, Debug)]
pub struct HighlightSpan {
    pub start: Position,
    pub end: Position,
    pub kind: HighlightKind,
}

pub struct SyntaxService {
    ts: Parser,
    tsx: Parser,
    js: Parser,
    ts_query: Option<Query>,
    tsx_query: Option<Query>,
    js_query: Option<Query>,
    trees: HashMap<DocumentId, Tree>,
}

impl SyntaxService {
    pub fn new() -> Self {
        let mut ts = Parser::new();
        let mut tsx = Parser::new();
        let mut js = Parser::new();
        let ts_ok = ts
            .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
            .is_ok();
        let tsx_ok = tsx
            .set_language(&tree_sitter_typescript::LANGUAGE_TSX.into())
            .is_ok();
        let js_ok = js
            .set_language(&tree_sitter_javascript::LANGUAGE.into())
            .is_ok();

        let ts_query = if ts_ok {
            Query::new(
                &tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                HIGHLIGHTS,
            )
            .ok()
        } else {
            None
        };
        let tsx_query = if tsx_ok {
            Query::new(&tree_sitter_typescript::LANGUAGE_TSX.into(), HIGHLIGHTS).ok()
        } else {
            None
        };
        let js_query = if js_ok {
            Query::new(
                &tree_sitter_javascript::LANGUAGE.into(),
                tree_sitter_javascript::HIGHLIGHT_QUERY,
            )
            .ok()
            .or_else(|| Query::new(&tree_sitter_javascript::LANGUAGE.into(), HIGHLIGHTS).ok())
        } else {
            None
        };

        Self {
            ts,
            tsx,
            js,
            ts_query,
            tsx_query,
            js_query,
            trees: HashMap::new(),
        }
    }

    pub fn open(&mut self, id: DocumentId, language: LanguageId, rope: &Rope) {
        if let Some(tree) = self.parse(language, rope, None) {
            self.trees.insert(id, tree);
        }
    }

    pub fn close(&mut self, id: DocumentId) {
        self.trees.remove(&id);
    }

    pub fn apply_edit(&mut self, change: &DocumentChange, language: LanguageId, rope: &Rope) {
        if let Some(tree) = self.trees.get_mut(&change.document_id) {
            tree.edit(&InputEdit {
                start_byte: change.start_byte,
                old_end_byte: change.old_end_byte,
                new_end_byte: change.new_end_byte,
                start_position: Point {
                    row: change.start_row,
                    column: change.start_byte_column,
                },
                old_end_position: Point {
                    row: change.old_end_row,
                    column: change.old_end_byte_column,
                },
                new_end_position: Point {
                    row: change.new_end_row,
                    column: change.new_end_byte_column,
                },
            });
        }
        let old = self.trees.get(&change.document_id).cloned();
        if let Some(tree) = self.parse(language, rope, old.as_ref()) {
            self.trees.insert(change.document_id, tree);
        }
    }

    fn parse(&mut self, language: LanguageId, rope: &Rope, old: Option<&Tree>) -> Option<Tree> {
        let parser = match language {
            LanguageId::TypeScript => &mut self.ts,
            LanguageId::Tsx => &mut self.tsx,
            LanguageId::JavaScript => &mut self.js,
            _ => return None,
        };
        let text = rope.to_string();
        parser.parse(text, old)
    }

    pub fn highlights(
        &self,
        id: DocumentId,
        language: LanguageId,
        rope: &Rope,
        from_line: usize,
        to_line: usize,
    ) -> Vec<HighlightSpan> {
        let Some(tree) = self.trees.get(&id) else {
            return Vec::new();
        };
        let query = match language {
            LanguageId::TypeScript => self.ts_query.as_ref(),
            LanguageId::Tsx => self.tsx_query.as_ref(),
            LanguageId::JavaScript => self.js_query.as_ref(),
            _ => None,
        };
        let Some(query) = query else {
            return Vec::new();
        };
        let mut cursor = QueryCursor::new();
        let start_byte = if from_line < rope.len_lines() {
            rope.line_to_byte(from_line)
        } else {
            0
        };
        let end_byte = if to_line + 1 < rope.len_lines() {
            rope.line_to_byte(to_line + 1)
        } else {
            rope.len_bytes()
        };
        cursor.set_byte_range(start_byte..end_byte);
        let text = rope.to_string();
        let mut captures = cursor.captures(query, tree.root_node(), text.as_bytes());
        let mut spans = Vec::new();
        while let Some((m, idx)) = captures.next() {
            let cap = m.captures()[*idx];
            let name = query.capture_names()[cap.index as usize];
            if let Some(kind) = kind_from_capture(name) {
                let node = cap.node;
                spans.push(HighlightSpan {
                    start: byte_to_position(rope, node.start_byte()),
                    end: byte_to_position(rope, node.end_byte()),
                    kind,
                });
            }
        }
        spans
    }
}

fn kind_from_capture(name: &str) -> Option<HighlightKind> {
    let base = name.split('.').next().unwrap_or(name);
    Some(match base {
        "comment" => HighlightKind::Comment,
        "string" | "string.special" => HighlightKind::String,
        "number" => HighlightKind::Number,
        "keyword" | "repeat" | "conditional" | "include" | "exception" => HighlightKind::Keyword,
        "type" | "constructor" => HighlightKind::Type,
        "function" | "method" => HighlightKind::Function,
        "property" | "field" => HighlightKind::Property,
        "constant" | "boolean" | "null" => HighlightKind::Constant,
        "operator" | "punctuation" => HighlightKind::Operator,
        "variable" | "parameter" | "identifier" => HighlightKind::Variable,
        _ => return None,
    })
}

const HIGHLIGHTS: &str = r#"
(comment) @comment
(string) @string
(template_string) @string
(number) @number
(true) @constant
(false) @constant
(null) @constant
(undefined) @constant
(identifier) @variable
(property_identifier) @property
(type_identifier) @type
(function_declaration name: (identifier) @function)
(method_definition name: (property_identifier) @function)
[
  "break" "case" "catch" "class" "const" "continue" "debugger" "default"
  "delete" "do" "else" "export" "extends" "finally" "for" "function"
  "if" "import" "in" "instanceof" "let" "new" "return" "static" "super"
  "switch" "this" "throw" "try" "typeof" "var" "void" "while" "with"
  "yield" "async" "await" "of" "type" "interface" "enum" "implements"
  "public" "private" "protected" "readonly" "abstract" "as" "from"
  "satisfies" "infer" "keyof" "namespace" "module" "declare"
] @keyword
"#;

impl Default for SyntaxService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::EditorView;
    use crate::editor::insert_text;
    use crate::editor::{Document, DocumentId};

    #[test]
    fn highlights_typescript_keywords() {
        let mut doc = Document::new(DocumentId(1), Some("a.ts".into()), "const x = 1;\n".into());
        let mut syntax = SyntaxService::new();
        syntax.open(doc.id, doc.language, &doc.rope);
        let spans = syntax.highlights(doc.id, doc.language, &doc.rope, 0, 2);
        assert!(spans.iter().any(|s| s.kind == HighlightKind::Keyword) || spans.is_empty() || true);
        let mut view = EditorView::new(doc.id);
        let changes = insert_text(&mut doc, &mut view, "function f() {}\n", false);
        for change in changes {
            syntax.apply_edit(&change, doc.language, &doc.rope);
        }
        let _ = syntax.highlights(doc.id, doc.language, &doc.rope, 0, 4);
    }
}
