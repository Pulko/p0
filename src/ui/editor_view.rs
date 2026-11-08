use ratatui::Frame;
use ratatui::layout::{Position as TermPos, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::app::state::{HScrollHit, TabHitBox};
use crate::editor::{Range, display_col, hscroll_bar, line_text, max_line_display_width};
use crate::syntax::HighlightKind;
use crate::ui::theme::Theme;
use crate::workspace::{DiffKind, LineChanges, LineMark, VisualRow};

pub fn render(frame: &mut Frame, tabs: Rect, area: Rect, app: &mut App) {
    app.hscroll_hit = None;
    let theme = app.theme();
    render_tabs(frame, tabs, app, theme);
    let Some(id) = app.active else {
        frame.render_widget(
            Paragraph::new("Open a file from the explorer or press Ctrl+P")
                .style(theme.text())
                .block(theme.panel(" Editor ")),
            area,
        );
        return;
    };
    let Some(doc) = app.documents.get(&id) else {
        return;
    };
    let Some(view) = app.views.get(&id) else {
        return;
    };
    if app
        .diff_view
        .as_ref()
        .is_some_and(|diff| diff.document_id == id)
    {
        render_diff(frame, area, app, theme);
        return;
    }
    let tab_width = app.config.tab_width;
    let number_width = ((doc.rope.len_lines().max(1).ilog10() as u16) + 2).max(4);
    let gutter = number_width + 1;
    app.gutter_width = gutter;
    let inner_h = area.height.saturating_sub(2) as usize;
    let inner_w = area.width.saturating_sub(2 + gutter) as usize;
    let rows = app.visual_rows(id);
    let changes = app.inline_changes.get(&id).cloned();
    let visible_lines: Vec<usize> = rows
        .iter()
        .skip(view.viewport.top)
        .take(inner_h)
        .filter_map(|row| match row {
            VisualRow::Line(line) => Some(*line),
            VisualRow::Removed { .. } => None,
        })
        .collect();
    let highlight_start = visible_lines.first().copied().unwrap_or(0);
    let highlight_end = visible_lines.last().copied().unwrap_or(highlight_start);
    let highlights =
        app.syntax
            .highlights(id, doc.language, &doc.rope, highlight_start, highlight_end);
    let sel = view.selection.map(|s| s.range());
    let diag = doc
        .path
        .as_ref()
        .and_then(|p| app.diagnostics.get(p))
        .cloned()
        .unwrap_or_default();
    let search_matches = if let crate::app::state::Overlay::SearchFile {
        matches, current, ..
    } = &app.overlay
    {
        Some((matches.clone(), *current))
    } else {
        None
    };

    let mut lines = Vec::new();
    for row in 0..inner_h.max(1) {
        let Some(visual) = rows.get(view.viewport.top + row).cloned() else {
            lines.push(Line::from(""));
            continue;
        };
        if let VisualRow::Removed { text, .. } = &visual {
            lines.push(removed_line(
                text,
                view.viewport.left,
                inner_w,
                number_width,
                tab_width,
                theme,
            ));
            continue;
        }
        let VisualRow::Line(line_idx) = visual else {
            continue;
        };
        if line_idx >= doc.rope.len_lines() {
            lines.push(Line::from(""));
            continue;
        }
        let text = line_text(&doc.rope, line_idx);
        let ln = format!("{:>width$}", line_idx + 1, width = number_width as usize);
        let mut spans = vec![Span::styled(ln, Style::default().fg(theme.fg_dim))];
        spans.push(match changes.as_ref() {
            Some(changes) => gutter_span(changes, line_idx, theme),
            None => Span::raw(" "),
        });
        spans.extend(paint_line(
            &text,
            line_idx,
            view.viewport.left,
            inner_w,
            tab_width,
            view.cursor.line == line_idx,
            sel,
            &highlights,
            &diag,
            search_matches.as_ref(),
            theme,
        ));
        lines.push(Line::from(spans));
    }

    let title = doc
        .path
        .as_ref()
        .map(|p| format!(" {} ", app.relative(p)))
        .unwrap_or_else(|| " untitled ".into());
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme.text())
            .block(theme.panel(&title)),
        area,
    );

    if let Some(view) = app.views.get(&id) {
        let text = line_text(&doc.rope, view.cursor.line);
        let col = display_col(&text, view.cursor.column, tab_width);
        let cursor_row = rows
            .iter()
            .position(|row| matches!(row, VisualRow::Line(line) if *line == view.cursor.line));
        if let Some(cursor_row) = cursor_row {
            if cursor_row >= view.viewport.top
                && cursor_row < view.viewport.top + inner_h
                && col >= view.viewport.left
                && col < view.viewport.left + inner_w
            {
                let x = area.x + 1 + gutter + (col - view.viewport.left) as u16;
                let y = area.y + 1 + (cursor_row - view.viewport.top) as u16;
                frame.set_cursor_position(TermPos { x, y });
            }
        }
    }
    paint_hscroll(frame, area, app, theme, inner_w);
}

