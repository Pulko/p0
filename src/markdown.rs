use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::editor::{DocumentId, LanguageId};
use crate::syntax::{HighlightSpan, SyntaxService};
use crate::ui::theme::{Theme, highlight_color, highlight_modifier};

pub struct Preview {
    pub document_id: DocumentId,
    pub scroll: usize,
    revision: u64,
    width: usize,
    theme: &'static str,
    cached: bool,
    lines: Vec<Line<'static>>,
}

impl Preview {
    pub fn open(document_id: DocumentId) -> Self {
        Self {
            document_id,
            scroll: 0,
            revision: 0,
            width: 0,
            theme: "",
            cached: false,
            lines: Vec::new(),
        }
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn is_current(&self, revision: u64, theme: &str, width: usize) -> bool {
        self.cached && self.revision == revision && self.width == width && self.theme == theme
    }

    pub fn store(
        &mut self,
        lines: Vec<Line<'static>>,
        revision: u64,
        theme: &'static str,
        width: usize,
    ) {
        self.lines = lines;
        self.revision = revision;
        self.width = width;
        self.theme = theme;
        self.cached = true;
        let max = self.lines.len().saturating_sub(1);
        self.scroll = self.scroll.min(max);
    }

    pub fn visible(&self, height: usize) -> Vec<Line<'static>> {
        self.lines
            .iter()
            .skip(self.scroll)
            .take(height)
            .cloned()
            .collect()
    }
}

fn normalize_newlines(source: &str) -> std::borrow::Cow<'_, str> {
    if !source.contains('\r') {
        return std::borrow::Cow::Borrowed(source);
    }
    std::borrow::Cow::Owned(source.replace("\r\n", "\n").replace('\r', "\n"))
}

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
}

struct ListFrame {
    next: Option<u64>,
    fresh: bool,
    marker_width: usize,
}

struct LinkState {
    url: String,
    label: String,
}

#[derive(Default)]
struct Cell {
    runs: Vec<(String, Style)>,
}

struct TableState {
    alignments: Vec<Alignment>,
    rows: Vec<Vec<Cell>>,
    cell: Cell,
    in_cell: bool,
    header_rows: usize,
}

struct Builder<'a> {
    theme: Theme,
    width: usize,
    lines: Vec<Line<'static>>,
    line: Vec<Span<'static>>,
    line_width: usize,
    line_open: bool,
    has_content: bool,
    style: Style,
    stack: Vec<Style>,
    quote: usize,
    lists: Vec<ListFrame>,
    task: Option<bool>,
    in_code: bool,
    code_buf: String,
    code_lang: Option<String>,
    links: Vec<LinkState>,
    table: Option<TableState>,
    syntax: &'a mut SyntaxService,
}

pub(crate) fn render(
    source: &str,
    theme: Theme,
    width: usize,
    syntax: &mut SyntaxService,
) -> Vec<Line<'static>> {
    let source = normalize_newlines(source);
    let mut builder = Builder {
        theme,
        width,
        lines: Vec::new(),
        line: Vec::new(),
        line_width: 0,
        line_open: false,
        has_content: false,
        style: Style::default().fg(theme.fg),
        stack: Vec::new(),
        quote: 0,
        lists: Vec::new(),
        task: None,
        in_code: false,
        code_buf: String::new(),
        code_lang: None,
        links: Vec::new(),
        table: None,
        syntax,
    };
    for event in Parser::new_ext(source.as_ref(), options()) {
        builder.event(event);
    }
    builder.finish_line();
    if builder.lines.is_empty() {
        builder.lines.push(Line::from(""));
    }
    builder.lines
}

