#![allow(dead_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::agent::{AgentRun, Thread};
use crate::editor::{
    Clipboard, Document, DocumentChange, DocumentId, EditorView, LanguageId, Position, Range,
};
use crate::language::{
    CompletionItem, Diagnostic, LanguageClient, Location, LspCmd, LspStatus, TextEdit,
};
use crate::syntax::SyntaxService;
use crate::workspace::{
    DiffBaseline, DiffLine, ExplorerEntry, FileDiffResult, FileFinder, GitDiffResult,
    GitPathStatus, GitWrite, GitWriteResult, LineChanges, ParsedGitDiff, SearchMatch,
};

use super::config::Config;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Editor,
    Explorer,
    Thread,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    CreateFile { is_dir: bool },
    RenameFile,
    RenameSymbol,
    SaveAs,
    AgentEdit,
}

#[derive(Clone, Debug)]
pub enum ConfirmAction {
    DeletePath(PathBuf),
    Quit,
    Reload(DocumentId),
    Close(DocumentId),
    CloseMany(Vec<DocumentId>),
    DiscardHunk { patch: String, path: PathBuf },
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
    CloseOthers(DocumentId),
    CloseAll,
    AskClaude,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SideTabHit {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub changes: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentTabHit {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub diff: bool,
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
    TypesNote,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusAction {
    Theme,
    Help,
    Agent,
    Types,
    Markdown,
    Git,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffSource {
    Unstaged,
    Staged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HunkAction {
    Stage,
    Unstage,
    Discard,
}

#[derive(Clone, Debug)]
pub enum GitClick {
    Message,
    Commit,
    StageAll,
    UnstageAll,
    Toggle { path: PathBuf, staged: bool },
    Open { path: PathBuf, staged: bool },
}

#[derive(Clone, Debug)]
pub struct GitHit {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub action: GitClick,
}

#[derive(Clone, Copy, Debug)]
pub struct DiffHit {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub hunk: usize,
    pub action: HunkAction,
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
    pub path: Option<PathBuf>,
    pub source: DiffSource,
    pub lines: Vec<DiffLine>,
    pub parsed: Option<ParsedGitDiff>,
    pub allow_hunks: bool,
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
    pub pane_mark: Option<(u16, u16, u16)>,
    pub types_copy: Option<(u16, u16, u16)>,
    pub last_click: Option<(Instant, u16, u16)>,
    pub dragging: bool,
    pub hscroll_hit: Option<HScrollHit>,
    pub hscroll_dragging: bool,
    pub explorer_resizing: bool,
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
    pub git_status: HashMap<PathBuf, GitPathStatus>,
    git_status_running: bool,
    git_status_again: bool,
    git_status_at: Option<Instant>,
    pub diff_view: Option<DiffView>,
    pub markdown_preview: Option<crate::markdown::Preview>,
    pub gutter_width: u16,
    pub menu_area: Option<(u16, u16, u16, u16)>,
    pub menu_rows: Vec<(usize, u16)>,
    pub inline_changes: HashMap<DocumentId, LineChanges>,
    pub diff_baselines: HashMap<DocumentId, DiffBaseline>,
    pub open_hunks: HashSet<(DocumentId, usize)>,
    pub thread: Thread,
    pub thread_visible: bool,
    pub thread_area: (u16, u16, u16, u16),
    pub thread_stop: Option<(u16, u16, u16)>,
    pub claude_installed: bool,
    pub agent_tx: mpsc::UnboundedSender<crate::agent::AgentEvent>,
    pub agent_run: Option<AgentRun>,
    pub git_branch: Option<String>,
    pub git_repo: bool,
    pub git_open: bool,
    pub git_selected: usize,
    pub git_scroll: usize,
    pub git_message: String,
    pub git_message_focus: bool,
    pub git_hits: Vec<GitHit>,
    pub side_tabs: Vec<SideTabHit>,
    pub content_tabs: Vec<ContentTabHit>,
    pub diff_hits: Vec<DiffHit>,
    pub git_write_tx: mpsc::UnboundedSender<GitWriteResult>,
    git_write_running: bool,
    git_write_queue: VecDeque<GitWrite>,
    pub file_diff_tx: mpsc::UnboundedSender<FileDiffResult>,
    file_diff_token: u64,
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
        let (git_write_tx, git_write_rx) = mpsc::unbounded_channel();
        drop(git_write_rx);
        let (file_diff_tx, file_diff_rx) = mpsc::unbounded_channel();
        drop(file_diff_rx);
        let (agent_tx, agent_rx) = mpsc::unbounded_channel();
        drop(agent_rx);
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
            pane_mark: None,
            types_copy: None,
            last_click: None,
            dragging: false,
            hscroll_hit: None,
            hscroll_dragging: false,
            explorer_resizing: false,
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
            markdown_preview: None,
            gutter_width: 4,
            menu_area: None,
            menu_rows: Vec::new(),
            inline_changes: HashMap::new(),
            diff_baselines: HashMap::new(),
            open_hunks: HashSet::new(),
            thread: Thread::new(),
            thread_visible: false,
            thread_area: (0, 0, 0, 0),
            thread_stop: None,
            claude_installed: which::which("claude").is_ok(),
            agent_tx,
            agent_run: None,
            git_branch: None,
            git_repo: false,
            git_open: false,
            git_selected: 0,
            git_scroll: 0,
            git_message: String::new(),
            git_message_focus: false,
            git_hits: Vec::new(),
            side_tabs: Vec::new(),
            content_tabs: Vec::new(),
            diff_hits: Vec::new(),
            git_write_tx,
            git_write_running: false,
            git_write_queue: VecDeque::new(),
            file_diff_tx,
            file_diff_token: 0,
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
        match result {
            Ok(snapshot) => {
                let discovered = !self.git_repo;
                self.git_repo = true;
                if self.git_status != snapshot.files {
                    self.git_status = snapshot.files;
                }
                self.git_branch = snapshot.branch;
                if discovered {
                    let open: Vec<_> = self.order.clone();
                    for id in open {
                        self.request_inline_diff(id);
                    }
                }
            }
            Err(crate::workspace::GitStatusError::NotARepository) => self.hide_git(),
            Err(crate::workspace::GitStatusError::Unavailable) => {}
        }
        if self.git_status_again {
            self.git_status_again = false;
            if self.git_status_at.is_none() {
                self.git_status_at = Some(Instant::now());
            }
            self.pump_git_status();
        }
        let rows = crate::workspace::git_panel_rows(&self.git_status);
        if !rows
            .get(self.git_selected)
            .is_some_and(|row| matches!(row, crate::workspace::GitPanelRow::File { .. }))
        {
            self.git_selected = rows
                .iter()
                .position(|row| matches!(row, crate::workspace::GitPanelRow::File { .. }))
                .unwrap_or(0);
        }
    }

    pub fn toggle_git(&mut self) {
        if self.git_repo {
            self.show_changes(!self.git_open);
        }
    }

    pub fn show_changes(&mut self, changes: bool) {
        if changes && !self.git_repo {
            return;
        }
        self.git_message_focus = false;
        self.git_open = changes;
        self.explorer_visible = true;
        self.focus = Focus::Explorer;
    }

    pub fn diff_is_showing(&self) -> bool {
        self.git_repo && self.diff_view.is_some()
    }

    fn hide_git(&mut self) {
        self.git_repo = false;
        self.git_open = false;
        self.git_message_focus = false;
        self.git_branch = None;
        self.git_status.clear();
        self.git_selected = 0;
        self.diff_view = None;
        self.inline_changes.clear();
        self.diff_baselines.clear();
        self.open_hunks.clear();
    }

    pub fn buffer_dirty(&self, path: &Path) -> bool {
        self.documents
            .values()
            .any(|doc| doc.is_dirty() && doc.path.as_deref() == Some(path))
    }

    fn dirty_change(&self) -> bool {
        self.git_status.keys().any(|path| self.buffer_dirty(path))
    }

    pub fn enqueue_git_write(&mut self, write: GitWrite) {
        if let Err(message) = crate::workspace::prepare_git_write(&self.root, &write) {
            self.message = Some(message);
            return;
        }
        if self.git_write_running {
            self.git_write_queue.push_back(write);
            return;
        }
        self.start_git_write(write);
    }

    fn start_git_write(&mut self, write: GitWrite) {
        self.git_write_running = true;
        crate::workspace::spawn_git_write(self.root.clone(), write, self.git_write_tx.clone());
    }

    pub fn note_git_write(&mut self, result: GitWriteResult) {
        self.git_write_running = false;
        if result.ok {
            if let Some(message) = &result.committed {
                if self.git_message.trim() == message {
                    self.git_message.clear();
                    self.git_message_focus = false;
                }
                self.message = Some("Committed".into());
            }
        } else if !result.message.is_empty() {
            self.message = Some(result.message);
        }
        if let Some(next) = self.git_write_queue.pop_front() {
            self.start_git_write(next);
        }
    }

    pub fn stage_path(&mut self, path: PathBuf) {
        if self.buffer_dirty(&path) {
            self.message = Some("Save first".into());
            return;
        }
        self.enqueue_git_write(GitWrite::Stage(path));
    }

    pub fn unstage_path(&mut self, path: PathBuf) {
        if self.buffer_dirty(&path) {
            self.message = Some("Save first".into());
            return;
        }
        self.enqueue_git_write(GitWrite::Unstage(path));
    }

    pub fn toggle_stage(&mut self, path: PathBuf, staged: bool) {
        if staged {
            self.unstage_path(path);
        } else {
            self.stage_path(path);
        }
    }

    pub fn stage_all(&mut self) {
        if self.dirty_change() {
            self.message = Some("Save first".into());
            return;
        }
        if !self
            .git_status
            .values()
            .any(|status| status.unstaged.is_some())
        {
            self.message = Some("No changes to stage".into());
            return;
        }
        self.enqueue_git_write(GitWrite::StageAll);
    }

    pub fn unstage_all(&mut self) {
        if self.dirty_change() {
            self.message = Some("Save first".into());
            return;
        }
        if !self
            .git_status
            .values()
            .any(|status| status.staged.is_some())
        {
            self.message = Some("Nothing staged".into());
            return;
        }
        self.enqueue_git_write(GitWrite::UnstageAll);
    }

    pub fn commit_git(&mut self) {
        if !crate::workspace::can_commit(&self.git_message, &self.git_status) {
            self.message = Some(if self.git_message.trim().is_empty() {
                "Write a commit message".into()
            } else {
                "Nothing staged".into()
            });
            return;
        }
        self.enqueue_git_write(GitWrite::Commit(self.git_message.clone()));
    }

    pub fn request_hunk(&mut self, index: usize, action: HunkAction) {
        let Some(view) = self.diff_view.clone() else {
            return;
        };
        if !view.allow_hunks {
            return;
        }
        let Some(path) = view.path.clone() else {
            return;
        };
        if self.buffer_dirty(&path) {
            self.message = Some("Save first".into());
            return;
        }
        let Some(parsed) = view.parsed.as_ref() else {
            return;
        };
        let Some(patch) = crate::workspace::hunk_patch(parsed, index) else {
            return;
        };
        match action {
            HunkAction::Stage => self.enqueue_git_write(GitWrite::Apply {
                patch,
                cached: true,
                reverse: false,
                reload: None,
            }),
            HunkAction::Unstage => self.enqueue_git_write(GitWrite::Apply {
                patch,
                cached: true,
                reverse: true,
                reload: None,
            }),
            HunkAction::Discard => {
                self.overlay = Overlay::Confirm {
                    message: "Discard this hunk?".into(),
                    action: ConfirmAction::DiscardHunk { patch, path },
                };
            }
        }
    }

    pub fn discard_hunk(&mut self, patch: String, path: PathBuf) {
        if self.buffer_dirty(&path) {
            self.message = Some("Save first".into());
            return;
        }
        self.enqueue_git_write(GitWrite::Apply {
            patch,
            cached: false,
            reverse: true,
            reload: Some(path),
        });
    }

    pub fn open_git_entry(&mut self, path: PathBuf, staged: bool) {
        self.git_message_focus = false;
        if path.is_file() {
            self.open_path(path.clone());
        }
        self.markdown_preview = None;
        self.load_file_diff(path, staged);
    }

    pub fn refresh_open_file_diff(&mut self) {
        let Some(diff) = self.diff_view.as_ref() else {
            return;
        };
        let staged = diff.source == DiffSource::Staged;
        if let Some(path) = diff.path.clone() {
            self.load_file_diff(path, staged);
        }
    }

    pub fn receive_file_diff(&mut self, result: FileDiffResult) {
        if result.token != self.file_diff_token {
            return;
        }
        let Some(view) = self.diff_view.as_mut() else {
            return;
        };
        let same = match view.source {
            DiffSource::Staged => result.staged,
            DiffSource::Unstaged => !result.staged,
        };
        if !same || view.path.as_ref() != Some(&result.path) {
            return;
        }
        view.lines = result.lines;
        view.parsed = result.parsed;
        view.allow_hunks = result.allow_hunks;
        view.error = result.error;
        view.loading = false;
    }

    fn load_file_diff(&mut self, path: PathBuf, staged: bool) {
        use crate::workspace::GitFileStatus;
        let status = self.git_status.get(&path).copied();
        let side = if staged {
            status.and_then(|item| item.staged)
        } else {
            status.and_then(|item| item.unstaged)
        };
        let document_id = self
            .documents
            .iter()
            .find(|(_, doc)| doc.path.as_ref() == Some(&path))
            .map(|(id, _)| *id)
            .unwrap_or(DocumentId(0));
        let source = if staged {
            DiffSource::Staged
        } else {
            DiffSource::Unstaged
        };
        self.file_diff_token = self.file_diff_token.saturating_add(1);
        if side == Some(GitFileStatus::Untracked) {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            self.diff_view = Some(DiffView {
                document_id,
                path: Some(path),
                source,
                lines: crate::workspace::untracked_diff_lines(&text),
                parsed: None,
                allow_hunks: false,
                error: None,
                scroll: 0,
                loading: false,
            });
            return;
        }
        let renamed = status.is_some_and(|item| {
            item.staged == Some(GitFileStatus::Renamed)
                || item.unstaged == Some(GitFileStatus::Renamed)
        });
        let allow = !renamed
            && matches!(
                side,
                Some(kind) if kind != GitFileStatus::Renamed && kind != GitFileStatus::Untracked
            );
        let scroll = self
            .diff_view
            .as_ref()
            .filter(|diff| diff.path.as_ref() == Some(&path) && diff.source == source)
            .map(|diff| diff.scroll)
            .unwrap_or(0);
        let token = self.file_diff_token;
        self.diff_view = Some(DiffView {
            document_id,
            path: Some(path.clone()),
            source,
            lines: Vec::new(),
            parsed: None,
            allow_hunks: false,
            error: None,
            scroll,
            loading: true,
        });
        crate::workspace::spawn_file_diff(
            self.root.clone(),
            path,
            staged,
            allow,
            token,
            self.file_diff_tx.clone(),
        );
    }

    pub fn reload_clean_document(&mut self, path: &Path) {
        let Some(id) = self.documents.iter().find_map(|(id, doc)| {
            (doc.path.as_deref() == Some(path) && !doc.is_dirty()).then_some(*id)
        }) else {
            return;
        };
        let Some(doc) = self.documents.get_mut(&id) else {
            return;
        };
        if doc.replace_from_disk().is_err() {
            return;
        }
        let language = doc.language;
        let rope = doc.rope.clone();
        let revision = doc.revision;
        let full = doc.text();
        self.syntax.close(id);
        self.syntax.open(id, language, &rope);
        let _ = self.lsp.tx.send(LspCmd::DidChange {
            path: path.to_path_buf(),
            version: revision as i64,
            change: clean_reload_change(id, revision, full.clone()),
            full_text: full,
            incremental: false,
        });
        if let Some(view) = self.views.get_mut(&id) {
            let len = rope.len_lines().max(1);
            if view.cursor.line >= len {
                view.cursor.line = len - 1;
                view.cursor.column = 0;
            }
            view.selection = None;
        }
        self.request_inline_diff(id);
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
            self.diff_view = None;
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
                self.diff_view = None;
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
        if self
            .markdown_preview
            .as_ref()
            .is_some_and(|preview| preview.document_id == id)
        {
            self.markdown_preview = None;
        }
    }

    pub fn close_document(&mut self, id: DocumentId, force: bool) {
        if self.documents.contains_key(&id) {
            self.active = Some(id);
            self.close_active(force);
        }
    }

    pub fn close_documents(&mut self, ids: &[DocumentId], force: bool) {
        let targets: Vec<DocumentId> = ids
            .iter()
            .copied()
            .filter(|id| self.documents.contains_key(id))
            .collect();
        if targets.is_empty() {
            return;
        }
        if !force
            && targets
                .iter()
                .any(|id| self.documents.get(id).is_some_and(|doc| doc.is_dirty()))
        {
            self.overlay = Overlay::Confirm {
                message: "Unsaved changes. Close anyway?".into(),
                action: ConfirmAction::CloseMany(targets),
            };
            return;
        }
        for id in targets {
            self.active = Some(id);
            self.close_active(true);
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
        if !self.git_repo {
            self.diff_view = None;
            return;
        }
        let Some(doc) = self.active_doc() else {
            self.diff_view = None;
            return;
        };
        let Some(path) = doc.path.clone() else {
            self.message = Some("Git diff requires a saved file".into());
            return;
        };
        if self
            .diff_view
            .as_ref()
            .is_some_and(|diff| diff.path.as_ref() == Some(&path))
        {
            self.diff_view = None;
            return;
        }
        self.markdown_preview = None;
        let staged = self
            .git_status
            .get(&path)
            .is_some_and(|status| status.unstaged.is_none() && status.staged.is_some());
        self.load_file_diff(path, staged);
    }

    pub fn markdown_preview_open(&self) -> bool {
        self.markdown_preview
            .as_ref()
            .is_some_and(|preview| self.active == Some(preview.document_id))
    }

    pub fn toggle_markdown_preview(&mut self) {
        let Some(id) = self.active else {
            return;
        };
        if self.markdown_preview_open() {
            self.markdown_preview = None;
            return;
        }
        let Some(doc) = self.documents.get(&id) else {
            return;
        };
        if doc.language != LanguageId::Markdown {
            self.message = Some("Markdown preview is for Markdown files".into());
            return;
        }
        self.diff_view = None;
        self.markdown_preview = Some(crate::markdown::Preview::open(id));
    }

    pub fn scroll_markdown_preview(&mut self, delta: i32) {
        let Some(active) = self.active else {
            return;
        };
        let Some(preview) = self.markdown_preview.as_mut() else {
            return;
        };
        if preview.document_id != active {
            return;
        }
        let max = preview.line_count().saturating_sub(1) as i64;
        let next = preview.scroll as i64 + i64::from(delta);
        preview.scroll = next.clamp(0, max) as usize;
    }

    pub fn jump_markdown_preview(&mut self, to_end: bool) {
        let Some(active) = self.active else {
            return;
        };
        let Some(preview) = self.markdown_preview.as_mut() else {
            return;
        };
        if preview.document_id != active {
            return;
        }
        preview.scroll = if to_end {
            preview.line_count().saturating_sub(1)
        } else {
            0
        };
    }

    pub fn request_inline_diff(&mut self, document_id: DocumentId) {
        if !self.git_repo {
            return;
        }
        let Some(doc) = self.documents.get(&document_id) else {
            return;
        };
        let Some(path) = doc.path.clone() else {
            self.inline_changes.remove(&document_id);
            self.diff_baselines.remove(&document_id);
            return;
        };
        crate::workspace::spawn_git_diff(
            self.root.clone(),
            document_id,
            path,
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

    pub fn revert_hunk_at_line(&mut self, document_id: DocumentId, line: usize) {
        let prepared = self.inline_changes.get(&document_id).and_then(|changes| {
            let line_count = changes.marks.len();
            crate::workspace::change_hunks(changes)
                .into_iter()
                .find(|hunk| line >= hunk.start && line < hunk.end.max(hunk.start + 1))
                .map(|hunk| (hunk, line_count))
        });
        let Some((hunk, line_count)) = prepared else {
            return;
        };
        let Some(doc) = self.documents.get_mut(&document_id) else {
            return;
        };
        let ends_with_newline =
            doc.rope.len_chars() > 0 && doc.rope.char(doc.rope.len_chars() - 1) == '\n';
        let replacement = crate::workspace::hunk_replacement(&hunk, line_count, ends_with_newline);
        let range = crate::workspace::hunk_edit_range(&doc.rope, &replacement);
        let change = doc.apply_replace(range, replacement.text(), false);
        let caret = change.start_position;
        if let Some(view) = self.views.get_mut(&document_id) {
            view.cursor = caret;
            view.selection = None;
            view.preferred_column = None;
        }
        self.apply_changes(vec![change]);
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

fn clean_reload_change(id: DocumentId, revision: u64, text: String) -> DocumentChange {
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
    use std::collections::HashMap;
    use std::path::PathBuf;

    use tokio::sync::mpsc;

    use crate::editor::{Document, DocumentId};
    use crate::language::LanguageClient;
    use crate::workspace::{GitFileStatus, GitPathStatus, GitSnapshot, GitStatusError};

    use super::*;

    fn with_runtime<T>(body: impl FnOnce() -> T) -> T {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        body()
    }

    fn test_app() -> App {
        let (tx, _rx) = mpsc::unbounded_channel();
        App::new(PathBuf::from("."), LanguageClient { tx })
    }

    fn open_saved(app: &mut App, path: PathBuf) {
        let id = DocumentId(app.next_id);
        app.next_id += 1;
        app.documents
            .insert(id, Document::new(id, Some(path), "fn main() {}\n".into()));
        app.order.push(id);
        app.active = Some(id);
    }

    fn sample_diff(path: &std::path::Path, source: DiffSource) -> DiffView {
        DiffView {
            document_id: DocumentId(0),
            path: Some(path.to_path_buf()),
            source,
            lines: Vec::new(),
            parsed: None,
            allow_hunks: true,
            error: None,
            scroll: 0,
            loading: false,
        }
    }

    #[test]
    fn toggle_diff_opens_the_changed_side() {
        with_runtime(|| {
            let mut app = test_app();
            app.git_repo = true;
            let path = PathBuf::from("/repo/src/main.rs");
            open_saved(&mut app, path.clone());
            app.git_status.insert(
                path.clone(),
                GitPathStatus {
                    staged: None,
                    unstaged: Some(GitFileStatus::Modified),
                },
            );
            app.toggle_diff();
            let diff = app.diff_view.as_ref().expect("unstaged diff");
            assert_eq!(diff.source, DiffSource::Unstaged);
            assert_eq!(diff.path.as_deref(), Some(path.as_path()));

            app.toggle_diff();
            assert!(app.diff_view.is_none());

            app.git_status.insert(
                path.clone(),
                GitPathStatus {
                    staged: Some(GitFileStatus::Modified),
                    unstaged: None,
                },
            );
            app.toggle_diff();
            assert_eq!(
                app.diff_view.as_ref().expect("staged diff").source,
                DiffSource::Staged
            );

            app.diff_view = None;
            app.git_status.insert(
                path.clone(),
                GitPathStatus {
                    staged: Some(GitFileStatus::Modified),
                    unstaged: Some(GitFileStatus::Modified),
                },
            );
            app.toggle_diff();
            assert_eq!(
                app.diff_view.as_ref().expect("worktree diff").source,
                DiffSource::Unstaged
            );
        });
    }

    #[test]
    fn open_path_closes_the_diff_and_changes_reopens_it() {
        with_runtime(|| {
            let mut app = test_app();
            let path = PathBuf::from("/repo/src/main.rs");
            open_saved(&mut app, path.clone());
            app.diff_view = Some(sample_diff(&path, DiffSource::Unstaged));
            app.open_path(path.clone());
            assert!(app.diff_view.is_none());

            let file = std::env::temp_dir().join(format!("r3vu-diff-{}", std::process::id()));
            std::fs::write(&file, "hello\n").unwrap();
            app.diff_view = Some(sample_diff(&path, DiffSource::Unstaged));
            app.open_path(file.clone());
            assert!(app.diff_view.is_none());
            assert!(
                app.documents
                    .values()
                    .any(|doc| doc.path.as_deref() == Some(file.as_path()))
            );

            app.open_git_entry(file.clone(), true);
            let diff = app.diff_view.as_ref().expect("changes diff");
            assert_eq!(diff.source, DiffSource::Staged);
            assert_eq!(diff.path.as_deref(), Some(file.as_path()));
            let _ = std::fs::remove_file(file);
        });
    }

    #[test]
    fn a_plain_folder_hides_git_tools() {
        with_runtime(|| {
            let mut app = test_app();
            let path = PathBuf::from("/repo/src/main.rs");
            open_saved(&mut app, path.clone());
            app.git_open = true;
            app.git_branch = Some("main".into());
            app.git_repo = true;
            app.diff_view = Some(sample_diff(&path, DiffSource::Unstaged));
            app.receive_git_status(Err(GitStatusError::NotARepository));
            assert!(!app.git_repo);
            assert!(!app.git_open);
            assert!(app.git_branch.is_none());
            assert!(app.diff_view.is_none());
            assert!(app.inline_changes.is_empty());
            app.toggle_diff();
            app.toggle_git();
            assert!(app.diff_view.is_none());
            assert!(!app.git_open);

            app.git_repo = true;
            app.git_branch = Some("main".into());
            app.receive_git_status(Err(GitStatusError::Unavailable));
            assert!(app.git_repo);
            assert_eq!(app.git_branch.as_deref(), Some("main"));

            app.git_repo = false;
            app.receive_git_status(Ok(GitSnapshot {
                files: HashMap::new(),
                branch: Some("main".into()),
            }));
            assert!(app.git_repo);
            assert_eq!(app.git_branch.as_deref(), Some("main"));
        });
    }

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