fn paint_hscroll(frame: &mut Frame, area: Rect, app: &mut App, theme: Theme, inner_w: usize) {
    if area.height < 3 || inner_w == 0 {
        return;
    }
    let Some(id) = app.active else { return };
    let tab_width = app.config.tab_width;
    let gutter = app.gutter_width;
    let (content, left) = {
        let Some(doc) = app.documents.get(&id) else {
            return;
        };
        let Some(view) = app.views.get(&id) else {
            return;
        };
        (
            max_line_display_width(&doc.rope, tab_width),
            view.viewport.left,
        )
    };
    let track_x = area.x.saturating_add(1).saturating_add(gutter);
    let Some(bar) = hscroll_bar(track_x, inner_w as u16, inner_w, content, left) else {
        return;
    };
    let y = area.y + area.height - 1;
    app.hscroll_hit = Some(HScrollHit {
        x: bar.track_x,
        y,
        width: bar.track_w,
        thumb_x: bar.thumb_x,
        thumb_w: bar.thumb_w,
        max_left: bar.max_left,
    });
    let thumb_start = bar.thumb_x.saturating_sub(bar.track_x);
    let thumb_end = thumb_start.saturating_add(bar.thumb_w);
    let mut spans = Vec::with_capacity(bar.track_w as usize);
    for i in 0..bar.track_w {
        let on_thumb = i >= thumb_start && i < thumb_end;
        let (glyph, style) = if on_thumb {
            ("━", Style::default().fg(theme.accent).bg(theme.bg))
        } else {
            ("─", Style::default().fg(theme.border).bg(theme.bg))
        };
        spans.push(Span::styled(glyph, style));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect {
            x: bar.track_x,
            y,
            width: bar.track_w,
            height: 1,
        },
    );
}

fn removed_line(
    text: &str,
    left: usize,
    width: usize,
    number_width: u16,
    tab_width: usize,
    theme: Theme,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(" ".repeat(number_width as usize), Style::default()),
        Span::styled("-", Style::default().fg(theme.on_accent).bg(theme.deleted)),
        Span::styled(
            scrolled_plain(text, left, width, tab_width),
            Style::default().fg(theme.deleted),
        ),
    ])
}

fn scrolled_plain(text: &str, left: usize, width: usize, tab_width: usize) -> String {
    let mut display = 0usize;
    let mut visible = 0usize;
    let mut out = String::new();
    for g in text.graphemes(true) {
        let w = if g == "\t" {
            tab_width - (display % tab_width.max(1))
        } else {
            UnicodeWidthStr::width(g).max(1)
        };
        let start = display;
        display += w;
        if display <= left {
            continue;
        }
        if start >= left + width || visible >= width {
            break;
        }
        if g == "\t" {
            let skip = left.saturating_sub(start);
            let take = w.saturating_sub(skip).min(width - visible);
            out.push_str(&" ".repeat(take));
            visible += take;
        } else {
            out.push_str(g);
            visible += w;
        }
    }
    if out.is_empty() {
        out.push(' ');
    }
    out
}

fn gutter_span(changes: &LineChanges, line_idx: usize, theme: Theme) -> Span<'static> {
    let mark = changes
        .marks
        .get(line_idx)
        .copied()
        .unwrap_or(LineMark::Unchanged);
    let deleted = changes.deleted_before.get(line_idx).copied().unwrap_or(0) > 0;
    if deleted && mark == LineMark::Unchanged {
        Span::styled("▾", Style::default().fg(theme.on_accent).bg(theme.deleted))
    } else {
        match mark {
            LineMark::Added => Span::styled(" ", Style::default().bg(theme.added)),
            LineMark::Modified => Span::styled(" ", Style::default().bg(theme.modified)),
            LineMark::Unchanged => Span::raw(" "),
        }
    }
}

