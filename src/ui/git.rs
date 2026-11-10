use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::app::state::{GitClick, GitHit};
use crate::ui::theme::Theme;
use crate::workspace::{GitFileStatus, GitPanelRow, git_panel_rows};

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    app.git_hits.clear();
    let theme = app.theme();
    let rows = git_panel_rows(&app.git_status);
    let inner_h = area.height.saturating_sub(2).max(1) as usize;
    let inner_w = area.width.saturating_sub(2) as usize;
    keep_selection_visible(app, rows.len(), inner_h);
    let staged_count = app
        .git_status
        .values()
        .filter(|status| status.staged.is_some())
        .count();
    let changed_count = app
        .git_status
        .values()
        .filter(|status| status.unstaged.is_some())
        .count();
    let ready = crate::workspace::can_commit(&app.git_message, &app.git_status);
    let branch = app.git_branch.clone();
    let message = app.git_message.clone();
    let message_focus = app.git_message_focus;
    let selected = app.git_selected;
    let scroll = app.git_scroll;
    let origin_x = area.x.saturating_add(1);
    let mut lines = Vec::new();
    for (index, row) in rows.iter().enumerate().skip(scroll).take(inner_h) {
        let y = area.y.saturating_add(1).saturating_add(lines.len() as u16);
        let on = index == selected;
        lines.push(row_line(
            app,
            row,
            theme,
            inner_w,
            origin_x,
            y,
            on,
            staged_count,
            changed_count,
            branch.as_deref(),
            &message,
            message_focus,
            ready,
        ));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme.text())
            .block(super::explorer::side_block(area, app)),
        area,
    );
}

fn keep_selection_visible(app: &mut App, len: usize, inner: usize) {
    if len == 0 {
        app.git_scroll = 0;
        return;
    }
    if app.git_selected >= len {
        app.git_selected = len - 1;
    }
    if app.git_selected < app.git_scroll {
        app.git_scroll = app.git_selected;
    } else if inner > 0 && app.git_selected >= app.git_scroll + inner {
        app.git_scroll = app.git_selected + 1 - inner;
    }
    let max = len.saturating_sub(inner);
    if app.git_scroll > max {
        app.git_scroll = max;
    }
}

fn row_line(
    app: &mut App,
    row: &GitPanelRow,
    theme: Theme,
    inner_w: usize,
    x: u16,
    y: u16,
    selected: bool,
    staged_count: usize,
    changed_count: usize,
    branch: Option<&str>,
    message: &str,
    message_focus: bool,
    ready: bool,
) -> Line<'static> {
    let selected_style = |color: Color| {
        if selected {
            Style::default()
                .fg(color)
                .bg(theme.bg_selected)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(color)
        }
    };
    match row {
        GitPanelRow::Summary => {
            let mut text = format!("{staged_count} staged · {changed_count} changed");
            if let Some(branch) = branch {
                let with_branch = format!("{text}  {branch}");
                if UnicodeWidthStr::width(with_branch.as_str()) <= inner_w {
                    text = with_branch;
                }
            }
            Line::from(Span::styled(
                fit(&text, inner_w),
                Style::default().fg(theme.fg_dim),
            ))
        }
        GitPanelRow::Message => {
            let shown = if message.is_empty() {
                " message".to_string()
            } else {
                tail(message, inner_w.saturating_sub(1))
            };
            let style = if message_focus {
                Style::default()
                    .fg(theme.fg)
                    .bg(theme.bg_selected)
                    .add_modifier(Modifier::BOLD)
            } else if message.is_empty() {
                Style::default().fg(theme.fg_dim)
            } else {
                Style::default().fg(theme.fg)
            };
            hit(app, x, y, inner_w as u16, GitClick::Message);
            Line::from(Span::styled(fit(&shown, inner_w), style))
        }
        GitPanelRow::Commit => {
            let label = " Commit ";
            let width = UnicodeWidthStr::width(label) as u16;
            hit(app, x, y, width, GitClick::Commit);
            let style = if ready {
                Style::default()
                    .fg(theme.on_accent)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg_dim)
            };
            Line::from(Span::styled(label, style))
        }
        GitPanelRow::Section { staged } => {
            let (mark, title) = if *staged {
                ("−", "Staged")
            } else {
                ("+", "Changes")
            };
            let action = if *staged {
                GitClick::UnstageAll
            } else {
                GitClick::StageAll
            };
            hit(app, x, y, UnicodeWidthStr::width(mark) as u16, action);
            let label = format!("{mark} {title}");
            Line::from(Span::styled(
                fit(&label, inner_w),
                selected_style(theme.accent),
            ))
        }
        GitPanelRow::File {
            path,
            status,
            staged,
        } => {
            let tick = if *staged { "−" } else { "+" };
            let tick_w = UnicodeWidthStr::width(tick);
            hit(
                app,
                x,
                y,
                tick_w as u16,
                GitClick::Toggle {
                    path: path.clone(),
                    staged: *staged,
                },
            );
            let name = app.relative(path);
            let mark = format!(" {}", letter(*status));
            let mark_w = UnicodeWidthStr::width(mark.as_str());
            let name_room = inner_w.saturating_sub(tick_w.saturating_add(1).saturating_add(mark_w));
            let name = fit(&name, name_room);
            let open_x = x.saturating_add(tick_w as u16).saturating_add(1);
            let open_w = inner_w.saturating_sub(tick_w.saturating_add(1)) as u16;
            if open_w > 0 {
                hit(
                    app,
                    open_x,
                    y,
                    open_w,
                    GitClick::Open {
                        path: path.clone(),
                        staged: *staged,
                    },
                );
            }
            let color = super::explorer::git_mark_color(*status, *staged, theme);
            let name_style = selected_style(if selected { theme.fg } else { theme.fg_muted });
            Line::from(vec![
                Span::styled(tick.to_string(), Style::default().fg(theme.accent)),
                Span::styled(" ", name_style),
                Span::styled(name, name_style),
                Span::styled(mark, Style::default().fg(color)),
            ])
        }
    }
}

fn hit(app: &mut App, x: u16, y: u16, width: u16, action: GitClick) {
    if width == 0 {
        return;
    }
    app.git_hits.push(GitHit {
        x,
        y,
        width,
        action,
    });
}

fn letter(status: GitFileStatus) -> &'static str {
    match status {
        GitFileStatus::Modified => "M",
        GitFileStatus::Added => "A",
        GitFileStatus::Deleted => "D",
        GitFileStatus::Renamed => "R",
        GitFileStatus::Untracked => "U",
    }
}

fn fit(text: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    let ellipsis = "…";
    let ellipsis_width = UnicodeWidthStr::width(ellipsis);
    if max_width <= ellipsis_width {
        return truncate(text, max_width);
    }
    format!("{}{ellipsis}", truncate(text, max_width - ellipsis_width))
}

fn tail(text: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    let graphemes: Vec<&str> = text.graphemes(true).collect();
    let mut width = 0;
    let mut start = graphemes.len();
    for grapheme in graphemes.iter().rev() {
        let next = width + UnicodeWidthStr::width(*grapheme);
        if next > max_width {
            break;
        }
        width = next;
        start -= 1;
    }
    graphemes[start..].concat()
}

fn truncate(text: &str, max_width: usize) -> String {
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