impl Builder<'_> {
    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => {
                if self.in_code {
                    self.code_buf.push_str(&text);
                } else {
                    self.emit(&text, self.style);
                }
            }
            Event::Code(text) => self.emit(
                &text,
                Style::default()
                    .fg(self.theme.string)
                    .bg(self.theme.bg_current),
            ),
            Event::Html(html) => self.emit(&html, Style::default().fg(self.theme.fg_dim)),
            Event::InlineHtml(html) => self.emit(&html, Style::default().fg(self.theme.fg_dim)),
            Event::FootnoteReference(label) => {
                self.emit(
                    &format!("[^{label}]"),
                    Style::default().fg(self.theme.fg_dim),
                );
            }
            Event::SoftBreak => {
                if self.in_code {
                    self.code_buf.push('\n');
                } else if self.in_cell() {
                    self.push_cell(" ", self.style);
                } else {
                    self.place_space(1, self.style);
                }
            }
            Event::HardBreak => {
                if self.in_code {
                    self.code_buf.push('\n');
                } else if self.in_cell() {
                    self.push_cell(" ", self.style);
                } else {
                    self.finish_line();
                }
            }
            Event::Rule => {
                self.finish_line();
                self.rule();
                self.separate();
            }
            Event::TaskListMarker(checked) => self.task = Some(checked),
            Event::InlineMath(text) | Event::DisplayMath(text) => {
                self.emit(&text, Style::default().fg(self.theme.string));
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph | Tag::TableHead => {}
            Tag::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    table.rows.push(Vec::new());
                }
            }
            Tag::TableCell => {
                if let Some(table) = self.table.as_mut() {
                    table.in_cell = true;
                    table.cell = Cell::default();
                }
            }
            Tag::Heading { level, .. } => {
                self.finish_line();
                self.push_style(heading_style(self.theme, level));
            }
            Tag::BlockQuote(_) => {
                self.finish_line();
                self.quote += 1;
            }
            Tag::CodeBlock(kind) => {
                self.finish_line();
                self.in_code = true;
                self.code_buf.clear();
                self.code_lang = match &kind {
                    CodeBlockKind::Fenced(lang) => {
                        let lang = lang.trim();
                        if lang.is_empty() {
                            None
                        } else {
                            Some(lang.to_string())
                        }
                    }
                    CodeBlockKind::Indented => None,
                };
            }
            Tag::HtmlBlock => {
                self.finish_line();
                self.push_style(Style::default().fg(self.theme.fg_dim));
            }
            Tag::List(start) => {
                if self.lists.last().is_some_and(|list| list.fresh) {
                    self.ensure_line();
                    self.finish_line();
                }
                self.lists.push(ListFrame {
                    next: start,
                    fresh: false,
                    marker_width: 2,
                });
            }
            Tag::Item => {
                self.finish_line();
                if let Some(list) = self.lists.last_mut() {
                    list.fresh = true;
                }
                self.task = None;
            }
            Tag::FootnoteDefinition(label) => {
                self.finish_line();
                self.write_text(
                    &format!("[^{label}]: "),
                    Style::default().fg(self.theme.fg_dim),
                );
            }
            Tag::Table(alignments) => {
                self.finish_line();
                self.table = Some(TableState {
                    alignments,
                    rows: Vec::new(),
                    cell: Cell::default(),
                    in_cell: false,
                    header_rows: 0,
                });
            }
            Tag::Emphasis => self.push_mod(Modifier::ITALIC, None),
            Tag::Strong => self.push_mod(Modifier::BOLD, None),
            Tag::Strikethrough => self.push_mod(Modifier::CROSSED_OUT, Some(self.theme.fg_dim)),
            Tag::Link { dest_url, .. } => {
                self.links.push(LinkState {
                    url: dest_url.to_string(),
                    label: String::new(),
                });
                self.push_mod(Modifier::UNDERLINED, Some(self.theme.accent));
            }
            Tag::Image { .. } => self.push_mod(Modifier::empty(), Some(self.theme.fg_muted)),
            Tag::MetadataBlock(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Superscript
            | Tag::Subscript => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::FootnoteDefinition => {
                if self.in_cell() {
                    self.push_cell(" ", self.style);
                } else {
                    self.finish_line();
                    self.separate();
                }
            }
            TagEnd::Heading(level) => {
                self.pop_style();
                self.finish_line();
                if self.lists.is_empty() && self.quote == 0 {
                    match level {
                        HeadingLevel::H1 => self.rule(),
                        HeadingLevel::H2 => {
                            self.rule_of((self.width / 2).max(3));
                        }
                        HeadingLevel::H3
                        | HeadingLevel::H4
                        | HeadingLevel::H5
                        | HeadingLevel::H6 => {}
                    }
                }
                self.separate();
            }
            TagEnd::BlockQuote(_) => {
                self.finish_line();
                self.quote = self.quote.saturating_sub(1);
                self.separate();
            }
            TagEnd::CodeBlock => {
                self.in_code = false;
                self.flush_code();
                self.separate();
            }
            TagEnd::HtmlBlock => {
                self.pop_style();
                self.finish_line();
                self.separate();
            }
            TagEnd::List(_) => {
                self.finish_line();
                self.lists.pop();
                self.separate();
            }
            TagEnd::Item => self.finish_line(),
            TagEnd::Table => self.render_table(),
            TagEnd::TableHead => {
                if let Some(table) = self.table.as_mut() {
                    table.header_rows = table.rows.len();
                }
            }
            TagEnd::TableRow => {}
            TagEnd::TableCell => {
                if let Some(table) = self.table.as_mut() {
                    table.in_cell = false;
                    let cell = std::mem::take(&mut table.cell);
                    if let Some(row) = table.rows.last_mut() {
                        row.push(cell);
                    }
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Image => {
                self.pop_style();
            }
            TagEnd::Link => {
                let link = self.links.pop();
                self.pop_style();
                if let Some(link) = link {
                    let label = link.label.trim();
                    if !link.url.is_empty() && link.url != label {
                        let dim = Style::default().fg(self.theme.fg_dim);
                        self.write_text(" ", dim);
                        self.write_text(&link.url, dim);
                    }
                }
            }
            TagEnd::MetadataBlock(_)
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Superscript
            | TagEnd::Subscript => {}
        }
    }

    fn separate(&mut self) {
        if self.lists.is_empty() && self.quote == 0 && self.table.is_none() {
            self.blank();
        }
    }

    fn emit(&mut self, text: &str, style: Style) {
        if let Some(link) = self.links.last_mut() {
            link.label.push_str(text);
        }
        self.write_text(text, style);
    }

    fn write_text(&mut self, text: &str, style: Style) {
        if self.in_cell() {
            if !text.is_empty() {
                self.push_cell(&text.replace('\n', " "), style);
            }
            return;
        }
        let mut rest = text;
        loop {
            if let Some((line, next)) = rest.split_once('\n') {
                self.write_wrapped(line, style);
                self.finish_line();
                rest = next;
            } else {
                self.write_wrapped(rest, style);
                break;
            }
        }
    }

    fn write_wrapped(&mut self, text: &str, style: Style) {
        let mut word = String::new();
        let mut word_width = 0usize;
        for grapheme in text.graphemes(true) {
            if grapheme.chars().all(char::is_whitespace) {
                self.place_word(&word, word_width, style);
                word.clear();
                word_width = 0;
                let width = if grapheme == "\t" {
                    4
                } else {
                    UnicodeWidthStr::width(grapheme).max(1)
                };
                self.place_space(width, style);
                continue;
            }
            word.push_str(grapheme);
            word_width += UnicodeWidthStr::width(grapheme).max(1);
        }
        self.place_word(&word, word_width, style);
    }

    fn place_space(&mut self, width: usize, style: Style) {
        if !self.has_content || width == 0 {
            return;
        }
        if self.line_width + width > self.width {
            self.finish_line();
            return;
        }
        self.push_span(&" ".repeat(width), style);
    }

    fn place_word(&mut self, word: &str, word_width: usize, style: Style) {
        if word.is_empty() {
            return;
        }
        self.ensure_line();
        let room = self.width.saturating_sub(self.line_width);
        if self.has_content && word_width > room {
            self.finish_line();
            self.ensure_line();
        }
        let room = self.width.saturating_sub(self.line_width);
        if room == 0 || word_width <= room {
            self.push_span(word, style);
            self.has_content = true;
            return;
        }
        for grapheme in word.graphemes(true) {
            let grapheme_width = UnicodeWidthStr::width(grapheme).max(1);
            self.ensure_line();
            let room = self.width.saturating_sub(self.line_width);
            if self.has_content && grapheme_width > room {
                self.finish_line();
                self.ensure_line();
            }
            self.push_span(grapheme, style);
            self.has_content = true;
        }
    }

    fn ensure_line(&mut self) {
        if self.line_open {
            return;
        }
        self.line_open = true;
        self.push_prefix();
    }

    fn push_prefix(&mut self) {
        let quote = Style::default().fg(self.theme.accent);
        for _ in 0..self.quote {
            self.push_span("│ ", quote);
        }
        let marker_style = Style::default().fg(self.theme.operator);
        let count = self.lists.len();
        for index in 0..count {
            if self.lists[index].fresh {
                let marker = self.item_marker(index);
                self.lists[index].marker_width = UnicodeWidthStr::width(marker.as_str()).max(1);
                self.lists[index].fresh = false;
                self.push_span(&marker, marker_style);
            } else {
                let width = self.lists[index].marker_width;
                self.push_span(&" ".repeat(width), marker_style);
            }
        }
    }

    fn item_marker(&mut self, index: usize) -> String {
        if index + 1 == self.lists.len()
            && let Some(checked) = self.task.take()
        {
            return if checked {
                "[x] ".into()
            } else {
                "[ ] ".into()
            };
        }
        if let Some(number) = self.lists[index].next {
            self.lists[index].next = Some(number.saturating_add(1));
            format!("{number}. ")
        } else {
            "• ".into()
        }
    }

    fn finish_line(&mut self) {
        if !self.line_open {
            return;
        }
        self.lines.push(Line::from(std::mem::take(&mut self.line)));
        self.line_width = 0;
        self.line_open = false;
        self.has_content = false;
    }

    fn blank(&mut self) {
        self.finish_line();
        let empty = self.lines.last().is_none_or(|line| {
            line.spans
                .iter()
                .all(|span| span.content.chars().all(char::is_whitespace))
        });
        if !empty {
            self.lines.push(Line::from(""));
        }
    }

    fn rule(&mut self) {
        self.rule_of(self.width.max(1));
    }

    fn rule_of(&mut self, width: usize) {
        let width = width.max(1).min(self.width.max(1));
        self.lines.push(Line::from(Span::styled(
            "─".repeat(width),
            Style::default().fg(self.theme.fg_dim),
        )));
    }

    fn flush_code(&mut self) {
        let source = std::mem::take(&mut self.code_buf);
        let lang = self.code_lang.take();
        let spans = match lang.as_deref().and_then(fence_language) {
            Some(language) => self.syntax.highlight_snippet(language, &source),
            None => Vec::new(),
        };
        let panel = Style::default()
            .fg(self.theme.string)
            .bg(self.theme.bg_current);
        if let Some(name) = lang
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            self.push_padded(
                name,
                Style::default()
                    .fg(self.theme.fg_dim)
                    .bg(self.theme.bg_current),
            );
        }
        for (index, line) in code_lines(&source).iter().enumerate() {
            self.push_code_line(index, line, &spans, panel);
        }
    }

    fn push_padded(&mut self, text: &str, style: Style) {
        let mut parts = vec![Span::styled(text.to_string(), style)];
        let used = UnicodeWidthStr::width(text);
        if used < self.width {
            parts.push(Span::styled(" ".repeat(self.width - used), style));
        }
        self.finish_line();
        self.lines.push(Line::from(parts));
    }

    fn push_code_line(
        &mut self,
        line_idx: usize,
        text: &str,
        spans: &[HighlightSpan],
        base: Style,
    ) {
        let mut parts = Vec::new();
        let mut display = 0usize;
        let mut column = 0usize;
        let mut buf = String::new();
        let mut current = base;
        let mut open = false;
        for ch in text.chars() {
            let style = code_char_style(spans, line_idx, column, self.theme, base);
            let piece = if ch == '\t' {
                "    ".to_string()
            } else {
                ch.to_string()
            };
            if open && style != current {
                display += UnicodeWidthStr::width(buf.as_str());
                parts.push(Span::styled(std::mem::take(&mut buf), current));
            }
            current = style;
            open = true;
            buf.push_str(&piece);
            column += 1;
        }
        if !buf.is_empty() {
            display += UnicodeWidthStr::width(buf.as_str());
            parts.push(Span::styled(buf, current));
        }
        if display < self.width {
            parts.push(Span::styled(" ".repeat(self.width - display), base));
        }
        if parts.is_empty() {
            parts.push(Span::styled(" ".repeat(self.width.max(1)), base));
        }
        self.finish_line();
        self.lines.push(Line::from(parts));
    }

    fn push_span(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        self.line_width += UnicodeWidthStr::width(text);
        self.line.push(Span::styled(text.to_string(), style));
    }

    fn push_mod(&mut self, modifier: Modifier, fg: Option<Color>) {
        let mut style = self.style.add_modifier(modifier);
        if let Some(fg) = fg {
            style = style.fg(fg);
        }
        self.push_style(style);
    }

    fn push_style(&mut self, style: Style) {
        self.stack.push(self.style);
        self.style = style;
    }

    fn pop_style(&mut self) {
        if let Some(style) = self.stack.pop() {
            self.style = style;
        }
    }

    fn in_cell(&self) -> bool {
        self.table.as_ref().is_some_and(|table| table.in_cell)
    }

    fn push_cell(&mut self, text: &str, style: Style) {
        let Some(table) = self.table.as_mut() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        if let Some((last, last_style)) = table.cell.runs.last_mut()
            && *last_style == style
        {
            last.push_str(text);
            return;
        }
        table.cell.runs.push((text.to_string(), style));
    }

    fn render_table(&mut self) {
        let Some(table) = self.table.take() else {
            return;
        };
        let cols = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        let mut widths = vec![0usize; cols];
        for row in &table.rows {
            for (index, cell) in row.iter().enumerate() {
                widths[index] = widths[index].max(runs_width(&cell.runs));
            }
        }
        let gap = 2usize;
        let natural = widths.iter().sum::<usize>() + gap * cols.saturating_sub(1);
        if self.width > 0 && natural > self.width {
            let budget = self
                .width
                .saturating_sub(gap * cols.saturating_sub(1))
                .max(cols);
            let total = widths.iter().sum::<usize>().max(1);
            let mut assigned = 0usize;
            for (index, width) in widths.iter_mut().enumerate() {
                if index + 1 == cols {
                    *width = budget.saturating_sub(assigned).max(1);
                } else {
                    let share = ((*width as u128 * budget as u128) / total as u128).max(1) as usize;
                    *width = share;
                    assigned += share;
                }
            }
        }
        for (row_index, row) in table.rows.iter().enumerate() {
            self.finish_line();
            self.ensure_line();
            for (index, width) in widths.iter().enumerate() {
                if index > 0 {
                    self.push_span("  ", Style::default().fg(self.theme.fg));
                }
                let align = table
                    .alignments
                    .get(index)
                    .copied()
                    .unwrap_or(Alignment::None);
                self.emit_cell(row.get(index), *width, align, row_index < table.header_rows);
            }
            self.has_content = true;
            self.finish_line();
            if row_index + 1 == table.header_rows {
                self.ensure_line();
                for (index, width) in widths.iter().enumerate() {
                    if index > 0 {
                        self.push_span("  ", Style::default().fg(self.theme.fg_dim));
                    }
                    self.push_span(
                        &"─".repeat((*width).max(1)),
                        Style::default().fg(self.theme.fg_dim),
                    );
                }
                self.finish_line();
            }
        }
        self.separate();
    }

    fn emit_cell(&mut self, cell: Option<&Cell>, width: usize, align: Alignment, header: bool) {
        let runs = cell
            .map(|cell| truncate_runs(&cell.runs, width))
            .unwrap_or_default();
        let used = runs_width(&runs);
        let extra = width.saturating_sub(used);
        let (left, right) = match align {
            Alignment::Center => (extra / 2, extra - extra / 2),
            Alignment::Right => (extra, 0),
            Alignment::Left | Alignment::None => (0, extra),
        };
        let pad = Style::default().fg(self.theme.fg);
        if left > 0 {
            self.push_span(&" ".repeat(left), pad);
        }
        for (text, mut style) in runs {
            if header {
                style = style.add_modifier(Modifier::BOLD).fg(self.theme.accent);
            }
            self.push_span(&text, style);
        }
        if right > 0 {
            self.push_span(&" ".repeat(right), pad);
        }
    }
}

