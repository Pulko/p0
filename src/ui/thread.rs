use ratatui::Frame;
use ratatui::layout::{Position as TermPos, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::agent::LineKind;
use crate::app::App;
use crate::app::state::Focus;
use crate::ui::theme::Theme;

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    app.thread_stop = None;
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = app.theme();
    let focused = app.focus == Focus::Thread;
    let title = if app.thread.running {
        format!(" Claude {} ", SPINNER[app.thread.spinner % SPINNER.len()])
    } else {
        " Claude ".into()
    };
    let block = theme.panel(&title, focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let compose_row = Rect {
        x: inner.x,
        y: inner.y + inner.height.saturating_sub(1),
        width: inner.width,
        height: 1,
    };
    let transcript = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: inner.height.saturating_sub(1),
    };
    render_transcript(frame, transcript, app, theme);
    render_compose(frame, compose_row, app, theme, focused);
}

fn render_transcript(frame: &mut Frame, area: Rect, app: &App, theme: Theme) {
    if area.height == 0 {
        return;
    }
    let width = area.width as usize;
    let wrapped = if app.thread.lines.is_empty() {
        vec![Line::from(Span::styled(
            fit("Select code and press Option+T", width),
            Style::default().fg(theme.fg_dim),
        ))]
    } else {
        let mut lines = Vec::new();
        for entry in &app.thread.lines {
            let style = match entry.kind {
                LineKind::User => Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
                LineKind::Reply => Style::default().fg(theme.fg),
                LineKind::Tool => Style::default().fg(theme.fg_dim),
                LineKind::Error => Style::default().fg(theme.warning),
            };
            let prefix = match entry.kind {
                LineKind::User => "> ",
                LineKind::Tool => "· ",
                LineKind::Error => "! ",
                LineKind::Reply => "  ",
            };
            for row in wrap_text(&format!("{prefix}{}", entry.text), width) {
                lines.push(Line::from(Span::styled(row, style)));
            }
        }
        lines
    };
    let height = area.height as usize;
    let max_scroll = wrapped.len().saturating_sub(height);
    let scroll = app.thread.scroll.min(max_scroll);
    let start = wrapped.len().saturating_sub(height + scroll);
    let visible: Vec<Line> = wrapped.into_iter().skip(start).take(height).collect();
    frame.render_widget(Paragraph::new(visible).style(theme.text()), area);
}

fn render_compose(frame: &mut Frame, area: Rect, app: &mut App, theme: Theme, focused: bool) {
    let stop = if app.thread.running { " Stop " } else { "" };
    let stop_width = UnicodeWidthStr::width(stop) as u16;
    if app.thread.running && stop_width > 0 && area.width > stop_width {
        let x = area.x + area.width - stop_width;
        app.thread_stop = Some((x, area.y, stop_width));
        frame.render_widget(
            Paragraph::new(Span::styled(
                stop,
                Style::default()
                    .fg(theme.on_accent)
                    .bg(theme.warning)
                    .add_modifier(Modifier::BOLD),
            )),
            Rect {
                x,
                y: area.y,
                width: stop_width,
                height: 1,
            },
        );
    }
    let room = area.width.saturating_sub(stop_width) as usize;
    if room == 0 {
        return;
    }
    let prompt = format!("> {}", app.thread.compose);
    let shown = tail_width(&prompt, room);
    let style = if focused {
        Style::default().fg(theme.fg).bg(theme.bg_current)
    } else {
        Style::default().fg(theme.fg_muted).bg(theme.bg)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(format!("{shown:<room$}"), style)).style(style),
        Rect {
            x: area.x,
            y: area.y,
            width: room as u16,
            height: 1,
        },
    );
    if focused {
        let cursor = UnicodeWidthStr::width(shown.as_str()).min(room) as u16;
        frame.set_cursor_position(TermPos {
            x: area.x + cursor,
            y: area.y,
        });
    }
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut rows = Vec::new();
    for raw in text.split('\n') {
        if raw.is_empty() {
            rows.push(String::new());
            continue;
        }
        let mut line = String::new();
        let mut used = 0usize;
        for grapheme in raw.graphemes(true) {
            let grapheme_width = UnicodeWidthStr::width(grapheme).max(1);
            if used + grapheme_width > width && !line.is_empty() {
                rows.push(std::mem::take(&mut line));
                used = 0;
            }
            line.push_str(grapheme);
            used += grapheme_width;
        }
        rows.push(line);
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows
}

fn fit(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    tail_width(text, width)
}

fn tail_width(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let graphemes: Vec<&str> = text.graphemes(true).collect();
    let mut used = 0usize;
    let mut start = graphemes.len();
    for grapheme in graphemes.iter().rev() {
        let grapheme_width = UnicodeWidthStr::width(*grapheme).max(1);
        if used + grapheme_width > width {
            break;
        }
        used += grapheme_width;
        start -= 1;
    }
    graphemes[start..].concat()
}
