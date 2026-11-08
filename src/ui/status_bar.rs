use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::app::events::status_text;
use crate::app::state::{Overlay, StatusAction, StatusHit};

struct Piece {
    text: String,
    style: Style,
    action: Option<StatusAction>,
}

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let theme = app.theme();
    let (name, dirty, lang, pos) =
        if let (Some(doc), Some(view)) = (app.active_doc(), app.active_view()) {
            let name = doc
                .path
                .as_ref()
                .map(|p| app.relative(p))
                .unwrap_or_else(|| "untitled".into());
            (
                name,
                if doc.is_dirty() { "●" } else { " " },
                doc.language.as_str(),
                format!(
                    "Ln {}, Col {}",
                    view.cursor.line + 1,
                    view.cursor.column + 1
                ),
            )
        } else {
            ("no file".into(), " ", "", String::new())
        };
    let diff_style = if app.diff_view.is_some() {
        Style::default()
            .fg(theme.on_accent)
            .bg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg_dim)
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
    let msg = cursor_diagnostic(app)
        .or_else(|| app.message.clone())
        .unwrap_or_default();
    let mut left = vec![
        Piece {
            text: if app.diff_view.is_some() {
                " DIFF ".into()
            } else {
                " Diff ".into()
            },
            style: diff_style,
            action: Some(StatusAction::Diff),
        },
        Piece {
            text: format!(" {dirty} {name} "),
            style: Style::default().fg(theme.fg),
            action: None,
        },
        Piece {
            text: format!(" {pos} "),
            style: Style::default().fg(theme.fg_muted),
            action: None,
        },
        Piece {
            text: format!(" {lang} "),
            style: Style::default().fg(theme.accent),
            action: None,
        },
        Piece {
            text: format!(" {} ", status_text(&app.lsp_status)),
            style: Style::default().fg(theme.fg_dim),
            action: None,
        },
    ];
    if !msg.is_empty() {
        left.push(Piece {
            text: format!(" {msg} "),
            style: Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::ITALIC),
            action: None,
        });
    }
    let right = [
        Piece {
            text: format!(" {} ", theme.name),
            style: theme_style,
            action: Some(StatusAction::Theme),
        },
        Piece {
            text: " Help ".into(),
            style: help_style,
            action: Some(StatusAction::Help),
        },
    ];
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
