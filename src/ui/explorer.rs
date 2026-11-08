use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::app::state::Focus;
use crate::workspace::GitFileStatus;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let theme = app.theme();
    let inner_h = area.height.saturating_sub(2) as usize;
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
        let mut style = Style::default().fg(if entry.is_dir {
            theme.folder
        } else {
            theme.fg_muted
        });
        if idx == app.explorer_selected {
            style = style.bg(theme.bg_selected).add_modifier(Modifier::BOLD);
            if app.focus == Focus::Explorer {
                style = style.fg(theme.fg);
            }
        }
        let dirty = app.documents.values().any(|doc| {
            doc.is_dirty()
                && doc.path.as_ref().is_some_and(|path| {
                    path == &entry.path || (entry.is_dir && path.starts_with(&entry.path))
                })
        });
        let git = if entry.is_dir {
            app.git_status
                .iter()
                .find(|(path, _)| path.starts_with(&entry.path))
                .map(|(_, status)| *status)
        } else {
            app.git_status.get(&entry.path).copied()
        };
        let (mark, mark_style) = if dirty {
            (" ●", Style::default().fg(theme.warning))
        } else {
            match git {
                Some(GitFileStatus::Modified) => (" M", Style::default().fg(theme.warning)),
                Some(GitFileStatus::Added) => (" A", Style::default().fg(theme.added)),
                Some(GitFileStatus::Deleted) => (" D", Style::default().fg(theme.deleted)),
                Some(GitFileStatus::Untracked) => (" U", Style::default().fg(theme.fg_dim)),
                None => ("", Style::default()),
            }
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{indent}{marker}{}", entry.name), style),
            Span::styled(mark, mark_style),
        ]));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme.text())
            .block(theme.panel(" Explorer ")),
        area,
    );
}
