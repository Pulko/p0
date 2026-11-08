use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use ignore::WalkBuilder;
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Nucleo};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::editor::Position;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitFileStatus {
    Modified,
    Added,
    Deleted,
    Untracked,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub start: usize,
    pub end: usize,
    pub deleted: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VisualRow {
    Line(usize),
    Fold {
        start: usize,
        end: usize,
        deleted: usize,
    },
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

pub fn spawn_git_status(root: PathBuf, tx: mpsc::UnboundedSender<HashMap<PathBuf, GitFileStatus>>) {
    tokio::spawn(async move {
        let output = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
            .output()
            .await;
        let statuses = output
            .ok()
            .filter(|out| out.status.success())
            .map(|out| parse_git_status(&root, &out.stdout))
            .unwrap_or_default();
        let _ = tx.send(statuses);
    });
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
    let mut index = 0;
    let mut pending_deletes = 0usize;
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                deleted_before[index] += pending_deletes;
                pending_deletes = 0;
                index += 1;
            }
            ChangeTag::Delete => pending_deletes += 1,
            ChangeTag::Insert => {
                marks[index] = if pending_deletes > 0 {
                    pending_deletes -= 1;
                    LineMark::Modified
                } else {
                    LineMark::Added
                };
                index += 1;
            }
        }
    }
    if index < deleted_before.len() {
        deleted_before[index] += pending_deletes;
    }
    LineChanges {
        marks,
        deleted_before,
    }
}

pub fn change_hunks(changes: &LineChanges) -> Vec<Hunk> {
    let mut hunks = Vec::new();
    let mut index = 0;
    while index < changes.marks.len() {
        if changes.deleted_before[index] > 0 && changes.marks[index] == LineMark::Unchanged {
            hunks.push(Hunk {
                start: index,
                end: index,
                deleted: changes.deleted_before[index],
            });
            index += 1;
            continue;
        }
        if changes.marks[index] != LineMark::Unchanged {
            let start = index;
            let mut deleted = changes.deleted_before[index];
            index += 1;
            while index < changes.marks.len() && changes.marks[index] != LineMark::Unchanged {
                deleted += changes.deleted_before[index];
                index += 1;
            }
            hunks.push(Hunk {
                start,
                end: index,
                deleted,
            });
            continue;
        }
        index += 1;
    }
    if let Some(deleted) = changes.deleted_before.last().copied() {
        if deleted > 0 {
            hunks.push(Hunk {
                start: changes.marks.len(),
                end: changes.marks.len(),
                deleted,
            });
        }
    }
    hunks
}

pub fn visual_rows(line_count: usize, hunks: &[Hunk], folded: &[usize]) -> Vec<VisualRow> {
    let mut rows = Vec::new();
    let mut index = 0;
    while index < line_count {
        if let Some(hunk) = hunks
            .iter()
            .find(|hunk| hunk.start == index && folded.contains(&hunk.start))
        {
            rows.push(VisualRow::Fold {
                start: hunk.start,
                end: hunk.end,
                deleted: hunk.deleted,
            });
            if hunk.end == hunk.start {
                rows.push(VisualRow::Line(index));
                index += 1;
            } else {
                index = hunk.end;
            }
            continue;
        }
        rows.push(VisualRow::Line(index));
        index += 1;
    }
    if line_count == 0
        && hunks
            .first()
            .is_some_and(|hunk| hunk.start == 0 && folded.contains(&0))
    {
        let hunk = hunks[0];
        rows.push(VisualRow::Fold {
            start: hunk.start,
            end: hunk.end,
            deleted: hunk.deleted,
        });
    }
    rows
}

pub fn visual_index_for_line(rows: &[VisualRow], line: usize) -> Option<usize> {
    rows.iter().position(|row| match row {
        VisualRow::Line(index) => *index == line,
        VisualRow::Fold { start, end, .. } => line >= *start && line < *end,
    })
}

pub fn buffer_line_at(rows: &[VisualRow], visual_index: usize) -> Option<usize> {
    match rows.get(visual_index)? {
        VisualRow::Line(line) => Some(*line),
        VisualRow::Fold { start, .. } => Some(*start),
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

pub fn spawn_workspace_search(
    root: PathBuf,
    query: String,
    tx: mpsc::UnboundedSender<Vec<SearchMatch>>,
) {
    tokio::spawn(async move {
        if query.trim().is_empty() {
            let _ = tx.send(Vec::new());
            return;
        }
        let mut cmd = Command::new("rg");
        cmd.arg("--json")
            .arg("--max-count")
            .arg("50")
            .arg("--max-filesize")
            .arg("2M")
            .arg("-S")
            .arg("--glob")
            .arg("!node_modules")
            .arg("--glob")
            .arg("!.git")
            .arg(&query)
            .arg(&root)
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let Ok(mut child) = cmd.spawn() else {
            let _ = tx.send(Vec::new());
            return;
        };
        let stdout = child.stdout.take();
        let mut results = Vec::new();
        if let Some(stdout) = stdout {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(m) = parse_rg_line(&root, &line) {
                    results.push(m);
                    if results.len() >= 400 {
                        break;
                    }
                }
            }
        }
        let _ = child.kill().await;
        let _ = tx.send(results);
    });
}

fn parse_rg_line(root: &Path, line: &str) -> Option<SearchMatch> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("type")?.as_str()? != "match" {
        return None;
    }
    let data = value.get("data")?;
    let path = data.get("path")?.get("text")?.as_str()?;
    let line_number = data.get("line_number")?.as_u64()? as usize;
    let text = data
        .get("lines")?
        .get("text")?
        .as_str()
        .unwrap_or("")
        .trim_end()
        .to_string();
    let column = data
        .get("submatches")
        .and_then(|s| s.as_array())
        .and_then(|a| a.first())
        .and_then(|m| m.get("start"))
        .and_then(|s| s.as_u64())
        .unwrap_or(0) as usize;
    let path = PathBuf::from(path);
    let path = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    Some(SearchMatch {
        path,
        line: line_number.saturating_sub(1),
        column,
        text,
    })
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
    fn classifies_and_folds_changed_lines() {
        let changes = classify_lines("alpha\nbeta\ngamma\n", "alpha\nBETA\nextra\ngamma\n");
        assert_eq!(changes.marks[1], LineMark::Modified);
        assert_eq!(changes.marks[2], LineMark::Added);
        let hunks = change_hunks(&changes);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].start, 1);
        assert_eq!(hunks[0].end, 3);
        let folded = visual_rows(changes.marks.len(), &hunks, &[1]);
        assert!(matches!(
            folded[1],
            VisualRow::Fold {
                start: 1,
                end: 3,
                ..
            }
        ));
        assert_eq!(buffer_line_at(&folded, 0), Some(0));
        assert_eq!(buffer_line_at(&folded, 2), Some(3));
        assert_eq!(visual_index_for_line(&folded, 3), Some(2));
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
