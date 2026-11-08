use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};

use crate::app::App;
use crate::app::state::Overlay;
use crate::ui::theme::Theme;

pub fn render(frame: &mut Frame, app: &mut App) {
    let theme = app.theme();
    if !matches!(app.overlay, Overlay::Themes { .. } | Overlay::Help) {
        app.overlay_area = None;
        app.overlay_rows.clear();
    }
    let completion_selected = match &app.overlay {
        Overlay::Completion { selected, .. } => Some(*selected),
        _ => None,
    };
    if let Some(selected) = completion_selected {
        render_completions(frame, app, selected, theme);
        return;
    }
    let themes_selected = match &app.overlay {
        Overlay::Themes { selected } => Some(*selected),
        _ => None,
    };
    if let Some(selected) = themes_selected {
        render_themes(frame, app, selected, theme);
        return;
    }
    if matches!(app.overlay, Overlay::Help) {
        render_help(frame, app, theme);
        return;
    }
    match &app.overlay {
        Overlay::FileFinder {
            query,
            results,
            selected,
        } => {
            render_list(
                frame,
                frame.area(),
                " File finder ",
                &format!(" {query} "),
                results,
                *selected,
                theme,
            );
        }
        Overlay::SearchWorkspace {
            query,
            results,
            selected,
            searching,
        } => {
            let items: Vec<String> = results
                .iter()
                .map(|m| {
                    format!(
                        "{}:{}  {}",
                        crate::app::App::relative_static(app, &m.path),
                        m.line + 1,
                        m.text
                    )
                })
                .collect();
            let title = if *searching {
                " Workspace search (searching) "
            } else {
                " Workspace search "
            };
            render_list(
                frame,
                frame.area(),
                title,
                &format!(" {query} "),
                &items,
                *selected,
                theme,
            );
        }
        Overlay::SearchFile {
            query,
            matches,
            current,
        } => {
            let info = format!(
                " {}  {}/{} ",
                query,
                if matches.is_empty() { 0 } else { current + 1 },
                matches.len()
            );
            let area = top_box(frame.area(), 3, 40);
            frame.render_widget(Clear, area);
            frame.render_widget(
                Paragraph::new(info)
                    .style(theme.text())
                    .block(theme.panel(" Find ")),
                area,
            );
        }
        Overlay::GoToLine { input } => {
            let area = top_box(frame.area(), 3, 24);
            frame.render_widget(Clear, area);
            frame.render_widget(
                Paragraph::new(format!(" {input} "))
                    .style(theme.text())
                    .block(theme.panel(" Go to line ")),
                area,
            );
        }
        Overlay::Completion { .. } => {}
        Overlay::Hover { text, x, y, .. } => {
            let content = parse_hover_content(text);
            let width = content
                .iter()
                .map(|(line, _)| line.chars().count() as u16)
                .max()
                .unwrap_or(20)
                .saturating_add(4)
                .clamp(24, 72)
                .min(frame.area().width.saturating_sub(2));
            let height = (content.len() as u16 + 2)
                .clamp(3, 14)
                .min(frame.area().height.saturating_sub(2));
            let mut area = Rect {
                x: x.saturating_add(1),
                y: y.saturating_add(1),
                width,
                height,
            };
            if area.right() > frame.area().right() {
                area.x = frame.area().right().saturating_sub(width);
            }
            if area.bottom() > frame.area().bottom() {
                area.y = y.saturating_sub(height);
            }
            frame.render_widget(Clear, area);
            let lines: Vec<Line> = content
                .into_iter()
                .take(height.saturating_sub(2) as usize)
                .map(|(line, kind)| {
                    let style = match kind {
                        HoverLineKind::Signature => Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::BOLD),
                        HoverLineKind::Tag => Style::default().fg(theme.warning),
                        HoverLineKind::Doc => Style::default().fg(theme.fg_muted),
                    };
                    Line::from(Span::styled(line, style))
                })
                .collect();
            frame.render_widget(
                Paragraph::new(lines)
                    .style(theme.text())
                    .wrap(Wrap { trim: false })
                    .block(theme.panel(" Type info ")),
                area,
            );
        }
        Overlay::Signature { text } => {
            let area = bottom_box(frame.area(), 8, 60);
            frame.render_widget(Clear, area);
            frame.render_widget(
                Paragraph::new(text.as_str())
                    .style(theme.text())
                    .wrap(Wrap { trim: false })
                    .block(theme.panel(" Info ")),
                area,
            );
        }
        Overlay::References { items, selected } => {
            let list: Vec<String> = items
                .iter()
                .map(|l| {
                    format!(
                        "{}:{}",
                        crate::app::App::relative_static(app, &l.path),
                        l.position.line + 1
                    )
                })
                .collect();
            render_list(
                frame,
                frame.area(),
                " References ",
                "",
                &list,
                *selected,
                theme,
            );
        }
        Overlay::Confirm { message, .. } => {
            let area = center(frame.area(), 6, 50);
            frame.render_widget(Clear, area);
            frame.render_widget(
                Paragraph::new(format!("{message}\n\nEnter = confirm   Esc = cancel"))
                    .style(theme.text())
                    .block(theme.panel(" Confirm ")),
                area,
            );
        }
        Overlay::Prompt { title, input, .. } => {
            let area = center(frame.area(), 5, 50);
            frame.render_widget(Clear, area);
            let prompt_title = format!(" {title} ");
            frame.render_widget(
                Paragraph::new(format!(" {input} "))
                    .style(theme.text())
                    .block(theme.panel(&prompt_title)),
                area,
            );
        }
        Overlay::Themes { .. } | Overlay::Help | Overlay::ContextMenu { .. } | Overlay::None => {}
    }
}

