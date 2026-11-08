use std::path::PathBuf;

use crate::editor::{
    Movement, Position, Range, Selection, delete_backward, delete_forward, indent_for_newline,
    insert_text, move_cursor, select_word_at, selected_text, set_cursor, word_bounds,
};
use crate::language::{
    CompletionItem, LspCmd, LspStatus, completion_prefix, filter_completions, strip_snippets,
    utf16_position_to_doc,
};
use crate::workspace::{create_path, delete_path, find_in_document, rename_path};

use super::state::{
    App, ConfirmAction, Focus, MenuAction, MenuItem, Overlay, PromptKind, decode_diagnostic_pos,
    is_ident_char,
};

pub fn save_active(app: &mut App) {
    let Some(id) = app.active else { return };
    let mut needs_save_as = false;
    if let Some(doc) = app.documents.get_mut(&id) {
        if doc.path.is_none() {
            needs_save_as = true;
        } else if let Err(err) = doc.save() {
            app.message = Some(format!("save failed: {err}"));
            return;
        } else {
            app.message = Some("saved".into());
        }
    }
    if needs_save_as {
        app.overlay = Overlay::Prompt {
            title: "Save as (relative path)".into(),
            input: String::new(),
            kind: PromptKind::SaveAs,
        };
    } else {
        app.refresh_git_status();
    }
}

pub fn type_text(app: &mut App, text: &str) {
    app.diff_view = None;
    app.hover_dwell = None;
    if !matches!(
        app.overlay,
        Overlay::None
            | Overlay::Completion { .. }
            | Overlay::Signature { .. }
            | Overlay::Hover { .. }
    ) {
        return;
    }
    let Some(id) = app.active else { return };
    let newline = text == "\n" || text == "\r\n";
    let extra = {
        let doc = app.documents.get(&id);
        let view = app.views.get(&id);
        if newline {
            if let (Some(doc), Some(view)) = (doc, view) {
                indent_for_newline(doc, view)
            } else {
                String::new()
            }
        } else {
            String::new()
        }
    };
    let payload = if newline {
        format!(
            "{}{}",
            app.active_doc()
                .map(|d| d.line_ending.as_str())
                .unwrap_or("\n"),
            extra
        )
    } else {
        text.to_string()
    };
    let changes =
        if let (Some(doc), Some(view)) = (app.documents.get_mut(&id), app.views.get_mut(&id)) {
            let group = payload.chars().count() == 1;
            insert_text(doc, view, &payload, group)
        } else {
            Vec::new()
        };
    app.apply_changes(changes);
    if payload.chars().count() == 1 && payload.chars().next().is_some_and(is_ident_char) {
        app.request_completion();
        refresh_completion_popup(app);
    } else if !matches!(app.overlay, Overlay::Completion { .. }) {
        // keep typing
    } else if payload.chars().next().is_some_and(|c| c == '(') {
        request_signature(app);
    } else {
        dismiss_soft_overlays(app);
    }
}

pub fn move_sel(app: &mut App, movement: Movement, selecting: bool) {
    if let Some(diff) = app.diff_view.as_mut() {
        match movement {
            Movement::Up | Movement::PageUp => {
                let amount = if movement == Movement::PageUp { 10 } else { 1 };
                diff.scroll = diff.scroll.saturating_sub(amount);
            }
            Movement::Down | Movement::PageDown => {
                let amount = if movement == Movement::PageDown {
                    10
                } else {
                    1
                };
                diff.scroll = (diff.scroll + amount).min(diff.lines.len().saturating_sub(1));
            }
            _ => {}
        }
        return;
    }
    if matches!(app.overlay, Overlay::Completion { .. })
        && matches!(movement, Movement::Up | Movement::Down)
    {
        let count = visible_completions(app).len();
        if count == 0 {
            return;
        }
        if let Overlay::Completion { selected, .. } = &mut app.overlay {
            if movement == Movement::Up {
                *selected = selected.checked_sub(1).unwrap_or(count - 1);
            } else {
                *selected = (*selected + 1) % count;
            }
        }
        return;
    }
    if !matches!(
        app.overlay,
        Overlay::None
            | Overlay::Completion { .. }
            | Overlay::Hover { .. }
            | Overlay::Signature { .. }
    ) {
        overlay_move(app, movement);
        return;
    }
    let height = app.editor_area.3.saturating_sub(2) as usize;
    let Some(id) = app.active else { return };
    if let (Some(doc), Some(view)) = (app.documents.get(&id), app.views.get_mut(&id)) {
        move_cursor(view, &doc.rope, movement, selecting, height.max(1));
        view.ensure_visible(
            &doc.rope,
            height.max(1),
            app.editor_area.2.saturating_sub(app.gutter_width) as usize,
            app.config.tab_width,
        );
    }
    reveal_cursor(app, id);
}

