pub(crate) mod context_menu;
mod editor_view;
mod explorer;
mod layout;
mod popup;
mod status_bar;
pub(crate) mod theme;

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
    if app.explorer_visible {
        explorer::render_search(frame, areas.search, app);
        explorer::render(frame, areas.explorer, app);
    }
    editor_view::render(frame, areas.tabs, areas.editor, app);
    status_bar::render(frame, areas.status, app);
    popup::render(frame, app);
    context_menu::render(frame, app);
}