fn render_themes(
    frame: &mut Frame,
    app: &mut App,
    selected: usize,
    theme: crate::ui::theme::Theme,
) {
    let names = crate::ui::theme::theme_names();
    let width = 18u16.min(frame.area().width.saturating_sub(2)).max(12);
    let height = (names.len() as u16 + 2).min(frame.area().height.saturating_sub(1));
    let anchor = app
        .status_hits
        .iter()
        .find(|hit| hit.action == crate::app::state::StatusAction::Theme);
    let x = anchor
        .map(|hit| hit.x)
        .unwrap_or(frame.area().right().saturating_sub(width))
        .min(frame.area().right().saturating_sub(width));
    let y = frame
        .area()
        .bottom()
        .saturating_sub(height.saturating_add(1));
    let area = Rect {
        x,
        y,
        width,
        height,
    };
    app.overlay_area = Some((area.x, area.y, area.width, area.height));
    app.overlay_rows = names
        .iter()
        .enumerate()
        .map(|(index, _)| (index, area.y + 1 + index as u16))
        .filter(|(_, row)| *row < area.bottom().saturating_sub(1))
        .collect();
    frame.render_widget(Clear, area);
    let items: Vec<ListItem> = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let current = *name == theme.name;
            let mut style = Style::default().fg(theme.fg);
            if index == selected {
                style = style.bg(theme.bg_selected).add_modifier(Modifier::BOLD);
            }
            let mark = if current { "● " } else { "  " };
            ListItem::new(Span::styled(format!("{mark}{name}"), style))
        })
        .collect();
    frame.render_widget(
        List::new(items)
            .style(theme.text())
            .block(theme.panel(" Theme ")),
        area,
    );
}

fn render_help(frame: &mut Frame, app: &mut App, theme: crate::ui::theme::Theme) {
    let rows = shortcut_rows();
    let width = 42u16.min(frame.area().width.saturating_sub(2)).max(24);
    let height = (rows.len() as u16 + 2)
        .min(frame.area().height.saturating_sub(2))
        .max(3);
    let area = center(frame.area(), height, width);
    app.overlay_area = Some((area.x, area.y, area.width, area.height));
    app.overlay_rows.clear();
    frame.render_widget(Clear, area);
    let lines: Vec<Line> = rows
        .into_iter()
        .take(height.saturating_sub(2) as usize)
        .map(|(keys, action)| {
            Line::from(vec![
                Span::styled(format!(" {keys:<16}"), Style::default().fg(theme.accent)),
                Span::styled(action, Style::default().fg(theme.fg)),
            ])
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme.text())
            .block(theme.panel(" Shortcuts ")),
        area,
    );
}

pub(crate) fn shortcut_rows() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Ctrl+/", "Show this list"),
        ("Ctrl+S", "Save"),
        ("Ctrl+P", "File finder"),
        ("Ctrl+F", "Search in file"),
        ("Ctrl+Shift+F", "Search workspace"),
        ("Ctrl+G", "Go to line"),
        ("Ctrl+Space", "Completion"),
        ("Ctrl+Enter", "Go to definition"),
        ("Ctrl+Shift+R", "Find references"),
        ("Ctrl+R", "Rename symbol"),
        ("Ctrl+Z / Y", "Undo / redo"),
        ("Ctrl+Tab", "Next / previous tab"),
        ("Ctrl+Shift+D", "Git diff"),
        ("Ctrl+B", "Toggle explorer"),
        ("Ctrl+W", "Close file"),
        ("Ctrl+N", "New file"),
        ("Ctrl+Q", "Quit"),
        ("Esc", "Close a popup"),
        ("Click theme", "Choose a color scheme"),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HoverLineKind {
    Signature,
    Tag,
    Doc,
}