fn heading_style(theme: Theme, level: HeadingLevel) -> Style {
    match level {
        HeadingLevel::H1 | HeadingLevel::H2 => Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
        HeadingLevel::H3 => Style::default()
            .fg(theme.type_name)
            .add_modifier(Modifier::BOLD),
        HeadingLevel::H4 | HeadingLevel::H5 | HeadingLevel::H6 => {
            Style::default().fg(theme.type_name)
        }
    }
}

fn fence_language(info: &str) -> Option<LanguageId> {
    let token = info.split_whitespace().next()?.trim_start_matches('.');
    match token.to_ascii_lowercase().as_str() {
        "ts" | "typescript" => Some(LanguageId::TypeScript),
        "tsx" => Some(LanguageId::Tsx),
        "js" | "jsx" | "javascript" | "mjs" | "cjs" => Some(LanguageId::JavaScript),
        "json" | "jsonc" => Some(LanguageId::Json),
        "yaml" | "yml" => Some(LanguageId::Yaml),
        "html" | "htm" => Some(LanguageId::Html),
        "css" => Some(LanguageId::Css),
        _ => None,
    }
}

fn code_lines(source: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = source.split('\n').collect();
    if lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines
}

fn code_char_style(
    spans: &[HighlightSpan],
    line: usize,
    column: usize,
    theme: Theme,
    base: Style,
) -> Style {
    let kind = spans.iter().rev().find_map(|span| {
        let starts =
            span.start.line < line || (span.start.line == line && span.start.column <= column);
        let ends = span.end.line > line || (span.end.line == line && span.end.column > column);
        (starts && ends).then_some(span.kind)
    });
    match kind {
        Some(kind) => Style::default()
            .fg(highlight_color(kind, theme))
            .bg(theme.bg_current)
            .add_modifier(highlight_modifier(kind)),
        None => base,
    }
}