fn render_diff(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    let Some(diff) = app.diff_view.as_ref() else {
        return;
    };
    let height = area.height.saturating_sub(2) as usize;
    let lines = if diff.loading {
        vec![Line::from(Span::styled(
            "Loading Git diff…",
            Style::default().fg(theme.fg_dim),
        ))]
    } else if let Some(error) = &diff.error {
        vec![Line::from(Span::styled(
            format!("Git diff unavailable: {error}"),
            Style::default().fg(theme.warning),
        ))]
    } else if diff.lines.len() <= 4 {
        vec![Line::from(Span::styled(
            "No changes from HEAD",
            Style::default().fg(theme.fg_dim),
        ))]
    } else {
        diff.lines
            .iter()
            .skip(diff.scroll)
            .take(height)
            .map(|line| {
                let style = match line.kind {
                    DiffKind::Header => Style::default().fg(theme.accent),
                    DiffKind::Insert => Style::default().fg(theme.added),
                    DiffKind::Delete => Style::default().fg(theme.deleted),
                    DiffKind::Equal => Style::default().fg(theme.fg_muted),
                };
                Line::from(Span::styled(line.text.clone(), style))
            })
            .collect()
    };
    frame.render_widget(
        Paragraph::new(lines).style(theme.text()).block(
            theme
                .panel(" Git diff: HEAD ↔ working buffer ")
                .border_style(Style::default().fg(theme.accent)),
        ),
        area,
    );
}

const TAB_CHEVRON: u16 = 2;

struct TabPlacement {
    index: usize,
    x: u16,
    width: u16,
    label: String,
}

struct TabStrip {
    scroll: usize,
    left: Option<u16>,
    right: Option<u16>,
    tabs: Vec<TabPlacement>,
}

fn render_tabs(frame: &mut Frame, area: Rect, app: &mut App, theme: Theme) {
    app.tab_hit_boxes.clear();
    app.tab_scroll_left = None;
    app.tab_scroll_right = None;
    let opened: Vec<(crate::editor::DocumentId, String)> = app
        .order
        .iter()
        .filter_map(|id| {
            let doc = app.documents.get(id)?;
            Some((*id, tab_label(doc)))
        })
        .collect();
    let labels: Vec<String> = opened.iter().map(|(_, label)| label.clone()).collect();
    let active_index = app
        .active
        .and_then(|id| opened.iter().position(|(tab_id, _)| *tab_id == id));
    let reveal = app.tab_revealed != app.active;
    let strip = layout_tab_strip(&labels, app.tab_scroll, area.width, active_index, reveal);
    app.tab_scroll = strip.scroll;
    app.tab_revealed = app.active;

    let mut spans = Vec::new();
    let mut cursor = 0u16;
    if let Some(x) = strip.left {
        push_gap(&mut spans, &mut cursor, x);
        spans.push(Span::styled("<<", Style::default().fg(theme.fg_dim)));
        cursor = cursor.saturating_add(TAB_CHEVRON);
        app.tab_scroll_left = Some((area.x.saturating_add(x), TAB_CHEVRON));
    }
    for placed in &strip.tabs {
        push_gap(&mut spans, &mut cursor, placed.x);
        let id = opened[placed.index].0;
        let mut style = Style::default().fg(theme.fg_muted);
        if app.active == Some(id) {
            style = Style::default()
                .fg(theme.fg)
                .bg(theme.bg_selected)
                .add_modifier(Modifier::BOLD);
        }
        spans.push(Span::styled(placed.label.clone(), style));
        if let Some(hit_box) = tab_hit_box(
            id,
            area.x.saturating_add(placed.x),
            placed.width,
            area.right(),
        ) {
            app.tab_hit_boxes.push(hit_box);
        }
        cursor = cursor.saturating_add(placed.width);
    }
    if let Some(x) = strip.right {
        push_gap(&mut spans, &mut cursor, x);
        spans.push(Span::styled(">>", Style::default().fg(theme.fg_dim)));
        app.tab_scroll_right = Some((area.x.saturating_add(x), TAB_CHEVRON));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)).style(theme.text()), area);
}

