use std::path::PathBuf;

use ropey::Rope;

use crate::agent::{AgentEvent, AgentRun, Anchor, LineKind, PromptStyle, TranscriptLine};
use crate::editor::{
    DocumentChange, DocumentId, EditorView, HScrollBar, Movement, Position, Range, Selection,
    clamp_horizontal_left, delete_backward, delete_forward, hscroll_left_at, indent_for_newline,
    insert_text, line_text, max_line_display_width, move_cursor, select_word_at, selected_text,
    set_cursor, word_bounds,
};
use crate::language::{
    CompletionItem, LspCmd, LspStatus, completion_prefix, filter_completions, strip_snippets,
    utf16_position_to_doc,
};
use crate::workspace::{WatchEvent, create_path, delete_path, find_in_document, rename_path};

use super::state::{
    App, ConfirmAction, Focus, HScrollHit, MenuAction, MenuItem, Overlay, PromptKind, SearchKind,
    clamp_explorer_scroll, decode_diagnostic_pos, explorer_index_at, explorer_inner_rows,
    is_ident_char, max_explorer_scroll,
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
    if app.markdown_preview_open() {
        app.markdown_preview = None;
    }
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
    if app.markdown_preview_open() {
        match movement {
            Movement::Up | Movement::Left => app.scroll_markdown_preview(-1),
            Movement::Down | Movement::Right => app.scroll_markdown_preview(1),
            Movement::PageUp => app.scroll_markdown_preview(-10),
            Movement::PageDown => app.scroll_markdown_preview(10),
            Movement::DocumentStart | Movement::LineStart => app.jump_markdown_preview(false),
            Movement::DocumentEnd | Movement::LineEnd => app.jump_markdown_preview(true),
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
    if app.markdown_preview_open() {
        app.markdown_preview = None;
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
    if app.markdown_preview_open() {
        app.markdown_preview = None;
    }
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

fn copy_path(app: &mut App) {
    let Some(path) = app
        .explorer_entries
        .get(app.explorer_selected)
        .map(|entry| entry.path.display().to_string())
    else {
        return;
    };
    app.clipboard.copy(&path);
    app.message = Some("path copied".into());
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

pub fn open_search(app: &mut App) {
    app.workspace_files = crate::workspace::list_workspace_files(&app.root);
    app.file_finder.set_files(&app.workspace_files);
    let files = app.file_finder.query("");
    app.overlay = Overlay::Search {
        kind: SearchKind::Find,
        query: String::new(),
        files,
        matches: Vec::new(),
        selected: 0,
        searching: false,
    };
}

pub fn open_content_search(app: &mut App) {
    app.overlay = Overlay::Search {
        kind: SearchKind::Content,
        query: String::new(),
        files: Vec::new(),
        matches: Vec::new(),
        selected: 0,
        searching: false,
    };
}

pub fn open_search_file(app: &mut App) {
    app.overlay = Overlay::SearchFile {
        query: String::new(),
        matches: Vec::new(),
        current: 0,
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
    let (range, path) = {
        let Some(doc) = app.documents.get(&document_id) else {
            return;
        };
        (word_bounds(&doc.rope, position), doc.path.clone())
    };
    let diagnostic_anchor = path.as_ref().and_then(|path| {
        app.diagnostics.get(path).and_then(|items| {
            crate::language::diagnostic_at(items, position).map(|item| item.range_start)
        })
    });
    if range.is_empty() && diagnostic_anchor.is_none() {
        cancel_hover(app);
        return;
    }
    let symbol_position = diagnostic_anchor.unwrap_or(range.start);
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

fn wrap_plain(text: &str, width: usize) -> String {
    let width = width.max(8);
    let mut lines = Vec::new();
    for raw in text.split('\n') {
        let paragraph = raw.trim_end_matches('\r');
        if paragraph.trim().is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            if current.is_empty() {
                take_word(&mut lines, &mut current, word, width);
            } else if current.chars().count() + 1 + word.chars().count() <= width {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(std::mem::take(&mut current));
                take_word(&mut lines, &mut current, word, width);
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }
    lines.join("\n")
}

fn take_word(lines: &mut Vec<String>, current: &mut String, word: &str, width: usize) {
    if word.chars().count() <= width {
        *current = word.to_string();
        return;
    }
    let mut rest = word;
    while rest.chars().count() > width {
        let split = rest
            .char_indices()
            .nth(width)
            .map(|(index, _)| index)
            .unwrap_or(rest.len());
        let (head, tail) = rest.split_at(split);
        lines.push(head.to_string());
        rest = tail;
    }
    *current = rest.to_string();
}

pub fn poll_hover(app: &mut App) {
    let Some(dwell) = app.hover_dwell.clone() else {
        return;
    };
    if dwell.since.elapsed() < super::state::HOVER_DWELL {
        return;
    }
    let Some(path) = app
        .documents
        .get(&dwell.document_id)
        .and_then(|doc| doc.path.clone())
    else {
        app.hover_dwell = None;
        return;
    };
    if let Some(diagnostic) = app
        .diagnostics
        .get(&path)
        .and_then(|items| crate::language::diagnostic_at(items, dwell.position))
        .cloned()
    {
        app.hover_dwell = None;
        app.hover_anchor = None;
        app.overlay = Overlay::Hover {
            title: crate::language::diagnostic_label(diagnostic.severity).into(),
            text: wrap_plain(diagnostic.message.trim(), 64),
            origin: dwell.position,
            x: dwell.screen_x,
            y: dwell.screen_y,
        };
        return;
    }
    let Some(doc) = app.documents.get(&dwell.document_id) else {
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
        ConfirmAction::CloseMany(ids) => app.close_documents(&ids, true),
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
        ConfirmAction::DiscardHunk { patch, path } => app.discard_hunk(patch, path),
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
        PromptKind::AgentEdit => {
            let message = input.trim().to_string();
            if message.is_empty() {
                return;
            }
            if app.thread.prior.is_empty() {
                app.thread.style = PromptStyle::Edit;
            }
            app.thread.anchor = capture_anchor(app);
            app.thread_visible = true;
            app.focus = Focus::Thread;
            send_message(app, message);
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
        Overlay::Search { query, .. }
        | Overlay::SearchFile { query, .. }
        | Overlay::Prompt { input: query, .. } => {
            query.push(ch);
        }
        _ => {}
    }
    refresh_overlay_query(app);
}

fn overlay_backspace(app: &mut App) -> bool {
    let handled = match &mut app.overlay {
        Overlay::Search { query, .. }
        | Overlay::SearchFile { query, .. }
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
        Overlay::Search {
            kind,
            query,
            selected,
            matches,
            ..
        } => {
            let files = if kind == SearchKind::Find {
                app.file_finder.query(&query)
            } else {
                Vec::new()
            };
            let searching = !query.trim().is_empty();
            let matches = if searching { matches } else { Vec::new() };
            app.search_generation = app.search_generation.wrapping_add(1);
            if searching {
                crate::workspace::spawn_workspace_search(
                    app.root.clone(),
                    query.clone(),
                    app.search_generation,
                    app.search_tx.clone(),
                );
            }
            app.overlay = Overlay::Search {
                kind,
                query,
                selected: clamp_search_selected(selected, files.len(), matches.len()),
                files,
                matches,
                searching,
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
        _ => {}
    }
}

fn clamp_search_selected(selected: usize, files: usize, matches: usize) -> usize {
    let total = files + matches;
    if total == 0 {
        0
    } else {
        selected.min(total - 1)
    }
}

fn overlay_move(app: &mut App, movement: Movement) {
    let down = matches!(movement, Movement::Down | Movement::Right);
    let up = matches!(movement, Movement::Up | Movement::Left);
    match &mut app.overlay {
        Overlay::Search { selected, .. } => {
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
    if let Overlay::Search {
        files,
        matches,
        selected,
        ..
    } = &mut app.overlay
    {
        let total = files.len() + matches.len();
        if total > 0 {
            *selected = (*selected).min(total - 1);
        }
    }
}

pub fn overlay_submit(app: &mut App) {
    match app.overlay.clone() {
        Overlay::Search {
            files,
            matches,
            selected,
            ..
        } => {
            if selected < files.len() {
                if let Some(path) = files.get(selected) {
                    app.open_path(app.root.join(path));
                }
            } else if let Some(item) = matches.get(selected - files.len()).cloned() {
                app.open_path(item.path);
                if let Some(id) = app.active {
                    jump_to(app, id, Position::new(item.line, item.column));
                }
            }
            app.overlay = Overlay::None;
        }
        Overlay::References { items, selected } => {
            if let Some(item) = items.get(selected).cloned() {
                app.open_path(item.path);
                let Some(id) = app.active else { return };
                if let Some(pos) = app
                    .documents
                    .get(&id)
                    .map(|doc| decode_diagnostic_pos(doc, item.position))
                {
                    jump_to(app, id, pos);
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
        Overlay::TypesNote => dismiss_types_note(app),
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

pub fn click_help(app: &mut App, y: u16) {
    if app.overlay_rows.iter().any(|(_, row)| *row == y) {
        toggle_claude(app);
    }
}

pub fn toggle_claude(app: &mut App) {
    app.config.claude_enabled = !app.config.claude_enabled;
    if !app.config.claude_enabled {
        if app.thread.running {
            stop_agent(app);
        }
        app.thread_visible = false;
        if app.focus == Focus::Thread {
            app.focus = Focus::Editor;
        }
    }
    let state = if app.config.claude_enabled {
        "on"
    } else {
        "off"
    };
    app.message = if let Err(err) = app.config.save() {
        Some(format!("Claude {state} (not saved: {err})"))
    } else {
        Some(format!("Claude {state}"))
    };
}

pub fn claude_ready(app: &mut App) -> bool {
    if !app.config.claude_enabled {
        app.message = Some("Claude is off. Turn it on in Settings".into());
        return false;
    }
    if !app.claude_installed {
        app.message = Some("claude is not installed".into());
        return false;
    }
    true
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

pub fn click_types_status(app: &mut App) {
    if which::which("typescript-language-server").is_ok() {
        refresh_types(app);
    } else {
        app.overlay = Overlay::TypesNote;
    }
}

pub fn refresh_types(app: &mut App) {
    app.lsp_status = LspStatus::Starting;
    let _ = app.lsp.tx.send(LspCmd::Restart {
        root: app.root.clone(),
    });
}

pub const TYPES_INSTALL: &str = "npm install -g typescript typescript-language-server";

pub fn click_types_note(app: &mut App, x: u16, y: u16) {
    let Some((hit_x, hit_y, width)) = app.types_copy else {
        return;
    };
    if y == hit_y && x >= hit_x && x < hit_x.saturating_add(width) {
        app.clipboard.copy(TYPES_INSTALL);
        app.message = Some("Copied".into());
    }
}

pub fn dismiss_types_note(app: &mut App) {
    app.overlay = Overlay::None;
    if app.config.types_note_dismissed {
        return;
    }
    app.config.types_note_dismissed = true;
    if let Err(err) = app.config.save() {
        app.message = Some(format!("note not saved: {err}"));
    }
}

pub fn dismiss_overlay(app: &mut App) {
    if matches!(app.overlay, Overlay::TypesNote) {
        dismiss_types_note(app);
        return;
    }
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

pub fn jump_to(app: &mut App, id: crate::editor::DocumentId, pos: Position) {
    place_and_reveal(app, id, |view, rope| set_cursor(view, rope, pos, false));
}

fn place_and_reveal(
    app: &mut App,
    id: crate::editor::DocumentId,
    place: impl FnOnce(&mut EditorView, &Rope),
) {
    let height = app.editor_area.3.saturating_sub(2).max(1) as usize;
    let width = app.editor_area.2.saturating_sub(app.gutter_width) as usize;
    let tab_width = app.config.tab_width;
    if let (Some(doc), Some(view)) = (app.documents.get(&id), app.views.get_mut(&id)) {
        place(view, &doc.rope);
        view.ensure_visible(&doc.rope, height, width, tab_width);
    }
    reveal_cursor(app, id);
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
    place_and_reveal(app, id, |view, _rope| {
        view.cursor = end;
        view.selection = Some(Selection {
            anchor: start,
            cursor: end,
        });
    });
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

pub fn editor_context_labels() -> [&'static str; 3] {
    ["Go to Definition", "Find References", "Ask Claude"]
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
            MenuItem {
                label: editor_context_labels()[2].into(),
                action: MenuAction::AskClaude,
                enabled: app.config.claude_enabled && app.claude_installed,
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

pub fn show_tab_menu(app: &mut App, document_id: crate::editor::DocumentId, x: u16, y: u16) {
    let others_open = app.order.len() > 1;
    app.overlay = Overlay::ContextMenu {
        x,
        y,
        selected: 0,
        show_type: false,
        type_text: String::new(),
        hover_token: 0,
        items: vec![
            MenuItem {
                label: "Close Others".into(),
                action: MenuAction::CloseOthers(document_id),
                enabled: others_open,
            },
            MenuItem {
                label: "Close All".into(),
                action: MenuAction::CloseAll,
                enabled: true,
            },
        ],
    };
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
                label: "Copy Path".into(),
                action: MenuAction::CopyPath,
                enabled: app.explorer_entries.get(app.explorer_selected).is_some(),
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
        MenuAction::CopyPath => copy_path(app),
        MenuAction::CloseOthers(id) => {
            let ids: Vec<_> = app
                .order
                .iter()
                .copied()
                .filter(|open| *open != id)
                .collect();
            app.close_documents(&ids, false);
        }
        MenuAction::CloseAll => {
            let ids = app.order.clone();
            app.close_documents(&ids, false);
        }
        MenuAction::AskClaude => start_thread(app),
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

pub fn handle_watch(app: &mut App, event: WatchEvent) {
    app.schedule_git_status();
    if event.kind.changes_tree() {
        app.refresh_explorer();
    }
    let path = event.path;
    let Some((id, dirty, known_mtime)) = app.documents.iter().find_map(|(id, doc)| {
        if doc.path.as_ref() == Some(&path) {
            Some((*id, doc.is_dirty(), doc.mtime))
        } else {
            None
        }
    }) else {
        return;
    };
    if let Ok(meta) = std::fs::metadata(&path) {
        if let (Ok(modified), Some(known)) = (meta.modified(), known_mtime) {
            if modified <= known + std::time::Duration::from_millis(750) {
                return;
            }
        }
    }
    if app.thread.running {
        return;
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
    if app.markdown_preview_open() {
        return;
    }
    if !selecting && in_revert_column(app, x, y) && shows_revert(app, x, y) {
        if let Some(line) = visual_target_line(app, x, y) {
            app.revert_hunk_at_line(id, line);
            let height = app.editor_area.3.saturating_sub(2).max(1) as usize;
            let width = app.editor_area.2.saturating_sub(app.gutter_width) as usize;
            let tab_width = app.config.tab_width;
            if let (Some(doc), Some(view)) = (app.documents.get(&id), app.views.get_mut(&id)) {
                view.ensure_visible(&doc.rope, height, width, tab_width);
            }
            reveal_cursor(app, id);
        }
        return;
    }
    let removed = removed_row_click(app, x, y);
    if removed {
        if !selecting {
            if let Some(line) = visual_target_line(app, x, y) {
                app.toggle_hunk_at_line(id, line);
            }
        }
        return;
    }
    if !selecting && gutter_click(app, x, y) {
        if let Some(line) = visual_target_line(app, x, y) {
            app.toggle_hunk_at_line(id, line);
        }
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

fn removed_row_click(app: &App, x: u16, y: u16) -> bool {
    let Some(row) = visual_row_at(app, x, y) else {
        return false;
    };
    matches!(row, crate::workspace::VisualRow::Removed { .. })
}

fn in_revert_column(app: &App, x: u16, y: u16) -> bool {
    editor_rel_x(app, x, y) == Some(0)
}

fn shows_revert(app: &App, x: u16, y: u16) -> bool {
    let Some(id) = app.active else {
        return false;
    };
    let Some(row) = visual_row_at(app, x, y) else {
        return false;
    };
    let hunks = app
        .inline_changes
        .get(&id)
        .map(crate::workspace::change_hunks)
        .unwrap_or_default();
    match row {
        crate::workspace::VisualRow::Line(line) => hunks.iter().any(|hunk| hunk.start == line),
        crate::workspace::VisualRow::Eof { .. } => true,
        crate::workspace::VisualRow::Removed { .. } => false,
    }
}

fn gutter_click(app: &App, x: u16, y: u16) -> bool {
    let Some(rel_x) = editor_rel_x(app, x, y) else {
        return false;
    };
    rel_x + 1 == app.gutter_width
}

fn editor_rel_x(app: &App, x: u16, y: u16) -> Option<u16> {
    let (ex, ey, _ew, eh) = app.editor_area;
    if y <= ey || x <= ex {
        return None;
    }
    let rel_y = y.saturating_sub(ey).saturating_sub(1);
    if rel_y >= eh.saturating_sub(2) {
        return None;
    }
    Some(x.saturating_sub(ex).saturating_sub(1))
}

fn visual_row_at(app: &App, x: u16, y: u16) -> Option<crate::workspace::VisualRow> {
    let id = app.active?;
    let (ex, ey, _ew, eh) = app.editor_area;
    if y <= ey || x <= ex {
        return None;
    }
    let rel_y = y.saturating_sub(ey).saturating_sub(1);
    if rel_y >= eh.saturating_sub(2) {
        return None;
    }
    let visual = app.views.get(&id)?.viewport.top + rel_y as usize;
    app.visual_rows(id).get(visual).cloned()
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
    let line = match app.visual_rows(id).get(visual) {
        Some(crate::workspace::VisualRow::Line(line)) => *line,
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
    if app.markdown_preview_open() {
        app.scroll_markdown_preview(delta);
        return;
    }
    let Some(id) = app.active else { return };
    let max = app.visual_rows(id).len().saturating_sub(1);
    if let Some(view) = app.views.get_mut(&id) {
        let next = view.viewport.top as i32 + delta;
        view.viewport.top = next.clamp(0, max as i32) as usize;
    }
}

pub fn scroll_explorer(app: &mut App, delta: i32) {
    if app.git_open {
        let len = crate::workspace::git_panel_rows(&app.git_status).len();
        let inner = explorer_inner_rows(app.explorer_area.3);
        let max = max_explorer_scroll(len, inner);
        let next = app.git_scroll as i64 + i64::from(delta);
        app.git_scroll = next.clamp(0, max as i64) as usize;
        return;
    }
    let inner = explorer_inner_rows(app.explorer_area.3);
    let max = max_explorer_scroll(app.explorer_entries.len(), inner);
    let next = app.explorer_scroll as i64 + i64::from(delta);
    app.explorer_scroll = next.clamp(0, max as i64) as usize;
}

pub fn reveal_explorer_selection(app: &mut App) {
    let inner = explorer_inner_rows(app.explorer_area.3);
    if inner == 0 || app.explorer_entries.is_empty() {
        return;
    }
    let sel = app
        .explorer_selected
        .min(app.explorer_entries.len().saturating_sub(1));
    if sel < app.explorer_scroll {
        app.explorer_scroll = sel;
    } else if sel >= app.explorer_scroll + inner {
        app.explorer_scroll = sel + 1 - inner;
    }
    app.explorer_scroll =
        clamp_explorer_scroll(app.explorer_scroll, app.explorer_entries.len(), inner);
}

pub fn scroll_editor_horizontal(app: &mut App, delta: i32) {
    if app.markdown_preview_open() {
        return;
    }
    let Some(id) = app.active else { return };
    let current = app
        .views
        .get(&id)
        .map(|view| view.viewport.left)
        .unwrap_or(0);
    assign_horizontal(app, current as i32 + delta);
}

pub fn begin_explorer_resize(app: &mut App, x: u16, y: u16) -> bool {
    if !app.explorer_visible || app.explorer_area.2 == 0 {
        return false;
    }
    let edge = app.explorer_area.0 + app.explorer_area.2 - 1;
    let top = app.search_area.1.min(app.explorer_area.1);
    let bottom = app.explorer_area.1.saturating_add(app.explorer_area.3);
    if x != edge || y < top || y >= bottom {
        return false;
    }
    app.explorer_resizing = true;
    true
}

pub fn drag_explorer_resize(app: &mut App, x: u16) {
    if !app.explorer_resizing {
        return;
    }
    app.config.explorer_width = crate::ui::layout::stored_explorer_width(
        app.status_area.2,
        app.thread_visible,
        app.explorer_area.0,
        x,
    );
}

pub fn end_explorer_resize(app: &mut App) {
    if !app.explorer_resizing {
        return;
    }
    app.explorer_resizing = false;
    if app.config.explorer_width < crate::app::config::EXPLORER_MIN_WIDTH {
        app.config.explorer_width = crate::app::config::EXPLORER_MIN_WIDTH;
    }
    if let Err(err) = app.config.save() {
        app.message = Some(format!("explorer width not saved: {err}"));
    }
}

pub fn begin_hscroll_drag(app: &mut App, x: u16, y: u16) -> bool {
    let Some(hit) = app.hscroll_hit else {
        return false;
    };
    if y != hit.y || x < hit.x || x >= hit.x.saturating_add(hit.width) {
        return false;
    }
    let on_thumb = x >= hit.thumb_x && x < hit.thumb_x.saturating_add(hit.thumb_w);
    let grab = if on_thumb {
        x.saturating_sub(hit.thumb_x)
    } else {
        hit.thumb_w / 2
    };
    let left = hscroll_left_at(bar_from_hit(hit), x, grab);
    app.hscroll_grab = grab;
    app.hscroll_dragging = true;
    assign_horizontal(app, left as i32);
    true
}

pub fn drag_hscroll(app: &mut App, x: u16) {
    let Some(hit) = app.hscroll_hit else {
        return;
    };
    let left = hscroll_left_at(bar_from_hit(hit), x, app.hscroll_grab);
    assign_horizontal(app, left as i32);
}

fn assign_horizontal(app: &mut App, left: i32) {
    if app.diff_view.is_some() {
        return;
    }
    let Some(id) = app.active else { return };
    let width = app.editor_area.2.saturating_sub(2 + app.gutter_width) as usize;
    let tab_width = app.config.tab_width;
    let content = app
        .documents
        .get(&id)
        .map(|doc| max_line_display_width(&doc.rope, tab_width))
        .unwrap_or(0);
    if let Some(view) = app.views.get_mut(&id) {
        view.viewport.left = clamp_horizontal_left(left, content, width);
    }
}

fn bar_from_hit(hit: HScrollHit) -> HScrollBar {
    HScrollBar {
        track_x: hit.x,
        track_w: hit.width,
        thumb_x: hit.thumb_x,
        thumb_w: hit.thumb_w,
        max_left: hit.max_left,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelAxis {
    Vertical(i32),
    Horizontal(i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelAction {
    Explorer(i32),
    EditorVertical(i32),
    EditorHorizontal(i32),
    Thread(i32),
    None,
}

pub fn wheel_action(
    in_popup: bool,
    in_explorer: bool,
    in_editor: bool,
    in_thread: bool,
    shift: bool,
    axis: WheelAxis,
) -> WheelAction {
    if in_popup {
        return WheelAction::None;
    }
    match axis {
        WheelAxis::Horizontal(delta) => {
            if in_editor {
                WheelAction::EditorHorizontal(delta)
            } else {
                WheelAction::None
            }
        }
        WheelAxis::Vertical(delta) if shift => {
            if in_editor {
                WheelAction::EditorHorizontal(if delta > 0 { 4 } else { -4 })
            } else {
                WheelAction::None
            }
        }
        WheelAxis::Vertical(delta) if in_thread => WheelAction::Thread(delta),
        WheelAxis::Vertical(delta) if in_explorer => WheelAction::Explorer(delta),
        WheelAxis::Vertical(delta) if in_editor => WheelAction::EditorVertical(delta),
        WheelAxis::Vertical(_) => WheelAction::None,
    }
}

pub fn start_thread(app: &mut App) {
    if !claude_ready(app) {
        return;
    }
    if app.thread.running {
        app.message = Some("Claude is still working".into());
        return;
    }
    app.thread.anchor = capture_anchor(app);
    if app.thread.prior.is_empty() {
        app.thread.style = PromptStyle::Chat;
    }
    app.thread_visible = true;
    app.focus = Focus::Thread;
    app.overlay = Overlay::None;
}

pub fn prompt_agent_edit(app: &mut App) {
    if !claude_ready(app) {
        return;
    }
    if app.thread.running {
        app.message = Some("Claude is still working".into());
        return;
    }
    app.overlay = Overlay::Prompt {
        title: "Edit with Claude".into(),
        input: String::new(),
        kind: PromptKind::AgentEdit,
    };
}

pub fn send_thread(app: &mut App) {
    let message = app.thread.compose.trim().to_string();
    if message.is_empty() {
        return;
    }
    app.thread.compose.clear();
    send_message(app, message);
}

pub fn stop_agent(app: &mut App) {
    if let Some(run) = app.agent_run.take() {
        run.stop();
    }
}

pub fn scroll_thread(app: &mut App, delta: i32) {
    if delta > 0 {
        app.thread.scroll = app.thread.scroll.saturating_sub(delta as usize);
    } else {
        app.thread.scroll = app
            .thread
            .scroll
            .saturating_add(delta.unsigned_abs() as usize);
    }
}

fn send_message(app: &mut App, message: String) {
    if !claude_ready(app) {
        return;
    }
    if app.thread.running {
        app.message = Some("Claude is still working".into());
        return;
    }
    let replay = !app.thread.prior.is_empty() && !app.thread.live;
    let prompt = crate::agent::compose_prompt(
        app.thread.style,
        app.thread.anchor.as_ref(),
        &app.thread.prior,
        &message,
        replay,
    );
    let resume = app.thread.live;
    let session_id = app.thread.session_id.clone();
    app.thread.lines.push(TranscriptLine {
        kind: LineKind::User,
        text: message.clone(),
    });
    app.thread.pending_user = message;
    app.thread.reply_buf.clear();
    app.thread.failure = None;
    app.thread.saw_session = false;
    app.thread.running = true;
    app.thread.scroll = 0;
    app.thread_visible = true;
    app.focus = Focus::Thread;
    if let Some(run) = app.agent_run.take() {
        run.stop();
    }
    let run = AgentRun::new();
    app.agent_run = Some(run.clone());
    crate::agent::spawn_turn(
        app.root.clone(),
        prompt,
        session_id,
        resume,
        app.agent_tx.clone(),
        run,
    );
}

pub fn handle_agent(app: &mut App, event: AgentEvent) {
    app.thread.spinner = app.thread.spinner.wrapping_add(1);
    match event {
        AgentEvent::Session => app.thread.saw_session = true,
        AgentEvent::Text { text, block } => append_reply(app, &text, block),
        AgentEvent::Tool { label } => {
            app.thread.reply_buf.clear();
            app.thread.lines.push(TranscriptLine {
                kind: LineKind::Tool,
                text: label,
            });
        }
        AgentEvent::Failure { text } => app.thread.failure = Some(text),
        AgentEvent::Result { text } => {
            if app.thread.reply_buf.trim().is_empty() {
                append_reply(app, &text, true);
            }
        }
        AgentEvent::Finished { success, stderr } => finish_turn(app, success, stderr),
    }
}

fn append_reply(app: &mut App, text: &str, block: bool) {
    if block && !app.thread.reply_buf.is_empty() && !app.thread.reply_buf.ends_with("\n\n") {
        app.thread.reply_buf.push_str("\n\n");
    }
    app.thread.reply_buf.push_str(text);
    let reply = app.thread.reply_buf.clone();
    if let Some(line) = app.thread.lines.last_mut()
        && line.kind == LineKind::Reply
    {
        line.text = reply;
    } else {
        app.thread.lines.push(TranscriptLine {
            kind: LineKind::Reply,
            text: reply,
        });
    }
}

fn finish_turn(app: &mut App, success: bool, stderr: String) {
    let failure = app.thread.failure.clone();
    let saw_session = app.thread.saw_session;
    app.thread.running = false;
    app.agent_run = None;
    if success || saw_session {
        app.thread.live = true;
    }
    let failed = !success || failure.is_some();
    if failed && app.thread.reply_buf.trim().is_empty() {
        let mut text = failure.unwrap_or_default();
        let stderr = stderr.trim();
        if !stderr.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(stderr);
        }
        if text.is_empty() {
            text = "Claude stopped".into();
        }
        app.thread.lines.push(TranscriptLine {
            kind: LineKind::Error,
            text,
        });
    }
    let user = std::mem::take(&mut app.thread.pending_user);
    let reply = app.thread.reply_buf.clone();
    if !user.is_empty() {
        app.thread.prior.push((user, reply));
    }
    app.thread.reply_buf.clear();
    settle_agent_edits(app);
}

pub fn settle_agent_edits(app: &mut App) {
    let ids: Vec<DocumentId> = app.documents.keys().copied().collect();
    let mut dirty_note = None;
    for id in ids {
        match refresh_from_disk(app, id) {
            DiskRefresh::Dirty(path) => {
                dirty_note = Some(format!("file changed on disk: {}", path.display()));
            }
            DiskRefresh::Reloaded | DiskRefresh::Same | DiskRefresh::Skipped => {}
        }
    }
    if let Some(note) = dirty_note {
        app.message = Some(note);
    }
}

enum DiskRefresh {
    Same,
    Dirty(PathBuf),
    Reloaded,
    Skipped,
}

fn refresh_from_disk(app: &mut App, id: DocumentId) -> DiskRefresh {
    let Some((path, current, dirty)) = app
        .documents
        .get(&id)
        .and_then(|doc| Some((doc.path.clone()?, doc.text(), doc.is_dirty())))
    else {
        return DiskRefresh::Skipped;
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return DiskRefresh::Skipped;
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return DiskRefresh::Skipped;
    };
    if text == current {
        if let Some(doc) = app.documents.get_mut(&id) {
            doc.mtime = std::fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .ok();
        }
        return DiskRefresh::Same;
    }
    if dirty {
        return DiskRefresh::Dirty(path);
    }
    let reloaded = {
        let Some(doc) = app.documents.get_mut(&id) else {
            return DiskRefresh::Skipped;
        };
        if doc.replace_from_disk().is_err() {
            return DiskRefresh::Skipped;
        }
        (doc.language, doc.rope.clone(), doc.revision, doc.text())
    };
    let (language, rope, revision, full) = reloaded;
    app.syntax.close(id);
    app.syntax.open(id, language, &rope);
    let _ = app.lsp.tx.send(LspCmd::DidChange {
        path: path.clone(),
        version: revision as i64,
        change: full_document_change(id, revision, full.clone()),
        full_text: full,
        incremental: false,
    });
    if let Some(view) = app.views.get_mut(&id) {
        let len = rope.len_lines().max(1);
        if view.cursor.line >= len {
            view.cursor.line = len - 1;
            view.cursor.column = 0;
        }
        view.selection = None;
    }
    app.request_inline_diff(id);
    DiskRefresh::Reloaded
}

fn full_document_change(id: DocumentId, revision: u64, text: String) -> DocumentChange {
    let origin = Position::default();
    DocumentChange {
        document_id: id,
        revision,
        start_byte: 0,
        old_end_byte: 0,
        new_end_byte: text.len(),
        start_char: 0,
        old_end_char: 0,
        new_end_char: text.chars().count(),
        start_position: origin,
        old_end_position: origin,
        new_end_position: origin,
        start_row: 0,
        start_byte_column: 0,
        old_end_row: 0,
        old_end_byte_column: 0,
        new_end_row: 0,
        new_end_byte_column: 0,
        inserted_text: text,
        deleted_text: String::new(),
    }
}

fn capture_anchor(app: &App) -> Option<Anchor> {
    let doc = app.active_doc()?;
    let view = app.active_view()?;
    let path = app.relative(doc.path.as_ref()?);
    let (start, end) =
        if let Some(selection) = view.selection.filter(|selection| !selection.is_empty()) {
            let range = selection.range();
            (range.start.line, range.end.line)
        } else {
            (view.cursor.line, view.cursor.line)
        };
    let mut snippet = Vec::new();
    for line in start..=end {
        if snippet.len() >= 40 {
            break;
        }
        snippet.push(line_text(&doc.rope, line));
    }
    Some(Anchor {
        path,
        start_line: start + 1,
        end_line: end.min(start + snippet.len().saturating_sub(1)) + 1,
        snippet: snippet.join("\n"),
    })
}

pub fn commit_git(app: &mut App) {
    app.commit_git();
}

pub fn move_git(app: &mut App, delta: i32) {
    let rows = crate::workspace::git_panel_rows(&app.git_status);
    let files: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| {
            matches!(row, crate::workspace::GitPanelRow::File { .. }).then_some(index)
        })
        .collect();
    if files.is_empty() {
        return;
    }
    let pos = files
        .iter()
        .position(|index| *index == app.git_selected)
        .unwrap_or(0);
    let next = if delta < 0 {
        pos.saturating_sub(1)
    } else {
        (pos + 1).min(files.len() - 1)
    };
    app.git_selected = files[next];
    let inner = explorer_inner_rows(app.explorer_area.3);
    if app.git_selected < app.git_scroll {
        app.git_scroll = app.git_selected;
    } else if inner > 0 && app.git_selected >= app.git_scroll + inner {
        app.git_scroll = app.git_selected + 1 - inner;
    }
}

pub fn open_selected_git(app: &mut App) {
    let rows = crate::workspace::git_panel_rows(&app.git_status);
    let Some(crate::workspace::GitPanelRow::File { path, staged, .. }) = rows.get(app.git_selected)
    else {
        return;
    };
    app.open_git_entry(path.clone(), *staged);
}

pub fn toggle_selected_git(app: &mut App) {
    let rows = crate::workspace::git_panel_rows(&app.git_status);
    let Some(crate::workspace::GitPanelRow::File { path, staged, .. }) = rows.get(app.git_selected)
    else {
        return;
    };
    app.toggle_stage(path.clone(), *staged);
}

pub fn click_git(app: &mut App, x: u16, y: u16) {
    app.focus = Focus::Explorer;
    if let Some(action) = app
        .git_hits
        .iter()
        .find(|hit| y == hit.y && x >= hit.x && x < hit.x.saturating_add(hit.width))
        .map(|hit| hit.action.clone())
    {
        match action {
            crate::app::state::GitClick::Message => app.git_message_focus = true,
            crate::app::state::GitClick::Commit => app.commit_git(),
            crate::app::state::GitClick::StageAll => app.stage_all(),
            crate::app::state::GitClick::UnstageAll => app.unstage_all(),
            crate::app::state::GitClick::Toggle { path, staged } => app.toggle_stage(path, staged),
            crate::app::state::GitClick::Open { path, staged } => {
                app.git_selected = crate::workspace::git_panel_rows(&app.git_status)
                    .iter()
                    .position(|row| {
                        matches!(
                            row,
                            crate::workspace::GitPanelRow::File {
                                path: row_path,
                                staged: row_staged,
                                ..
                            } if row_path == &path && *row_staged == staged
                        )
                    })
                    .unwrap_or(app.git_selected);
                app.open_git_entry(path, staged);
            }
        }
        return;
    }
    app.git_message_focus = false;
    let top = app.explorer_area.1.saturating_add(1);
    let bottom = app
        .explorer_area
        .1
        .saturating_add(app.explorer_area.3.saturating_sub(1));
    if y >= top && y < bottom {
        let index = app.git_scroll + usize::from(y - top);
        let len = crate::workspace::git_panel_rows(&app.git_status).len();
        if index < len {
            app.git_selected = index;
        }
    }
}

pub fn receive_git_write(app: &mut App, result: crate::workspace::GitWriteResult) {
    let reload = result.reload.clone();
    let ok = result.ok;
    app.note_git_write(result);
    if ok {
        if let Some(path) = reload {
            app.reload_clean_document(&path);
        }
        let open = app.order.clone();
        for id in open {
            app.request_inline_diff(id);
        }
    }
    app.refresh_git_status();
    app.refresh_open_file_diff();
}

pub fn click_explorer(app: &mut App, y: u16) {
    let (_x, ey, _w, eh) = app.explorer_area;
    let Some(idx) = explorer_index_at(app.explorer_scroll, ey, eh, y, app.explorer_entries.len())
    else {
        return;
    };
    app.explorer_selected = idx;
    app.focus = Focus::Explorer;
    explorer_activate(app);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tokio::sync::mpsc;

    use crate::app::state::{App, ConfirmAction, Overlay};
    use crate::editor::DocumentId;
    use crate::language::LanguageClient;

    use super::{
        MenuAction, WheelAction, WheelAxis, confirm_yes, editor_context_labels, run_menu,
        wheel_action,
    };

    fn with_runtime<T>(body: impl FnOnce() -> T) -> T {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        body()
    }

    fn app_with_tabs(count: usize) -> (App, Vec<DocumentId>) {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(PathBuf::from("."), LanguageClient { tx });
        for _ in 0..count {
            app.open_untitled();
        }
        let ids = app.order.clone();
        (app, ids)
    }

    #[test]
    fn claude_stays_closed_until_it_is_enabled() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(PathBuf::from("."), LanguageClient { tx });
        app.config.claude_enabled = false;
        super::start_thread(&mut app);
        assert!(!app.thread_visible);
        assert_eq!(
            app.message.as_deref(),
            Some("Claude is off. Turn it on in Settings")
        );
    }

    #[test]
    fn editor_menu_keeps_navigation_actions_only() {
        assert_eq!(
            editor_context_labels(),
            ["Go to Definition", "Find References", "Ask Claude"]
        );
    }

    #[test]
    fn wheel_follows_the_pane_under_the_pointer() {
        assert_eq!(
            wheel_action(false, true, false, false, false, WheelAxis::Vertical(3)),
            WheelAction::Explorer(3)
        );
        assert_eq!(
            wheel_action(false, false, true, false, false, WheelAxis::Vertical(-3)),
            WheelAction::EditorVertical(-3)
        );
        assert_eq!(
            wheel_action(false, false, false, true, false, WheelAxis::Vertical(3)),
            WheelAction::Thread(3)
        );
        assert_eq!(
            wheel_action(false, false, false, false, false, WheelAxis::Vertical(3)),
            WheelAction::None
        );
        assert_eq!(
            wheel_action(true, true, true, true, false, WheelAxis::Vertical(3)),
            WheelAction::None
        );
        assert_eq!(
            wheel_action(false, true, false, false, true, WheelAxis::Vertical(3)),
            WheelAction::None
        );
        assert_eq!(
            wheel_action(false, false, true, false, true, WheelAxis::Vertical(3)),
            WheelAction::EditorHorizontal(4)
        );
        assert_eq!(
            wheel_action(false, false, true, false, false, WheelAxis::Horizontal(-4)),
            WheelAction::EditorHorizontal(-4)
        );
        assert_eq!(
            wheel_action(false, true, false, false, false, WheelAxis::Horizontal(4)),
            WheelAction::None
        );
    }

    #[test]
    fn close_others_keeps_the_clicked_tab_even_when_it_is_not_active() {
        let (mut app, ids) = app_with_tabs(3);
        app.active = Some(ids[0]);
        run_menu(&mut app, MenuAction::CloseOthers(ids[1]));
        assert_eq!(app.order, vec![ids[1]]);
        assert_eq!(app.active, Some(ids[1]));
    }

    #[test]
    fn close_all_clears_every_tab() {
        let (mut app, _) = app_with_tabs(3);
        run_menu(&mut app, MenuAction::CloseAll);
        assert!(app.order.is_empty());
        assert_eq!(app.active, None);
    }

    #[test]
    fn close_others_confirms_before_dropping_a_dirty_tab() {
        let (mut app, ids) = app_with_tabs(3);
        app.documents.get_mut(&ids[2]).unwrap().revision = 1;
        run_menu(&mut app, MenuAction::CloseOthers(ids[1]));
        match &app.overlay {
            Overlay::Confirm {
                action: ConfirmAction::CloseMany(targets),
                ..
            } => {
                assert_eq!(targets, &vec![ids[0], ids[2]]);
            }
            other => panic!("expected confirm, got {other:?}"),
        }
        assert_eq!(app.order, ids);
        confirm_yes(&mut app);
        assert_eq!(app.order, vec![ids[1]]);
        assert_eq!(app.active, Some(ids[1]));
    }

    #[test]
    fn close_others_skips_the_prompt_when_only_the_kept_tab_is_dirty() {
        let (mut app, ids) = app_with_tabs(2);
        app.documents.get_mut(&ids[0]).unwrap().revision = 1;
        run_menu(&mut app, MenuAction::CloseOthers(ids[0]));
        assert!(matches!(app.overlay, Overlay::None));
        assert_eq!(app.order, vec![ids[0]]);
        assert_eq!(app.active, Some(ids[0]));
    }

    #[test]
    fn a_running_turn_does_not_ask_to_reload() {
        let dir = std::env::temp_dir().join(format!("p0-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("file.ts");
        std::fs::write(&path, "const a = 1;\n").unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = with_runtime(|| {
            let mut app = App::new(dir, LanguageClient { tx });
            app.open_path(path.clone());
            app
        });
        app.documents.values_mut().next().unwrap().mtime = Some(std::time::SystemTime::UNIX_EPOCH);
        std::fs::write(&path, "const a = 2;\n").unwrap();
        app.thread.running = true;
        super::handle_watch(
            &mut app,
            crate::workspace::WatchEvent {
                path: path.clone(),
                kind: crate::workspace::WatchKind::Modify,
            },
        );
        assert!(matches!(app.overlay, Overlay::None));
        app.thread.running = false;
        super::handle_watch(
            &mut app,
            crate::workspace::WatchEvent {
                path: path.clone(),
                kind: crate::workspace::WatchKind::Modify,
            },
        );
        assert!(matches!(
            app.overlay,
            Overlay::Confirm {
                action: ConfirmAction::Reload(_),
                ..
            }
        ));
        let _ = std::fs::remove_dir_all(app.root.clone());
    }

    #[test]
    fn settle_reloads_clean_buffers_and_keeps_dirty_ones() {
        let dir = std::env::temp_dir().join(format!("p0-settle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let clean = dir.join("clean.ts");
        let dirty = dir.join("dirty.ts");
        std::fs::write(&clean, "export const clean = 1;\n").unwrap();
        std::fs::write(&dirty, "export const dirty = 1;\n").unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = with_runtime(|| {
            let mut app = App::new(dir, LanguageClient { tx });
            app.open_path(clean.clone());
            app.open_path(dirty.clone());
            app
        });
        let dirty_id = app
            .documents
            .iter()
            .find(|(_, doc)| doc.path.as_ref() == Some(&dirty))
            .map(|(id, _)| *id)
            .unwrap();
        app.documents.get_mut(&dirty_id).unwrap().revision = 1;
        std::fs::write(&clean, "export const clean = 2;\n").unwrap();
        std::fs::write(&dirty, "export const dirty = 2;\n").unwrap();
        with_runtime(|| super::settle_agent_edits(&mut app));
        let clean_text = app
            .documents
            .values()
            .find(|doc| doc.path.as_ref() == Some(&clean))
            .unwrap()
            .text();
        let dirty_text = app.documents.get(&dirty_id).unwrap().text();
        assert_eq!(clean_text, "export const clean = 2;\n");
        assert_eq!(dirty_text, "export const dirty = 1;\n");
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("changed on disk")
        );
        let _ = std::fs::remove_dir_all(app.root.clone());
    }
}
