use ratatui::Frame;
use ratatui::layout::{Alignment, Position as TermPos, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::app::state::{
    ContentTabHit, DiffHit, DiffSource, Focus, HScrollHit, HunkAction, TabHitBox,
};
use crate::editor::{Range, display_col, hscroll_bar, line_text, max_line_display_width};
use crate::syntax::HighlightKind;
use crate::ui::theme::{Theme, highlight_color, highlight_modifier};
use crate::workspace::{DiffKind, LineChanges, LineMark, VisualRow};

pub fn render(frame: &mut Frame, tabs: Rect, area: Rect, app: &mut App) {
    app.hscroll_hit = None;
    app.diff_hits.clear();
    let theme = app.theme();
    render_tabs(frame, tabs, app, theme);
    if app.diff_is_showing() {
        render_diff(frame, area, app, theme);
        return;
    }
    let Some(id) = app.active else {
        frame.render_widget(
            Paragraph::new("Open a file from the explorer or press Ctrl+P")
                .style(theme.text())
                .block(content_block(area, app, "Editor")),
            area,
        );
        return;
    };
    if app
        .markdown_preview
        .as_ref()
        .is_some_and(|preview| preview.document_id == id)
    {
        render_markdown(frame, area, app, theme);
        return;
    }
    let Some(doc) = app.documents.get(&id) else {
        return;
    };
    let Some(view) = app.views.get(&id) else {
        return;
    };
    let tab_width = app.config.tab_width;
    let number_width = ((doc.rope.len_lines().max(1).ilog10() as u16) + 2).max(4);
    let rows = app.visual_rows(id);
    let changes = app.inline_changes.get(&id).cloned();
    let hunks = changes
        .as_ref()
        .map(crate::workspace::change_hunks)
        .unwrap_or_default();
    let gutter = number_width + 1;
    app.gutter_width = gutter;
    let inner_h = area.height.saturating_sub(2) as usize;
    let inner_w = area.width.saturating_sub(2 + gutter) as usize;
    let visible_lines: Vec<usize> = rows
        .iter()
        .skip(view.viewport.top)
        .take(inner_h)
        .filter_map(|row| match row {
            VisualRow::Line(line) => Some(*line),
            VisualRow::Removed { .. } | VisualRow::Eof { .. } => None,
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
        if matches!(visual, VisualRow::Eof { .. }) {
            lines.push(eof_line(number_width, theme));
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
        let button = changes.as_ref().and_then(|changes| {
            hunks
                .iter()
                .any(|hunk| hunk.start == line_idx)
                .then(|| change_style(changes, line_idx, theme))
                .flatten()
        });
        let mut spans = line_number(line_idx, number_width, button, theme);
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
        .map(|p| app.relative(p))
        .unwrap_or_else(|| "untitled".into());
    let cursor = app.views.get(&id).and_then(|view| {
        let text = line_text(&doc.rope, view.cursor.line);
        let col = display_col(&text, view.cursor.column, tab_width);
        let cursor_row = rows
            .iter()
            .position(|row| matches!(row, VisualRow::Line(line) if *line == view.cursor.line))?;
        (cursor_row >= view.viewport.top
            && cursor_row < view.viewport.top + inner_h
            && col >= view.viewport.left
            && col < view.viewport.left + inner_w)
            .then_some(TermPos {
                x: area.x + 1 + gutter + (col - view.viewport.left) as u16,
                y: area.y + 1 + (cursor_row - view.viewport.top) as u16,
            })
    });
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme.text())
            .block(content_block(area, app, &title)),
        area,
    );
    if let Some(cursor) = cursor
        && app.focus == Focus::Editor
        && matches!(
            app.overlay,
            crate::app::state::Overlay::None
                | crate::app::state::Overlay::Completion { .. }
                | crate::app::state::Overlay::Hover { .. }
                | crate::app::state::Overlay::Signature { .. }
        )
    {
        frame.set_cursor_position(cursor);
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
            ("─", Style::default().fg(theme.accent).bg(theme.bg))
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

fn line_number(
    line_idx: usize,
    number_width: u16,
    revert: Option<Style>,
    theme: Theme,
) -> Vec<Span<'static>> {
    let ln = format!("{:>width$}", line_idx + 1, width = number_width as usize);
    let Some(revert) = revert else {
        return vec![Span::styled(ln, Style::default().fg(theme.fg_dim))];
    };
    let mut spans = vec![Span::styled("↩", revert)];
    if number_width > 1 {
        spans.push(Span::styled(
            ln[1..].to_string(),
            Style::default().fg(theme.fg_dim),
        ));
    }
    spans
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

fn eof_line(number_width: u16, theme: Theme) -> Line<'static> {
    let deleted = Style::default().fg(theme.on_accent).bg(theme.deleted);
    let mut spans = vec![Span::styled("↩", deleted)];
    if number_width > 1 {
        spans.push(Span::raw(" ".repeat(number_width as usize - 1)));
    }
    spans.push(Span::styled("▾", deleted));
    Line::from(spans)
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
    let Some(style) = change_style(changes, line_idx, theme) else {
        return Span::raw(" ");
    };
    let deleted = changes.deleted_before.get(line_idx).copied().unwrap_or(0) > 0;
    let mark = changes
        .marks
        .get(line_idx)
        .copied()
        .unwrap_or(LineMark::Unchanged);
    if deleted && mark == LineMark::Unchanged {
        Span::styled("▾", style)
    } else {
        Span::styled(" ", style)
    }
}

fn change_style(changes: &LineChanges, line_idx: usize, theme: Theme) -> Option<Style> {
    let mark = changes
        .marks
        .get(line_idx)
        .copied()
        .unwrap_or(LineMark::Unchanged);
    let deleted = changes.deleted_before.get(line_idx).copied().unwrap_or(0) > 0;
    let color = if deleted && mark == LineMark::Unchanged {
        theme.deleted
    } else {
        match mark {
            LineMark::Added => theme.added,
            LineMark::Modified => theme.modified,
            LineMark::Unchanged => return None,
        }
    };
    Some(Style::default().fg(theme.on_accent).bg(color))
}

fn render_diff(frame: &mut Frame, area: Rect, app: &mut App, theme: Theme) {
    let Some(diff) = app.diff_view.clone() else {
        return;
    };
    let height = area.height.saturating_sub(2) as usize;
    let inner_w = area.width.saturating_sub(2) as usize;
    let title = diff
        .path
        .as_ref()
        .map(|path| app.relative(path))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Editor".into());
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
    } else if diff.lines.is_empty() {
        vec![Line::from(Span::styled(
            "No changes from HEAD",
            Style::default().fg(theme.fg_dim),
        ))]
    } else {
        diff.lines
            .iter()
            .enumerate()
            .skip(diff.scroll)
            .take(height)
            .map(|(index, line)| {
                let row = index - diff.scroll;
                let y = area.y.saturating_add(1).saturating_add(row as u16);
                diff_line(
                    app,
                    line,
                    theme,
                    inner_w,
                    area.x.saturating_add(1),
                    y,
                    diff.source,
                    diff.allow_hunks,
                )
            })
            .collect()
    };
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme.text())
            .block(content_block(area, app, &title)),
        area,
    );
}