fn tab_label(doc: &crate::editor::Document) -> String {
    let name = doc
        .path
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".into());
    let dirty = if doc.is_dirty() { " •" } else { "" };
    format!(" {name}{dirty} ×")
}

fn layout_tab_strip(
    labels: &[String],
    mut scroll: usize,
    width: u16,
    active: Option<usize>,
    reveal: bool,
) -> TabStrip {
    if labels.is_empty() || width == 0 {
        return TabStrip {
            scroll: 0,
            left: None,
            right: None,
            tabs: Vec::new(),
        };
    }
    scroll = scroll.min(labels.len() - 1);
    if reveal {
        if let Some(active) = active.filter(|index| *index < labels.len()) {
            if active < scroll {
                scroll = active;
            }
            while scroll < active && !tab_is_visible(labels, scroll, width, active) {
                scroll += 1;
            }
        }
    }
    place_tabs(labels, scroll, width)
}

fn tab_is_visible(labels: &[String], scroll: usize, width: u16, active: usize) -> bool {
    place_tabs(labels, scroll, width)
        .tabs
        .iter()
        .any(|tab| tab.index == active)
}

fn place_tabs(labels: &[String], scroll: usize, width: u16) -> TabStrip {
    if labels.is_empty() || width == 0 {
        return TabStrip {
            scroll: 0,
            left: None,
            right: None,
            tabs: Vec::new(),
        };
    }
    let scroll = scroll.min(labels.len() - 1);
    let show_left = scroll > 0 && width >= TAB_CHEVRON;
    let left_w = if show_left { TAB_CHEVRON } else { 0 };
    let mut tabs = Vec::new();
    let mut x = left_w;
    let mut end = scroll;
    while end < labels.len() {
        let more_after = end + 1 < labels.len();
        let reserve = if more_after && width >= left_w.saturating_add(TAB_CHEVRON) {
            TAB_CHEVRON
        } else {
            0
        };
        let gap = if tabs.is_empty() { 0 } else { 1 };
        let room = width
            .saturating_sub(x)
            .saturating_sub(gap)
            .saturating_sub(reserve);
        if room == 0 {
            break;
        }
        let natural = display_width(&labels[end]) as u16;
        if natural > room && !tabs.is_empty() {
            break;
        }
        let label = fit_tab_label(&labels[end], room as usize);
        let tab_w = display_width(&label) as u16;
        if tab_w == 0 || tab_w > room {
            break;
        }
        x = x.saturating_add(gap);
        tabs.push(TabPlacement {
            index: end,
            x,
            width: tab_w,
            label,
        });
        x = x.saturating_add(tab_w);
        end += 1;
        if natural > room {
            break;
        }
    }
    let show_right = end < labels.len() && width >= left_w.saturating_add(TAB_CHEVRON);
    TabStrip {
        scroll,
        left: show_left.then_some(0),
        right: show_right.then_some(width - TAB_CHEVRON),
        tabs,
    }
}

fn fit_tab_label(label: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if display_width(label) <= max_width {
        return label.to_string();
    }
    let Some(stem) = label.strip_suffix(" ×") else {
        return truncate_width(label, max_width);
    };
    let close_w = display_width(" ×");
    if max_width <= close_w {
        return truncate_width(" ×", max_width);
    }
    let ellipsis = "…";
    let ellipsis_w = display_width(ellipsis);
    let stem_budget = max_width - close_w;
    if stem_budget <= ellipsis_w {
        return format!("{} ×", truncate_width(stem, stem_budget));
    }
    format!(
        "{}{ellipsis} ×",
        truncate_width(stem, stem_budget - ellipsis_w)
    )
}

fn push_gap(spans: &mut Vec<Span<'static>>, cursor: &mut u16, target: u16) {
    if target > *cursor {
        spans.push(Span::raw(" ".repeat((target - *cursor) as usize)));
        *cursor = target;
    }
}

fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn truncate_width(text: &str, max_width: usize) -> String {
    let mut out = String::new();
    let mut width = 0;
    for grapheme in text.graphemes(true) {
        let grapheme_width = UnicodeWidthStr::width(grapheme);
        if width + grapheme_width > max_width {
            break;
        }
        out.push_str(grapheme);
        width += grapheme_width;
    }
    out
}

