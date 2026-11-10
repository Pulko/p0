use std::cell::RefCell;
use std::collections::HashMap;

use ropey::Rope;
use tree_sitter::{
    InputEdit, Language, Node, Parser, Point, Query, QueryCursor, StreamingIterator, Tree,
};

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
    Heading,
    Emphasis,
    Strong,
    Strike,
    Link,
}

#[derive(Clone, Copy, Debug)]
pub struct HighlightSpan {
    pub start: Position,
    pub end: Position,
    pub kind: HighlightKind,
}

struct Grammar {
    parser: Parser,
    query: Option<Query>,
}

struct InlineGrammar {
    parser: RefCell<Parser>,
    query: Option<Query>,
}

pub struct SyntaxService {
    grammars: HashMap<LanguageId, Grammar>,
    markdown_inline: InlineGrammar,
    trees: HashMap<DocumentId, Tree>,
}

impl SyntaxService {
    pub fn new() -> Self {
        let mut grammars = HashMap::new();
        install_joined(
            &mut grammars,
            LanguageId::TypeScript,
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            &[
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
            ],
        );
        install_joined(
            &mut grammars,
            LanguageId::Tsx,
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            &[
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
            ],
        );
        install_joined(
            &mut grammars,
            LanguageId::JavaScript,
            tree_sitter_javascript::LANGUAGE.into(),
            &[
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
            ],
        );
        install(
            &mut grammars,
            LanguageId::Json,
            tree_sitter_json::LANGUAGE.into(),
            &[tree_sitter_json::HIGHLIGHTS_QUERY, JSON_FALLBACK],
        );
        install(
            &mut grammars,
            LanguageId::Yaml,
            tree_sitter_yaml::LANGUAGE.into(),
            &[tree_sitter_yaml::HIGHLIGHTS_QUERY, YAML_FALLBACK],
        );
        install(
            &mut grammars,
            LanguageId::Html,
            tree_sitter_html::LANGUAGE.into(),
            &[tree_sitter_html::HIGHLIGHTS_QUERY, HTML_FALLBACK],
        );
        install(
            &mut grammars,
            LanguageId::Css,
            tree_sitter_css::LANGUAGE.into(),
            &[tree_sitter_css::HIGHLIGHTS_QUERY, CSS_FALLBACK],
        );
        install(
            &mut grammars,
            LanguageId::Markdown,
            tree_sitter_md::LANGUAGE.into(),
            &[tree_sitter_md::HIGHLIGHT_QUERY_BLOCK],
        );
        Self {
            grammars,
            markdown_inline: markdown_inline(),
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
        let grammar = self.grammars.get_mut(&language)?;
        let text = rope.to_string();
        grammar.parser.parse(text, old)
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
        self.highlight_range(tree, language, rope, start_byte, end_byte)
    }

    pub fn highlight_snippet(&mut self, language: LanguageId, source: &str) -> Vec<HighlightSpan> {
        let rope = Rope::from_str(source);
        let tree = {
            let Some(grammar) = self.grammars.get_mut(&language) else {
                return Vec::new();
            };
            let Some(tree) = grammar.parser.parse(source, None) else {
                return Vec::new();
            };
            tree
        };
        self.highlight_range(&tree, language, &rope, 0, source.len())
    }

    fn highlight_range(
        &self,
        tree: &Tree,
        language: LanguageId,
        rope: &Rope,
        start_byte: usize,
        end_byte: usize,
    ) -> Vec<HighlightSpan> {
        let Some(query) = self
            .grammars
            .get(&language)
            .and_then(|grammar| grammar.query.as_ref())
        else {
            return Vec::new();
        };
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(start_byte..end_byte);
        let text = rope.to_string();
        let mut captures = cursor.captures(query, tree.root_node(), text.as_bytes());
        let mut spans = Vec::new();
        while let Some((m, idx)) = captures.next() {
            let cap = m.captures()[*idx];
            let name = query.capture_names()[cap.index as usize];
            let node = cap.node;
            let kind = if language == LanguageId::Markdown {
                markdown_kind(name, node.kind())
            } else {
                kind_from_capture(name)
            };
            if let Some(kind) = kind {
                spans.push(HighlightSpan {
                    start: byte_to_position(rope, node.start_byte()),
                    end: byte_to_position(rope, node.end_byte()),
                    kind,
                });
            }
        }
        if language == LanguageId::Markdown {
            spans.extend(self.inline_highlights(tree, rope, &text, start_byte, end_byte));
        }
        // A JSON key is also a string. Keep the key color when both spans cover it.
        let keys: Vec<_> = spans
            .iter()
            .filter(|span| span.kind == HighlightKind::Property)
            .map(|span| (span.start, span.end))
            .collect();
        spans.retain(|span| {
            span.kind != HighlightKind::String
                || !keys
                    .iter()
                    .any(|(start, end)| *start == span.start && *end == span.end)
        });
        spans
    }
}

fn install(
    grammars: &mut HashMap<LanguageId, Grammar>,
    id: LanguageId,
    language: Language,
    sources: &[&str],
) {
    let mut parser = Parser::new();
    if parser.set_language(&language).is_err() {
        return;
    }
    let query = sources
        .iter()
        .find_map(|source| Query::new(&language, source).ok());
    grammars.insert(id, Grammar { parser, query });
}

fn install_joined(
    grammars: &mut HashMap<LanguageId, Grammar>,
    id: LanguageId,
    language: Language,
    sources: &[&str],
) {
    let mut parser = Parser::new();
    if parser.set_language(&language).is_err() {
        return;
    }
    let query = (1..=sources.len()).rev().find_map(|len| {
        let combined = sources[..len].join("\n");
        Query::new(&language, &combined).ok()
    });
    grammars.insert(id, Grammar { parser, query });
}

fn markdown_inline() -> InlineGrammar {
    let mut parser = Parser::new();
    let language: Language = tree_sitter_md::INLINE_LANGUAGE.into();
    if parser.set_language(&language).is_err() {
        return InlineGrammar {
            parser: RefCell::new(parser),
            query: None,
        };
    }
    let with_strike = format!(
        "{}\n(strikethrough) @text.strike\n",
        tree_sitter_md::HIGHLIGHT_QUERY_INLINE
    );
    let query = Query::new(&language, &with_strike)
        .ok()
        .or_else(|| Query::new(&language, tree_sitter_md::HIGHLIGHT_QUERY_INLINE).ok());
    InlineGrammar {
        parser: RefCell::new(parser),
        query,
    }
}

impl SyntaxService {
    fn inline_highlights(
        &self,
        tree: &Tree,
        rope: &Rope,
        text: &str,
        start_byte: usize,
        end_byte: usize,
    ) -> Vec<HighlightSpan> {
        let Some(query) = self.markdown_inline.query.as_ref() else {
            return Vec::new();
        };
        let mut parser = self.markdown_inline.parser.borrow_mut();
        let mut spans = Vec::new();
        collect_inline(
            tree.root_node(),
            rope,
            text,
            start_byte,
            end_byte,
            &mut parser,
            query,
            &mut spans,
        );
        spans
    }
}

fn collect_inline(
    node: Node,
    rope: &Rope,
    text: &str,
    start_byte: usize,
    end_byte: usize,
    parser: &mut Parser,
    query: &Query,
    spans: &mut Vec<HighlightSpan>,
) {
    if node.end_byte() <= start_byte || node.start_byte() >= end_byte {
        return;
    }
    if matches!(node.kind(), "inline" | "pipe_table_cell") {
        highlight_inline_node(node, rope, text, parser, query, spans);
        return;
    }
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            collect_inline(
                cursor.node(),
                rope,
                text,
                start_byte,
                end_byte,
                parser,
                query,
                spans,
            );
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

fn highlight_inline_node(
    node: Node,
    rope: &Rope,
    text: &str,
    parser: &mut Parser,
    query: &Query,
    spans: &mut Vec<HighlightSpan>,
) {
    let start = node.start_byte();
    let end = node.end_byte();
    if start >= end || end > text.len() {
        return;
    }
    let slice = &text[start..end];
    let Some(inline_tree) = parser.parse(slice, None) else {
        return;
    };
    let mut cursor = QueryCursor::new();
    let mut captures = cursor.captures(query, inline_tree.root_node(), slice.as_bytes());
    while let Some((m, idx)) = captures.next() {
        let cap = m.captures()[*idx];
        let name = query.capture_names()[cap.index as usize];
        let inline_node = cap.node;
        let Some(kind) = markdown_kind(name, inline_node.kind()) else {
            continue;
        };
        let span_start = start + inline_node.start_byte();
        let span_end = start + inline_node.end_byte();
        if span_end <= span_start {
            continue;
        }
        spans.push(HighlightSpan {
            start: byte_to_position(rope, span_start),
            end: byte_to_position(rope, span_end),
            kind,
        });
    }
}

fn markdown_kind(name: &str, node_kind: &str) -> Option<HighlightKind> {
    match node_kind {
        "atx_h1_marker"
        | "atx_h2_marker"
        | "atx_h3_marker"
        | "atx_h4_marker"
        | "atx_h5_marker"
        | "atx_h6_marker"
        | "setext_h1_underline"
        | "setext_h2_underline" => {
            return Some(HighlightKind::Heading);
        }
        "block_quote_marker" | "block_continuation" => return Some(HighlightKind::Comment),
        "list_marker_plus"
        | "list_marker_minus"
        | "list_marker_star"
        | "list_marker_dot"
        | "list_marker_parenthesis"
        | "thematic_break" => {
            return Some(HighlightKind::Operator);
        }
        "strikethrough" => return Some(HighlightKind::Strike),
        _ => {}
    }
    match name {
        "text.title" => Some(HighlightKind::Heading),
        "text.emphasis" => Some(HighlightKind::Emphasis),
        "text.strong" => Some(HighlightKind::Strong),
        "text.strike" => Some(HighlightKind::Strike),
        "text.literal" => Some(HighlightKind::String),
        "text.uri" | "text.reference" => Some(HighlightKind::Link),
        "none" => None,
        _ => kind_from_capture(name),
    }
}

fn kind_from_capture(name: &str) -> Option<HighlightKind> {
    if name == "string.special.key" {
        return Some(HighlightKind::Property);
    }
    let base = name.split('.').next().unwrap_or(name);
    Some(match base {
        "comment" => HighlightKind::Comment,
        "string" | "string.special" => HighlightKind::String,
        "number" => HighlightKind::Number,
        "keyword" | "repeat" | "conditional" | "include" | "exception" => HighlightKind::Keyword,
        "type" | "constructor" | "tag" => HighlightKind::Type,
        "function" | "method" => HighlightKind::Function,
        "property" | "field" | "attribute" => HighlightKind::Property,
        "constant" | "boolean" | "null" => HighlightKind::Constant,
        "operator" | "punctuation" => HighlightKind::Operator,
        "variable" | "parameter" | "identifier" => HighlightKind::Variable,
        _ => return None,
    })
}

const JSON_FALLBACK: &str = r#"
(pair key: (_) @property)
(string) @string
(number) @number
[(true) (false) (null)] @constant
(comment) @comment
"#;

const YAML_FALLBACK: &str = r#"
(comment) @comment
(block_mapping_pair
  key: (flow_node (plain_scalar (string_scalar) @property)))
(string_scalar) @string
[(integer_scalar) (float_scalar)] @number
(boolean_scalar) @constant
"#;

const HTML_FALLBACK: &str = r#"
(tag_name) @tag
(attribute_name) @attribute
(attribute_value) @string
(comment) @comment
"#;

const CSS_FALLBACK: &str = r#"
(comment) @comment
(tag_name) @tag
(property_name) @property
(class_name) @property
(string_value) @string
[(integer_value) (float_value)] @number
(at_keyword) @keyword
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

    fn open(path: &str, text: &str) -> (Document, SyntaxService) {
        let doc = Document::new(DocumentId(1), Some(path.into()), text.into());
        let mut syntax = SyntaxService::new();
        syntax.open(doc.id, doc.language, &doc.rope);
        (doc, syntax)
    }

    fn kind_at(spans: &[HighlightSpan], line: usize, column: usize) -> Option<HighlightKind> {
        spans.iter().rev().find_map(|span| {
            let starts_before =
                span.start.line < line || (span.start.line == line && span.start.column <= column);
            let ends_after =
                span.end.line > line || (span.end.line == line && span.end.column > column);
            (starts_before && ends_after).then_some(span.kind)
        })
    }

    #[test]
    fn highlights_typescript_keywords() {
        let mut doc = Document::new(DocumentId(1), Some("a.ts".into()), "const x = 1;\n".into());
        let mut syntax = SyntaxService::new();
        syntax.open(doc.id, doc.language, &doc.rope);
        let spans = syntax.highlights(doc.id, doc.language, &doc.rope, 0, 2);
        assert_eq!(kind_at(&spans, 0, 0), Some(HighlightKind::Keyword));
        let mut view = EditorView::new(doc.id);
        let changes = insert_text(&mut doc, &mut view, "function f() {}\n", false);
        for change in changes {
            syntax.apply_edit(&change, doc.language, &doc.rope);
        }
        let spans = syntax.highlights(doc.id, doc.language, &doc.rope, 0, 4);
        assert!(spans.iter().any(|span| span.kind == HighlightKind::Keyword));
    }

    #[test]
    fn highlights_tsx_keywords_and_tags() {
        let source = "export const View = () => <button title=\"x\" />;\n";
        let (doc, syntax) = open("a.tsx", source);
        let spans = syntax.highlights(doc.id, doc.language, &doc.rope, 0, 1);
        let tag = source.find("button").unwrap();
        assert_eq!(kind_at(&spans, 0, 0), Some(HighlightKind::Keyword));
        assert_eq!(
            kind_at(&spans, 0, tag),
            Some(HighlightKind::Type),
            "{spans:?}"
        );
    }

    #[test]
    fn highlights_json_keys_numbers_and_literals() {
        let (doc, syntax) = open("a.json", "{\n  \"name\": true,\n  \"count\": 1\n}\n");
        let spans = syntax.highlights(doc.id, doc.language, &doc.rope, 0, 4);
        assert_eq!(kind_at(&spans, 1, 3), Some(HighlightKind::Property));
        assert_eq!(kind_at(&spans, 1, 11), Some(HighlightKind::Constant));
        assert_eq!(kind_at(&spans, 2, 11), Some(HighlightKind::Number));
    }

    #[test]
    fn highlights_yaml_comments_and_keys() {
        let (doc, syntax) = open("a.yaml", "# note\nname: app\n");
        let spans = syntax.highlights(doc.id, doc.language, &doc.rope, 0, 2);
        assert_eq!(kind_at(&spans, 0, 2), Some(HighlightKind::Comment));
        assert_eq!(kind_at(&spans, 1, 0), Some(HighlightKind::Property));
    }

    #[test]
    fn highlights_html_tag_names() {
        let (doc, syntax) = open("a.html", "<p class=\"x\">hi</p>\n");
        let spans = syntax.highlights(doc.id, doc.language, &doc.rope, 0, 1);
        assert_eq!(kind_at(&spans, 0, 1), Some(HighlightKind::Type));
        assert_eq!(kind_at(&spans, 0, 3), Some(HighlightKind::Property));
    }

    #[test]
    fn highlights_markdown_heading_bold_and_code() {
        let source = "# Title\n\n**bold** and `code` and ~~gone~~\n";
        let (doc, syntax) = open("a.md", source);
        assert_eq!(doc.language, LanguageId::Markdown);
        let spans = syntax.highlights(doc.id, doc.language, &doc.rope, 0, 3);
        let line = source.lines().nth(2).unwrap();
        assert_eq!(kind_at(&spans, 0, 2), Some(HighlightKind::Heading));
        assert_eq!(
            kind_at(&spans, 2, line.find("bold").unwrap()),
            Some(HighlightKind::Strong)
        );
        assert_eq!(
            kind_at(&spans, 2, line.find("code").unwrap()),
            Some(HighlightKind::String)
        );
        assert_eq!(
            kind_at(&spans, 2, line.find("gone").unwrap()),
            Some(HighlightKind::Strike)
        );
    }

    #[test]
    fn plain_text_has_no_spans() {
        let (doc, syntax) = open("a.txt", "hello\n");
        let spans = syntax.highlights(doc.id, doc.language, &doc.rope, 0, 1);
        assert!(spans.is_empty());
    }
}
