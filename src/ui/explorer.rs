use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::app::state::{Focus, HeaderAction, HeaderHit, Overlay, SearchKind, SideTabHit};
use crate::ui::theme::Theme;
use crate::workspace::{DirectoryStatus, GitFileStatus};

pub fn render_search(frame: &mut Frame, area: Rect, app: &mut App) {
    app.header_hits.clear();
    let theme = app.theme();
    let find_on = matches!(
        app.overlay,
        Overlay::Search {
            kind: SearchKind::Find,
            ..
        }
    );
    let search_on = matches!(
        app.overlay,
        Overlay::Search {
            kind: SearchKind::Content,
            ..
        }
    );
    let tool = |on| {
        if on {
            Style::default()
                .fg(theme.on_accent)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.accent).bg(theme.bg)
        }
    };
    let find = " Find ";
    let search = " Search ";
    let find_width = UnicodeWidthStr::width(find) as u16;
    let search_width = UnicodeWidthStr::width(search) as u16;
    let search_x = area.x.saturating_add(find_width).saturating_add(1);
    app.header_hits.push(HeaderHit {
        action: HeaderAction::Find,
        x: area.x,
        width: find_width,
    });
    app.header_hits.push(HeaderHit {
        action: HeaderAction::Search,
        x: search_x,
        width: search_width,
    });
    let row_bg = Style::default().bg(theme.bg);
    let used = find_width.saturating_add(search_width).saturating_add(1);
    let rest = " ".repeat(area.width.saturating_sub(used) as usize);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(find, tool(find_on)),
            Span::styled(" ", row_bg),
            Span::styled(search, tool(search_on)),
            Span::styled(rest, row_bg),
        ]))
        .style(row_bg),
        area,
    );
}

pub(crate) fn side_block<'a>(area: Rect, app: &mut App) -> Block<'a> {
    let theme = app.theme();
    let focused = app.focus == Focus::Explorer;
    let explorer = " Explorer ";
    let changes = " Changes ";
    let explorer_w = UnicodeWidthStr::width(explorer) as u16;
    let changes_w = UnicodeWidthStr::width(changes) as u16;
    let left_x = area.x.saturating_add(1);
    let right_x = area
        .x
        .saturating_add(area.width)
        .saturating_sub(1)
        .saturating_sub(changes_w);
    app.side_tabs.clear();
    let room = area.width.saturating_sub(2);
    let show_changes = app.git_repo;
    if room > 0 {
        app.side_tabs.push(SideTabHit {
            x: left_x,
            y: area.y,
            width: explorer_w.min(room),
            changes: false,
        });
        if show_changes && right_x >= left_x.saturating_add(explorer_w) && changes_w <= room {
            app.side_tabs.push(SideTabHit {
                x: right_x,
                y: area.y,
                width: changes_w,
                changes: true,
            });
        }
    }
    let tab = |label: &'static str, on: bool, alignment: Alignment| {
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
    let explorer_on = if show_changes { !app.git_open } else { focused };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border))
        .style(theme.text())
        .title(tab(explorer, explorer_on, Alignment::Left));
    if show_changes {
        block = block.title(tab(changes, app.git_open, Alignment::Right));
    }
    block
}

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let theme = app.theme();
    let inner_h = area.height.saturating_sub(2) as usize;
    let inner_w = area.width.saturating_sub(2) as usize;
    let mut lines = Vec::new();
    for (idx, entry) in app
        .explorer_entries
        .iter()
        .enumerate()
        .skip(app.explorer_scroll)
        .take(inner_h.max(1))
    {
        let indent = " ".repeat(entry.depth.saturating_mul(2));
        let marker = if entry.is_dir {
            if entry.expanded { "▾ " } else { "▸ " }
        } else {
            "  "
        };
        let dirty = app.documents.values().any(|doc| {
            doc.is_dirty()
                && doc.path.as_ref().is_some_and(|path| {
                    path == &entry.path || (entry.is_dir && path.starts_with(&entry.path))
                })
        });
        let git = if entry.is_dir {
            crate::workspace::directory_status(&app.git_status, &entry.path)
        } else {
            app.git_status.get(&entry.path).and_then(|status| {
                if let Some(kind) = status.unstaged {
                    Some(crate::workspace::DirectoryStatus {
                        kind,
                        staged: false,
                    })
                } else {
                    status
                        .staged
                        .map(|kind| crate::workspace::DirectoryStatus { kind, staged: true })
                }
            })
        };
        let status = status_color(dirty, git, theme);
        let mut style = Style::default().fg(status.unwrap_or(if entry.is_dir {
            theme.folder
        } else {
            theme.fg_muted
        }));
        if idx == app.explorer_selected {
            style = style.bg(theme.bg_selected).add_modifier(Modifier::BOLD);
            if app.focus == Focus::Explorer && status.is_none() {
                style = style.fg(theme.fg);
            }
        }
        let mark = status_mark(dirty, git);
        let mark_style = status
            .map(|color| Style::default().fg(color))
            .unwrap_or_default();
        let (label, mark) =
            fit_explorer_row(&format!("{indent}{marker}{}", entry.name), mark, inner_w);
        lines.push(Line::from(vec![
            Span::styled(label, style),
            Span::styled(mark, mark_style),
        ]));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme.text())
            .block(side_block(area, app)),
        area,
    );
}

