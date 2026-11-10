pub(crate) mod context_menu;
mod editor_view;
mod explorer;
pub(crate) mod layout;
mod popup;
mod status_bar;
pub(crate) mod theme;
mod thread;

use ratatui::Frame;

use crate::app::App;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let areas = layout::layout(frame.area(), app);
    app.search_area = (
        areas.search.x,
        areas.search.y,
        areas.search.width,
        areas.search.height,
    );
    app.explorer_area = (
        areas.explorer.x,
        areas.explorer.y,
        areas.explorer.width,
        areas.explorer.height,
    );
    app.editor_area = (
        areas.editor.x,
        areas.editor.y,
        areas.editor.width,
        areas.editor.height,
    );
    app.tabs_area = (
        areas.tabs.x,
        areas.tabs.y,
        areas.tabs.width,
        areas.tabs.height,
    );
    app.status_area = (
        areas.status.x,
        areas.status.y,
        areas.status.width,
        areas.status.height,
    );
    app.thread_area = (
        areas.threads.x,
        areas.threads.y,
        areas.threads.width,
        areas.threads.height,
    );
    if app.explorer_visible {
        explorer::render_search(frame, areas.search, app);
        explorer::render(frame, areas.explorer, app);
    }
    editor_view::render(frame, areas.tabs, areas.editor, app);
    if app.thread_visible {
        thread::render(frame, areas.threads, app);
    }
    status_bar::render(frame, areas.status, app);
    popup::render(frame, app);
    context_menu::render(frame, app);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use tokio::sync::mpsc;

    use crate::app::state::{App, Focus, Overlay};
    use crate::language::LanguageClient;

    use super::draw;

    #[test]
    fn focused_thread_pane_uses_the_accent_border() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(PathBuf::from("."), LanguageClient { tx });
        app.thread_visible = true;
        app.focus = Focus::Thread;
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let (x, y, _, _) = app.thread_area;
        let accent = Color::Rgb(110, 186, 196);
        assert_eq!(terminal.backend().buffer()[(x, y)].fg, accent);
        assert!(
            buffer_text(terminal.backend().buffer()).contains("Select code and press Option+T")
        );
        app.focus = Focus::Editor;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let (x, y, _, _) = app.thread_area;
        assert_ne!(terminal.backend().buffer()[(x, y)].fg, accent);
    }

    #[test]
    fn explorer_header_fills_the_row_after_hide() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(PathBuf::from("."), LanguageClient { tx });
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let bg = Color::Rgb(30, 30, 30);
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].bg, bg);
        assert_eq!(buffer[(22, 0)].symbol(), " ");
        assert_eq!(buffer[(22, 0)].bg, bg);
        assert_eq!(buffer[(27, 0)].bg, bg);
    }

    #[test]
    fn pane_mark_stays_when_the_explorer_is_hidden() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(PathBuf::from("."), LanguageClient { tx });
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let (x, y, _) = app.pane_mark.expect("pane mark");
        assert!(x > 0);
        assert_eq!(terminal.backend().buffer()[(x, y)].symbol(), "◧");
        app.explorer_visible = false;
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let (x, y, _) = app.pane_mark.expect("pane mark");
        assert_eq!((x, y), (0, 0));
        assert_eq!(terminal.backend().buffer()[(x, y)].symbol(), "◧");
    }

    #[test]
    fn types_note_shows_the_install_command() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(PathBuf::from("."), LanguageClient { tx });
        app.overlay = Overlay::TypesNote;
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("Editing works without it."));
        assert!(text.contains("npm install -g typescript typescript-language-server"));
        assert!(text.contains("typescript-language-server on PATH."));
        assert!(text.contains("Click ⊘ types off to check again"));
        assert!(text.contains("Copy"));
        let (x, y, width) = app.types_copy.expect("copy button");
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(x + 1, y)].symbol(), "C");
        assert!(width >= 4);
        crate::app::commands::click_types_note(&mut app, x, y);
        assert_eq!(app.message.as_deref(), Some("Copied"));
        assert!(matches!(app.overlay, Overlay::TypesNote));
    }

    fn buffer_text(buffer: &ratatui::buffer::Buffer) -> String {
        let mut text = String::new();
        for row in 0..buffer.area.height {
            for col in 0..buffer.area.width {
                text.push_str(buffer[(col, row)].symbol());
            }
        }
        text
    }
}