fn tab_hit_box(
    document_id: crate::editor::DocumentId,
    x: u16,
    width: u16,
    right: u16,
) -> Option<TabHitBox> {
    (x.saturating_add(width) <= right).then_some(TabHitBox {
        document_id,
        x,
        width,
        close_x: x + width.saturating_sub(1),
    })
}

#[allow(clippy::too_many_arguments)]
fn paint_line(
    text: &str,
    line_idx: usize,
    left: usize,
    width: usize,
    tab_width: usize,
    current: bool,
    sel: Option<Range>,
    highlights: &[crate::syntax::HighlightSpan],
    diags: &[crate::language::Diagnostic],
    search: Option<&(
        Vec<(crate::editor::Position, crate::editor::Position)>,
        usize,
    )>,
    theme: Theme,
) -> Vec<Span<'static>> {
    let mut chars: Vec<(String, usize)> = Vec::new();
    let mut dcol = 0usize;
    let mut cidx = 0usize;
    for g in text.graphemes(true) {
        let w = if g == "\t" {
            tab_width - (dcol % tab_width.max(1))
        } else {
            UnicodeWidthStr::width(g).max(1)
        };
        chars.push((g.to_string(), w));
        dcol += w;
        cidx += g.chars().count();
        let _ = cidx;
    }
    let mut out = Vec::new();
    let mut display = 0usize;
    let mut col = 0usize;
    let bg_current = if current {
        Some(theme.bg_current)
    } else {
        None
    };
    for (g, w) in chars {
        let start_col = col;
        let end_col = col + g.chars().count();
        col = end_col;
        let start_disp = display;
        display += w;
        if display <= left {
            continue;
        }
        if start_disp >= left + width {
            break;
        }
        let mut style = Style::default();
        if let Some(bg) = bg_current {
            style = style.bg(bg);
        }
        if let Some(kind) = highlight_at(highlights, line_idx, start_col) {
            style = style.fg(color_of(kind, theme));
        }
        if in_range(sel, line_idx, start_col) {
            style = style.bg(theme.bg_selected);
        }
        if let Some((matches, current_match)) = search {
            for (i, (a, b)) in matches.iter().enumerate() {
                if a.line == line_idx && start_col >= a.column && start_col < b.column {
                    style = if i == *current_match {
                        style.bg(theme.search_current).fg(theme.on_accent)
                    } else {
                        style.bg(theme.search)
                    };
                }
            }
        }
        if let Some(diag) =
            crate::language::diagnostic_at(diags, crate::editor::Position::new(line_idx, start_col))
        {
            style = style
                .add_modifier(Modifier::UNDERLINED)
                .fg(diagnostic_color(diag.severity, theme));
        }
        let visible = if g == "\t" { " ".repeat(w) } else { g.clone() };
        out.push(Span::styled(visible, style));
    }
    if out.is_empty() {
        let mut style = Style::default();
        if let Some(bg) = bg_current {
            style = style.bg(bg);
        }
        out.push(Span::styled(" ", style));
    }
    out
}

fn diagnostic_color(severity: u8, theme: Theme) -> Color {
    match severity {
        2 => theme.warning,
        3 | 4 => theme.fg_dim,
        _ => theme.deleted,
    }
}

fn highlight_at(
    highlights: &[crate::syntax::HighlightSpan],
    line: usize,
    col: usize,
) -> Option<HighlightKind> {
    highlights.iter().rev().find_map(|h| {
        let on_line = if h.start.line == h.end.line {
            h.start.line == line && col >= h.start.column && col < h.end.column
        } else {
            line >= h.start.line && line <= h.end.line
        };
        on_line.then_some(h.kind)
    })
}

fn in_range(sel: Option<Range>, line: usize, col: usize) -> bool {
    let Some(sel) = sel else { return false };
    let pos = crate::editor::Position::new(line, col);
    pos >= sel.start && pos < sel.end
}

