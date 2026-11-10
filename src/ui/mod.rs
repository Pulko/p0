pub(crate) mod context_menu;
mod editor_view;
mod explorer;
mod git;
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
    app.side_tabs.clear();
    app.content_tabs.clear();
    if app.explorer_visible {
        explorer::render_search(frame, areas.search, app);
        if app.git_open {
            git::render(frame, areas.explorer, app);
        } else {
            explorer::render(frame, areas.explorer, app);
        }
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
    use crate::workspace::{GitFileStatus, GitPathStatus};

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

    #[test]
    fn explorer_border_puts_changes_on_the_right() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(PathBuf::from("."), LanguageClient { tx });
        app.git_repo = true;
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let y = app.explorer_area.1;
        let row = (0..app.explorer_area.2)
            .map(|col| {
                terminal.backend().buffer()[(app.explorer_area.0 + col, y)]
                    .symbol()
                    .to_string()
            })
            .collect::<String>();
        let explorer = row.find("Explorer").expect("explorer title");
        let changes = row.find("Changes").expect("changes title");
        assert!(explorer < changes);
        assert_eq!(app.side_tabs.len(), 2);
        assert!(!app.side_tabs[0].changes);
        assert!(app.side_tabs[1].changes);
        assert!(app.side_tabs[0].x < app.side_tabs[1].x);
        let accent = Color::Rgb(110, 186, 196);
        let buffer = terminal.backend().buffer();
        let explorer_cell = &buffer[(app.side_tabs[0].x + 1, y)];
        assert_eq!(explorer_cell.symbol(), "E");
        assert_eq!(explorer_cell.fg, accent);
        let changes_cell = &buffer[(app.side_tabs[1].x + 1, y)];
        assert_eq!(changes_cell.symbol(), "C");
        assert_ne!(changes_cell.fg, accent);
    }

    #[test]
    fn a_plain_folder_has_no_git_controls() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(PathBuf::from("."), LanguageClient { tx });
        app.git_branch = Some("topic".into());
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let explorer = (0..app.explorer_area.2)
            .map(|col| {
                terminal.backend().buffer()[(app.explorer_area.0 + col, app.explorer_area.1)]
                    .symbol()
                    .to_string()
            })
            .collect::<String>();
        let editor = (0..app.editor_area.2)
            .map(|col| {
                terminal.backend().buffer()[(app.editor_area.0 + col, app.editor_area.1)]
                    .symbol()
                    .to_string()
            })
            .collect::<String>();
        let status = (0..app.status_area.2)
            .map(|col| {
                terminal.backend().buffer()[(app.status_area.0 + col, app.status_area.1)]
                    .symbol()
                    .to_string()
            })
            .collect::<String>();
        assert!(explorer.contains("Explorer"));
        assert!(!explorer.contains("Changes"));
        assert!(editor.contains("Editor"));
        assert!(!editor.contains("Diff"));
        assert!(!status.contains("topic"));
        assert_eq!(app.side_tabs.len(), 1);
        assert_eq!(app.content_tabs.len(), 1);
        assert!(!app.content_tabs[0].diff);
    }

    #[test]
    fn editor_border_puts_diff_on_the_right() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(PathBuf::from("."), LanguageClient { tx });
        app.git_repo = true;
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let y = app.editor_area.1;
        let row = (0..app.editor_area.2)
            .map(|col| {
                terminal.backend().buffer()[(app.editor_area.0 + col, y)]
                    .symbol()
                    .to_string()
            })
            .collect::<String>();
        let editor = row.find("Editor").expect("file title");
        let diff = row.find("Diff").expect("diff title");
        assert!(editor < diff);
        assert_eq!(app.content_tabs.len(), 2);
        assert!(!app.content_tabs[0].diff);
        assert!(app.content_tabs[1].diff);
        assert!(app.content_tabs[0].x < app.content_tabs[1].x);
        let accent = Color::Rgb(110, 186, 196);
        let buffer = terminal.backend().buffer();
        let editor_cell = &buffer[(app.content_tabs[0].x + 1, y)];
        assert_eq!(editor_cell.symbol(), "E");
        assert_eq!(editor_cell.fg, accent);
        let diff_cell = &buffer[(app.content_tabs[1].x + 1, y)];
        assert_eq!(diff_cell.symbol(), "D");
        assert_ne!(diff_cell.fg, accent);
        let status = (0..app.status_area.2)
            .map(|col| {
                buffer[(app.status_area.0 + col, app.status_area.1)]
                    .symbol()
                    .to_string()
            })
            .collect::<String>();
        assert!(!status.contains("Diff"));

        app.diff_view = Some(crate::app::state::DiffView {
            document_id: crate::editor::DocumentId(0),
            path: Some(PathBuf::from("src/main.rs")),
            source: crate::app::state::DiffSource::Unstaged,
            lines: Vec::new(),
            parsed: None,
            allow_hunks: false,
            error: None,
            scroll: 0,
            loading: false,
        });
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let y = app.editor_area.1;
        let buffer = terminal.backend().buffer();
        let file_cell = &buffer[(app.content_tabs[0].x + 1, y)];
        assert_eq!(file_cell.symbol(), "s");
        assert_ne!(file_cell.fg, accent);
        let diff_cell = &buffer[(app.content_tabs[1].x + 1, y)];
        assert_eq!(diff_cell.symbol(), "D");
        assert_eq!(diff_cell.fg, accent);
    }

    #[test]
    fn git_panel_lists_staged_changes_and_commit() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(PathBuf::from("/repo"), LanguageClient { tx });
        app.git_repo = true;
        app.git_open = true;
        app.git_branch = Some("main".into());
        app.git_message = "Keep the review".into();
        app.git_status.insert(
            PathBuf::from("/repo/src/main.rs"),
            GitPathStatus {
                staged: Some(GitFileStatus::Modified),
                unstaged: Some(GitFileStatus::Modified),
            },
        );
        app.git_status.insert(
            PathBuf::from("/repo/notes.txt"),
            GitPathStatus {
                staged: None,
                unstaged: Some(GitFileStatus::Untracked),
            },
        );
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("Staged"));
        assert!(text.contains("Changes"));
        assert!(text.contains("Commit"));
        assert!(text.contains("main.rs"));
        assert!(text.contains("notes.txt"));
        assert!(text.contains("Keep the review"));
        assert!(
            app.git_hits
                .iter()
                .any(|hit| matches!(hit.action, crate::app::state::GitClick::Commit))
        );
        assert!(app.git_hits.iter().any(|hit| matches!(
            hit.action,
            crate::app::state::GitClick::Toggle { staged: true, .. }
        )));
        assert!(app.git_hits.iter().any(|hit| matches!(
            hit.action,
            crate::app::state::GitClick::Toggle { staged: false, .. }
        )));
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
