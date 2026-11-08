#![allow(dead_code)]

use crate::app::commands;
use crate::app::state::{App, ConfirmAction, Focus, Overlay, click_was_double};
use crate::language::{LspEvent, LspStatus};

pub fn handle_lsp(app: &mut App, event: LspEvent) {
    match event {
        LspEvent::Status(status) => {
            app.lsp_status = status;
        }
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
                if let (Some(doc), Some(view)) = (app.documents.get(&id), app.views.get_mut(&id)) {
                    let pos = crate::app::state::decode_diagnostic_pos(doc, item.position);
                    crate::editor::set_cursor(view, &doc.rope, pos, false);
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

pub fn status_text(status: &LspStatus) -> String {
    match status {
        LspStatus::Ready => "LSP ready".into(),
        LspStatus::Starting => "LSP starting".into(),
        LspStatus::Unavailable(s) => format!("LSP off ({s})"),
        LspStatus::Error(s) => format!("LSP error ({s})"),
    }
}

pub fn handle_search_results(app: &mut App, results: Vec<crate::workspace::SearchMatch>) {
    if let Overlay::SearchWorkspace {
        results: out,
        selected,
        searching,
        ..
    } = &mut app.overlay
    {
        *out = results;
        *selected = 0;
        *searching = false;
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
