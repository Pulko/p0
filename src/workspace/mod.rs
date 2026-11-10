use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ignore::WalkBuilder;
use notify::event::{EventKind, ModifyKind};
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Nucleo};
use tokio::process::Command;
use tokio::sync::mpsc;

use ropey::Rope;

use crate::editor::{Position, Range, editor_content_lines, editor_lines, line_visible_chars};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitFileStatus {
    Modified,
    Added,
    Deleted,
    Untracked,
    Renamed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct GitPathStatus {
    pub staged: Option<GitFileStatus>,
    pub unstaged: Option<GitFileStatus>,
}

impl GitPathStatus {
    pub fn summary(self) -> Option<GitFileStatus> {
        self.unstaged.or(self.staged)
    }
}

pub struct GitSnapshot {
    pub files: HashMap<PathBuf, GitPathStatus>,
    pub branch: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitStatusError {
    NotARepository,
    Unavailable,
}

pub type GitStatusUpdate = Result<GitSnapshot, GitStatusError>;

pub fn git_status_error(stderr: &str) -> GitStatusError {
    if stderr.to_ascii_lowercase().contains("not a git repository") {
        GitStatusError::NotARepository
    } else {
        GitStatusError::Unavailable
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchKind {
    Create,
    Remove,
    Modify,
}

impl WatchKind {
    pub fn changes_tree(self) -> bool {
        matches!(self, WatchKind::Create | WatchKind::Remove)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchEvent {
    pub path: PathBuf,
    pub kind: WatchKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKind {
    Header,
    Equal,
    Insert,
    Delete,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
    pub hunk: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineMark {
    Unchanged,
    Added,
    Modified,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct LineChanges {
    pub marks: Vec<LineMark>,
    pub deleted_before: Vec<usize>,
    pub removed_before: Vec<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub start: usize,
    pub end: usize,
    pub removed: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VisualRow {
    Line(usize),
    Removed { text: String, anchor: usize },
    Eof { anchor: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HunkReplacement {
    Lines {
        start: usize,
        end: usize,
        text: String,
    },
    ThroughEnd {
        start: usize,
        text: String,
    },
    AtEnd {
        text: String,
    },
}

impl HunkReplacement {
    pub fn text(&self) -> &str {
        match self {
            HunkReplacement::Lines { text, .. }
            | HunkReplacement::ThroughEnd { text, .. }
            | HunkReplacement::AtEnd { text } => text,
        }
    }
}

#[derive(Clone, Debug)]
pub enum DiffBaseline {
    Index(String),
    Absent,
}

impl DiffBaseline {
    pub fn original(&self) -> &str {
        match self {
            DiffBaseline::Index(text) => text,
            DiffBaseline::Absent => "",
        }
    }
}

#[derive(Clone, Debug)]
pub struct GitDiffResult {
    pub document_id: crate::editor::DocumentId,
    pub baseline: Option<DiffBaseline>,
}

#[derive(Clone, Debug)]
pub struct ExplorerEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub depth: usize,
    pub expanded: bool,
}

#[derive(Clone, Debug)]
pub struct SearchMatch {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub text: String,
}

pub struct WorkspaceSearch {
    pub generation: u64,
    pub matches: Vec<SearchMatch>,
    pub message: Option<String>,
}

pub struct FileFinder {
    matcher: Nucleo<String>,
    last_query: String,
}

impl FileFinder {
    pub fn new() -> Self {
        Self {
            matcher: Nucleo::new(Config::DEFAULT, Arc::new(|| {}), Some(1), 1),
            last_query: String::new(),
        }
    }

    pub fn set_files(&mut self, files: &[String]) {
        self.matcher.restart(true);
        let injector = self.matcher.injector();
        for file in files {
            injector.push(file.clone(), |item, cols| {
                cols[0] = item.as_str().into();
            });
        }
        self.last_query.clear();
    }

    pub fn query(&mut self, query: &str) -> Vec<String> {
        let append = query.starts_with(&self.last_query) && !self.last_query.is_empty();
        self.matcher
            .pattern
            .reparse(0, query, CaseMatching::Smart, Normalization::Smart, append);
        self.last_query = query.to_string();
        self.matcher.tick(20);
        let snap = self.matcher.snapshot();
        snap.matched_items(..)
            .take(200)
            .map(|item| item.data.clone())
            .collect()
    }
}

pub fn list_workspace_files(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .build();
    for entry in walker.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                let rel = rel.to_string_lossy().replace('\\', "/");
                if rel.starts_with(".git/") {
                    continue;
                }
                files.push(rel);
            }
        }
    }
    files.sort();
    files
}

pub fn load_directory(
    root: &Path,
    dir: &Path,
    expanded: &std::collections::HashSet<PathBuf>,
) -> Vec<ExplorerEntry> {
    let mut entries = Vec::new();
    collect_dir(root, dir, 0, expanded, &mut entries);
    entries
}

fn collect_dir(
    _root: &Path,
    dir: &Path,
    depth: usize,
    expanded: &std::collections::HashSet<PathBuf>,
    out: &mut Vec<ExplorerEntry>,
) {
    let mut children: Vec<_> = WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(false)
        .git_ignore(true)
        .filter_entry(|e| e.file_name() != ".git")
        .build()
        .flatten()
        .filter(|e| e.path() != dir)
        .collect();
    children.sort_by(|a, b| {
        let ad = a.path().is_dir();
        let bd = b.path().is_dir();
        bd.cmp(&ad).then_with(|| a.file_name().cmp(b.file_name()))
    });
    for child in children {
        let path = child.path().to_path_buf();
        let is_dir = path.is_dir();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name == "." || name == ".." {
            continue;
        }
        let is_expanded = is_dir && expanded.contains(&path);
        out.push(ExplorerEntry {
            path: path.clone(),
            name,
            is_dir,
            depth,
            expanded: is_expanded,
        });
        if is_expanded {
            collect_dir(_root, &path, depth + 1, expanded, out);
        }
    }
}

pub fn create_path(path: &Path, is_dir: bool) -> io::Result<()> {
    if is_dir {
        fs::create_dir_all(path)
    } else {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map(|_| ())
    }
}

pub fn rename_path(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

pub fn delete_path(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

pub fn spawn_git_status(root: PathBuf, tx: mpsc::UnboundedSender<GitStatusUpdate>) {
    tokio::spawn(async move {
        let output = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
            .output()
            .await;
        let update = match output {
            Ok(output) if output.status.success() => Ok(GitSnapshot {
                files: parse_git_status(&root, &output.stdout),
                branch: git_branch(&root).await,
            }),
            Ok(output) => {
                let message = String::from_utf8_lossy(&output.stderr);
                tracing::warn!("git status exited {}: {}", output.status, message.trim());
                Err(git_status_error(&message))
            }
            Err(error) => {
                tracing::warn!("git status failed: {error}");
                Err(GitStatusError::Unavailable)
            }
        };
        let _ = tx.send(update);
    });
}

async fn git_branch(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

pub fn classify_watch_kind(kind: EventKind) -> Option<WatchKind> {
    match kind {
        EventKind::Create(_) => Some(WatchKind::Create),
        EventKind::Remove(_) => Some(WatchKind::Remove),
        EventKind::Modify(ModifyKind::Name(_)) => Some(WatchKind::Create),
        EventKind::Modify(_) => Some(WatchKind::Modify),
        EventKind::Any => Some(WatchKind::Modify),
        EventKind::Other => Some(WatchKind::Create),
        EventKind::Access(_) => None,
    }
}

pub fn is_watch_noise(path: &Path) -> bool {
    if path
        .components()
        .any(|component| component.as_os_str() == ".git")
    {
        return true;
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".lock"))
}

pub fn accept_watch_path(root: &Path, path: &Path, ignore: &mut ignore::IncrementalIgnore) -> bool {
    if is_watch_noise(path) {
        return false;
    }
    let relative = if let Ok(relative) = path.strip_prefix(root) {
        relative.to_path_buf()
    } else if let Some(relative) = ignore.normalize(path) {
        relative
    } else {
        return false;
    };
    if relative.as_os_str().is_empty() {
        return false;
    }
    !ignore.matched(&relative, path.is_dir()).is_ignore()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirectoryStatus {
    pub kind: GitFileStatus,
    pub staged: bool,
}

pub fn directory_status(
    statuses: &HashMap<PathBuf, GitPathStatus>,
    dir: &Path,
) -> Option<DirectoryStatus> {
    let mut unstaged = None;
    let mut staged = None;
    for (path, status) in statuses {
        if !path.starts_with(dir) {
            continue;
        }
        if let Some(kind) = status.unstaged {
            unstaged = Some(prefer_status(unstaged, kind));
        }
        if let Some(kind) = status.staged {
            staged = Some(prefer_status(staged, kind));
        }
    }
    if let Some(kind) = unstaged {
        Some(DirectoryStatus {
            kind,
            staged: false,
        })
    } else {
        staged.map(|kind| DirectoryStatus { kind, staged: true })
    }
}

fn prefer_status(current: Option<GitFileStatus>, next: GitFileStatus) -> GitFileStatus {
    match current {
        Some(status) if status_rank(status) <= status_rank(next) => status,
        _ => next,
    }
}

fn status_rank(status: GitFileStatus) -> u8 {
    match status {
        GitFileStatus::Modified => 0,
        GitFileStatus::Added => 1,
        GitFileStatus::Deleted => 2,
        GitFileStatus::Renamed => 3,
        GitFileStatus::Untracked => 4,
    }
}

pub fn watch_matcher(root: &Path) -> ignore::IncrementalIgnore {
    let mut matchers = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build_matchers();
    matchers
        .pop()
        .expect("watch matcher for the workspace root")
}

pub fn parse_git_status(root: &Path, output: &[u8]) -> HashMap<PathBuf, GitPathStatus> {
    let mut result = HashMap::new();
    let mut records = output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty());
    while let Some(record) = records.next() {
        if record.len() < 3 {
            continue;
        }
        let x = record[0];
        let y = record[1];
        let path_at = if record.get(2) == Some(&b' ') { 3 } else { 2 };
        let first = String::from_utf8_lossy(&record[path_at..]).into_owned();
        let renamed = matches!(x, b'R' | b'C') || matches!(y, b'R' | b'C');
        let path = if renamed {
            let Some(next) = records.next() else {
                continue;
            };
            String::from_utf8_lossy(next).into_owned()
        } else {
            first
        };
        if path.is_empty() {
            continue;
        }
        let Some(status) = path_status(x, y) else {
            continue;
        };
        result.insert(root.join(path), status);
    }
    result
}

fn path_status(x: u8, y: u8) -> Option<GitPathStatus> {
    if x == b'?' && y == b'?' {
        return Some(GitPathStatus {
            staged: None,
            unstaged: Some(GitFileStatus::Untracked),
        });
    }
    let staged = column_status(x);
    let unstaged = column_status(y);
    if staged.is_none() && unstaged.is_none() {
        None
    } else {
        Some(GitPathStatus { staged, unstaged })
    }
}

fn column_status(byte: u8) -> Option<GitFileStatus> {
    match byte {
        b'M' | b'T' | b'U' => Some(GitFileStatus::Modified),
        b'A' => Some(GitFileStatus::Added),
        b'D' => Some(GitFileStatus::Deleted),
        b'R' | b'C' => Some(GitFileStatus::Renamed),
        _ => None,
    }
}

pub fn repo_relative(root: &Path, path: &Path) -> Option<String> {
    let relative = if path.is_absolute() {
        path.strip_prefix(root).ok()?
    } else {
        path
    };
    if relative.as_os_str().is_empty() || relative.is_absolute() {
        return None;
    }
    if relative
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return None;
    }
    let text = relative.to_string_lossy().replace('\\', "/");
    if text.is_empty() || text.starts_with('/') {
        None
    } else {
        Some(text)
    }
}

pub fn can_commit(message: &str, files: &HashMap<PathBuf, GitPathStatus>) -> bool {
    !message.trim().is_empty() && files.values().any(|status| status.staged.is_some())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitWrite {
    Stage(PathBuf),
    Unstage(PathBuf),
    StageAll,
    UnstageAll,
    Apply {
        patch: String,
        cached: bool,
        reverse: bool,
        reload: Option<PathBuf>,
    },
    Commit(String),
}

pub struct GitArgv {
    pub args: Vec<String>,
    pub stdin: Option<String>,
}

pub fn prepare_git_write(root: &Path, write: &GitWrite) -> Result<GitArgv, String> {
    match write {
        GitWrite::Stage(path) => {
            let rel = repo_relative(root, path)
                .ok_or_else(|| "Path is outside the project".to_string())?;
            Ok(GitArgv {
                args: vec!["add".into(), "--".into(), rel],
                stdin: None,
            })
        }
        GitWrite::Unstage(path) => {
            let rel = repo_relative(root, path)
                .ok_or_else(|| "Path is outside the project".to_string())?;
            Ok(GitArgv {
                args: vec!["restore".into(), "--staged".into(), "--".into(), rel],
                stdin: None,
            })
        }
        GitWrite::StageAll => Ok(GitArgv {
            args: vec!["add".into(), "-A".into()],
            stdin: None,
        }),
        GitWrite::UnstageAll => Ok(GitArgv {
            args: vec!["restore".into(), "--staged".into(), ":/".into()],
            stdin: None,
        }),
        GitWrite::Apply {
            patch,
            cached,
            reverse,
            ..
        } => {
            let mut args = vec!["apply".into()];
            if *reverse {
                args.push("--reverse".into());
            }
            if *cached {
                args.push("--cached".into());
            }
            args.push("-".into());
            Ok(GitArgv {
                args,
                stdin: Some(patch.clone()),
            })
        }
        GitWrite::Commit(message) => {
            let message = message.trim();
            if message.is_empty() {
                return Err("Write a commit message".into());
            }
            if message.contains('\0') {
                return Err("Commit message is invalid".into());
            }
            Ok(GitArgv {
                args: vec!["commit".into(), "-m".into(), message.to_string()],
                stdin: None,
            })
        }
    }
}

#[derive(Clone, Debug)]
pub struct GitWriteResult {
    pub ok: bool,
    pub message: String,
    pub reload: Option<PathBuf>,
    pub committed: Option<String>,
}

pub fn spawn_git_write(root: PathBuf, write: GitWrite, tx: mpsc::UnboundedSender<GitWriteResult>) {
    tokio::spawn(async move {
        let prepared = prepare_git_write(&root, &write);
        let mut result = match prepared {
            Ok(argv) => run_git(&root, &argv.args, argv.stdin).await,
            Err(message) => GitWriteResult {
                ok: false,
                message,
                reload: None,
                committed: None,
            },
        };
        if result.ok {
            match &write {
                GitWrite::Commit(message) => result.committed = Some(message.trim().to_string()),
                GitWrite::Apply { reload, .. } => result.reload = reload.clone(),
                _ => {}
            }
        }
        let _ = tx.send(result);
    });
}

async fn run_git(root: &Path, args: &[String], stdin_data: Option<String>) -> GitWriteResult {
    let mut command = Command::new("git");
    command.arg("-C").arg(root);
    for arg in args {
        command.arg(arg);
    }
    if let Some(data) = stdin_data {
        command.stdin(std::process::Stdio::piped());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        let spawned = command.spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => return git_failed(format!("Git unavailable: {error}")),
        };
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            if let Err(error) = stdin.write_all(data.as_bytes()).await {
                return git_failed(format!("Git unavailable: {error}"));
            }
        }
        match child.wait_with_output().await {
            Ok(output) => git_finished(output),
            Err(error) => git_failed(format!("Git unavailable: {error}")),
        }
    } else {
        match command.output().await {
            Ok(output) => git_finished(output),
            Err(error) => git_failed(format!("Git unavailable: {error}")),
        }
    }
}

fn git_finished(output: std::process::Output) -> GitWriteResult {
    if output.status.success() {
        GitWriteResult {
            ok: true,
            message: String::new(),
            reload: None,
            committed: None,
        }
    } else {
        let message = String::from_utf8_lossy(&output.stderr)
            .trim()
            .replace("fatal: ", "");
        git_failed(if message.is_empty() {
            format!("git exited {}", output.status)
        } else {
            message
        })
    }
}

fn git_failed(message: String) -> GitWriteResult {
    GitWriteResult {
        ok: false,
        message,
        reload: None,
        committed: None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHunk {
    pub header: String,
    pub lines: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedGitDiff {
    pub preamble: Vec<String>,
    pub hunks: Vec<GitHunk>,
}

pub fn parse_git_diff(text: &str) -> ParsedGitDiff {
    let mut preamble = Vec::new();
    let mut hunks = Vec::new();
    let mut current: Option<GitHunk> = None;
    // Split on `\n` only. `str::lines()` also strips a trailing `\r`, and that
    // byte is part of a CR or CRLF markdown line that `git apply` must match.
    for line in git_diff_lines(text) {
        if line.starts_with("@@") {
            if let Some(hunk) = current.take() {
                hunks.push(hunk);
            }
            current = Some(GitHunk {
                header: line.to_string(),
                lines: Vec::new(),
            });
        } else if let Some(hunk) = current.as_mut() {
            hunk.lines.push(line.to_string());
        } else {
            preamble.push(line.to_string());
        }
    }
    if let Some(hunk) = current {
        hunks.push(hunk);
    }
    ParsedGitDiff { preamble, hunks }
}

pub fn hunk_patch(diff: &ParsedGitDiff, index: usize) -> Option<String> {
    let hunk = diff.hunks.get(index)?;
    let mut out = String::new();
    for line in &diff.preamble {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&hunk.header);
    out.push('\n');
    for line in &hunk.lines {
        out.push_str(line);
        out.push('\n');
    }
    Some(out)
}

/// Diff rows split on LF only. A CR before that LF stays on the row.
fn git_diff_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            lines.push(&text[start..index]);
            start = index + 1;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

pub fn lines_from_git_diff(diff: &ParsedGitDiff) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    for line in &diff.preamble {
        lines.push(DiffLine {
            kind: DiffKind::Header,
            text: line.clone(),
            hunk: None,
        });
    }
    for (index, hunk) in diff.hunks.iter().enumerate() {
        let mut body = Vec::new();
        for line in &hunk.lines {
            body.extend(expand_diff_line(line));
        }
        let old_count = body
            .iter()
            .filter(|line| matches!(line.kind, DiffKind::Delete | DiffKind::Equal))
            .count();
        let new_count = body
            .iter()
            .filter(|line| matches!(line.kind, DiffKind::Insert | DiffKind::Equal))
            .count();
        lines.push(DiffLine {
            kind: DiffKind::Header,
            text: displayed_hunk_header(&hunk.header, old_count, new_count),
            hunk: Some(index),
        });
        lines.extend(body);
    }
    lines
}

fn expand_diff_line(line: &str) -> Vec<DiffLine> {
    if line.starts_with('\\') {
        return vec![DiffLine {
            kind: DiffKind::Header,
            text: line.to_string(),
            hunk: None,
        }];
    }
    let (kind, marker, content) = if let Some(content) = line.strip_prefix('+') {
        (DiffKind::Insert, "+", content)
    } else if let Some(content) = line.strip_prefix('-') {
        (DiffKind::Delete, "-", content)
    } else if let Some(content) = line.strip_prefix(' ') {
        (DiffKind::Equal, " ", content)
    } else {
        return vec![DiffLine {
            kind: DiffKind::Equal,
            text: line.to_string(),
            hunk: None,
        }];
    };
    let mut parts = editor_content_lines(content);
    // `+\r` is one empty CRLF row. Splitting treats that lone CR as a break
    // and would otherwise drop the row.
    if parts.is_empty() {
        parts.push(String::new());
    }
    parts
        .into_iter()
        .map(|part| DiffLine {
            kind,
            text: format!("{marker}{part}"),
            hunk: None,
        })
        .collect()
}

fn displayed_hunk_header(header: &str, old_count: usize, new_count: usize) -> String {
    let Some((old_start, parsed_old, new_start, parsed_new, tail)) = parse_hunk_header(header)
    else {
        return header.to_string();
    };
    if parsed_old == old_count && parsed_new == new_count {
        return header.to_string();
    }
    format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@{tail}")
}

fn parse_hunk_header(header: &str) -> Option<(usize, usize, usize, usize, &str)> {
    let rest = header.strip_prefix("@@")?.trim_start();
    let mut tokens = rest.split_whitespace();
    let old = tokens.next()?;
    let new = tokens.next()?;
    let (old_start, old_count) = parse_hunk_ref(old)?;
    let (new_start, new_count) = parse_hunk_ref(new)?;
    let tail_at = header[2..].find("@@")? + 2;
    let tail = header.get(tail_at + 2..)?;
    Some((old_start, old_count, new_start, new_count, tail))
}

fn parse_hunk_ref(token: &str) -> Option<(usize, usize)> {
    let token = token.trim_start_matches(['-', '+']);
    let mut parts = token.split(',');
    let start = parts.next()?.parse().ok()?;
    let count = match parts.next() {
        Some(count) => count.parse().ok()?,
        None => 1,
    };
    Some((start, count))
}

pub fn untracked_diff(relative: &str, text: &str) -> (Vec<DiffLine>, Option<ParsedGitDiff>) {
    if let Some(parsed) = new_file_diff(relative, text) {
        return (lines_from_git_diff(&parsed), Some(parsed));
    }
    let mut lines = vec![
        DiffLine {
            kind: DiffKind::Header,
            text: "--- /dev/null".into(),
            hunk: None,
        },
        DiffLine {
            kind: DiffKind::Header,
            text: "+++ new file".into(),
            hunk: None,
        },
    ];
    for line in editor_content_lines(text) {
        lines.push(DiffLine {
            kind: DiffKind::Insert,
            text: format!("+{line}"),
            hunk: None,
        });
    }
    (lines, None)
}

/// A new-file patch `git apply` can stage, unstage, and discard.
/// Rows stay split on LF only so a trailing CR still matches the file.
fn new_file_diff(relative: &str, text: &str) -> Option<ParsedGitDiff> {
    if text.is_empty()
        || relative.is_empty()
        || relative.contains(['\n', '\r', '\0'])
        || relative.starts_with('/')
    {
        return None;
    }
    let rows = git_diff_lines(text);
    let count = rows.len();
    if count == 0 {
        return None;
    }
    let header = if count == 1 {
        "@@ -0,0 +1 @@".to_string()
    } else {
        format!("@@ -0,0 +1,{count} @@")
    };
    let mut lines: Vec<String> = rows.into_iter().map(|line| format!("+{line}")).collect();
    if !text.ends_with('\n') {
        lines.push("\\ No newline at end of file".into());
    }
    Some(ParsedGitDiff {
        preamble: vec![
            format!("diff --git a/{relative} b/{relative}"),
            "new file mode 100644".into(),
            "--- /dev/null".into(),
            format!("+++ b/{relative}"),
        ],
        hunks: vec![GitHunk { header, lines }],
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitPanelRow {
    Summary,
    Message,
    Commit,
    Section {
        staged: bool,
    },
    File {
        path: PathBuf,
        status: GitFileStatus,
        staged: bool,
    },
}

pub fn git_panel_rows(files: &HashMap<PathBuf, GitPathStatus>) -> Vec<GitPanelRow> {
    let mut staged = Vec::new();
    let mut changed = Vec::new();
    for (path, status) in files {
        if let Some(kind) = status.staged {
            staged.push((path.clone(), kind));
        }
        if let Some(kind) = status.unstaged {
            changed.push((path.clone(), kind));
        }
    }
    staged.sort_by(|left, right| left.0.cmp(&right.0));
    changed.sort_by(|left, right| left.0.cmp(&right.0));
    let mut rows = vec![
        GitPanelRow::Summary,
        GitPanelRow::Message,
        GitPanelRow::Commit,
        GitPanelRow::Section { staged: true },
    ];
    for (path, status) in staged {
        rows.push(GitPanelRow::File {
            path,
            status,
            staged: true,
        });
    }
    rows.push(GitPanelRow::Section { staged: false });
    for (path, status) in changed {
        rows.push(GitPanelRow::File {
            path,
            status,
            staged: false,
        });
    }
    rows
}

#[derive(Clone, Debug)]
pub struct FileDiffResult {
    pub token: u64,
    pub path: PathBuf,
    pub staged: bool,
    pub lines: Vec<DiffLine>,
    pub parsed: Option<ParsedGitDiff>,
    pub allow_hunks: bool,
    pub error: Option<String>,
}

pub fn spawn_file_diff(
    root: PathBuf,
    path: PathBuf,
    staged: bool,
    allow_hunks: bool,
    token: u64,
    tx: mpsc::UnboundedSender<FileDiffResult>,
) {
    tokio::spawn(async move {
        let result = match repo_relative(&root, &path) {
            None => FileDiffResult {
                token,
                path,
                staged,
                lines: Vec::new(),
                parsed: None,
                allow_hunks: false,
                error: Some("Path is outside the project".into()),
            },
            Some(relative) => {
                let mut args = vec![
                    "diff".to_string(),
                    "-U3".into(),
                    "--no-ext-diff".into(),
                    "--no-color".into(),
                ];
                if staged {
                    args.push("--cached".into());
                }
                args.push("--".into());
                args.push(relative);
                match Command::new("git")
                    .arg("-C")
                    .arg(&root)
                    .args(&args)
                    .output()
                    .await
                {
                    Ok(output) if output.status.success() => {
                        let text = String::from_utf8_lossy(&output.stdout).into_owned();
                        let parsed = parse_git_diff(&text);
                        let has_hunks = !parsed.hunks.is_empty();
                        let lines = lines_from_git_diff(&parsed);
                        FileDiffResult {
                            token,
                            path,
                            staged,
                            lines,
                            parsed: (allow_hunks && has_hunks).then_some(parsed),
                            allow_hunks: allow_hunks && has_hunks,
                            error: None,
                        }
                    }
                    Ok(output) => {
                        let message = String::from_utf8_lossy(&output.stderr)
                            .trim()
                            .replace("fatal: ", "");
                        FileDiffResult {
                            token,
                            path,
                            staged,
                            lines: Vec::new(),
                            parsed: None,
                            allow_hunks: false,
                            error: Some(if message.is_empty() {
                                format!("git exited {}", output.status)
                            } else {
                                message
                            }),
                        }
                    }
                    Err(error) => FileDiffResult {
                        token,
                        path,
                        staged,
                        lines: Vec::new(),
                        parsed: None,
                        allow_hunks: false,
                        error: Some(format!("Git unavailable: {error}")),
                    },
                }
            }
        };
        let _ = tx.send(result);
    });
}

pub fn path_absent_from_head(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("exists on disk, but not in") || message.contains("does not exist in")
}

pub fn spawn_git_diff(
    root: PathBuf,
    document_id: crate::editor::DocumentId,
    path: PathBuf,
    tx: mpsc::UnboundedSender<GitDiffResult>,
) {
    tokio::spawn(async move {
        let relative = path.strip_prefix(&root).unwrap_or(&path);
        let spec = format!(":{}", relative.to_string_lossy().replace('\\', "/"));
        let output = Command::new("git")
            .arg("-C")
            .arg(&root)
            .arg("show")
            .arg(spec)
            .output()
            .await;
        let result = match output {
            Ok(output) if output.status.success() => {
                let original = String::from_utf8_lossy(&output.stdout).into_owned();
                GitDiffResult {
                    document_id,
                    baseline: Some(DiffBaseline::Index(original)),
                }
            }
            Ok(output) => {
                let message = String::from_utf8_lossy(&output.stderr)
                    .trim()
                    .to_string()
                    .replace("fatal: ", "");
                GitDiffResult {
                    document_id,
                    baseline: path_absent_from_head(&message).then_some(DiffBaseline::Absent),
                }
            }
            Err(_) => GitDiffResult {
                document_id,
                baseline: None,
            },
        };
        let _ = tx.send(result);
    });
}

pub fn classify_lines(original: &str, current: &str) -> LineChanges {
    use similar::{ChangeTag, TextDiff};

    let old_lines = split_lines(original);
    let new_lines = split_lines(current);
    let old_refs: Vec<&str> = old_lines.iter().map(String::as_str).collect();
    let new_refs: Vec<&str> = new_lines.iter().map(String::as_str).collect();
    let diff = TextDiff::from_slices(&old_refs, &new_refs);
    let mut marks = vec![LineMark::Unchanged; new_lines.len()];
    let mut deleted_before = vec![0; new_lines.len() + 1];
    let mut removed_before = vec![Vec::new(); new_lines.len() + 1];
    let mut index = 0;
    let mut pending_deletes = 0usize;
    let mut pending_text: Vec<String> = Vec::new();
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                removed_before[index].append(&mut pending_text);
                deleted_before[index] += pending_deletes;
                pending_deletes = 0;
                index += 1;
            }
            ChangeTag::Delete => {
                pending_text.push(diff_line_text(change.value()));
                pending_deletes += 1;
            }
            ChangeTag::Insert => {
                if pending_deletes > 0 {
                    pending_deletes -= 1;
                    if !pending_text.is_empty() {
                        removed_before[index].push(pending_text.remove(0));
                    }
                    marks[index] = LineMark::Modified;
                } else {
                    marks[index] = LineMark::Added;
                }
                index += 1;
            }
        }
    }
    if index < deleted_before.len() {
        removed_before[index].append(&mut pending_text);
        deleted_before[index] += pending_deletes;
    }
    LineChanges {
        marks,
        deleted_before,
        removed_before,
    }
}

fn diff_line_text(value: &str) -> String {
    value.trim_end_matches(['\n', '\r']).to_string()
}

pub fn change_hunks(changes: &LineChanges) -> Vec<Hunk> {
    let mut hunks = Vec::new();
    let mut index = 0;
    while index < changes.marks.len() {
        if changes.deleted_before[index] > 0 && changes.marks[index] == LineMark::Unchanged {
            hunks.push(Hunk {
                start: index,
                end: index,
                removed: removed_at(changes, index),
            });
            index += 1;
            continue;
        }
        if changes.marks[index] != LineMark::Unchanged {
            let start = index;
            let mut removed = removed_at(changes, index);
            index += 1;
            while index < changes.marks.len() && changes.marks[index] != LineMark::Unchanged {
                removed.extend(removed_at(changes, index));
                index += 1;
            }
            hunks.push(Hunk {
                start,
                end: index,
                removed,
            });
            continue;
        }
        index += 1;
    }
    if changes
        .deleted_before
        .last()
        .copied()
        .is_some_and(|deleted| deleted > 0)
    {
        let start = changes.marks.len();
        hunks.push(Hunk {
            start,
            end: start,
            removed: removed_at(changes, start),
        });
    }
    hunks
}

fn removed_at(changes: &LineChanges, index: usize) -> Vec<String> {
    changes
        .removed_before
        .get(index)
        .cloned()
        .unwrap_or_default()
}

pub fn hunk_replacement(
    hunk: &Hunk,
    line_count: usize,
    ends_with_newline: bool,
) -> HunkReplacement {
    if hunk.start >= line_count {
        let mut text = String::new();
        if line_count > 0 && !ends_with_newline && !hunk.removed.is_empty() {
            text.push('\n');
        }
        if !hunk.removed.is_empty() {
            text.push_str(&hunk.removed.join("\n"));
        }
        return HunkReplacement::AtEnd { text };
    }
    if hunk.end >= line_count {
        return HunkReplacement::ThroughEnd {
            start: hunk.start,
            text: hunk.removed.join("\n"),
        };
    }
    let mut text = String::new();
    for line in &hunk.removed {
        text.push_str(line);
        text.push('\n');
    }
    HunkReplacement::Lines {
        start: hunk.start,
        end: hunk.end,
        text,
    }
}

pub fn hunk_edit_range(rope: &Rope, replacement: &HunkReplacement) -> Range {
    match replacement {
        HunkReplacement::Lines { start, end, .. } => {
            Range::new(Position::new(*start, 0), Position::new(*end, 0))
        }
        HunkReplacement::ThroughEnd { start, .. } => {
            Range::new(Position::new(*start, 0), rope_end(rope))
        }
        HunkReplacement::AtEnd { .. } => {
            let end = rope_end(rope);
            Range::new(end, end)
        }
    }
}

fn rope_end(rope: &Rope) -> Position {
    if rope.len_chars() == 0 {
        return Position::default();
    }
    let line = rope.len_lines() - 1;
    Position::new(line, line_visible_chars(rope, line))
}

pub fn visual_rows(line_count: usize, hunks: &[Hunk], open: &[usize]) -> Vec<VisualRow> {
    let mut rows = Vec::new();
    let mut index = 0;
    while index < line_count {
        if let Some(hunk) = hunks
            .iter()
            .find(|hunk| hunk.start == index && open.contains(&hunk.start))
        {
            push_removed(&mut rows, hunk);
        }
        rows.push(VisualRow::Line(index));
        index += 1;
    }
    if let Some(hunk) = hunks
        .iter()
        .find(|hunk| hunk.start == line_count && hunk.end == line_count)
    {
        if open.contains(&hunk.start) {
            push_removed(&mut rows, hunk);
        }
        rows.push(VisualRow::Eof { anchor: line_count });
    }
    rows
}

fn push_removed(rows: &mut Vec<VisualRow>, hunk: &Hunk) {
    for line in &hunk.removed {
        rows.push(VisualRow::Removed {
            text: line.clone(),
            anchor: hunk.start,
        });
    }
}

pub fn visual_index_for_line(rows: &[VisualRow], line: usize) -> Option<usize> {
    rows.iter()
        .position(|row| matches!(row, VisualRow::Line(index) if *index == line))
}

pub fn buffer_line_at(rows: &[VisualRow], visual_index: usize) -> Option<usize> {
    match rows.get(visual_index)? {
        VisualRow::Line(line) => Some(*line),
        VisualRow::Removed { anchor, .. } | VisualRow::Eof { anchor } => Some(*anchor),
    }
}

fn split_lines(text: &str) -> Vec<String> {
    editor_lines(text)
}

const MAX_SEARCH_HITS: usize = 400;
const MAX_SEARCH_FILE: u64 = 2 * 1024 * 1024;

pub fn spawn_workspace_search(
    root: PathBuf,
    query: String,
    generation: u64,
    tx: mpsc::UnboundedSender<WorkspaceSearch>,
) {
    tokio::spawn(async move {
        if query.trim().is_empty() {
            let _ = tx.send(WorkspaceSearch {
                generation,
                matches: Vec::new(),
                message: None,
            });
            return;
        }
        let scanned = tokio::task::spawn_blocking(move || search_workspace(&root, &query)).await;
        let _ = tx.send(WorkspaceSearch {
            generation,
            matches: scanned.unwrap_or_default(),
            message: None,
        });
    });
}

pub fn search_workspace(root: &Path, query: &str) -> Vec<SearchMatch> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    let mut matches = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .filter_entry(|entry| !skipped_search_name(entry.file_name()))
        .build();
    for entry in walker.flatten() {
        if matches.len() >= MAX_SEARCH_HITS {
            break;
        }
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(meta) = path.metadata() else {
            continue;
        };
        if meta.len() > MAX_SEARCH_FILE {
            continue;
        }
        let Ok(bytes) = fs::read(path) else {
            continue;
        };
        if bytes.contains(&0) {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        for (start, _) in find_in_document(&text, query, true) {
            if matches.len() >= MAX_SEARCH_HITS {
                break;
            }
            let line_text = lines
                .get(start.line)
                .copied()
                .unwrap_or("")
                .trim_end()
                .to_string();
            matches.push(SearchMatch {
                path: path.to_path_buf(),
                line: start.line,
                column: start.column,
                text: line_text,
            });
        }
    }
    matches
}

fn skipped_search_name(name: &std::ffi::OsStr) -> bool {
    name == ".git" || name == "node_modules"
}

pub fn find_in_document(
    text: &str,
    query: &str,
    case_insensitive: bool,
) -> Vec<(Position, Position)> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut matches = Vec::new();
    let hay = if case_insensitive {
        text.to_lowercase()
    } else {
        text.to_string()
    };
    let needle = if case_insensitive {
        query.to_lowercase()
    } else {
        query.to_string()
    };
    for (line_idx, line) in hay.lines().enumerate() {
        let mut start = 0;
        while let Some(found) = line[start..].find(&needle) {
            let col = start + found;
            let char_start = line[..col].chars().count();
            let char_end = char_start + query.chars().count();
            matches.push((
                Position::new(line_idx, char_start),
                Position::new(line_idx, char_end),
            ));
            start = col + needle.len().max(1);
        }
    }
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_search_skips_node_modules() {
        let root = std::env::temp_dir().join(format!("p0-search-{}", uuid::Uuid::new_v4()));
        let source = root.join("src");
        let vendor = root.join("node_modules").join("pkg");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&vendor).unwrap();
        std::fs::write(
            source.join("App.tsx"),
            "export const MainButtonProvider = 1;\n",
        )
        .unwrap();
        std::fs::write(
            vendor.join("index.js"),
            "export const MainButtonProvider = 2;\n",
        )
        .unwrap();

        let found = search_workspace(&root, "MainButtonProvider");
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(found.len(), 1);
        assert!(found[0].path.ends_with("src/App.tsx"));
        assert_eq!(found[0].line, 0);
        assert!(found[0].text.contains("MainButtonProvider"));
    }

    #[test]
    fn finds_unicode_and_multiple() {
        let text = "café café\nnaïve";
        let found = find_in_document(text, "café", true);
        assert_eq!(found.len(), 2);
        let none = find_in_document(text, "zzz", true);
        assert!(none.is_empty());
    }

    #[test]
    fn parses_porcelain_statuses() {
        let root = Path::new("/repo");
        let statuses = parse_git_status(
            root,
            b" M src/main.ts\0M  src/staged.ts\0MM src/both.ts\0A  src/new.ts\0D  old.ts\0 D gone.ts\0?? notes.txt\0R  from.ts\0to.ts\0",
        );
        let main = statuses.get(&root.join("src/main.ts")).copied().unwrap();
        assert_eq!(main.staged, None);
        assert_eq!(main.unstaged, Some(GitFileStatus::Modified));
        assert_eq!(main.summary(), Some(GitFileStatus::Modified));
        let staged = statuses.get(&root.join("src/staged.ts")).copied().unwrap();
        assert_eq!(staged.staged, Some(GitFileStatus::Modified));
        assert_eq!(staged.unstaged, None);
        let both = statuses.get(&root.join("src/both.ts")).copied().unwrap();
        assert_eq!(both.staged, Some(GitFileStatus::Modified));
        assert_eq!(both.unstaged, Some(GitFileStatus::Modified));
        assert_eq!(
            statuses.get(&root.join("src/new.ts")).unwrap().staged,
            Some(GitFileStatus::Added)
        );
        assert_eq!(
            statuses.get(&root.join("old.ts")).unwrap().staged,
            Some(GitFileStatus::Deleted)
        );
        assert_eq!(
            statuses.get(&root.join("gone.ts")).unwrap().unstaged,
            Some(GitFileStatus::Deleted)
        );
        assert_eq!(
            statuses.get(&root.join("notes.txt")).unwrap().unstaged,
            Some(GitFileStatus::Untracked)
        );
        assert!(statuses.get(&root.join("from.ts")).is_none());
        assert_eq!(
            statuses.get(&root.join("to.ts")).unwrap().staged,
            Some(GitFileStatus::Renamed)
        );
    }

    #[test]
    fn hunk_patch_keeps_the_file_header() {
        let text = "\
diff --git a/src/main.rs b/src/main.rs
index 111..222 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,3 @@
 keep
-old
+new
 keep
@@ -10,2 +10,2 @@
 a
-b
+c
";
        let parsed = parse_git_diff(text);
        assert_eq!(parsed.hunks.len(), 2);
        let patch = hunk_patch(&parsed, 1).unwrap();
        assert!(patch.contains("diff --git a/src/main.rs b/src/main.rs"));
        assert!(patch.contains("--- a/src/main.rs"));
        assert!(patch.contains("+++ b/src/main.rs"));
        assert!(patch.contains("@@ -10,2 +10,2 @@"));
        assert!(!patch.contains("@@ -1,3 +1,3 @@"));
        assert!(patch.contains("-b\n"));
        assert!(patch.contains("+c\n"));
        assert!(patch.ends_with("+c\n"));
    }

    #[test]
    fn shows_every_markdown_line_hidden_inside_a_git_line() {
        let text = "\
diff --git a/app/readme.md b/app/readme.md
new file mode 100644
--- /dev/null
+++ b/app/readme.md
@@ -0,0 +1,1 @@
+# r3vu\rA mouse-driven terminal IDE\r```bash
\\ No newline at end of file
";
        let parsed = parse_git_diff(text);
        assert_eq!(parsed.hunks[0].header, "@@ -0,0 +1,1 @@");
        let patch = hunk_patch(&parsed, 0).unwrap();
        assert!(patch.contains("@@ -0,0 +1,1 @@"));
        assert!(patch.contains("+# r3vu\rA mouse-driven terminal IDE\r```bash\n"));

        let lines = lines_from_git_diff(&parsed);
        let header = lines.iter().find(|line| line.hunk == Some(0)).unwrap();
        assert_eq!(header.text, "@@ -0,0 +1,3 @@");
        let inserts: Vec<_> = lines
            .iter()
            .filter(|line| line.kind == DiffKind::Insert)
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(
            inserts,
            vec!["+# r3vu", "+A mouse-driven terminal IDE", "+```bash",]
        );
        assert!(
            lines
                .iter()
                .any(|line| line.text == "\\ No newline at end of file")
        );

        let crlf = ParsedGitDiff {
            preamble: Vec::new(),
            hunks: vec![GitHunk {
                header: "@@ -0,0 +1 @@".into(),
                lines: vec!["+keep\r".into()],
            }],
        };
        let crlf_lines = lines_from_git_diff(&crlf);
        let crlf_inserts: Vec<_> = crlf_lines
            .iter()
            .filter(|line| line.kind == DiffKind::Insert)
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(crlf_inserts, vec!["+keep"]);
        assert_eq!(
            crlf_lines
                .iter()
                .find(|line| line.hunk == Some(0))
                .unwrap()
                .text,
            "@@ -0,0 +1 @@"
        );
        assert!(hunk_patch(&crlf, 0).unwrap().contains("+keep\r\n"));

        let cr = "\
diff --git a/readme.md b/readme.md
--- a/readme.md
+++ b/readme.md
@@ -1 +1 @@
-# r3vu\r\rA mouse.\r
\\ No newline at end of file
+# r3vu
+
+A mouse.
";
        let parsed_cr = parse_git_diff(cr);
        let cr_patch = hunk_patch(&parsed_cr, 0).unwrap();
        assert!(
            cr_patch.contains("-# r3vu\r\rA mouse.\r\n"),
            "stage patch dropped a carriage return: {cr_patch:?}"
        );
        let blank = ParsedGitDiff {
            preamble: Vec::new(),
            hunks: vec![GitHunk {
                header: "@@ -1,2 +1,2 @@".into(),
                lines: vec![" keep\r".into(), "+\r".into()],
            }],
        };
        let blank_rows: Vec<_> = lines_from_git_diff(&blank)
            .into_iter()
            .filter(|line| line.hunk.is_none() && line.kind != DiffKind::Header)
            .map(|line| line.text)
            .collect();
        assert_eq!(blank_rows, vec![" keep", "+"]);

        let (untracked, parsed_new) = untracked_diff(
            "app/readme.md",
            "# r3vu\rA mouse-driven terminal IDE\r```bash",
        );
        let untracked_inserts: Vec<_> = untracked
            .iter()
            .filter(|line| line.kind == DiffKind::Insert)
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(untracked_inserts, inserts);
        assert!(untracked.iter().any(|line| line.hunk == Some(0)));
        let new_patch = hunk_patch(parsed_new.as_ref().unwrap(), 0).unwrap();
        assert!(new_patch.contains("+++ b/app/readme.md\n"));
        assert!(new_patch.contains("+# r3vu\rA mouse-driven terminal IDE\r```bash\n"));
        assert!(new_patch.contains("\\ No newline at end of file\n"));

        let (lf_lines, lf_parsed) = untracked_diff("app/readme.md", "# r3vu\n\nA mouse.\n");
        assert!(lf_lines.iter().any(|line| line.hunk == Some(0)));
        let lf_patch = hunk_patch(lf_parsed.as_ref().unwrap(), 0).unwrap();
        assert!(lf_patch.contains("@@ -0,0 +1,3 @@\n+# r3vu\n+\n+A mouse.\n"));
        assert!(!lf_patch.contains("No newline"));
        let (empty_lines, empty_parsed) = untracked_diff("app/readme.md", "");
        assert!(empty_parsed.is_none());
        assert!(empty_lines.iter().all(|line| line.hunk.is_none()));

        let marks = classify_lines("", "# r3vu\rA mouse-driven terminal IDE\r```bash");
        assert_eq!(
            marks.marks,
            vec![LineMark::Added, LineMark::Added, LineMark::Added]
        );
    }

    #[test]
    fn rejects_a_path_that_escapes_the_project() {
        let root = Path::new("/repo");
        assert_eq!(
            repo_relative(root, Path::new("/repo/src/main.rs")).as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(
            repo_relative(root, Path::new("src/main.rs")).as_deref(),
            Some("src/main.rs")
        );
        assert!(repo_relative(root, Path::new("/etc/passwd")).is_none());
        assert!(repo_relative(root, Path::new("")).is_none());
        assert!(repo_relative(root, Path::new("..")).is_none());
        assert!(repo_relative(root, Path::new("src/../../etc/passwd")).is_none());
        assert!(repo_relative(root, root).is_none());
        let escaped =
            prepare_git_write(root, &GitWrite::Stage(PathBuf::from("/repo/../etc/passwd")));
        assert!(escaped.is_err());
        let commit = prepare_git_write(root, &GitWrite::Commit("  ".into()));
        assert!(commit.is_err());
        let ok = prepare_git_write(root, &GitWrite::Commit("Keep the review".into())).unwrap();
        assert_eq!(ok.args, ["commit", "-m", "Keep the review"]);
        assert!(!ok.args.iter().any(|arg| arg == "--no-verify"));
    }

    #[test]
    fn commit_needs_a_message_and_a_staged_file() {
        let mut files = HashMap::new();
        assert!(!can_commit("Keep it", &files));
        assert!(!can_commit("  ", &files));
        files.insert(
            PathBuf::from("a.ts"),
            GitPathStatus {
                staged: None,
                unstaged: Some(GitFileStatus::Modified),
            },
        );
        assert!(!can_commit("Keep it", &files));
        files.insert(
            PathBuf::from("b.ts"),
            GitPathStatus {
                staged: Some(GitFileStatus::Added),
                unstaged: None,
            },
        );
        assert!(!can_commit("   ", &files));
        assert!(can_commit("Keep it", &files));
    }

    #[test]
    fn panel_lists_a_partial_file_on_both_sides() {
        let mut files = HashMap::new();
        files.insert(
            PathBuf::from("both.ts"),
            GitPathStatus {
                staged: Some(GitFileStatus::Modified),
                unstaged: Some(GitFileStatus::Modified),
            },
        );
        files.insert(
            PathBuf::from("new.ts"),
            GitPathStatus {
                staged: None,
                unstaged: Some(GitFileStatus::Untracked),
            },
        );
        files.insert(
            PathBuf::from("done.ts"),
            GitPathStatus {
                staged: Some(GitFileStatus::Added),
                unstaged: None,
            },
        );
        let rows = git_panel_rows(&files);
        let staged: Vec<_> = rows
            .iter()
            .filter_map(|row| match row {
                GitPanelRow::File {
                    path, staged: true, ..
                } => Some(path.clone()),
                _ => None,
            })
            .collect();
        let changed: Vec<_> = rows
            .iter()
            .filter_map(|row| match row {
                GitPanelRow::File {
                    path,
                    staged: false,
                    ..
                } => Some(path.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            staged,
            vec![PathBuf::from("both.ts"), PathBuf::from("done.ts")]
        );
        assert_eq!(
            changed,
            vec![PathBuf::from("both.ts"), PathBuf::from("new.ts")]
        );
    }

    #[test]
    fn classifies_and_shows_removed_lines() {
        let changes = classify_lines("alpha\nbeta\ngamma\n", "alpha\nBETA\nextra\ngamma\n");
        assert_eq!(changes.marks[1], LineMark::Modified);
        assert_eq!(changes.marks[2], LineMark::Added);
        let hunks = change_hunks(&changes);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].start, 1);
        assert_eq!(hunks[0].end, 3);
        assert_eq!(hunks[0].removed, vec!["beta".to_string()]);
        let closed = visual_rows(changes.marks.len(), &hunks, &[]);
        assert!(closed.iter().all(|row| matches!(row, VisualRow::Line(_))));
        let open = visual_rows(changes.marks.len(), &hunks, &[1]);
        assert_eq!(open[0], VisualRow::Line(0));
        assert_eq!(
            open[1],
            VisualRow::Removed {
                text: "beta".into(),
                anchor: 1,
            }
        );
        assert_eq!(open[2], VisualRow::Line(1));
        assert_eq!(open[3], VisualRow::Line(2));
        assert_eq!(buffer_line_at(&open, 0), Some(0));
        assert_eq!(buffer_line_at(&open, 1), Some(1));
        assert_eq!(buffer_line_at(&open, 4), Some(3));
        assert_eq!(visual_index_for_line(&open, 3), Some(4));
    }

    #[test]
    fn shows_a_row_for_a_deletion_at_the_end() {
        let changes = classify_lines("keep\ngone", "keep");
        let hunks = change_hunks(&changes);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].start, 1);
        let closed = visual_rows(1, &hunks, &[]);
        assert_eq!(closed[0], VisualRow::Line(0));
        assert_eq!(closed[1], VisualRow::Eof { anchor: 1 });
        let open = visual_rows(1, &hunks, &[1]);
        assert_eq!(
            open[1],
            VisualRow::Removed {
                text: "gone".into(),
                anchor: 1,
            }
        );
        assert_eq!(open[2], VisualRow::Eof { anchor: 1 });
        assert_eq!(buffer_line_at(&open, 1), Some(1));
        assert_eq!(buffer_line_at(&open, 2), Some(1));
    }

    #[test]
    fn revert_restores_one_hunk() {
        assert_eq!(
            restored("alpha\nbeta\ngamma\n", "alpha\nBETA\ngamma\n"),
            "alpha\nbeta\ngamma\n"
        );
        assert_eq!(restored("alpha\nbeta", "alpha\nBETA"), "alpha\nbeta");
        assert_eq!(
            restored("alpha\ngamma\n", "alpha\nextra\ngamma\n"),
            "alpha\ngamma\n"
        );
        assert_eq!(
            restored("keep\ngone\nstay\n", "keep\nstay\n"),
            "keep\ngone\nstay\n"
        );
        assert_eq!(restored("a\n\nb\n", "a\nb\n"), "a\n\nb\n");
        assert_eq!(restored("keep\ngone", "keep"), "keep\ngone");
    }

    #[test]
    fn revert_edit_undoes_in_one_step() {
        let original = "alpha\nbeta\n";
        let current = "alpha\nBETA\n";
        let mut doc =
            crate::editor::Document::new(crate::editor::DocumentId(1), None, current.into());
        let changes = classify_lines(original, current);
        let hunks = change_hunks(&changes);
        let replacement = hunk_replacement(&hunks[0], changes.marks.len(), true);
        let range = hunk_edit_range(&doc.rope, &replacement);
        doc.apply_replace(range, replacement.text(), false);
        assert_eq!(doc.text(), original);
        doc.undo().unwrap();
        assert_eq!(doc.text(), current);
    }

    fn restored(original: &str, current: &str) -> String {
        let changes = classify_lines(original, current);
        let hunks = change_hunks(&changes);
        assert_eq!(hunks.len(), 1);
        let replacement = hunk_replacement(&hunks[0], changes.marks.len(), current.ends_with('\n'));
        let mut doc =
            crate::editor::Document::new(crate::editor::DocumentId(1), None, current.to_string());
        let range = hunk_edit_range(&doc.rope, &replacement);
        doc.apply_replace(range, replacement.text(), false);
        doc.text()
    }

    #[test]
    fn marks_deleted_lines_before_the_next_line() {
        let changes = classify_lines("keep\ngone\n", "keep\n");
        assert_eq!(changes.marks[0], LineMark::Unchanged);
        assert!(changes.deleted_before.iter().any(|count| *count > 0));
    }

    #[test]
    fn empty_baseline_marks_every_line_added() {
        let changes = classify_lines("", "alpha\nbeta");
        assert_eq!(changes.marks, vec![LineMark::Added, LineMark::Added]);
        assert!(changes.deleted_before.iter().all(|count| *count == 0));
    }

    #[test]
    fn folder_mark_prefers_modified_and_stays_inside_the_directory() {
        let root = Path::new("/repo");
        let mut statuses = HashMap::new();
        statuses.insert(
            root.join("src/notes.txt"),
            GitPathStatus {
                staged: None,
                unstaged: Some(GitFileStatus::Untracked),
            },
        );
        statuses.insert(
            root.join("src/main.ts"),
            GitPathStatus {
                staged: None,
                unstaged: Some(GitFileStatus::Modified),
            },
        );
        statuses.insert(
            root.join("src2/other.ts"),
            GitPathStatus {
                staged: Some(GitFileStatus::Added),
                unstaged: None,
            },
        );
        assert_eq!(
            directory_status(&statuses, &root.join("src")),
            Some(DirectoryStatus {
                kind: GitFileStatus::Modified,
                staged: false,
            })
        );
        assert_eq!(
            directory_status(&statuses, &root.join("src2")),
            Some(DirectoryStatus {
                kind: GitFileStatus::Added,
                staged: true,
            })
        );
        assert_eq!(directory_status(&statuses, &root.join("empty")), None);
    }

    #[test]
    fn classifies_watch_events_and_drops_noise() {
        use notify::event::{AccessKind, CreateKind, DataChange, RemoveKind, RenameMode};

        assert_eq!(
            classify_watch_kind(EventKind::Create(CreateKind::File)),
            Some(WatchKind::Create)
        );
        assert_eq!(
            classify_watch_kind(EventKind::Remove(RemoveKind::File)),
            Some(WatchKind::Remove)
        );
        assert_eq!(
            classify_watch_kind(EventKind::Modify(ModifyKind::Data(DataChange::Content))),
            Some(WatchKind::Modify)
        );
        assert_eq!(
            classify_watch_kind(EventKind::Modify(ModifyKind::Name(RenameMode::Any))),
            Some(WatchKind::Create)
        );
        assert_eq!(classify_watch_kind(EventKind::Any), Some(WatchKind::Modify));
        assert_eq!(
            classify_watch_kind(EventKind::Other),
            Some(WatchKind::Create)
        );
        assert_eq!(
            classify_watch_kind(EventKind::Access(AccessKind::Read)),
            None
        );
        assert!(WatchKind::Create.changes_tree());
        assert!(WatchKind::Remove.changes_tree());
        assert!(!WatchKind::Modify.changes_tree());

        let root = Path::new("/repo");
        assert!(is_watch_noise(&root.join(".git/index")));
        assert!(is_watch_noise(&root.join(".git/index.lock")));
        assert!(is_watch_noise(&root.join("index.lock")));
        assert!(!is_watch_noise(&root.join("src/main.rs")));
    }

    #[test]
    fn watch_filter_skips_gitignored_paths() {
        struct TempDir(PathBuf);
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        let temp = TempDir(std::env::temp_dir().join(format!("p0-watch-{}", uuid::Uuid::new_v4())));
        std::fs::create_dir_all(temp.0.join(".git")).unwrap();
        std::fs::create_dir_all(temp.0.join("target")).unwrap();
        std::fs::create_dir_all(temp.0.join("src")).unwrap();
        std::fs::write(temp.0.join(".gitignore"), "/target\n*.p0tmp\n").unwrap();
        std::fs::write(temp.0.join("target/out"), "x").unwrap();
        std::fs::write(temp.0.join("src/main.rs"), "x").unwrap();
        std::fs::write(temp.0.join("notes.p0tmp"), "x").unwrap();
        let root = temp.0.canonicalize().unwrap();
        let mut ignore = watch_matcher(&root);
        assert!(accept_watch_path(
            &root,
            &root.join("src/main.rs"),
            &mut ignore
        ));
        assert!(!accept_watch_path(
            &root,
            &root.join("target/out"),
            &mut ignore
        ));
        assert!(!accept_watch_path(
            &root,
            &root.join("notes.p0tmp"),
            &mut ignore
        ));
        assert!(!accept_watch_path(
            &root,
            &root.join(".git/HEAD"),
            &mut ignore
        ));
    }

    #[test]
    fn tells_a_missing_repository_from_other_git_failures() {
        assert_eq!(
            git_status_error(
                "fatal: not a git repository (or any of the parent directories): .git"
            ),
            GitStatusError::NotARepository
        );
        assert_eq!(
            git_status_error("fatal: unable to read current working directory"),
            GitStatusError::Unavailable
        );
    }

    #[test]
    fn detects_paths_missing_from_head() {
        assert!(path_absent_from_head(
            "path 'src/new.ts' exists on disk, but not in 'HEAD'"
        ));
        assert!(path_absent_from_head(
            "path 'src/new.ts' does not exist in 'HEAD'"
        ));
        assert!(path_absent_from_head(
            "path 'src/new.ts' exists on disk, but not in the index"
        ));
        assert!(!path_absent_from_head("not a git repository"));
    }
}