fn status_mark(dirty: bool, git: Option<DirectoryStatus>) -> &'static str {
    if dirty {
        return " ●";
    }
    match git.map(|status| status.kind) {
        Some(GitFileStatus::Modified) => " M",
        Some(GitFileStatus::Added) => " A",
        Some(GitFileStatus::Deleted) => " D",
        Some(GitFileStatus::Renamed) => " R",
        Some(GitFileStatus::Untracked) => " U",
        None => "",
    }
}

fn status_color(dirty: bool, git: Option<DirectoryStatus>, theme: Theme) -> Option<Color> {
    if dirty {
        return Some(theme.warning);
    }
    git.map(|status| git_mark_color(status.kind, status.staged, theme))
}

pub(crate) fn git_mark_color(status: GitFileStatus, staged: bool, theme: Theme) -> Color {
    if staged {
        return theme.added;
    }
    match status {
        GitFileStatus::Modified | GitFileStatus::Renamed => theme.warning,
        GitFileStatus::Deleted => theme.deleted,
        GitFileStatus::Added | GitFileStatus::Untracked => theme.untracked,
    }
}

fn fit_explorer_row(label: &str, mark: &str, inner_width: usize) -> (String, String) {
    if inner_width == 0 {
        return (String::new(), String::new());
    }
    let mark_width = UnicodeWidthStr::width(mark);
    if mark.is_empty() {
        return (truncate_with_ellipsis(label, inner_width), String::new());
    }
    if mark_width >= inner_width {
        return (String::new(), truncate_width(mark, inner_width));
    }
    (
        truncate_with_ellipsis(label, inner_width - mark_width),
        mark.to_string(),
    )
}

fn truncate_with_ellipsis(text: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    let ellipsis = "…";
    let ellipsis_width = UnicodeWidthStr::width(ellipsis);
    if max_width <= ellipsis_width {
        return truncate_width(text, max_width);
    }
    format!(
        "{}{ellipsis}",
        truncate_width(text, max_width - ellipsis_width)
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_filename_keeps_the_git_mark_inside_the_row() {
        let label = format!("    {}", "main-button-context.tsx");
        let (fitted, mark) = fit_explorer_row(&label, " M", 26);
        assert_eq!(mark, " M");
        assert!(fitted.contains('…'));
        assert!(
            UnicodeWidthStr::width(fitted.as_str()) + UnicodeWidthStr::width(mark.as_str()) <= 26
        );
    }

    #[test]
    fn short_filename_keeps_the_mark_beside_the_name() {
        let (fitted, mark) = fit_explorer_row("▾ lib", " M", 26);
        assert_eq!(fitted, "▾ lib");
        assert_eq!(mark, " M");
    }

    #[test]
    fn changed_name_uses_the_mark_color() {
        let theme = super::super::theme::by_name("p0");
        assert_eq!(
            status_color(
                false,
                Some(DirectoryStatus {
                    kind: GitFileStatus::Modified,
                    staged: false,
                }),
                theme
            ),
            Some(theme.warning)
        );
        assert_eq!(
            status_color(
                false,
                Some(DirectoryStatus {
                    kind: GitFileStatus::Modified,
                    staged: true,
                }),
                theme
            ),
            Some(theme.added)
        );
        assert_eq!(status_color(true, None, theme), Some(theme.warning));
        assert_eq!(
            status_color(
                false,
                Some(DirectoryStatus {
                    kind: GitFileStatus::Added,
                    staged: true,
                }),
                theme
            ),
            Some(theme.added)
        );
        assert_eq!(
            status_color(
                false,
                Some(DirectoryStatus {
                    kind: GitFileStatus::Deleted,
                    staged: false,
                }),
                theme
            ),
            Some(theme.deleted)
        );
        assert_eq!(
            status_color(
                false,
                Some(DirectoryStatus {
                    kind: GitFileStatus::Untracked,
                    staged: false,
                }),
                theme
            ),
            Some(theme.untracked)
        );
        assert_eq!(status_color(false, None, theme), None);
    }
}