fn content_block(area: Rect, app: &mut App, title: &str) -> Block<'static> {
    let theme = app.theme();
    let focused = app.focus == Focus::Editor;
    let diff_on = app.diff_is_showing();
    let show_diff = app.git_repo;
    let diff_label = " Diff ";
    let diff_w = UnicodeWidthStr::width(diff_label) as u16;
    let room = area.width.saturating_sub(2);
    let left_max = (if show_diff {
        room.saturating_sub(diff_w.saturating_add(1))
    } else {
        room
    }) as usize;
    let left = fit_border_title(title, left_max);
    let left_w = UnicodeWidthStr::width(left.as_str()) as u16;
    let left_x = area.x.saturating_add(1);
    let right_x = area
        .x
        .saturating_add(area.width)
        .saturating_sub(1)
        .saturating_sub(diff_w);
    app.content_tabs.clear();
    if room > 0 && left_w > 0 {
        app.content_tabs.push(ContentTabHit {
            x: left_x,
            y: area.y,
            width: left_w.min(room),
            diff: false,
        });
        if show_diff && right_x >= left_x.saturating_add(left_w) && diff_w <= room {
            app.content_tabs.push(ContentTabHit {
                x: right_x,
                y: area.y,
                width: diff_w,
                diff: true,
            });
        }
    }
    let tab = |label: String, on: bool, alignment: Alignment| {
        let style = if on {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg_muted)
        };
        Line::from(Span::styled(label, style)).alignment(alignment)
    };
    let border = if focused { theme.accent } else { theme.border };
    let left_on = if show_diff { !diff_on } else { focused };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border))
        .style(theme.text())
        .title(tab(left, left_on, Alignment::Left));
    if show_diff {
        block = block.title(tab(diff_label.to_string(), diff_on, Alignment::Right));
    }
    block
}

