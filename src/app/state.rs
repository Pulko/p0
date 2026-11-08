#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::editor::{Clipboard, Document, DocumentChange, DocumentId, EditorView, Position, Range};
use crate::language::{
    CompletionItem, Diagnostic, LanguageClient, Location, LspCmd, LspStatus, TextEdit,
};
use crate::syntax::SyntaxService;
use crate::workspace::{
    DiffBaseline, DiffLine, ExplorerEntry, FileFinder, GitDiffResult, GitFileStatus, LineChanges,
    SearchMatch,
};

use super::config::Config;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Editor,
    Explorer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    CreateFile { is_dir: bool },
    RenameFile,
    RenameSymbol,
    SaveAs,
}

#[derive(Clone, Debug)]
pub enum ConfirmAction {
    DeletePath(PathBuf),
    Quit,
    Reload(DocumentId),
    Close(DocumentId),
}

#[derive(Clone, Debug)]
pub struct MenuItem {
    pub label: String,
    pub action: MenuAction,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    GoToDefinition,
    FindReferences,
    Hover,
    RenameSymbol,
    Copy,
    Cut,
    Paste,
    NewFile,
    NewFolder,
    RenameFile,
    CopyPath,
    DeletePath,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchKind {
    Find,
    Content,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderAction {
    Find,
    Search,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeaderHit {
    pub action: HeaderAction,
    pub x: u16,
    pub width: u16,
}

#[derive(Clone, Debug)]
pub enum Overlay {
    None,
    Search {
        kind: SearchKind,
        query: String,
        files: Vec<String>,
        matches: Vec<SearchMatch>,
        selected: usize,
        searching: bool,
    },
    SearchFile {
        query: String,
        matches: Vec<(Position, Position)>,
        current: usize,
    },
    Completion {
        items: Vec<CompletionItem>,
        selected: usize,
        origin: Position,
    },
    Hover {
        title: String,
        text: String,
        origin: Position,
        x: u16,
        y: u16,
    },
    Signature {
        text: String,
    },
    ContextMenu {
        items: Vec<MenuItem>,
        selected: usize,
        x: u16,
        y: u16,
        show_type: bool,
        type_text: String,
        hover_token: u64,
    },
    Confirm {
        message: String,
        action: ConfirmAction,
    },
    Prompt {
        title: String,
        input: String,
        kind: PromptKind,
    },
    References {
        items: Vec<Location>,
        selected: usize,
    },
    Themes {
        selected: usize,
    },
    Help,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusAction {
    Diff,
    Theme,
    Help,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusHit {
    pub action: StatusAction,
    pub x: u16,
    pub width: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HScrollHit {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub thumb_x: u16,
    pub thumb_w: u16,
    pub max_left: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabHitBox {
    pub document_id: DocumentId,
    pub x: u16,
    pub width: u16,
    pub close_x: u16,
}

#[derive(Clone, Debug)]
pub struct DiffView {
    pub document_id: DocumentId,
    pub lines: Vec<DiffLine>,
    pub error: Option<String>,
    pub scroll: usize,
    pub loading: bool,
}

pub const HOVER_DWELL: Duration = Duration::from_millis(400);
const GIT_STATUS_DEBOUNCE: Duration = Duration::from_secs(1);

#[derive(Clone, Debug)]
pub struct HoverDwell {
    pub document_id: DocumentId,
    pub position: Position,
    pub screen_x: u16,
    pub screen_y: u16,
    pub since: Instant,
}

pub struct App {
    pub root: PathBuf,
    pub config: Config,
    pub documents: HashMap<DocumentId, Document>,
    pub views: HashMap<DocumentId, EditorView>,
    pub order: Vec<DocumentId>,
    pub active: Option<DocumentId>,
    pub next_id: u64,
    pub focus: Focus,
    pub overlay: Overlay,
    pub explorer_visible: bool,
    pub explorer_entries: Vec<ExplorerEntry>,
    pub explorer_selected: usize,
    pub explorer_scroll: usize,
    pub expanded: HashSet<PathBuf>,
    pub clipboard: Clipboard,
    pub syntax: SyntaxService,
    pub lsp: LanguageClient,
    pub lsp_status: LspStatus,
    pub diagnostics: HashMap<PathBuf, Vec<Diagnostic>>,
    pub file_finder: FileFinder,
    pub workspace_files: Vec<String>,
    pub message: Option<String>,
    pub should_quit: bool,
    pub editor_area: (u16, u16, u16, u16),
    pub search_area: (u16, u16, u16, u16),
    pub explorer_area: (u16, u16, u16, u16),
    pub tabs_area: (u16, u16, u16, u16),
    pub status_area: (u16, u16, u16, u16),
    pub status_hits: Vec<StatusHit>,
    pub header_hits: Vec<HeaderHit>,
    pub overlay_area: Option<(u16, u16, u16, u16)>,
    pub overlay_rows: Vec<(usize, u16)>,
    pub tab_hit_boxes: Vec<TabHitBox>,
    pub tab_scroll: usize,
    pub tab_revealed: Option<DocumentId>,
    pub tab_scroll_left: Option<(u16, u16)>,
    pub tab_scroll_right: Option<(u16, u16)>,
    pub last_click: Option<(Instant, u16, u16)>,
    pub dragging: bool,
    pub hscroll_hit: Option<HScrollHit>,
    pub hscroll_dragging: bool,
    pub hscroll_grab: u16,
    pub lsp_token: u64,
    pub completion_token: u64,
    pub hover_token: u64,
    pub hover_dwell: Option<HoverDwell>,
    pub hover_anchor: Option<(u64, u16, u16, Position)>,
    pub search_generation: u64,
    pub search_tx: mpsc::UnboundedSender<crate::workspace::WorkspaceSearch>,
    pub git_status_tx: mpsc::UnboundedSender<crate::workspace::GitStatusUpdate>,
    pub diff_tx: mpsc::UnboundedSender<GitDiffResult>,
    pub git_status: HashMap<PathBuf, GitFileStatus>,
    git_status_running: bool,
    git_status_again: bool,
    git_status_at: Option<Instant>,
    pub diff_view: Option<DiffView>,
    pub gutter_width: u16,
    pub menu_area: Option<(u16, u16, u16, u16)>,
    pub menu_rows: Vec<(usize, u16)>,
    pub inline_changes: HashMap<DocumentId, LineChanges>,
    pub diff_baselines: HashMap<DocumentId, DiffBaseline>,
    pub open_hunks: HashSet<(DocumentId, usize)>,
}

impl App {
    pub fn new(root: PathBuf, lsp: LanguageClient) -> Self {
        let config = Config::load();
        let mut expanded = HashSet::new();
        expanded.insert(root.clone());
        let (search_tx, _search_rx) = mpsc::unbounded_channel();
        drop(_search_rx);
        let (git_status_tx, git_status_rx) = mpsc::unbounded_channel();
        drop(git_status_rx);
        let (diff_tx, diff_rx) = mpsc::unbounded_channel();
        drop(diff_rx);
        let mut app = Self {
            explorer_visible: config.explorer_visible,
            explorer_entries: Vec::new(),
            explorer_selected: 0,
            explorer_scroll: 0,
            expanded,
            clipboard: Clipboard::new(),
            syntax: SyntaxService::new(),
            lsp,
            lsp_status: LspStatus::Starting,
            diagnostics: HashMap::new(),
            file_finder: FileFinder::new(),
            workspace_files: crate::workspace::list_workspace_files(&root),
            message: None,
            should_quit: false,
            editor_area: (0, 0, 0, 0),
            search_area: (0, 0, 0, 0),
            explorer_area: (0, 0, 0, 0),
            tabs_area: (0, 0, 0, 0),
            status_area: (0, 0, 0, 0),
            status_hits: Vec::new(),
            header_hits: Vec::new(),
            overlay_area: None,
            overlay_rows: Vec::new(),
            tab_hit_boxes: Vec::new(),
            tab_scroll: 0,
            tab_revealed: None,
            tab_scroll_left: None,
            tab_scroll_right: None,
            last_click: None,
            dragging: false,
            hscroll_hit: None,
            hscroll_dragging: false,
            hscroll_grab: 0,
            lsp_token: 0,
            completion_token: 0,
            hover_token: 0,
            hover_dwell: None,
            hover_anchor: None,
            search_generation: 0,
            search_tx,
            git_status_tx,
            diff_tx,
            git_status: HashMap::new(),
            git_status_running: false,
            git_status_again: false,
            git_status_at: None,
            diff_view: None,
            gutter_width: 4,
            menu_area: None,
            menu_rows: Vec::new(),
            inline_changes: HashMap::new(),
            diff_baselines: HashMap::new(),
            open_hunks: HashSet::new(),
            documents: HashMap::new(),
            views: HashMap::new(),
            order: Vec::new(),
            active: None,
            next_id: 1,
            focus: Focus::Editor,
            overlay: Overlay::None,
            root,
            config,
        };
        app.file_finder.set_files(&app.workspace_files);
        app.refresh_explorer();
        let _ = app.lsp.tx.send(LspCmd::Initialize {
            root: app.root.clone(),
        });
        app
    }

    pub fn refresh_explorer(&mut self) {
        self.explorer_entries =
            crate::workspace::load_directory(&self.root, &self.root, &self.expanded);
        if self.explorer_selected >= self.explorer_entries.len() {
            self.explorer_selected = self.explorer_entries.len().saturating_sub(1);
        }
        let inner = explorer_inner_rows(self.explorer_area.3);
        self.explorer_scroll =
            clamp_explorer_scroll(self.explorer_scroll, self.explorer_entries.len(), inner);
    }

    pub fn refresh_git_status(&mut self) {
        self.git_status_at = Some(Instant::now());
        self.pump_git_status();
    }

    pub fn schedule_git_status(&mut self) {
        self.git_status_at = Some(Instant::now() + GIT_STATUS_DEBOUNCE);
    }

    pub fn git_status_delay(&self) -> Option<Duration> {
        self.git_status_at
            .map(|at| at.saturating_duration_since(Instant::now()))
    }

    pub fn pump_git_status(&mut self) {
        let due = self.git_status_at.is_some_and(|at| at <= Instant::now());
        if !due {
            return;
        }
        if self.git_status_running {
            self.git_status_again = true;
            self.git_status_at = None;
            return;
        }
        self.git_status_at = None;
        self.git_status_running = true;
        crate::workspace::spawn_git_status(self.root.clone(), self.git_status_tx.clone());
    }

    pub fn receive_git_status(&mut self, result: crate::workspace::GitStatusUpdate) {
        self.git_status_running = false;
        if let Ok(statuses) = result
            && self.git_status != statuses
        {
            self.git_status = statuses;
        }
        if self.git_status_again {
            self.git_status_again = false;
            if self.git_status_at.is_none() {
                self.git_status_at = Some(Instant::now());
            }
            self.pump_git_status();
        }
    }

    pub fn active_doc(&self) -> Option<&Document> {
        self.active.and_then(|id| self.documents.get(&id))
    }

    pub fn active_doc_mut(&mut self) -> Option<&mut Document> {
        self.active.and_then(|id| self.documents.get_mut(&id))
    }

    pub fn active_view(&self) -> Option<&EditorView> {
        self.active.and_then(|id| self.views.get(&id))
    }

    pub fn active_view_mut(&mut self) -> Option<&mut EditorView> {
        self.active.and_then(|id| self.views.get_mut(&id))
    }

    pub fn open_path(&mut self, path: PathBuf) {
        let path = if path.is_absolute() {
            path
        } else {
            self.root.join(path)
        };
        if let Some((id, _)) = self
            .documents
            .iter()
            .find(|(_, d)| d.path.as_ref() == Some(&path))
        {
            let id = *id;
            self.active = Some(id);
            self.focus = Focus::Editor;
            return;
        }
        match Document::load(DocumentId(self.next_id), path.clone()) {
            Ok(doc) => {
                self.next_id += 1;
                let id = doc.id;
                self.syntax.open(id, doc.language, &doc.rope);
                if let Some(p) = doc.path.clone() {
                    let _ = self.lsp.tx.send(LspCmd::DidOpen {
                        path: p,
                        language: doc.language,
                        text: doc.text(),
                        version: doc.revision as i64,
                    });
                }
                self.views.insert(id, EditorView::new(id));
                self.order.push(id);
                self.documents.insert(id, doc);
                self.active = Some(id);
                self.focus = Focus::Editor;
                self.overlay = Overlay::None;
                self.request_inline_diff(id);
            }
            Err(err) => {
                self.message = Some(format!("open failed: {err}"));
            }
        }
    }

    pub fn open_untitled(&mut self) {
        let doc = Document::new(DocumentId(self.next_id), None, String::new());
        self.next_id += 1;
        let id = doc.id;
        self.views.insert(id, EditorView::new(id));
        self.order.push(id);
        self.syntax.open(id, doc.language, &doc.rope);
        self.documents.insert(id, doc);
        self.active = Some(id);
        self.focus = Focus::Editor;
    }

    pub fn apply_changes(&mut self, changes: Vec<DocumentChange>) {
        let mut edited = Vec::new();
        for change in changes {
            let lsp_change = self
                .documents
                .get(&change.document_id)
                .map(|doc| (doc.path.clone(), doc.language, doc.revision, doc.text()));
            if let Some(doc) = self.documents.get(&change.document_id) {
                self.syntax.apply_edit(&change, doc.language, &doc.rope);
            }
            if let Some((Some(path), _language, revision, text)) = lsp_change {
                let _ = self.lsp.tx.send(LspCmd::DidChange {
                    path,
                    version: revision as i64,
                    change: change.clone(),
                    full_text: text,
                    incremental: true,
                });
                edited.push(change.document_id);
            }
        }
        let mut seen = HashSet::new();
        for document_id in edited {
            if seen.insert(document_id) {
                self.recompute_inline(document_id);
            }
        }
    }

    pub fn close_active(&mut self, force: bool) {
        let Some(id) = self.active else { return };
        if !force {
            if let Some(doc) = self.documents.get(&id) {
                if doc.is_dirty() {
                    self.overlay = Overlay::Confirm {
                        message: "Unsaved changes. Close anyway?".into(),
                        action: ConfirmAction::Close(id),
                    };
                    return;
                }
            }
        }
        if let Some(doc) = self.documents.remove(&id) {
            self.syntax.close(id);
            if let Some(path) = doc.path {
                let _ = self.lsp.tx.send(LspCmd::DidClose { path });
            }
        }
        self.views.remove(&id);
        self.order.retain(|x| *x != id);
        self.inline_changes.remove(&id);
        self.diff_baselines.remove(&id);
        self.open_hunks.retain(|(doc_id, _)| *doc_id != id);
        self.active = self.order.last().copied();
        if self
            .diff_view
            .as_ref()
            .is_some_and(|diff| diff.document_id == id)
        {
            self.diff_view = None;
        }
    }

    pub fn close_document(&mut self, id: DocumentId, force: bool) {
        if self.documents.contains_key(&id) {
            self.active = Some(id);
            self.close_active(force);
        }
    }

    pub fn cycle_tab(&mut self, backwards: bool) {
        let Some(active) = self.active else { return };
        let Some(index) = self.order.iter().position(|id| *id == active) else {
            return;
        };
        if self.order.is_empty() {
            return;
        }
        let next = if backwards {
            index.checked_sub(1).unwrap_or(self.order.len() - 1)
        } else {
            (index + 1) % self.order.len()
        };
        self.active = Some(self.order[next]);
        self.focus = Focus::Editor;
        self.overlay = Overlay::None;
        self.diff_view = None;
    }

    pub fn toggle_diff(&mut self) {
        if self.diff_view.is_some() {
            self.diff_view = None;
            return;
        }
        let Some(doc) = self.active_doc() else {
            return;
        };
        let Some(path) = doc.path.clone() else {
            self.message = Some("Git diff requires a saved file".into());
            return;
        };
        let document_id = doc.id;
        let current = doc.text();
        self.diff_view = Some(DiffView {
            document_id,
            lines: Vec::new(),
            error: None,
            scroll: 0,
            loading: true,
        });
        crate::workspace::spawn_git_diff(
            self.root.clone(),
            document_id,
            path,
            current,
            self.diff_tx.clone(),
        );
    }

    pub fn request_inline_diff(&mut self, document_id: DocumentId) {
        let Some(doc) = self.documents.get(&document_id) else {
            return;
        };
        let Some(path) = doc.path.clone() else {
            self.inline_changes.remove(&document_id);
            self.diff_baselines.remove(&document_id);
            return;
        };
        let current = doc.text();
        crate::workspace::spawn_git_diff(
            self.root.clone(),
            document_id,
            path,
            current,
            self.diff_tx.clone(),
        );
    }

    pub fn recompute_inline(&mut self, document_id: DocumentId) {
        let Some(original) = self
            .diff_baselines
            .get(&document_id)
            .map(|baseline| baseline.original().to_string())
        else {
            return;
        };
        let Some(text) = self.documents.get(&document_id).map(|doc| doc.text()) else {
            return;
        };
        let changes = crate::workspace::classify_lines(&original, &text);
        let valid_starts: HashSet<usize> = crate::workspace::change_hunks(&changes)
            .into_iter()
            .map(|hunk| hunk.start)
            .collect();
        self.open_hunks
            .retain(|(id, start)| *id != document_id || valid_starts.contains(start));
        self.inline_changes.insert(document_id, changes);
    }

    pub fn receive_diff(&mut self, result: GitDiffResult) {
        let document_id = result.document_id;
        match result.baseline.clone() {
            Some(baseline) => {
                self.diff_baselines.insert(document_id, baseline);
                self.recompute_inline(document_id);
            }
            None => {
                self.diff_baselines.remove(&document_id);
                self.inline_changes.remove(&document_id);
                self.open_hunks.retain(|(id, _)| *id != document_id);
            }
        }
        let refreshed = self.diff_baselines.get(&document_id).and_then(|baseline| {
            self.documents
                .get(&document_id)
                .map(|doc| crate::workspace::build_diff(baseline.original(), &doc.text()))
        });
        if let Some(view) = self
            .diff_view
            .as_mut()
            .filter(|view| view.document_id == document_id)
        {
            if let Some(lines) = refreshed {
                view.lines = lines;
                view.error = None;
            } else {
                view.lines = result.lines;
                view.error = result.error;
            }
            view.loading = false;
            view.scroll = 0;
        }
    }

    pub fn visual_rows(&self, document_id: DocumentId) -> Vec<crate::workspace::VisualRow> {
        let line_count = self
            .documents
            .get(&document_id)
            .map(|doc| doc.rope.len_lines())
            .unwrap_or(0);
        let hunks = self
            .inline_changes
            .get(&document_id)
            .map(crate::workspace::change_hunks)
            .unwrap_or_default();
        let folded: Vec<usize> = self
            .open_hunks
            .iter()
            .filter(|(id, _)| *id == document_id)
            .map(|(_, start)| *start)
            .collect();
        crate::workspace::visual_rows(line_count, &hunks, &folded)
    }

    pub fn toggle_hunk_at_line(&mut self, document_id: DocumentId, line: usize) {
        let Some(changes) = self.inline_changes.get(&document_id) else {
            return;
        };
        let Some(hunk) = crate::workspace::change_hunks(changes)
            .into_iter()
            .find(|hunk| {
                line >= hunk.start && line < hunk.end.max(hunk.start + 1) && hunk.start <= line
            })
        else {
            return;
        };
        let key = (document_id, hunk.start);
        if !self.open_hunks.insert(key) {
            self.open_hunks.remove(&key);
        }
    }

    pub fn theme(&self) -> crate::ui::theme::Theme {
        crate::ui::theme::by_name(&self.config.theme)
    }

    pub fn set_theme(&mut self, name: &str) {
        let theme = crate::ui::theme::by_name(name);
        self.config.theme = theme.name.to_string();
        self.message = if let Err(err) = self.config.save() {
            Some(format!("theme {} (not saved: {err})", theme.name))
        } else {
            Some(format!("theme {}", theme.name))
        };
    }

    pub fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    pub fn next_token(&mut self) -> u64 {
        self.lsp_token += 1;
        self.lsp_token
    }

    pub fn request_completion(&mut self) {
        let Some((path, position)) = (|| {
            let doc = self.active_doc()?;
            let view = self.active_view()?;
            Some((doc.path.clone()?, utf16_pos(doc, view.cursor)))
        })() else {
            return;
        };
        let token = self.next_token();
        self.completion_token = token;
        let _ = self.lsp.tx.send(LspCmd::Completion {
            token,
            path,
            position,
        });
    }

    pub fn apply_edits(&mut self, edits: Vec<TextEdit>) {
        let mut by_path: HashMap<PathBuf, Vec<TextEdit>> = HashMap::new();
        for edit in edits {
            by_path.entry(edit.path.clone()).or_default().push(edit);
        }
        for (path, mut list) in by_path {
            if !self
                .documents
                .values()
                .any(|d| d.path.as_ref() == Some(&path))
            {
                self.open_path(path.clone());
            }
            let Some((id, _)) = self
                .documents
                .iter()
                .find(|(_, d)| d.path.as_ref() == Some(&path))
                .map(|(id, _)| (*id, ()))
            else {
                self.message = Some(format!("rename skipped missing file {}", path.display()));
                continue;
            };
            list.sort_by(|a, b| b.start.cmp(&a.start));
            let mut changes = Vec::new();
            if let Some(doc) = self.documents.get_mut(&id) {
                for edit in list {
                    let start = crate::language::utf16_position_to_doc(
                        doc,
                        edit.start.line,
                        edit.start.column as u32,
                    );
                    let end = crate::language::utf16_position_to_doc(
                        doc,
                        edit.end.line,
                        edit.end.column as u32,
                    );
                    changes.push(doc.apply_replace(Range::new(start, end), &edit.new_text, false));
                }
                if let Some(view) = self.views.get_mut(&id) {
                    view.cursor = crate::editor::clamp_position(&doc.rope, view.cursor);
                }
            }
            self.apply_changes(changes);
        }
    }
}

fn utf16_pos(doc: &Document, pos: Position) -> Position {
    Position::new(
        pos.line,
        crate::editor::position_to_utf16(&doc.rope, pos) as usize,
    )
}

pub fn decode_diagnostic_pos(doc: &Document, pos: Position) -> Position {
    crate::language::utf16_position_to_doc(doc, pos.line, pos.column as u32)
}

pub fn is_ident_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '$' || ch == '.'
}

pub fn click_was_double(last: Option<(Instant, u16, u16)>, x: u16, y: u16) -> bool {
    last.is_some_and(|(t, lx, ly)| lx == x && ly == y && t.elapsed() < Duration::from_millis(400))
}

pub fn explorer_inner_rows(height: u16) -> usize {
    height.saturating_sub(2) as usize
}

pub fn max_explorer_scroll(len: usize, inner: usize) -> usize {
    if inner == 0 {
        0
    } else {
        len.saturating_sub(inner)
    }
}

pub fn clamp_explorer_scroll(scroll: usize, len: usize, inner: usize) -> usize {
    scroll.min(max_explorer_scroll(len, inner))
}

pub fn explorer_index_at(
    scroll: usize,
    area_y: u16,
    area_height: u16,
    y: u16,
    len: usize,
) -> Option<usize> {
    let inner = explorer_inner_rows(area_height);
    if inner == 0 || y <= area_y {
        return None;
    }
    let rel = y.saturating_sub(area_y).saturating_sub(1) as usize;
    if rel >= inner {
        return None;
    }
    let idx = scroll + rel;
    (idx < len).then_some(idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_scroll_clamps_to_the_last_page() {
        assert_eq!(clamp_explorer_scroll(100, 10, 4), 6);
        assert_eq!(clamp_explorer_scroll(5, 3, 10), 0);
        assert_eq!(clamp_explorer_scroll(2, 8, 0), 0);
    }

    #[test]
    fn explorer_index_includes_scroll_and_skips_the_border() {
        assert_eq!(explorer_index_at(5, 2, 10, 3, 20), Some(5));
        assert_eq!(explorer_index_at(5, 2, 10, 10, 20), Some(12));
        assert_eq!(explorer_index_at(5, 2, 10, 2, 20), None);
        assert_eq!(explorer_index_at(5, 2, 10, 11, 20), None);
        assert_eq!(explorer_index_at(5, 2, 10, 3, 5), None);
    }
}