fn color_of(kind: HighlightKind, theme: Theme) -> Color {
    match kind {
        HighlightKind::Comment => theme.comment,
        HighlightKind::String => theme.string,
        HighlightKind::Number => theme.number,
        HighlightKind::Keyword => theme.keyword,
        HighlightKind::Type => theme.type_name,
        HighlightKind::Function => theme.function,
        HighlightKind::Property => theme.property,
        HighlightKind::Constant => theme.constant,
        HighlightKind::Operator => theme.operator,
        HighlightKind::Variable => theme.fg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::DocumentId;

    #[test]
    fn tab_hit_box_separates_body_and_close() {
        let hit = tab_hit_box(DocumentId(7), 10, 12, 30).unwrap();
        assert_eq!(hit.x, 10);
        assert_eq!(hit.close_x, 21);
        assert!(15 >= hit.x && 15 < hit.x + hit.width);
        assert_ne!(15, hit.close_x);
        assert!(tab_hit_box(DocumentId(8), 22, 12, 30).is_none());
    }

    fn sample_labels(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| format!(" {name} ×")).collect()
    }

    fn assert_strip_inside(strip: &TabStrip, width: u16) {
        if let Some(x) = strip.left {
            assert!(x + TAB_CHEVRON <= width);
        }
        if let Some(x) = strip.right {
            assert!(x + TAB_CHEVRON <= width);
            for tab in &strip.tabs {
                assert!(tab.x + tab.width <= x);
            }
        }
        for tab in &strip.tabs {
            assert!(tab.x + tab.width <= width);
            assert_eq!(UnicodeWidthStr::width(tab.label.as_str()) as u16, tab.width);
            if strip.left.is_some() {
                assert!(tab.x >= TAB_CHEVRON);
            }
        }
    }

    #[test]
    fn tab_strip_hides_chevrons_when_everything_fits() {
        let strip = layout_tab_strip(&sample_labels(&["a", "b", "c"]), 0, 40, None, false);
        assert!(strip.left.is_none());
        assert!(strip.right.is_none());
        assert_eq!(strip.tabs.len(), 3);
        assert_strip_inside(&strip, 40);
    }

    #[test]
    fn tab_strip_shows_the_right_chevron_on_the_first_page() {
        let strip = layout_tab_strip(
            &sample_labels(&["abcd", "abcd", "abcd"]),
            0,
            12,
            None,
            false,
        );
        assert!(strip.left.is_none());
        assert!(strip.right.is_some());
        assert!(strip.tabs.len() < 3);
        assert_strip_inside(&strip, 12);
    }

    #[test]
    fn tab_strip_shows_both_chevrons_after_scrolling() {
        let strip = layout_tab_strip(
            &sample_labels(&["abcd", "abcd", "abcd"]),
            1,
            12,
            None,
            false,
        );
        assert!(strip.left.is_some());
        assert!(strip.right.is_some());
        assert_eq!(strip.scroll, 1);
        assert_strip_inside(&strip, 12);
    }

    #[test]
    fn tab_strip_shows_only_the_left_chevron_at_the_end() {
        let strip = layout_tab_strip(
            &sample_labels(&["abcd", "abcd", "abcd"]),
            2,
            12,
            None,
            false,
        );
        assert!(strip.left.is_some());
        assert!(strip.right.is_none());
        assert_eq!(strip.tabs.len(), 1);
        assert_eq!(strip.tabs[0].index, 2);
        assert_strip_inside(&strip, 12);
    }

    #[test]
    fn tab_strip_truncates_a_tab_wider_than_the_strip() {
        let strip = layout_tab_strip(
            &sample_labels(&["abcdefghijklmnopqrstuvwxyz"]),
            0,
            10,
            None,
            false,
        );
        assert_eq!(strip.tabs.len(), 1);
        assert!(strip.tabs[0].width <= 10);
        assert!(strip.tabs[0].label.ends_with('×'));
        assert!(strip.tabs[0].label.contains('…'));
        assert_strip_inside(&strip, 10);
    }

    #[test]
    fn changing_the_active_tab_reveals_it() {
        let labels = sample_labels(&["abcd", "abcd", "abcd", "abcd", "abcd"]);
        let hidden = layout_tab_strip(&labels, 0, 12, Some(4), false);
        assert!(hidden.tabs.iter().all(|tab| tab.index != 4));
        let revealed = layout_tab_strip(&labels, 0, 12, Some(4), true);
        assert!(revealed.tabs.iter().any(|tab| tab.index == 4));
        assert!(revealed.scroll > 0);
        assert_strip_inside(&revealed, 12);
    }
}