fn fit_border_title(title: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let padded = format!(" {title} ");
    if UnicodeWidthStr::width(padded.as_str()) <= max_width {
        return padded;
    }
    if max_width <= 2 {
        return title.graphemes(true).take(max_width).collect();
    }
    let inner = max_width - 2;
    let mut body = String::new();
    let mut width = 0usize;
    for grapheme in title.graphemes(true) {
        let next = UnicodeWidthStr::width(grapheme);
        if width + next + 1 > inner {
            break;
        }
        body.push_str(grapheme);
        width += next;
    }
    format!(" {body}… ")
}

fn diff_line(
    app: &mut App,
    line: &crate::workspace::DiffLine,
    theme: Theme,
    inner_w: usize,
    x: u16,
    y: u16,
    source: DiffSource,
    allow_hunks: bool,
) -> Line<'static> {
    let style = match line.kind {
        DiffKind::Header => Style::default().fg(theme.accent),
        DiffKind::Insert => Style::default().fg(theme.added),
        DiffKind::Delete => Style::default().fg(theme.deleted),
        DiffKind::Equal => Style::default().fg(theme.fg_muted),
    };
    let actions: Vec<(&str, HunkAction)> = match (allow_hunks, line.hunk, source) {
        (true, Some(_), DiffSource::Unstaged) => {
            vec![
                (" Stage ", HunkAction::Stage),
                (" Discard ", HunkAction::Discard),
            ]
        }
        (true, Some(_), DiffSource::Staged) => vec![(" Unstage ", HunkAction::Unstage)],
        _ => Vec::new(),
    };
    if actions.is_empty() {
        return Line::from(Span::styled(fit_diff(&line.text, inner_w), style));
    }
    let hunk = line.hunk.unwrap_or(0);
    let actions_w: usize = actions
        .iter()
        .map(|(label, _)| UnicodeWidthStr::width(*label))
        .sum();
    let text_w = inner_w.saturating_sub(actions_w.saturating_add(1));
    let text = fit_diff(&line.text, text_w);
    let gap =
        inner_w.saturating_sub(UnicodeWidthStr::width(text.as_str()).saturating_add(actions_w));
    let mut spans = vec![Span::styled(text, style), Span::raw(" ".repeat(gap))];
    let mut cursor = x.saturating_add((inner_w.saturating_sub(actions_w)) as u16);
    for (label, action) in actions {
        let width = UnicodeWidthStr::width(label) as u16;
        app.diff_hits.push(DiffHit {
            x: cursor,
            y,
            width,
            hunk,
            action,
        });
        let action_style = match action {
            HunkAction::Discard => Style::default().fg(theme.deleted),
            HunkAction::Stage | HunkAction::Unstage => Style::default()
                .fg(theme.on_accent)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        };
        spans.push(Span::styled((*label).to_string(), action_style));
        cursor = cursor.saturating_add(width);
    }
    Line::from(spans)
}

fn fit_diff(text: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    let ellipsis = "…";
    let keep = max_width.saturating_sub(UnicodeWidthStr::width(ellipsis));
    let mut out = String::new();
    let mut width = 0;
    for grapheme in text.graphemes(true) {
        let grapheme_width = UnicodeWidthStr::width(grapheme);
        if width + grapheme_width > keep {
            break;
        }
        out.push_str(grapheme);
        width += grapheme_width;
    }
    out.push('…');
    out
}

