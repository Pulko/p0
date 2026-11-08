use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::app::App;
use crate::app::commands::{self, WheelAction, WheelAxis};
use crate::app::events;
use crate::app::state::{Focus, HeaderAction, Overlay};
use crate::editor::Movement;

pub fn handle_event(app: &mut App, event: Event) {
    match event {
        Event::Key(key) if key.kind != KeyEventKind::Release => handle_key(app, key),
        Event::Mouse(mouse) => handle_mouse(app, mouse),
        Event::Paste(text) => {
            if events::editor_has_focus(app) {
                commands::type_text(app, &text);
            }
        }
        Event::Resize(_, _) => {}
        _ => {}
    }
}

fn handle_key(app: &mut App, key: KeyEvent) {
    if app.hover_dwell.is_some() || matches!(app.overlay, Overlay::Hover { .. }) {
        commands::cancel_hover(app);
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    if matches!(key.code, KeyCode::Esc) {
        if !matches!(app.overlay, Overlay::None) {
            commands::dismiss_overlay(app);
            return;
        }
    }

    if ctrl {
        match key.code {
            KeyCode::Tab => {
                app.cycle_tab(shift);
                return;
            }
            KeyCode::Char('s') | KeyCode::Char('S') => return commands::save_active(app),
            KeyCode::Char('p') | KeyCode::Char('P') => return commands::open_search(app),
            KeyCode::Char('d') | KeyCode::Char('D') if shift => {
                app.toggle_diff();
                return;
            }
            KeyCode::Char('f') | KeyCode::Char('F') => return commands::open_search_file(app),
            KeyCode::Char('g') | KeyCode::Char('G') => {
                app.overlay = Overlay::GoToLine {
                    input: String::new(),
                };
                return;
            }
            KeyCode::Char(' ') => return app.request_completion(),
            KeyCode::Char('z') | KeyCode::Char('Z') => return commands::undo(app),
            KeyCode::Char('y') | KeyCode::Char('Y') => return commands::redo(app),
            KeyCode::Char('b') | KeyCode::Char('B') => {
                app.explorer_visible = !app.explorer_visible;
                return;
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => return commands::maybe_quit(app),
            KeyCode::Char('w') | KeyCode::Char('W') => return app.close_active(false),
            KeyCode::Char('c') | KeyCode::Char('C') => return commands::copy(app),
            KeyCode::Char('x') | KeyCode::Char('X') => return commands::cut(app),
            KeyCode::Char('v') | KeyCode::Char('V') => return commands::paste(app),
            KeyCode::Char('n') | KeyCode::Char('N') => return app.open_untitled(),
            KeyCode::Char('/') | KeyCode::Char('_') | KeyCode::Char('?') => {
                return commands::open_help(app);
            }
            KeyCode::Enter => return commands::go_to_definition(app),
            KeyCode::Char('r') | KeyCode::Char('R') if shift => {
                return commands::find_references(app);
            }
            KeyCode::Char('r') | KeyCode::Char('R') => return commands::start_rename_symbol(app),
            _ => {}
        }
    }

    if !matches!(
        app.overlay,
        Overlay::None
            | Overlay::Completion { .. }
            | Overlay::Hover { .. }
            | Overlay::Signature { .. }
    ) {
        match key.code {
            KeyCode::Enter => return commands::overlay_submit(app),
            KeyCode::Backspace => {
                commands::backspace(app);
                return;
            }
            KeyCode::Up => return commands::move_sel(app, Movement::Up, false),
            KeyCode::Down => return commands::move_sel(app, Movement::Down, false),
            KeyCode::Char(ch) => {
                commands::overlay_char(app, ch);
                return;
            }
            _ => return,
        }
    }

    if matches!(app.overlay, Overlay::Completion { .. }) {
        match key.code {
            KeyCode::Enter | KeyCode::Tab => return commands::accept_completion(app),
            KeyCode::Esc => {
                commands::dismiss_overlay(app);
                return;
            }
            _ => {}
        }
    }

    if app.focus == Focus::Explorer && matches!(app.overlay, Overlay::None) {
        match key.code {
            KeyCode::Up => {
                app.explorer_selected = app.explorer_selected.saturating_sub(1);
                commands::reveal_explorer_selection(app);
                return;
            }
            KeyCode::Down => {
                app.explorer_selected =
                    (app.explorer_selected + 1).min(app.explorer_entries.len().saturating_sub(1));
                commands::reveal_explorer_selection(app);
                return;
            }
            KeyCode::Enter => {
                commands::explorer_activate(app);
                return;
            }
            KeyCode::Tab => {
                app.focus = Focus::Editor;
                return;
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::Backspace => commands::backspace(app),
        KeyCode::Delete => commands::delete_key(app),
        KeyCode::Enter => commands::type_text(app, "\n"),
        KeyCode::Tab => commands::type_text(app, "    "),
        KeyCode::Left if ctrl => commands::move_sel(app, Movement::WordLeft, shift),
        KeyCode::Right if ctrl => commands::move_sel(app, Movement::WordRight, shift),
        KeyCode::Left => commands::move_sel(app, Movement::Left, shift),
        KeyCode::Right => commands::move_sel(app, Movement::Right, shift),
        KeyCode::Up => commands::move_sel(app, Movement::Up, shift),
        KeyCode::Down => commands::move_sel(app, Movement::Down, shift),
        KeyCode::Home if ctrl => commands::move_sel(app, Movement::DocumentStart, shift),
        KeyCode::End if ctrl => commands::move_sel(app, Movement::DocumentEnd, shift),
        KeyCode::Home => commands::move_sel(app, Movement::LineStart, shift),
        KeyCode::End => commands::move_sel(app, Movement::LineEnd, shift),
        KeyCode::PageUp => commands::move_sel(app, Movement::PageUp, shift),
        KeyCode::PageDown => commands::move_sel(app, Movement::PageDown, shift),
        KeyCode::Char(ch) => commands::type_text(app, &ch.to_string()),
        _ => {}
    }
}

fn scroll_tabs(app: &mut App, x: u16) -> bool {
    if let Some((hit_x, width)) = app.tab_scroll_left
        && x >= hit_x
        && x < hit_x.saturating_add(width)
    {
        app.tab_scroll = app.tab_scroll.saturating_sub(1);
        return true;
    }
    if let Some((hit_x, width)) = app.tab_scroll_right
        && x >= hit_x
        && x < hit_x.saturating_add(width)
    {
        app.tab_scroll = app.tab_scroll.saturating_add(1);
        return true;
    }
    false
}

fn handle_mouse(app: &mut App, mouse: MouseEvent) {
    let x = mouse.column;
    let y = mouse.row;
    let in_search = app.explorer_visible
        && x >= app.search_area.0
        && x < app.search_area.0 + app.search_area.2
        && y >= app.search_area.1
        && y < app.search_area.1 + app.search_area.3;
    let in_explorer = app.explorer_visible
        && x >= app.explorer_area.0
        && x < app.explorer_area.0 + app.explorer_area.2
        && y >= app.explorer_area.1
        && y < app.explorer_area.1 + app.explorer_area.3;
    let in_editor = x >= app.editor_area.0
        && x < app.editor_area.0 + app.editor_area.2
        && y >= app.editor_area.1
        && y < app.editor_area.1 + app.editor_area.3;
    let in_tabs = x >= app.tabs_area.0
        && x < app.tabs_area.0 + app.tabs_area.2
        && y >= app.tabs_area.1
        && y < app.tabs_area.1 + app.tabs_area.3;
    let status_hit = (y == app.status_area.1).then(|| {
        app.status_hits
            .iter()
            .find(|hit| x >= hit.x && x < hit.x.saturating_add(hit.width))
            .map(|hit| hit.action)
    });

    match mouse.kind {
        MouseEventKind::Moved => {
            if in_editor {
                commands::schedule_hover(app, x, y);
            } else {
                commands::cancel_hover(app);
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            commands::cancel_hover(app);
            if matches!(app.overlay, Overlay::Themes { .. } | Overlay::Help) {
                let was_themes = matches!(app.overlay, Overlay::Themes { .. });
                let was_help = matches!(app.overlay, Overlay::Help);
                let inside = app.overlay_area.is_some_and(|(ax, ay, width, height)| {
                    x >= ax
                        && x < ax.saturating_add(width)
                        && y >= ay
                        && y < ay.saturating_add(height)
                });
                if inside {
                    if was_themes {
                        commands::click_themes(app, y);
                    }
                    return;
                }
                commands::dismiss_overlay(app);
                if let Some(Some(action)) = status_hit {
                    let same = (was_themes && action == crate::app::state::StatusAction::Theme)
                        || (was_help && action == crate::app::state::StatusAction::Help);
                    if same {
                        return;
                    }
                }
            }
            if matches!(app.overlay, Overlay::ContextMenu { .. }) {
                if let Some(action) = crate::ui::context_menu::action_at(app, x, y) {
                    app.overlay = Overlay::None;
                    commands::run_menu(app, action);
                } else if app.menu_area.is_some_and(|(ax, ay, width, height)| {
                    x >= ax
                        && x < ax.saturating_add(width)
                        && y >= ay
                        && y < ay.saturating_add(height)
                }) {
                } else {
                    commands::dismiss_overlay(app);
                }
                return;
            }
            let dbl = events::note_click(app, x, y);
            if let Some(Some(action)) = status_hit {
                match action {
                    crate::app::state::StatusAction::Diff => app.toggle_diff(),
                    crate::app::state::StatusAction::Theme => commands::open_themes(app),
                    crate::app::state::StatusAction::Help => commands::open_help(app),
                }
            } else if in_tabs && !scroll_tabs(app, x) {
                if let Some(tab) = app
                    .tab_hit_boxes
                    .iter()
                    .find(|tab| x >= tab.x && x < tab.x + tab.width)
                    .copied()
                {
                    if x == tab.close_x {
                        app.close_document(tab.document_id, false);
                    } else {
                        app.active = Some(tab.document_id);
                        app.focus = Focus::Editor;
                        app.overlay = Overlay::None;
                        app.diff_view = None;
                    }
                }
            } else if in_search {
                if let Some(action) = app
                    .header_hits
                    .iter()
                    .find(|hit| x >= hit.x && x < hit.x.saturating_add(hit.width))
                    .map(|hit| hit.action)
                {
                    match action {
                        HeaderAction::Find => commands::open_search(app),
                        HeaderAction::Search => commands::open_content_search(app),
                    }
                }
            } else if in_explorer {
                app.focus = Focus::Explorer;
                commands::click_explorer(app, y);
            } else if in_editor {
                app.focus = Focus::Editor;
                if !commands::begin_hscroll_drag(app, x, y) {
                    if mouse.modifiers.contains(KeyModifiers::CONTROL) {
                        commands::click_editor(app, x, y, false);
                        commands::go_to_definition(app);
                    } else if dbl {
                        commands::double_click_editor(app, x, y);
                    } else {
                        commands::click_editor(app, x, y, false);
                        app.dragging = true;
                    }
                }
            }
        }
        MouseEventKind::Drag(MouseButton::Left) if app.hscroll_dragging => {
            commands::drag_hscroll(app, x);
        }
        MouseEventKind::Drag(MouseButton::Left) if app.dragging => {
            commands::click_editor(app, x, y, true);
        }
        MouseEventKind::Up(MouseButton::Left) => {
            app.dragging = false;
            app.hscroll_dragging = false;
        }
        MouseEventKind::Down(MouseButton::Middle) if in_tabs => {
            if let Some(tab) = app
                .tab_hit_boxes
                .iter()
                .find(|tab| x >= tab.x && x < tab.x + tab.width)
                .copied()
            {
                app.close_document(tab.document_id, false);
            }
        }
        MouseEventKind::Down(MouseButton::Right) => {
            commands::cancel_hover(app);
            if in_explorer {
                app.focus = Focus::Explorer;
                if let Some(idx) = crate::app::state::explorer_index_at(
                    app.explorer_scroll,
                    app.explorer_area.1,
                    app.explorer_area.3,
                    y,
                    app.explorer_entries.len(),
                ) {
                    app.explorer_selected = idx;
                }
                commands::show_explorer_menu(app, x, y);
            } else if in_editor {
                app.focus = Focus::Editor;
                commands::click_editor(app, x, y, false);
                commands::show_editor_menu(app, x, y);
            }
        }
        MouseEventKind::ScrollDown => {
            dispatch_wheel(
                app,
                mouse,
                x,
                y,
                in_explorer,
                in_editor,
                WheelAxis::Vertical(3),
            );
        }
        MouseEventKind::ScrollUp => {
            dispatch_wheel(
                app,
                mouse,
                x,
                y,
                in_explorer,
                in_editor,
                WheelAxis::Vertical(-3),
            );
        }
        MouseEventKind::ScrollLeft => {
            dispatch_wheel(
                app,
                mouse,
                x,
                y,
                in_explorer,
                in_editor,
                WheelAxis::Horizontal(-4),
            );
        }
        MouseEventKind::ScrollRight => {
            dispatch_wheel(
                app,
                mouse,
                x,
                y,
                in_explorer,
                in_editor,
                WheelAxis::Horizontal(4),
            );
        }
        _ => {}
    }
}

fn dispatch_wheel(
    app: &mut App,
    mouse: MouseEvent,
    x: u16,
    y: u16,
    in_explorer: bool,
    in_editor: bool,
    axis: WheelAxis,
) {
    commands::cancel_hover(app);
    let in_popup = in_rect(app.overlay_area, x, y) || in_rect(app.menu_area, x, y);
    let shift = mouse.modifiers.contains(KeyModifiers::SHIFT);
    match commands::wheel_action(in_popup, in_explorer, in_editor, shift, axis) {
        WheelAction::Explorer(delta) => commands::scroll_explorer(app, delta),
        WheelAction::EditorVertical(delta) => commands::scroll_editor(app, delta),
        WheelAction::EditorHorizontal(delta) => commands::scroll_editor_horizontal(app, delta),
        WheelAction::None => {}
    }
}

fn in_rect(area: Option<(u16, u16, u16, u16)>, x: u16, y: u16) -> bool {
    area.is_some_and(|(ax, ay, w, h)| {
        x >= ax && x < ax.saturating_add(w) && y >= ay && y < ay.saturating_add(h)
    })
}