pub(crate) fn parse_hover_content(text: &str) -> Vec<(String, HoverLineKind)> {
    let mut in_fence = false;
    let mut result: Vec<(String, HoverLineKind)> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_end();
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        let clean = line.replace("**", "").replace('`', "").trim().to_string();
        if clean.is_empty() {
            if result.last().is_some_and(|(line, _)| !line.is_empty()) {
                result.push((String::new(), HoverLineKind::Doc));
            }
            continue;
        }
        let kind = if in_fence {
            HoverLineKind::Signature
        } else if clean.starts_with('@') {
            HoverLineKind::Tag
        } else {
            HoverLineKind::Doc
        };
        result.push((clean, kind));
    }
    result
}

impl crate::app::App {
    pub fn relative_static(app: &App, path: &std::path::Path) -> String {
        app.relative(path)
    }
}

fn render_completions(frame: &mut Frame, app: &App, selected: usize, theme: Theme) {
    let items = crate::app::commands::visible_completions(app);
    if items.is_empty() {
        return;
    }
    let selected = selected.min(items.len() - 1);
    let (ex, ey, ew, eh) = app.editor_area;
    let origin = app.active_view().map(|v| v.cursor).unwrap_or_default();
    let y = ey.saturating_add(1).saturating_add(
        (origin
            .line
            .saturating_sub(app.active_view().map(|v| v.viewport.top).unwrap_or(0))) as u16,
    );
    let mut height = (items.len() as u16 + 2).min(12).min(eh.saturating_sub(1));
    let mut rect = Rect {
        x: (ex + app.gutter_width + 2).min(ex + ew.saturating_sub(24)),
        y: (y + 1).min(ey + eh.saturating_sub(height)),
        width: 40.min(ew.saturating_sub(2)).max(16),
        height,
    };
    if rect.y + rect.height > ey + eh {
        height = (y.saturating_sub(ey)).min(12).max(3);
        rect.y = y.saturating_sub(height);
        rect.height = height;
    }
    frame.render_widget(Clear, rect);
    let list_items: Vec<ListItem> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let mut style = Style::default().fg(theme.fg);
            if i == selected {
                style = style.bg(theme.bg_selected).add_modifier(Modifier::BOLD);
            }
            let tag = crate::language::completion_kind_tag(item.kind);
            let tag_style = if i == selected {
                style.fg(theme.accent)
            } else {
                Style::default().fg(theme.accent)
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{tag} "), tag_style),
                Span::styled(item.label.clone(), style),
            ]))
        })
        .collect();
    frame.render_widget(
        List::new(list_items)
            .style(theme.text())
            .block(theme.panel(" Completions ")),
        rect,
    );
}

fn render_list(
    frame: &mut Frame,
    full: Rect,
    title: &str,
    query: &str,
    items: &[String],
    selected: usize,
    theme: Theme,
) {
    let area = center(full, 18, 70);
    frame.render_widget(Clear, area);
    let chunks = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Min(1),
        ])
        .split(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .inner(area),
        );
    frame.render_widget(theme.panel(title), area);
    let inner = Block::default().borders(Borders::ALL).inner(area);
    let rows = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Min(1),
        ])
        .split(inner);
    frame.render_widget(
        Paragraph::new(query.to_string()).style(theme.text()),
        rows[0],
    );
    let list_items: Vec<ListItem> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let mut style = Style::default().fg(theme.fg);
            if i == selected {
                style = style.bg(theme.bg_selected);
            }
            ListItem::new(Span::styled(item.clone(), style))
        })
        .collect();
    frame.render_widget(List::new(list_items), rows[1]);
    let _ = chunks;
}

fn center(area: Rect, height: u16, width: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2)).max(10);
    let height = height.min(area.height.saturating_sub(2)).max(3);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 3,
        width,
        height,
    }
}

fn top_box(area: Rect, height: u16, width: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2)).max(10);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + 1,
        width,
        height: height.min(area.height),
    }
}

fn bottom_box(area: Rect, height: u16, width: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(4)).max(12);
    let height = height.min(area.height.saturating_sub(3)).max(3);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.bottom().saturating_sub(height + 2),
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_hover_signature_docs_and_tags() {
        let parsed = parse_hover_content(
            "```typescript\ninterface User {\n  name: string\n}\n```\nA user record.\n@param name display name",
        );
        assert!(parsed.iter().any(|(line, kind)| {
            line == "interface User {" && *kind == HoverLineKind::Signature
        }));
        assert!(
            parsed
                .iter()
                .any(|(line, kind)| { line == "A user record." && *kind == HoverLineKind::Doc })
        );
        assert!(
            parsed
                .iter()
                .any(|(line, kind)| { line.starts_with("@param") && *kind == HoverLineKind::Tag })
        );
    }
}
