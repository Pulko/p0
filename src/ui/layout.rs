use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::app::App;

pub struct Areas {
    pub search: Rect,
    pub explorer: Rect,
    pub tabs: Rect,
    pub editor: Rect,
    pub status: Rect,
}

pub fn layout(area: Rect, app: &App) -> Areas {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    let body = rows[0];
    let status = rows[1];
    let cols = if app.explorer_visible {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(app.config.explorer_width.max(12)),
                Constraint::Min(10),
            ])
            .split(body)
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(0), Constraint::Min(10)])
            .split(body)
    };
    let explorer_col = cols[0];
    let editor_col = cols[1];
    let explorer_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(explorer_col);
    let editor_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(editor_col);
    Areas {
        search: explorer_rows[0],
        explorer: explorer_rows[1],
        tabs: editor_rows[0],
        editor: editor_rows[1],
        status,
    }
}