pub fn backspace(app: &mut App) {
    if overlay_backspace(app) {
        return;
    }
    let completing = matches!(app.overlay, Overlay::Completion { .. });
    let Some(id) = app.active else { return };
    let changes =
        if let (Some(doc), Some(view)) = (app.documents.get_mut(&id), app.views.get_mut(&id)) {
            delete_backward(doc, view)
        } else {
            Vec::new()
        };
    app.apply_changes(changes);
    if completing {
        app.request_completion();
        refresh_completion_popup(app);
    }
}

pub fn delete_key(app: &mut App) {
    let Some(id) = app.active else { return };
    let changes =
        if let (Some(doc), Some(view)) = (app.documents.get_mut(&id), app.views.get_mut(&id)) {
            delete_forward(doc, view)
        } else {
            Vec::new()
        };
    app.apply_changes(changes);
}

pub fn undo(app: &mut App) {
    let Some(id) = app.active else { return };
    if let Some(doc) = app.documents.get_mut(&id) {
        if let Some(changes) = doc.undo() {
            if let Some(last) = changes.last() {
                if let Some(view) = app.views.get_mut(&id) {
                    view.cursor = last.start_position;
                    view.selection = None;
                }
            }
            app.apply_changes(changes);
        }
    }
}

pub fn redo(app: &mut App) {
    let Some(id) = app.active else { return };
    if let Some(doc) = app.documents.get_mut(&id) {
        if let Some(changes) = doc.redo() {
            if let Some(last) = changes.last() {
                if let Some(view) = app.views.get_mut(&id) {
                    view.cursor = last.new_end_position;
                    view.selection = None;
                }
            }
            app.apply_changes(changes);
        }
    }
}

pub fn copy(app: &mut App) {
    let Some(id) = app.active else { return };
    if let (Some(doc), Some(view)) = (app.documents.get(&id), app.views.get(&id)) {
        let text = selected_text(doc, view);
        if !text.is_empty() {
            app.clipboard.copy(&text);
            app.message = Some("copied".into());
        }
    }
}

