use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::app::events::status_text;
use crate::app::state::{Overlay, StatusAction, StatusHit};
use crate::language::LspStatus;

struct Piece {
    text: String,
    style: Style,
    action: Option<StatusAction>,
}

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let theme = app.theme();
    let (lang, pos, markdown) =
        if let (Some(doc), Some(view)) = (app.active_doc(), app.active_view()) {
            (
                doc.language.as_str(),
                format!(
                    "Ln {}, Col {}",
                    view.cursor.line + 1,
                    view.cursor.column + 1
                ),
                doc.language == crate::editor::LanguageId::Markdown,
            )
        } else {
            ("", String::new(), false)
        };
    let theme_style = if matches!(app.overlay, Overlay::Themes { .. }) {
        Style::default()
            .fg(theme.on_accent)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.accent)
    };
    let help_style = if matches!(app.overlay, Overlay::Help) {
        Style::default()
            .fg(theme.on_accent)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg_muted)
    };
    let types_off = matches!(app.lsp_status, LspStatus::Unavailable(_));
    let types_down = matches!(
        app.lsp_status,
        LspStatus::Unavailable(_) | LspStatus::Error(_)
    );
    let types_style = if matches!(app.overlay, Overlay::TypesNote) {
        Style::default()
            .fg(theme.on_accent)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else if types_off {
        Style::default().fg(theme.deleted)
    } else if matches!(app.lsp_status, LspStatus::Error(_)) {
        Style::default().fg(theme.warning)
    } else {
        Style::default().fg(theme.fg_dim)
    };
    let (errors, warnings) = diagnostic_counts(app);
    let msg = cursor_diagnostic(app)
        .or_else(|| app.message.clone())
        .unwrap_or_default();
    let mut left = vec![
        Piece {
            text: format!(" {pos} "),
            style: Style::default().fg(theme.fg_muted),
            action: None,
        },
        Piece {
            text: format!(" {lang} "),
            style: if markdown && app.markdown_preview_open() {
                Style::default()
                    .fg(theme.on_accent)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.accent)
            },
            action: markdown.then_some(StatusAction::Markdown),
        },
        Piece {
            text: format!(" {} ", status_text(&app.lsp_status)),
            style: types_style,
            action: types_down.then_some(StatusAction::Types),
        },
    ];
    if errors > 0 {
        left.push(Piece {
            text: format!(" {errors}E "),
            style: Style::default().fg(theme.deleted),
            action: None,
        });
    }
    if warnings > 0 {
        left.push(Piece {
            text: format!(" {warnings}W "),
            style: Style::default().fg(theme.warning),
            action: None,
        });
    }
    if app.git_repo
        && let Some(branch) = &app.git_branch
    {
        let branch_style = if app.git_open {
            Style::default()
                .fg(theme.on_accent)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg_muted)
        };
        left.push(Piece {
            text: format!(" {branch} "),
            style: branch_style,
            action: Some(StatusAction::Git),
        });
    }
    if !msg.is_empty() {
        left.push(Piece {
            text: format!(" {msg} "),
            style: Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::ITALIC),
            action: None,
        });
    }
    let mut right = Vec::new();
    if let Some((text, style)) = agent_piece(app, theme) {
        right.push(Piece {
            text,
            style,
            action: Some(StatusAction::Agent),
        });
    }
    right.push(Piece {
        text: format!(" {} ", theme.name),
        style: theme_style,
        action: Some(StatusAction::Theme),
    });
    right.push(Piece {
        text: " Help ".into(),
        style: help_style,
        action: Some(StatusAction::Help),
    });
    fit_left(&mut left, &right, area.width);
    let left_width: u16 = left.iter().map(|piece| chars(&piece.text)).sum();
    let right_width: u16 = right.iter().map(|piece| chars(&piece.text)).sum();
    let gap = area
        .width
        .saturating_sub(left_width.saturating_add(right_width));

    app.status_hits.clear();
    let mut spans = Vec::new();
    let mut x = 0u16;
    for piece in left {
        push_piece(&mut spans, app, area.x, &mut x, piece);
    }
    if gap > 0 {
        spans.push(Span::raw(" ".repeat(gap as usize)));
        x = x.saturating_add(gap);
    }
    for piece in right {
        push_piece(&mut spans, app, area.x, &mut x, piece);
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.bg_status).fg(theme.fg)),
        area,
    );
}

fn push_piece(
    spans: &mut Vec<Span<'static>>,
    app: &mut App,
    origin: u16,
    x: &mut u16,
    piece: Piece,
) {
    let width = chars(&piece.text);
    if let Some(action) = piece.action {
        app.status_hits.push(StatusHit {
            action,
            x: origin.saturating_add(*x),
            width,
        });
    }
    spans.push(Span::styled(piece.text, piece.style));
    *x = x.saturating_add(width);
}

fn fit_left(left: &mut [Piece], right: &[Piece], width: u16) {
    let right_width: u16 = right.iter().map(|piece| chars(&piece.text)).sum();
    let mut used = right_width;
    for piece in left.iter_mut().rev() {
        let available = width.saturating_sub(used);
        if chars(&piece.text) > available {
            piece.text = truncate_chars(&piece.text, available as usize);
        }
        used = used.saturating_add(chars(&piece.text));
    }
}

fn diagnostic_counts(app: &App) -> (usize, usize) {
    let mut errors = 0;
    let mut warnings = 0;
    for items in app.diagnostics.values() {
        for item in items {
            match item.severity {
                2 => warnings += 1,
                3 | 4 => {}
                _ => errors += 1,
            }
        }
    }
    (errors, warnings)
}

fn agent_piece(app: &App, theme: crate::ui::theme::Theme) -> Option<(String, Style)> {
    if !app.config.claude_enabled {
        return None;
    }
    if !app.claude_installed {
        return Some((
            " Claude ".into(),
            Style::default().fg(theme.fg_dim).bg(theme.bg_status),
        ));
    }
    if app.thread.running {
        return Some((
            " Claude … ".into(),
            Style::default()
                .fg(theme.warning)
                .bg(theme.bg_status)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if app.thread_visible {
        return Some((
            " Claude ".into(),
            Style::default()
                .fg(theme.on_accent)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    }
    Some((
        " Claude ".into(),
        Style::default().fg(theme.accent).bg(theme.bg_status),
    ))
}

fn cursor_diagnostic(app: &App) -> Option<String> {
    let doc = app.active_doc()?;
    let view = app.active_view()?;
    let path = doc.path.as_ref()?;
    let items = app.diagnostics.get(path)?;
    let diagnostic = crate::language::diagnostic_at(items, view.cursor)?;
    Some(crate::language::diagnostic_status(diagnostic))
}

fn chars(text: &str) -> u16 {
    text.chars().count() as u16
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    if max > 1 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_long_status_text() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello", 4), "hel…");
        assert_eq!(truncate_chars("hello", 0), "");
    }
}