fn runs_width(runs: &[(String, Style)]) -> usize {
    runs.iter()
        .map(|(text, _)| UnicodeWidthStr::width(text.as_str()))
        .sum()
}

fn truncate_runs(runs: &[(String, Style)], max: usize) -> Vec<(String, Style)> {
    if runs_width(runs) <= max {
        return runs.to_vec();
    }
    let mut out = Vec::new();
    let mut used = 0usize;
    let limit = max.saturating_sub(1);
    for (text, style) in runs {
        let mut chunk = String::new();
        for grapheme in text.graphemes(true) {
            let grapheme_width = UnicodeWidthStr::width(grapheme).max(1);
            if used + grapheme_width > limit {
                if !chunk.is_empty() {
                    out.push((chunk, *style));
                }
                out.push(("…".into(), *style));
                return out;
            }
            chunk.push_str(grapheme);
            used += grapheme_width;
        }
        if !chunk.is_empty() {
            out.push((chunk, *style));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::SyntaxService;
    use crate::ui::theme::by_name;

    fn draw(source: &str, width: usize) -> Vec<Line<'static>> {
        let mut syntax = SyntaxService::new();
        render(source, by_name("p0"), width, &mut syntax)
    }

    fn plain(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn preview_shows_heading_and_list_without_markup() {
        let source = "# Title\n\n- item\n\n**bold**\n\n```\ncode\n```\n";
        let text = plain(&draw(source, 40));
        assert!(text.contains("Title"), "{text}");
        assert!(text.contains("•"), "{text}");
        assert!(text.contains("item"), "{text}");
        assert!(text.contains("bold"), "{text}");
        assert!(text.contains("code"), "{text}");
        assert!(!text.contains("**"), "{text}");
        assert!(!text.contains("```"), "{text}");
    }

    #[test]
    fn preview_renders_tasks_links_and_tables() {
        let source = "- [x] done\n\n[docs](https://example.com)\n\n| Name | Value |\n| --- | --- |\n| Ada | 1 |\n";
        let text = plain(&draw(source, 48));
        assert!(text.contains("[x]"), "{text}");
        assert!(text.contains("done"), "{text}");
        assert!(text.contains("docs"), "{text}");
        assert!(text.contains("https://example.com"), "{text}");
        assert!(text.contains("Ada"), "{text}");
        assert!(text.contains('1'), "{text}");
        assert!(!text.contains('|'), "{text}");
    }

    #[test]
    fn code_fence_keeps_indent_and_blank_lines() {
        let source = "```\n    let x = 1\n\n    return x\n```\n";
        let lines = draw(source, 40);
        let text = plain(&lines);
        let rows: Vec<&str> = text.split('\n').collect();
        let first = rows.iter().position(|row| row.starts_with("    let x = 1"));
        let second = rows.iter().position(|row| row.starts_with("    return x"));
        let (Some(first), Some(second)) = (first, second) else {
            panic!("code lines missing:\n{text}");
        };
        assert!(second > first + 1, "{text}");
        assert!(rows[first + 1].chars().all(char::is_whitespace), "{text}");
    }

    #[test]
    fn headings_step_down_toward_body_text() {
        let theme = by_name("p0");
        let mut syntax = SyntaxService::new();
        let lines = render("# Title\n\nHello\n\n### Smaller\n", theme, 40, &mut syntax);
        let title = span_with(&lines, "Title");
        assert_eq!(title.style.fg, Some(theme.accent));
        assert!(title.style.add_modifier.contains(Modifier::BOLD));
        let body = span_with(&lines, "Hello");
        assert_eq!(body.style.fg, Some(theme.fg));
        assert!(!body.style.add_modifier.contains(Modifier::BOLD));
        let smaller = span_with(&lines, "Smaller");
        assert_eq!(smaller.style.fg, Some(theme.type_name));
    }

    #[test]
    fn bash_fence_does_not_swallow_the_next_section() {
        let theme = by_name("p0");
        let body = "```bash\nnpm install -g typescript typescript-language-server\n```\n\n## Limits\n\n- Git marks need a worktree.\n";
        for source in [
            body.to_string(),
            body.replace('\n', "\r\n"),
            body.replace('\n', "\r"),
        ] {
            let lines = draw(&source, 72);
            let command = span_with(&lines, "npm install");
            assert_eq!(command.style.bg, Some(theme.bg_current), "{lines:?}");
            let limits = span_with(&lines, "Limits");
            assert_eq!(limits.style.fg, Some(theme.accent));
            assert!(limits.style.add_modifier.contains(Modifier::BOLD));
            assert_eq!(limits.style.bg, None);
            let item = span_with(&lines, "marks");
            assert_eq!(item.style.fg, Some(theme.fg));
            assert_eq!(item.style.bg, None);
            let text = plain(&lines);
            assert!(text.contains('•'), "{text}");
            assert!(!text.contains("##"), "{text}");
            assert!(!text.contains("```"), "{text}");
        }
    }

    #[test]
    fn javascript_fence_colors_keywords() {
        let theme = by_name("p0");
        let mut syntax = SyntaxService::new();
        let lines = render("```js\nconst x = 1\n```\n", theme, 40, &mut syntax);
        let keyword = span_with(&lines, "const");
        assert_eq!(keyword.style.fg, Some(theme.keyword), "{lines:?}");
    }

    fn span_with<'a>(lines: &'a [Line<'a>], text: &str) -> &'a Span<'a> {
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|span| span.content.contains(text))
            .unwrap_or_else(|| panic!("missing {text}"))
    }
}