pub fn cut(app: &mut App) {
    copy(app);
    let Some(id) = app.active else { return };
    let changes =
        if let (Some(doc), Some(view)) = (app.documents.get_mut(&id), app.views.get_mut(&id)) {
            if view.selection.is_some() {
                crate::editor::delete_selection(doc, view)
                    .into_iter()
                    .collect()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
    app.apply_changes(changes);
}

pub fn paste(app: &mut App) {
    let text = app.clipboard.paste();
    if !text.is_empty() {
        type_text(app, &text);
    }
}

pub fn open_file_finder(app: &mut App) {
    app.workspace_files = crate::workspace::list_workspace_files(&app.root);
    app.file_finder.set_files(&app.workspace_files);
    let results = app.file_finder.query("");
    app.overlay = Overlay::FileFinder {
        query: String::new(),
        results,
        selected: 0,
    };
}

pub fn open_search_file(app: &mut App) {
    app.overlay = Overlay::SearchFile {
        query: String::new(),
        matches: Vec::new(),
        current: 0,
    };
}

pub fn open_search_workspace(app: &mut App) {
    app.overlay = Overlay::SearchWorkspace {
        query: String::new(),
        results: Vec::new(),
        selected: 0,
        searching: false,
    };
}

fn lsp_target(app: &App) -> Option<(PathBuf, Position)> {
    let doc = app.active_doc()?;
    let view = app.active_view()?;
    Some((doc.path.clone()?, utf16(doc, view.cursor)))
}

pub fn go_to_definition(app: &mut App) {
    let Some((path, position)) = lsp_target(app) else {
        return;
    };
    let token = app.next_token();
    let _ = app.lsp.tx.send(LspCmd::Definition {
        token,
        path,
        position,
    });
}

pub fn find_references(app: &mut App) {
    let Some((path, position)) = lsp_target(app) else {
        return;
    };
    let token = app.next_token();
    let _ = app.lsp.tx.send(LspCmd::References {
        token,
        path,
        position,
    });
}

pub fn hover(app: &mut App) {
    let Some((path, position)) = lsp_target(app) else {
        return;
    };
    let token = app.next_token();
    app.hover_token = token;
    let origin = app
        .active_view()
        .map(|view| view.cursor)
        .unwrap_or_default();
    let x = app.editor_area.0.saturating_add(app.gutter_width + 2);
    let y = app.editor_area.1.saturating_add(2);
    app.hover_anchor = Some((token, x, y, origin));
    let _ = app.lsp.tx.send(LspCmd::Hover {
        token,
        path,
        position,
    });
}

pub fn schedule_hover(app: &mut App, x: u16, y: u16) {
    if app.diff_view.is_some() || !matches!(app.overlay, Overlay::None | Overlay::Hover { .. }) {
        cancel_hover(app);
        return;
    }
    let Some(document_id) = app.active else {
        cancel_hover(app);
        return;
    };
    let Some(position) = editor_position_at(app, x, y) else {
        cancel_hover(app);
        return;
    };
    let Some(doc) = app.documents.get(&document_id) else {
        return;
    };
    let range = word_bounds(&doc.rope, position);
    if range.is_empty() {
        cancel_hover(app);
        return;
    }
    let symbol_position = range.start;
    if app
        .hover_dwell
        .as_ref()
        .is_some_and(|dwell| dwell.document_id == document_id && dwell.position == symbol_position)
    {
        return;
    }
    if matches!(app.overlay, Overlay::Hover { .. }) {
        app.overlay = Overlay::None;
    }
    app.hover_dwell = Some(super::state::HoverDwell {
        document_id,
        position: symbol_position,
        screen_x: x,
        screen_y: y,
        since: std::time::Instant::now(),
    });
}

pub fn cancel_hover(app: &mut App) {
    app.hover_dwell = None;
    app.hover_anchor = None;
    if matches!(app.overlay, Overlay::Hover { .. }) {
        app.overlay = Overlay::None;
    }
}

pub fn poll_hover(app: &mut App) {
    let Some(dwell) = app.hover_dwell.clone() else {
        return;
    };
    if dwell.since.elapsed() < std::time::Duration::from_millis(400) {
        return;
    }
    let Some(doc) = app.documents.get(&dwell.document_id) else {
        app.hover_dwell = None;
        return;
    };
    let Some(path) = doc.path.clone() else {
        app.hover_dwell = None;
        return;
    };
    let position = Position::new(
        dwell.position.line,
        crate::editor::position_to_utf16(&doc.rope, dwell.position) as usize,
    );
    let token = app.next_token();
    app.hover_token = token;
    app.hover_anchor = Some((token, dwell.screen_x, dwell.screen_y, dwell.position));
    app.hover_dwell = None;
    let _ = app.lsp.tx.send(LspCmd::Hover {
        token,
        path,
        position,
    });
}

pub fn request_signature(app: &mut App) {
    let Some((path, position)) = lsp_target(app) else {
        return;
    };
    let token = app.next_token();
    let _ = app.lsp.tx.send(LspCmd::Signature {
        token,
        path,
        position,
    });
}

pub fn start_rename_symbol(app: &mut App) {
    app.overlay = Overlay::Prompt {
        title: "Rename symbol".into(),
        input: String::new(),
        kind: PromptKind::RenameSymbol,
    };
}

pub fn visible_completions(app: &App) -> Vec<CompletionItem> {
    let Overlay::Completion { items, .. } = &app.overlay else {
        return Vec::new();
    };
    let prefix = app
        .active_doc()
        .zip(app.active_view())
        .map(|(doc, view)| completion_prefix(doc, view.cursor).1)
        .unwrap_or_default();
    filter_completions(items, &prefix)
}

pub fn refresh_completion_popup(app: &mut App) {
    if !matches!(app.overlay, Overlay::Completion { .. }) {
        return;
    }
    let count = visible_completions(app).len();
    if count == 0 {
        app.overlay = Overlay::None;
        return;
    }
    if let Overlay::Completion { selected, .. } = &mut app.overlay
        && *selected >= count
    {
        *selected = count - 1;
    }
}

pub fn accept_completion(app: &mut App) {
    let visible = visible_completions(app);
    let Overlay::Completion { selected, .. } = &app.overlay else {
        return;
    };
    let Some(item) = visible.get(*selected).cloned() else {
        app.overlay = Overlay::None;
        return;
    };
    app.overlay = Overlay::None;
    let Some(id) = app.active else { return };
    let changes =
        if let (Some(doc), Some(view)) = (app.documents.get_mut(&id), app.views.get_mut(&id)) {
            let (prefix_start, _) = completion_prefix(doc, view.cursor);
            let (start, end, text) = completion_replacement(doc, view.cursor, prefix_start, &item);
            let change = doc.apply_replace(Range::new(start, end), &text, false);
            view.cursor = change.new_end_position;
            view.selection = None;
            vec![change]
        } else {
            Vec::new()
        };
    app.apply_changes(changes);
}

fn completion_replacement(
    doc: &crate::editor::Document,
    cursor: Position,
    prefix_start: Position,
    item: &CompletionItem,
) -> (Position, Position, String) {
    if let Some(edit) = &item.edit {
        let start = utf16_position_to_doc(doc, edit.start.line, edit.start.column as u32);
        let end = utf16_position_to_doc(doc, edit.end.line, edit.end.column as u32);
        if start.line == cursor.line && end.line == cursor.line {
            return (start, end, strip_snippets(&edit.new_text));
        }
    }
    (prefix_start, cursor, strip_snippets(&item.insert))
}

pub fn confirm_yes(app: &mut App) {
    let Overlay::Confirm { action, .. } = app.overlay.clone() else {
        return;
    };
    app.overlay = Overlay::None;
    match action {
        ConfirmAction::Quit => app.should_quit = true,
        ConfirmAction::Close(id) => {
            app.active = Some(id);
            app.close_active(true);
        }
        ConfirmAction::Reload(id) => {
            if let Some(doc) = app.documents.get(&id) {
                if let Some(path) = doc.path.clone() {
                    app.documents.remove(&id);
                    app.views.remove(&id);
                    app.order.retain(|x| *x != id);
                    app.open_path(path);
                }
            }
        }
        ConfirmAction::DeletePath(path) => {
            if let Err(err) = delete_path(&path) {
                app.message = Some(format!("delete failed: {err}"));
            } else {
                let ids: Vec<_> = app
                    .documents
                    .iter()
                    .filter(|(_, d)| {
                        d.path
                            .as_ref()
                            .is_some_and(|p| p == &path || p.starts_with(&path))
                    })
                    .map(|(id, _)| *id)
                    .collect();
                for id in ids {
                    app.active = Some(id);
                    app.close_active(true);
                }
                app.refresh_explorer();
                app.refresh_git_status();
            }
        }
    }
}

pub fn submit_prompt(app: &mut App) {
    let Overlay::Prompt { input, kind, .. } = app.overlay.clone() else {
        return;
    };
    app.overlay = Overlay::None;
    match kind {
        PromptKind::SaveAs => {
            let path = app.root.join(input);
            let mut opened = None;
            if let Some(doc) = app.active_doc_mut() {
                doc.path = Some(path.clone());
                doc.language = crate::editor::LanguageId::from_path(&path);
                if let Err(err) = doc.save() {
                    app.message = Some(format!("save failed: {err}"));
                } else {
                    opened = Some((doc.language, doc.text(), doc.revision as i64));
                }
            }
            if let Some((language, text, version)) = opened {
                let _ = app.lsp.tx.send(LspCmd::DidOpen {
                    path,
                    language,
                    text,
                    version,
                });
            }
        }
        PromptKind::CreateFile { is_dir } => {
            let dir = app
                .explorer_entries
                .get(app.explorer_selected)
                .map(|e| {
                    if e.is_dir {
                        e.path.clone()
                    } else {
                        e.path.parent().unwrap_or(&app.root).to_path_buf()
                    }
                })
                .unwrap_or_else(|| app.root.clone());
            let path = dir.join(input);
            if let Err(err) = create_path(&path, is_dir) {
                app.message = Some(format!("create failed: {err}"));
            } else {
                if !is_dir {
                    app.open_path(path);
                }
                app.refresh_explorer();
                app.refresh_git_status();
            }
        }
        PromptKind::RenameFile => {
            if let Some(entry) = app.explorer_entries.get(app.explorer_selected) {
                let to = entry.path.parent().unwrap_or(&app.root).join(&input);
                let from = entry.path.clone();
                if let Err(err) = rename_path(&from, &to) {
                    app.message = Some(format!("rename failed: {err}"));
                } else {
                    if let Some(doc) = app
                        .documents
                        .values_mut()
                        .find(|d| d.path.as_ref() == Some(&from))
                    {
                        doc.path = Some(to.clone());
                    }
                    app.refresh_explorer();
                    app.refresh_git_status();
                }
            }
        }
        PromptKind::RenameSymbol => {
            let Some((path, position)) = lsp_target(app) else {
                return;
            };
            let token = app.next_token();
            let _ = app.lsp.tx.send(LspCmd::Rename {
                token,
                path,
                position,
                new_name: input,
            });
        }
    }
}

pub fn overlay_char(app: &mut App, ch: char) {
    match &mut app.overlay {
        Overlay::FileFinder { query, .. }
        | Overlay::SearchFile { query, .. }
        | Overlay::SearchWorkspace { query, .. }
        | Overlay::Prompt { input: query, .. } => {
            query.push(ch);
        }
        Overlay::GoToLine { input } => {
            if ch.is_ascii_digit() {
                input.push(ch);
            }
        }
        _ => {}
    }
    refresh_overlay_query(app);
}

fn overlay_backspace(app: &mut App) -> bool {
    let handled = match &mut app.overlay {
        Overlay::FileFinder { query, .. }
        | Overlay::SearchFile { query, .. }
        | Overlay::SearchWorkspace { query, .. }
        | Overlay::GoToLine { input: query }
        | Overlay::Prompt { input: query, .. } => {
            query.pop();
            true
        }
        Overlay::None
        | Overlay::Completion { .. }
        | Overlay::Hover { .. }
        | Overlay::Signature { .. } => false,
        _ => true,
    };
    if handled {
        refresh_overlay_query(app);
    }
    handled
}

fn refresh_overlay_query(app: &mut App) {
    match app.overlay.clone() {
        Overlay::FileFinder {
            query, selected, ..
        } => {
            let results = app.file_finder.query(&query);
            app.overlay = Overlay::FileFinder {
                query,
                results,
                selected: selected.min(0),
            };
        }
        Overlay::SearchFile { query, current, .. } => {
            let matches = app
                .active_doc()
                .map(|d| find_in_document(&d.text(), &query, true))
                .unwrap_or_default();
            app.overlay = Overlay::SearchFile {
                current: current.min(matches.len().saturating_sub(1)),
                query,
                matches,
            };
            jump_search(app);
        }
        Overlay::SearchWorkspace {
            query,
            selected,
            results,
            ..
        } => {
            crate::workspace::spawn_workspace_search(
                app.root.clone(),
                query.clone(),
                app.search_tx.clone(),
            );
            app.overlay = Overlay::SearchWorkspace {
                query,
                results,
                selected,
                searching: true,
            };
        }
        _ => {}
    }
}

fn overlay_move(app: &mut App, movement: Movement) {
    let down = matches!(movement, Movement::Down | Movement::Right);
    let up = matches!(movement, Movement::Up | Movement::Left);
    match &mut app.overlay {
        Overlay::FileFinder { selected, .. } | Overlay::SearchWorkspace { selected, .. } => {
            if down {
                *selected = selected.saturating_add(1);
            }
            if up {
                *selected = selected.saturating_sub(1);
            }
        }
        Overlay::References { items, selected } => {
            if down {
                *selected = (*selected + 1).min(items.len().saturating_sub(1));
            }
            if up {
                *selected = selected.saturating_sub(1);
            }
        }
        Overlay::ContextMenu {
            items, selected, ..
        } => {
            if down {
                *selected = (*selected + 1).min(items.len().saturating_sub(1));
            }
            if up {
                *selected = selected.saturating_sub(1);
            }
        }
        Overlay::Themes { selected } => {
            let last = crate::ui::theme::theme_names().len().saturating_sub(1);
            if down {
                *selected = (*selected + 1).min(last);
            }
            if up {
                *selected = selected.saturating_sub(1);
            }
        }
        Overlay::SearchFile {
            matches, current, ..
        } => {
            if matches.is_empty() {
                return;
            }
            if down {
                *current = (*current + 1) % matches.len();
            }
            if up {
                *current = current.checked_sub(1).unwrap_or(matches.len() - 1);
            }
            jump_search(app);
        }
        _ => {}
    }
    if let Overlay::FileFinder {
        results, selected, ..
    } = &mut app.overlay
    {
        if !results.is_empty() {
            *selected = (*selected).min(results.len() - 1);
        }
    }
    if let Overlay::SearchWorkspace {
        results, selected, ..
    } = &mut app.overlay
    {
        if !results.is_empty() {
            *selected = (*selected).min(results.len() - 1);
        }
    }
}

pub fn overlay_submit(app: &mut App) {
    match app.overlay.clone() {
        Overlay::FileFinder {
            results, selected, ..
        } => {
            if let Some(path) = results.get(selected) {
                app.open_path(app.root.join(path));
            }
            app.overlay = Overlay::None;
        }
        Overlay::GoToLine { input } => {
            if let Ok(line) = input.parse::<usize>() {
                let Some(id) = app.active else { return };
                if let (Some(doc), Some(view)) = (app.documents.get(&id), app.views.get_mut(&id)) {
                    set_cursor(
                        view,
                        &doc.rope,
                        Position::new(line.saturating_sub(1), 0),
                        false,
                    );
                }
            }
            app.overlay = Overlay::None;
        }
        Overlay::SearchWorkspace {
            results, selected, ..
        } => {
            if let Some(item) = results.get(selected).cloned() {
                app.open_path(item.path);
                let Some(id) = app.active else { return };
                if let (Some(doc), Some(view)) = (app.documents.get(&id), app.views.get_mut(&id)) {
                    set_cursor(
                        view,
                        &doc.rope,
                        Position::new(item.line, item.column),
                        false,
                    );
                }
            }
            app.overlay = Overlay::None;
        }
        Overlay::References { items, selected } => {
            if let Some(item) = items.get(selected).cloned() {
                app.open_path(item.path);
                let Some(id) = app.active else { return };
                if let (Some(doc), Some(view)) = (app.documents.get(&id), app.views.get_mut(&id)) {
                    let pos = decode_diagnostic_pos(doc, item.position);
                    set_cursor(view, &doc.rope, pos, false);
                }
            }
            app.overlay = Overlay::None;
        }
        Overlay::Confirm { .. } => confirm_yes(app),
        Overlay::Prompt { .. } => submit_prompt(app),
        Overlay::Completion { .. } => accept_completion(app),
        Overlay::ContextMenu {
            items, selected, ..
        } => {
            if let Some(item) = items.get(selected).cloned() {
                app.overlay = Overlay::None;
                run_menu(app, item.action);
            }
        }
        Overlay::Themes { selected } => {
            if let Some(name) = crate::ui::theme::theme_names().get(selected) {
                app.set_theme(name);
            }
            app.overlay = Overlay::None;
        }
        Overlay::SearchFile { .. } | Overlay::Help => {}
        _ => app.overlay = Overlay::None,
    }
}

pub fn open_help(app: &mut App) {
    app.overlay = if matches!(app.overlay, Overlay::Help) {
        Overlay::None
    } else {
        Overlay::Help
    };
}

pub fn open_themes(app: &mut App) {
    let names = crate::ui::theme::theme_names();
    let selected = names
        .iter()
        .position(|name| *name == app.theme().name)
        .unwrap_or(0);
    app.overlay = Overlay::Themes { selected };
}

pub fn click_themes(app: &mut App, y: u16) {
    let Some(index) = app
        .overlay_rows
        .iter()
        .find(|(_, row)| *row == y)
        .map(|row| row.0)
    else {
        return;
    };
    if let Some(name) = crate::ui::theme::theme_names().get(index) {
        app.set_theme(name);
    }
    app.overlay = Overlay::None;
}

pub fn dismiss_overlay(app: &mut App) {
    if !matches!(app.overlay, Overlay::None) {
        app.overlay = Overlay::None;
    }
}

fn dismiss_soft_overlays(app: &mut App) {
    if matches!(
        app.overlay,
        Overlay::Completion { .. } | Overlay::Hover { .. } | Overlay::Signature { .. }
    ) {
        app.overlay = Overlay::None;
    }
}

fn jump_search(app: &mut App) {
    let Overlay::SearchFile {
        matches, current, ..
    } = app.overlay.clone()
    else {
        return;
    };
    let Some(&(start, end)) = matches.get(current) else {
        return;
    };
    let Some(id) = app.active else { return };
    if let (Some(doc), Some(view)) = (app.documents.get(&id), app.views.get_mut(&id)) {
        view.cursor = end;
        view.selection = Some(Selection {
            anchor: start,
            cursor: end,
        });
        view.ensure_visible(&doc.rope, 20, 80, app.config.tab_width);
    }
    reveal_cursor(app, id);
}

pub fn explorer_activate(app: &mut App) {
    let Some(entry) = app.explorer_entries.get(app.explorer_selected).cloned() else {
        return;
    };
    if entry.is_dir {
        if app.expanded.contains(&entry.path) {
            app.expanded.remove(&entry.path);
        } else {
            app.expanded.insert(entry.path);
        }
        app.refresh_explorer();
    } else {
        app.open_path(entry.path);
    }
}

pub fn editor_context_labels() -> [&'static str; 2] {
    ["Go to Definition", "Find References"]
}

pub fn show_editor_menu(app: &mut App, x: u16, y: u16) {
    let lsp_ok = matches!(app.lsp_status, LspStatus::Ready);
    let token = app.next_token();
    app.hover_token = token;
    app.overlay = Overlay::ContextMenu {
        x,
        y,
        selected: 0,
        show_type: true,
        type_text: if lsp_ok {
            "Loading type information…".into()
        } else {
            "Type information unavailable".into()
        },
        hover_token: token,
        items: vec![
            MenuItem {
                label: editor_context_labels()[0].into(),
                action: MenuAction::GoToDefinition,
                enabled: lsp_ok,
            },
            MenuItem {
                label: editor_context_labels()[1].into(),
                action: MenuAction::FindReferences,
                enabled: lsp_ok,
            },
        ],
    };
    if lsp_ok {
        if let Some((path, position)) = lsp_target(app) {
            let _ = app.lsp.tx.send(LspCmd::Hover {
                token,
                path,
                position,
            });
        }
    }
}

pub fn show_explorer_menu(app: &mut App, x: u16, y: u16) {
    app.overlay = Overlay::ContextMenu {
        x,
        y,
        selected: 0,
        show_type: false,
        type_text: String::new(),
        hover_token: 0,
        items: vec![
            MenuItem {
                label: "New File".into(),
                action: MenuAction::NewFile,
                enabled: true,
            },
            MenuItem {
                label: "New Folder".into(),
                action: MenuAction::NewFolder,
                enabled: true,
            },
            MenuItem {
                label: "Rename".into(),
                action: MenuAction::RenameFile,
                enabled: true,
            },
            MenuItem {
                label: "Delete".into(),
                action: MenuAction::DeletePath,
                enabled: true,
            },
        ],
    };
}

pub fn run_menu(app: &mut App, action: MenuAction) {
    match action {
        MenuAction::GoToDefinition => go_to_definition(app),
        MenuAction::FindReferences => find_references(app),
        MenuAction::Hover => hover(app),
        MenuAction::RenameSymbol => start_rename_symbol(app),
        MenuAction::Copy => copy(app),
        MenuAction::Cut => cut(app),
        MenuAction::Paste => paste(app),
        MenuAction::NewFile => {
            app.overlay = Overlay::Prompt {
                title: "New file".into(),
                input: String::new(),
                kind: PromptKind::CreateFile { is_dir: false },
            };
        }
        MenuAction::NewFolder => {
            app.overlay = Overlay::Prompt {
                title: "New folder".into(),
                input: String::new(),
                kind: PromptKind::CreateFile { is_dir: true },
            };
        }
        MenuAction::RenameFile => {
            let name = app
                .explorer_entries
                .get(app.explorer_selected)
                .map(|e| e.name.clone())
                .unwrap_or_default();
            app.overlay = Overlay::Prompt {
                title: "Rename".into(),
                input: name,
                kind: PromptKind::RenameFile,
            };
        }
        MenuAction::DeletePath => {
            if let Some(entry) = app.explorer_entries.get(app.explorer_selected) {
                app.overlay = Overlay::Confirm {
                    message: format!("Delete {}?", entry.name),
                    action: ConfirmAction::DeletePath(entry.path.clone()),
                };
            }
        }
    }
}

pub fn maybe_quit(app: &mut App) {
    let dirty = app.documents.values().any(|d| d.is_dirty());
    if dirty {
        app.overlay = Overlay::Confirm {
            message: "Unsaved changes. Quit anyway?".into(),
            action: ConfirmAction::Quit,
        };
    } else {
        app.should_quit = true;
    }
}

fn utf16(doc: &crate::editor::Document, pos: Position) -> Position {
    Position::new(
        pos.line,
        crate::editor::position_to_utf16(&doc.rope, pos) as usize,
    )
}

pub fn handle_watch(app: &mut App, path: PathBuf) {
    app.refresh_git_status();
    let Some((id, dirty, known_mtime)) = app.documents.iter().find_map(|(id, doc)| {
        if doc.path.as_ref() == Some(&path) {
            Some((*id, doc.is_dirty(), doc.mtime))
        } else {
            None
        }
    }) else {
        app.refresh_explorer();
        return;
    };
    if let Ok(meta) = std::fs::metadata(&path) {
        if let (Ok(modified), Some(known)) = (meta.modified(), known_mtime) {
            if modified <= known + std::time::Duration::from_millis(750) {
                return;
            }
        }
    }
    if dirty {
        app.message = Some(format!("file changed on disk: {}", path.display()));
    } else {
        app.overlay = Overlay::Confirm {
            message: format!("Reload {}?", path.display()),
            action: ConfirmAction::Reload(id),
        };
    }
}

pub fn click_editor(app: &mut App, x: u16, y: u16, selecting: bool) {
    let Some(id) = app.active else { return };
    if gutter_or_fold_click(app, x, y) {
        app.toggle_hunk_at_line(id, visual_target_line(app, x, y).unwrap_or(0));
        return;
    }
    let Some(pos) = editor_position_at(app, x, y) else {
        return;
    };
    let eh = app.editor_area.3;
    if let (Some(doc), Some(view)) = (app.documents.get(&id), app.views.get_mut(&id)) {
        set_cursor(view, &doc.rope, pos, selecting);
        view.ensure_visible(
            &doc.rope,
            eh as usize,
            app.editor_area.2.saturating_sub(app.gutter_width) as usize,
            app.config.tab_width,
        );
    }
    reveal_cursor(app, id);
}

fn gutter_or_fold_click(app: &App, x: u16, y: u16) -> bool {
    let Some(id) = app.active else { return false };
    let (ex, ey, _ew, eh) = app.editor_area;
    if y <= ey || x <= ex {
        return false;
    }
    let rel_x = x.saturating_sub(ex).saturating_sub(1);
    let rel_y = y.saturating_sub(ey).saturating_sub(1);
    if rel_y >= eh.saturating_sub(2) {
        return false;
    }
    let rows = app.visual_rows(id);
    let visual = app
        .views
        .get(&id)
        .map(|view| view.viewport.top)
        .unwrap_or(0)
        + rel_y as usize;
    let folded = matches!(
        rows.get(visual),
        Some(crate::workspace::VisualRow::Fold { .. })
    );
    folded || rel_x + 1 == app.gutter_width
}

fn visual_target_line(app: &App, _x: u16, y: u16) -> Option<usize> {
    let id = app.active?;
    let (_, ey, _, eh) = app.editor_area;
    let rel_y = y.saturating_sub(ey).saturating_sub(1);
    if rel_y >= eh.saturating_sub(2) {
        return None;
    }
    let visual = app.views.get(&id)?.viewport.top + rel_y as usize;
    crate::workspace::buffer_line_at(&app.visual_rows(id), visual)
}

pub fn editor_position_at(app: &App, x: u16, y: u16) -> Option<Position> {
    let id = app.active?;
    let doc = app.documents.get(&id)?;
    let view = app.views.get(&id)?;
    let (ex, ey, _ew, eh) = app.editor_area;
    if y <= ey || x <= ex {
        return None;
    }
    let rel_x = x.saturating_sub(ex).saturating_sub(1);
    let rel_y = y.saturating_sub(ey).saturating_sub(1);
    if rel_y >= eh.saturating_sub(2) || rel_x < app.gutter_width {
        return None;
    }
    let visual = view.viewport.top + rel_y as usize;
    let line = match app.visual_rows(id).get(visual).copied() {
        Some(crate::workspace::VisualRow::Line(line)) => line,
        _ => return None,
    };
    if line >= doc.rope.len_lines() {
        return None;
    }
    let display = view.viewport.left + (rel_x - app.gutter_width) as usize;
    Some(crate::editor::position_at_display(
        &doc.rope,
        line,
        display,
        app.config.tab_width,
    ))
}

pub fn double_click_editor(app: &mut App, x: u16, y: u16) {
    click_editor(app, x, y, false);
    let Some(id) = app.active else { return };
    if let (Some(doc), Some(view)) = (app.documents.get(&id), app.views.get_mut(&id)) {
        let pos = view.cursor;
        select_word_at(view, &doc.rope, pos);
    }
}

pub fn reveal_cursor(app: &mut App, id: crate::editor::DocumentId) {
    let line = app.views.get(&id).map(|view| view.cursor.line).unwrap_or(0);
    app.unfold_at_cursor(id, line);
    let rows = app.visual_rows(id);
    let index = crate::workspace::visual_index_for_line(&rows, line).unwrap_or(0);
    let height = app.editor_area.3.saturating_sub(2).max(1) as usize;
    if let Some(view) = app.views.get_mut(&id) {
        if index < view.viewport.top {
            view.viewport.top = index;
        } else if index >= view.viewport.top + height {
            view.viewport.top = index + 1 - height;
        }
    }
}

pub fn scroll_editor(app: &mut App, delta: i32) {
    if let Some(diff) = app.diff_view.as_mut() {
        diff.scroll = (diff.scroll as i32 + delta)
            .clamp(0, diff.lines.len().saturating_sub(1) as i32) as usize;
        return;
    }
    let Some(id) = app.active else { return };
    let max = app.visual_rows(id).len().saturating_sub(1);
    if let Some(view) = app.views.get_mut(&id) {
        let next = view.viewport.top as i32 + delta;
        view.viewport.top = next.clamp(0, max as i32) as usize;
    }
}

pub fn click_explorer(app: &mut App, y: u16) {
    let (_x, ey, _w, _h) = app.explorer_area;
    if y < ey {
        return;
    }
    let idx = y.saturating_sub(ey).saturating_sub(1) as usize;
    if idx < app.explorer_entries.len() {
        app.explorer_selected = idx;
        app.focus = Focus::Explorer;
        explorer_activate(app);
    }
}

#[cfg(test)]
mod tests {
    use super::editor_context_labels;

    #[test]
    fn editor_menu_keeps_navigation_actions_only() {
        assert_eq!(
            editor_context_labels(),
            ["Go to Definition", "Find References"]
        );
    }
}