fn render_markdown(frame: &mut Frame, area: Rect, app: &mut App, theme: Theme) {
    let width = area.width.saturating_sub(2) as usize;
    let height = area.height.saturating_sub(2) as usize;
    let title = app
        .markdown_preview
        .as_ref()
        .and_then(|preview| app.documents.get(&preview.document_id))
        .and_then(|doc| doc.path.as_ref().map(|path| app.relative(path)))
        .map(|name| format!("Preview {name}"))
        .unwrap_or_else(|| "Preview".into());
    let refresh = app.markdown_preview.as_ref().and_then(|preview| {
        let doc = app.documents.get(&preview.document_id)?;
        Some((
            doc.text(),
            doc.revision,
            preview.is_current(doc.revision, theme.name, width.max(1)),
        ))
    });
    if let Some((text, revision, false)) = refresh {
        let lines = crate::markdown::render(&text, theme, width.max(1), &mut app.syntax);
        if let Some(preview) = app.markdown_preview.as_mut() {
            preview.store(lines, revision, theme.name, width.max(1));
        }
    }
    let lines = app
        .markdown_preview
        .as_ref()
        .map(|preview| preview.visible(height.max(1)))
        .unwrap_or_default();
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme.text())
            .block(content_block(area, app, &title)),
        area,
    );
}

const TAB_CHEVRON: u16 = 2;
const PANE_MARK: &str = "◧";

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
    app.pane_mark = None;
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
    let mark_w = UnicodeWidthStr::width(PANE_MARK) as u16;
    let reserved = if area.width > mark_w {
        mark_w.saturating_add(1)
    } else {
        area.width
    };
    let mut strip = layout_tab_strip(
        &labels,
        app.tab_scroll,
        area.width.saturating_sub(reserved),
        active_index,
        reveal,
    );
    app.tab_scroll = strip.scroll;
    app.tab_revealed = app.active;
    if let Some(x) = strip.left.as_mut() {
        *x = x.saturating_add(reserved);
    }
    if let Some(x) = strip.right.as_mut() {
        *x = x.saturating_add(reserved);
    }
    for tab in &mut strip.tabs {
        tab.x = tab.x.saturating_add(reserved);
    }

    let mut spans = Vec::new();
    let mut cursor = 0u16;
    if mark_w > 0 && area.width >= mark_w {
        spans.push(Span::styled(
            PANE_MARK,
            Style::default().fg(theme.accent).bg(theme.bg),
        ));
        cursor = mark_w;
        app.pane_mark = Some((area.x, area.y, mark_w));
        if reserved > mark_w {
            spans.push(Span::styled(" ", Style::default().bg(theme.bg)));
            cursor = reserved;
        }
    }
    if let Some(x) = strip.left {
        push_gap(&mut spans, &mut cursor, x);
        spans.push(Span::styled("<<", Style::default().fg(theme.fg_dim)));
        cursor = cursor.saturating_add(TAB_CHEVRON);
        app.tab_scroll_left = Some((area.x.saturating_add(x), TAB_CHEVRON));
    }
    for placed in &strip.tabs {
        push_gap(&mut spans, &mut cursor, placed.x);
        let id = opened[placed.index].0;
        let active = app.active == Some(id);
        let bg = if active { theme.bg_current } else { theme.bg };
        let mut style = Style::default().fg(theme.fg_muted).bg(bg);
        if active {
            style = Style::default()
                .fg(theme.accent)
                .bg(bg)
                .add_modifier(Modifier::BOLD);
        }
        if let Some((left, right)) = placed.label.split_once('•') {
            spans.push(Span::styled(left.to_string(), style));
            spans.push(Span::styled("•", Style::default().fg(theme.warning).bg(bg)));
            spans.push(Span::styled(right.to_string(), style));
        } else {
            spans.push(Span::styled(placed.label.clone(), style));
        }
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
            style = style
                .fg(highlight_color(kind, theme))
                .add_modifier(highlight_modifier(kind));
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
