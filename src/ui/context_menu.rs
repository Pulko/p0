use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::app::state::Overlay;
use crate::ui::popup::{HoverLineKind, parse_hover_content};
use crate::ui::theme::Theme;

pub fn render(frame: &mut Frame, app: &mut App) {
    let Overlay::ContextMenu {
        items,
        selected,
        x,
        y,
        show_type,
        type_text,
        ..
    } = app.overlay.clone()
    else {
        app.menu_area = None;
        app.menu_rows.clear();
        return;
    };
    let type_lines = if show_type {
        let parsed = parse_hover_content(&type_text);
        if parsed.is_empty() {
            vec![(type_text.clone(), HoverLineKind::Doc)]
        } else {
            parsed
        }
    } else {
        Vec::new()
    };
    let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
    let layout = layout_symbol_menu(
        &type_lines,
        &labels,
        frame.area().width,
        frame.area().height,
        show_type,
    );
    let area = Rect {
        x: x.min(frame.area().width.saturating_sub(layout.width)),
        y: y.min(frame.area().height.saturating_sub(layout.height)),
        width: layout.width,
        height: layout.height,
    };
    let type_height = layout.type_rows.len() as u16;
    let separator = u16::from(layout.show_separator);
    app.menu_area = Some((area.x, area.y, area.width, area.height));
    app.menu_rows = items
        .iter()
        .enumerate()
        .map(|(index, _)| (index, area.y + 1 + type_height + separator + index as u16))
        .filter(|(_, row)| *row < area.bottom().saturating_sub(1))
        .collect();

    let theme = app.theme();
    frame.render_widget(Clear, area);
    frame.render_widget(
        theme.panel(if show_type { " Symbol " } else { " Actions " }, false),
        area,
    );
    let mut lines = Vec::new();
    for (text, kind) in &layout.type_rows {
        lines.push(Line::from(Span::styled(
            fit_row(text, layout.content_width),
            type_style(*kind, theme),
        )));
    }
    if layout.show_separator {
        lines.push(Line::from(Span::styled(
            fit_row(&separator_rule(layout.content_width), layout.content_width),
            Style::default().fg(theme.border),
        )));
    }
    for (index, item) in items.iter().enumerate() {
        let mut style = if item.enabled {
            Style::default().fg(theme.fg)
        } else {
            Style::default().fg(theme.fg_dim)
        };
        if index == selected {
            style = style.bg(theme.bg_selected).add_modifier(Modifier::BOLD);
        }
        lines.push(Line::from(Span::styled(
            fit_row(&item.label, layout.content_width),
            style,
        )));
    }
    let inner = Block::default().borders(Borders::ALL).inner(area);
    frame.render_widget(Paragraph::new(lines).style(theme.text()), inner);
}

fn type_style(kind: HoverLineKind, theme: Theme) -> Style {
    match kind {
        HoverLineKind::Signature => Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
        HoverLineKind::Tag => Style::default().fg(theme.warning),
        HoverLineKind::Doc => Style::default().fg(theme.fg_muted),
    }
}

pub(crate) struct MenuLayout {
    pub width: u16,
    pub height: u16,
    pub content_width: usize,
    pub type_rows: Vec<(String, HoverLineKind)>,
    pub show_separator: bool,
}

pub(crate) fn layout_symbol_menu(
    type_lines: &[(String, HoverLineKind)],
    action_labels: &[&str],
    screen_width: u16,
    screen_height: u16,
    show_type: bool,
) -> MenuLayout {
    let max_width = screen_width.max(1);
    let longest = action_labels
        .iter()
        .map(|label| UnicodeWidthStr::width(*label) + 2)
        .chain(
            type_lines
                .iter()
                .map(|(line, _)| UnicodeWidthStr::width(line.as_str())),
        )
        .max()
        .unwrap_or(24) as u16;
    let desired = longest.saturating_add(4);
    let min_width = 28.min(max_width);
    let preferred = 72.min(max_width).max(min_width);
    let width = desired.clamp(min_width, preferred);
    let content_width = (width as usize).saturating_sub(4).max(1);
    let mut wrapped = Vec::new();
    if show_type {
        for (text, kind) in type_lines {
            for row in wrap_to_width(text, content_width) {
                wrapped.push((row, *kind));
            }
        }
        if wrapped.is_empty() {
            wrapped.push((String::new(), HoverLineKind::Doc));
        }
    }
    let max_height = screen_height.saturating_sub(1).max(1);
    let action_count = action_labels.len() as u16;
    let chrome = 2 + action_count;
    let room_for_type = max_height.saturating_sub(chrome + u16::from(show_type)) as usize;
    let type_rows: Vec<_> = wrapped.into_iter().take(room_for_type).collect();
    let show_separator = show_type && !type_rows.is_empty();
    let height = (2 + type_rows.len() as u16 + u16::from(show_separator) + action_count)
        .min(max_height)
        .max(3.min(max_height));
    MenuLayout {
        width,
        height,
        content_width,
        type_rows,
        show_separator,
    }
}

