use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::app::App;
use crate::app::config::EXPLORER_MIN_WIDTH;

const EDITOR_RESERVE: u16 = 20;
const THREAD_WIDTH: u16 = 36;

pub struct Areas {
    pub search: Rect,
    pub explorer: Rect,
    pub tabs: Rect,
    pub editor: Rect,
    pub threads: Rect,
    pub status: Rect,
}

pub fn layout(area: Rect, app: &App) -> Areas {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    let body = rows[0];
    let status = rows[1];
    let (explorer_width, thread_width) = column_widths(
        body.width,
        app.explorer_visible,
        app.config.explorer_width,
        app.thread_visible,
    );
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(explorer_width),
            Constraint::Min(10),
            Constraint::Length(thread_width),
        ])
        .split(body);
    let explorer_col = cols[0];
    let editor_col = cols[1];
    let thread_col = cols[2];
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
        threads: thread_col,
        status,
    }
}

pub fn column_widths(
    body: u16,
    explorer_visible: bool,
    configured: u16,
    thread_visible: bool,
) -> (u16, u16) {
    let thread_pref = if thread_visible { THREAD_WIDTH } else { 0 };
    let room = body
        .saturating_sub(EDITOR_RESERVE)
        .saturating_sub(thread_pref);
    let explorer = if explorer_visible {
        let preferred = configured.max(EXPLORER_MIN_WIDTH);
        if room < EXPLORER_MIN_WIDTH {
            room
        } else {
            preferred.min(room)
        }
    } else {
        0
    };
    let thread = if thread_visible {
        THREAD_WIDTH.min(body.saturating_sub(explorer).saturating_sub(EDITOR_RESERVE))
    } else {
        0
    };
    (explorer, thread)
}

pub fn dragged_explorer_width(origin: u16, x: u16) -> u16 {
    x.saturating_sub(origin)
        .saturating_add(1)
        .max(EXPLORER_MIN_WIDTH)
}

pub fn stored_explorer_width(body: u16, thread_visible: bool, origin: u16, x: u16) -> u16 {
    let preferred = dragged_explorer_width(origin, x);
    let thread_pref = if thread_visible { THREAD_WIDTH } else { 0 };
    let room = body
        .saturating_sub(EDITOR_RESERVE)
        .saturating_sub(thread_pref);
    if room < EXPLORER_MIN_WIDTH {
        EXPLORER_MIN_WIDTH
    } else {
        preferred.min(room)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::layout::Rect;
    use tokio::sync::mpsc;

    use crate::app::state::App;
    use crate::language::LanguageClient;

    use super::{column_widths, dragged_explorer_width, layout};

    #[test]
    fn thread_column_is_hidden_until_opened() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(PathBuf::from("."), LanguageClient { tx });
        let area = Rect::new(0, 0, 120, 40);
        let closed = layout(area, &app);
        assert_eq!(closed.threads.width, 0);
        app.thread_visible = true;
        let open = layout(area, &app);
        assert_eq!(open.threads.width, 36);
        assert_eq!(open.threads.x + open.threads.width, 120);
        assert!(open.editor.width >= 10);
    }

    #[test]
    fn explorer_grows_past_a_third_and_not_below_28() {
        assert_eq!(column_widths(120, true, 50, false), (50, 0));
        assert_eq!(column_widths(120, true, 10, true), (28, 36));
        assert_eq!(column_widths(40, true, 28, false).0, 20);
        assert_eq!(dragged_explorer_width(0, 10), 28);
        assert_eq!(dragged_explorer_width(0, 40), 41);
        assert_eq!(super::stored_explorer_width(120, true, 0, 200), 64);
        assert_eq!(super::stored_explorer_width(30, false, 0, 0), 28);
    }
}
