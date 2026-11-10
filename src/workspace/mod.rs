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

use crate::editor::{Position, Range, line_visible_chars};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitFileStatus {
    Modified,
    Added,
    Deleted,
    Untracked,
}

pub struct GitSnapshot {
    pub files: HashMap<PathBuf, GitFileStatus>,
    pub branch: Option<String>,
}

pub type GitStatusUpdate = Result<GitSnapshot, ()>;

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
    Head(String),
    Absent,
}

impl DiffBaseline {
    pub fn original(&self) -> &str {
        match self {
            DiffBaseline::Head(text) => text,
            DiffBaseline::Absent => "",
        }
    }
}

#[derive(Clone, Debug)]
pub struct GitDiffResult {
    pub document_id: crate::editor::DocumentId,
    pub lines: Vec<DiffLine>,
    pub baseline: Option<DiffBaseline>,
    pub error: Option<String>,
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
                tracing::warn!("git status exited {}", output.status);
                Err(())
            }
            Err(error) => {
                tracing::warn!("git status failed: {error}");
                Err(())
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

pub fn directory_status(
    statuses: &HashMap<PathBuf, GitFileStatus>,
    dir: &Path,
) -> Option<GitFileStatus> {
    statuses
        .iter()
        .filter(|(path, _)| path.starts_with(dir))
        .map(|(_, status)| *status)
        .min_by_key(|status| match status {
            GitFileStatus::Modified => 0,
            GitFileStatus::Added => 1,
            GitFileStatus::Deleted => 2,
            GitFileStatus::Untracked => 3,
        })
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

pub fn parse_git_status(root: &Path, output: &[u8]) -> HashMap<PathBuf, GitFileStatus> {
    let mut result = HashMap::new();
    let mut records = output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty());
    while let Some(record) = records.next() {
        if record.len() < 4 {
            continue;
        }
        let xy = &record[..2];
        let status = match xy {
            b"??" => GitFileStatus::Untracked,
            pair if pair.contains(&b'D') => GitFileStatus::Deleted,
            pair if pair.contains(&b'A') => GitFileStatus::Added,
            pair if pair.contains(&b'M') || pair.contains(&b'R') => GitFileStatus::Modified,
            _ => continue,
        };
        let path = String::from_utf8_lossy(&record[3..]).into_owned();
        if xy.contains(&b'R') {
            let _ = records.next();
        }
        result.insert(root.join(path), status);
    }
    result
}

pub fn path_absent_from_head(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("exists on disk, but not in") || message.contains("does not exist in")
}

pub fn spawn_git_diff(
    root: PathBuf,
    document_id: crate::editor::DocumentId,
    path: PathBuf,
    current: String,
    tx: mpsc::UnboundedSender<GitDiffResult>,
) {
    tokio::spawn(async move {
        let relative = path.strip_prefix(&root).unwrap_or(&path);
        let spec = format!("HEAD:{}", relative.to_string_lossy().replace('\\', "/"));
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
                    lines: build_diff(&original, &current),
                    baseline: Some(DiffBaseline::Head(original)),
                    error: None,
                }
            }
            Ok(output) => {
                let message = String::from_utf8_lossy(&output.stderr)
                    .trim()
                    .to_string()
                    .replace("fatal: ", "");
                if path_absent_from_head(&message) {
                    GitDiffResult {
                        document_id,
                        lines: build_diff("", &current),
                        baseline: Some(DiffBaseline::Absent),
                        error: None,
                    }
                } else {
                    GitDiffResult {
                        document_id,
                        lines: Vec::new(),
                        baseline: None,
                        error: Some(message),
                    }
                }
            }
            Err(error) => GitDiffResult {
                document_id,
                lines: Vec::new(),
                baseline: None,
                error: Some(format!("Git unavailable: {error}")),
            },
        };
        let _ = tx.send(result);
    });
}

pub fn build_diff(original: &str, current: &str) -> Vec<DiffLine> {
    use similar::{ChangeTag, TextDiff};

    let diff = TextDiff::from_lines(original, current);
    let mut lines = vec![
        DiffLine {
            kind: DiffKind::Header,
            text: "--- HEAD".into(),
        },
        DiffLine {
            kind: DiffKind::Header,
            text: "+++ working buffer".into(),
        },
    ];
    for group in diff.grouped_ops(3) {
        lines.push(DiffLine {
            kind: DiffKind::Header,
            text: "@@".into(),
        });
        for op in group {
            for change in diff.iter_changes(&op) {
                let (kind, prefix) = match change.tag() {
                    ChangeTag::Delete => (DiffKind::Delete, "-"),
                    ChangeTag::Insert => (DiffKind::Insert, "+"),
                    ChangeTag::Equal => (DiffKind::Equal, " "),
                };
                lines.push(DiffLine {
                    kind,
                    text: format!("{prefix}{}", change.value().trim_end_matches('\n')),
                });
            }
        }
    }
    lines
}

pub fn classify_lines(original: &str, current: &str) -> LineChanges {
    use similar::{ChangeTag, TextDiff};

    let old_lines = split_lines(original);
    let new_lines = split_lines(current);
    let diff = TextDiff::from_slices(&old_lines, &new_lines);
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

fn split_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
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
            b" M src/main.ts\0A  src/new.ts\0?? notes.txt\0D  old.ts\0",
        );
        assert_eq!(
            statuses.get(&root.join("src/main.ts")),
            Some(&GitFileStatus::Modified)
        );
        assert_eq!(
            statuses.get(&root.join("src/new.ts")),
            Some(&GitFileStatus::Added)
        );
        assert_eq!(
            statuses.get(&root.join("notes.txt")),
            Some(&GitFileStatus::Untracked)
        );
        assert_eq!(
            statuses.get(&root.join("old.ts")),
            Some(&GitFileStatus::Deleted)
        );
    }

    #[test]
    fn builds_colored_line_diff() {
        let lines = build_diff("const x = 1;\n", "const x = 2;\n");
        assert!(lines.iter().any(|line| line.kind == DiffKind::Delete));
        assert!(lines.iter().any(|line| line.kind == DiffKind::Insert));
        assert!(lines.iter().any(|line| line.kind == DiffKind::Header));
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
        statuses.insert(root.join("src/notes.txt"), GitFileStatus::Untracked);
        statuses.insert(root.join("src/main.ts"), GitFileStatus::Modified);
        statuses.insert(root.join("src2/other.ts"), GitFileStatus::Added);
        assert_eq!(
            directory_status(&statuses, &root.join("src")),
            Some(GitFileStatus::Modified)
        );
        assert_eq!(
            directory_status(&statuses, &root.join("src2")),
            Some(GitFileStatus::Added)
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
    fn detects_paths_missing_from_head() {
        assert!(path_absent_from_head(
            "path 'src/new.ts' exists on disk, but not in 'HEAD'"
        ));
        assert!(path_absent_from_head(
            "path 'src/new.ts' does not exist in 'HEAD'"
        ));
        assert!(!path_absent_from_head("not a git repository"));
    }
}