pub(crate) fn separator_rule(content_width: usize) -> String {
    "─".repeat(content_width)
}

fn wrap_to_width(text: &str, width: usize) -> Vec<String> {
    if width == 0 || text.is_empty() {
        return vec![String::new()];
    }
    let mut rows = Vec::new();
    let mut line = String::new();
    let mut used = 0usize;
    for grapheme in text.graphemes(true) {
        let grapheme_width = UnicodeWidthStr::width(grapheme).max(1);
        if used > 0 && used + grapheme_width > width {
            rows.push(std::mem::take(&mut line));
            used = 0;
        }
        line.push_str(grapheme);
        used += grapheme_width;
        if used >= width {
            rows.push(std::mem::take(&mut line));
            used = 0;
        }
    }
    if !line.is_empty() {
        rows.push(line);
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows
}

fn fit_row(text: &str, content_width: usize) -> String {
    let fitted = truncate_width(text, content_width);
    let gap = content_width.saturating_sub(UnicodeWidthStr::width(fitted.as_str()));
    format!(" {fitted}{} ", " ".repeat(gap))
}

fn truncate_width(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    let limit = width.saturating_sub(1);
    let mut out = String::new();
    let mut used = 0usize;
    for grapheme in text.graphemes(true) {
        let grapheme_width = UnicodeWidthStr::width(grapheme).max(1);
        if used + grapheme_width > limit {
            break;
        }
        out.push_str(grapheme);
        used += grapheme_width;
    }
    out.push('…');
    out
}

pub fn action_at(app: &App, x: u16, y: u16) -> Option<crate::app::state::MenuAction> {
    let (ax, ay, width, height) = app.menu_area?;
    let inside = x >= ax && x < ax + width && y >= ay && y < ay + height;
    if !inside {
        return None;
    }
    let index = app.menu_rows.iter().find(|(_, row)| *row == y)?.0;
    let Overlay::ContextMenu { items, .. } = &app.overlay else {
        return None;
    };
    items
        .get(index)
        .filter(|item| item.enabled)
        .map(|item| item.action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_long_signature_inside_the_frame() {
        let signature = "const describeUser: (user: User, options: Options) => string";
        let lines = vec![(signature.to_string(), HoverLineKind::Signature)];
        let actions = ["Go to Definition", "Find References"];
        let layout = layout_symbol_menu(&lines, &actions, 40, 24, true);
        assert!(layout.content_width + 4 <= layout.width as usize);
        assert!(layout.type_rows.len() > 1);
        for (row, _) in &layout.type_rows {
            assert!(UnicodeWidthStr::width(row.as_str()) <= layout.content_width);
        }
        assert_eq!(
            separator_rule(layout.content_width).chars().count(),
            layout.content_width
        );
        assert!(
            UnicodeWidthStr::width(separator_rule(layout.content_width).as_str())
                <= layout.content_width
        );
    }

    #[test]
    fn drops_type_rows_before_actions_on_a_short_screen() {
        let lines = vec![
            ("alpha".into(), HoverLineKind::Doc),
            ("beta".into(), HoverLineKind::Doc),
            ("gamma".into(), HoverLineKind::Doc),
            ("delta".into(), HoverLineKind::Doc),
        ];
        let actions = ["Go to Definition", "Find References"];
        let layout = layout_symbol_menu(&lines, &actions, 80, 8, true);
        assert!(layout.type_rows.len() < lines.len());
        assert!(layout.show_separator || layout.type_rows.is_empty());
        assert!(layout.height <= 7);
    }
}
