#![allow(dead_code)]

use crate::app::commands;
use crate::app::state::{App, ConfirmAction, Focus, Overlay, click_was_double};
use crate::language::{LspEvent, LspStatus};

pub fn handle_lsp(app: &mut App, event: LspEvent) {
    match event {
        LspEvent::Status(status) => apply_lsp_status(app, status),
        LspEvent::Diagnostics { path, mut items } => {
            if let Some(doc) = app
                .documents
                .values()
                .find(|d| d.path.as_ref() == Some(&path))
            {
                for item in &mut items {
                    item.range_start = crate::language::utf16_position_to_doc(
                        doc,
                        item.range_start.line,
                        item.range_start.column as u32,
                    );
                    item.range_end = crate::language::utf16_position_to_doc(
                        doc,
                        item.range_end.line,
                        item.range_end.column as u32,
                    );
                }
            }
            app.diagnostics.insert(path, items);
        }
        LspEvent::Completion { token, items } => {
            if token != app.completion_token {
                return;
            }
            let origin = app.active_view().map(|v| v.cursor).unwrap_or_default();
            app.overlay = Overlay::Completion {
                items,
                selected: 0,
                origin,
            };
            commands::refresh_completion_popup(app);
        }
        LspEvent::Hover { token, text } => {
            if let Overlay::ContextMenu {
                show_type: true,
                hover_token,
                type_text,
                ..
            } = &mut app.overlay
            {
                if *hover_token == token {
                    *type_text = if text.trim().is_empty() {
                        "Type information unavailable".into()
                    } else {
                        text
                    };
                    return;
                }
            }
            if token == app.hover_token
                && !text.is_empty()
                && let Some((anchor_token, x, y, origin)) = app.hover_anchor
                && anchor_token == token
            {
                app.overlay = Overlay::Hover {
                    title: "Type info".into(),
                    text,
                    origin,
                    x,
                    y,
                };
            }
        }
        LspEvent::Signature { text, .. } => {
            if !text.is_empty() {
                app.overlay = Overlay::Signature { text };
            }
        }
        LspEvent::Locations { items, .. } => {
            if items.len() == 1 {
                let item = items[0].clone();
                app.open_path(item.path);
                let Some(id) = app.active else { return };
                if let Some(pos) = app
                    .documents
                    .get(&id)
                    .map(|doc| crate::app::state::decode_diagnostic_pos(doc, item.position))
                {
                    commands::jump_to(app, id, pos);
                }
            } else if !items.is_empty() {
                app.overlay = Overlay::References { items, selected: 0 };
            } else {
                app.message = Some("no locations".into());
            }
        }
        LspEvent::Edits { items, .. } => {
            if items.is_empty() {
                app.message = Some("rename produced no edits".into());
            } else {
                app.apply_edits(items);
                app.message = Some("renamed".into());
            }
        }
    }
}

pub fn apply_lsp_status(app: &mut App, status: LspStatus) {
    let missing = matches!(status, LspStatus::Unavailable(_));
    let ready = matches!(status, LspStatus::Ready);
    app.lsp_status = status;
    if ready && matches!(app.overlay, Overlay::TypesNote) {
        app.overlay = Overlay::None;
        return;
    }
    if missing && !app.config.types_note_dismissed && matches!(app.overlay, Overlay::None) {
        app.overlay = Overlay::TypesNote;
    }
}

pub fn status_text(status: &LspStatus) -> String {
    match status {
        LspStatus::Ready | LspStatus::Starting => "Types".into(),
        LspStatus::Unavailable(_) => "⊘ types off".into(),
        LspStatus::Error(_) => "Types stopped".into(),
    }
}

pub fn handle_search_results(app: &mut App, update: crate::workspace::WorkspaceSearch) {
    if update.generation != app.search_generation {
        return;
    }
    if let Some(message) = update.message {
        app.message = Some(message);
    }
    if let Overlay::Search {
        query,
        files,
        matches,
        selected,
        searching,
        ..
    } = &mut app.overlay
    {
        if query.trim().is_empty() {
            return;
        }
        *matches = update.matches;
        *searching = false;
        let total = files.len() + matches.len();
        if total == 0 {
            *selected = 0;
        } else if *selected >= files.len() {
            *selected = (*selected).min(total - 1);
        }
    }
}

pub fn request_quit(app: &mut App) {
    commands::maybe_quit(app);
}

pub fn confirm_external_reload_if_needed(app: &mut App) {
    if matches!(
        app.overlay,
        Overlay::Confirm {
            action: ConfirmAction::Reload(_),
            ..
        }
    ) {
        return;
    }
}

pub fn editor_has_focus(app: &App) -> bool {
    app.focus == Focus::Editor
        && matches!(
            app.overlay,
            Overlay::None
                | Overlay::Completion { .. }
                | Overlay::Hover { .. }
                | Overlay::Signature { .. }
        )
}

pub fn note_click(app: &mut App, x: u16, y: u16) -> bool {
    let dbl = click_was_double(app.last_click, x, y);
    app.last_click = Some((std::time::Instant::now(), x, y));
    dbl
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tokio::sync::mpsc;

    use crate::app::state::{App, Overlay};
    use crate::language::{LanguageClient, LspCmd, LspStatus};

    use super::{apply_lsp_status, status_text};

    #[test]
    fn status_names_types_without_calling_it_an_error() {
        assert_eq!(status_text(&LspStatus::Ready), "Types");
        assert_eq!(status_text(&LspStatus::Starting), "Types");
        assert_eq!(
            status_text(&LspStatus::Unavailable("missing".into())),
            "⊘ types off"
        );
        assert_eq!(
            status_text(&LspStatus::Error("language server exited".into())),
            "Types stopped"
        );
    }

    #[test]
    fn a_missing_server_opens_the_note_once() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(PathBuf::from("."), LanguageClient { tx });
        app.config.types_note_dismissed = false;
        app.overlay = Overlay::None;
        apply_lsp_status(
            &mut app,
            LspStatus::Unavailable("typescript-language-server not found".into()),
        );
        assert!(matches!(app.overlay, Overlay::TypesNote));
        app.overlay = Overlay::None;
        app.config.types_note_dismissed = true;
        apply_lsp_status(
            &mut app,
            LspStatus::Unavailable("typescript-language-server not found".into()),
        );
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn types_ready_closes_the_note_without_remembering_a_dismissal() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(PathBuf::from("."), LanguageClient { tx });
        app.overlay = Overlay::TypesNote;
        app.config.types_note_dismissed = false;
        apply_lsp_status(&mut app, LspStatus::Ready);
        assert!(matches!(app.overlay, Overlay::None));
        assert!(!app.config.types_note_dismissed);
    }

    #[test]
    fn refresh_asks_the_language_server_to_start_again() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(PathBuf::from("."), LanguageClient { tx });
        let _ = rx.try_recv();
        app.lsp_status = LspStatus::Unavailable("missing".into());
        crate::app::commands::refresh_types(&mut app);
        assert!(matches!(app.lsp_status, LspStatus::Starting));
        assert!(matches!(rx.try_recv(), Ok(LspCmd::Restart { .. })));
    }
}
