use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use r3vu::editor::{Document, DocumentId, Position, Range};
use r3vu::syntax::SyntaxService;
use r3vu::workspace::{
    FileFinder, find_in_document, list_workspace_files, parse_git_status, search_workspace,
};

const LINE_COUNTS: [usize; 3] = [200, 5_000, 30_000];

fn typescript_source(lines: usize) -> String {
    let mut out = String::with_capacity(lines.saturating_mul(48));
    out.push_str("export function run(user: { id: number; name: string }): string {\n");
    let body = lines.saturating_sub(2).max(1);
    for i in 0..body {
        out.push_str("  const value");
        out.push_str(&i.to_string());
        out.push_str(" = user.id + ");
        out.push_str(&i.to_string());
        out.push_str(";\n");
    }
    out.push_str("  return String(value0);\n}\n");
    out
}

fn document(lines: usize) -> Document {
    Document::new(
        DocumentId(1),
        Some(PathBuf::from("src/bench.ts")),
        typescript_source(lines),
    )
}

fn edit_at(doc: &mut Document) {
    let line = doc.rope.len_lines() / 2;
    let pos = Position::new(line, 2);
    let change = doc.apply_replace(Range::new(pos, pos), "x", false);
    let end = change.new_end_position;
    doc.apply_replace(Range::new(pos, end), "", false);
}

fn keystroke(c: &mut Criterion) {
    let mut group = c.benchmark_group("keystroke");
    for lines in LINE_COUNTS {
        group.bench_function(format!("{lines}_lines"), |b| {
            let mut doc = document(lines);
            b.iter(|| edit_at(black_box(&mut doc)));
        });
    }
    group.finish();
}

fn reparse(c: &mut Criterion) {
    let mut group = c.benchmark_group("reparse");
    for lines in LINE_COUNTS {
        group.bench_function(format!("{lines}_lines"), |b| {
            let mut syntax = SyntaxService::new();
            let mut doc = document(lines);
            syntax.open(doc.id, doc.language, &doc.rope);
            b.iter(|| {
                let line = doc.rope.len_lines() / 2;
                let pos = Position::new(line, 2);
                let change = doc.apply_replace(Range::new(pos, pos), "x", false);
                syntax.apply_edit(&change, doc.language, &doc.rope);
                let end = change.new_end_position;
                let change = doc.apply_replace(Range::new(pos, end), "", false);
                syntax.apply_edit(&change, doc.language, &doc.rope);
                black_box(&doc);
            });
        });
    }
    group.finish();
}

fn highlight(c: &mut Criterion) {
    let mut group = c.benchmark_group("viewport_highlight");
    for lines in LINE_COUNTS {
        group.bench_function(format!("{lines}_lines"), |b| {
            let mut syntax = SyntaxService::new();
            let doc = document(lines);
            syntax.open(doc.id, doc.language, &doc.rope);
            let start = (doc.rope.len_lines() / 2).saturating_sub(30);
            let end = (start + 60).min(doc.rope.len_lines().saturating_sub(1));
            b.iter(|| {
                black_box(syntax.highlights(doc.id, doc.language, &doc.rope, start, end));
            });
        });
    }
    group.finish();
}

fn open_file(c: &mut Criterion) {
    let mut group = c.benchmark_group("open_file");
    for lines in LINE_COUNTS {
        group.bench_function(format!("{lines}_lines"), |b| {
            let mut syntax = SyntaxService::new();
            let source = typescript_source(lines);
            b.iter_batched(
                || source.clone(),
                |text| {
                    let doc =
                        Document::new(DocumentId(1), Some(PathBuf::from("src/bench.ts")), text);
                    syntax.open(doc.id, doc.language, &doc.rope);
                    black_box(doc);
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn syntax_startup(c: &mut Criterion) {
    c.bench_function("syntax_service_new", |b| {
        b.iter(|| black_box(SyntaxService::new()));
    });
}

fn relative_paths(count: usize) -> Vec<String> {
    (0..count)
        .map(|i| format!("src/pkg{}/mod{}/file{i}.ts", i % 40, i % 25))
        .collect()
}

fn file_finder(c: &mut Criterion) {
    let mut group = c.benchmark_group("file_finder");
    for count in [1_000usize, 20_000] {
        group.bench_function(format!("index_{count}"), |b| {
            let files = relative_paths(count);
            b.iter_batched(
                || files.clone(),
                |files| {
                    let mut finder = FileFinder::new();
                    finder.set_files(&files);
                    black_box(finder.query("file12"));
                },
                BatchSize::SmallInput,
            );
        });
        group.bench_function(format!("query_{count}"), |b| {
            let mut finder = FileFinder::new();
            finder.set_files(&relative_paths(count));
            finder.query("warmup");
            b.iter(|| {
                black_box(finder.query("zzz"));
                black_box(finder.query("file12"));
            });
        });
    }
    group.finish();
}

fn workspace_root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = std::env::temp_dir().join("r3vu-bench-workspace");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/pkg")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        std::fs::create_dir_all(root.join("ignored")).unwrap();
        std::fs::write(root.join("ignored/skip.ts"), "BENCH_NEEDLE\n").unwrap();
        std::fs::write(
            root.join("node_modules/pkg/index.js"),
            "export const BENCH_NEEDLE = 1;\n",
        )
        .unwrap();
        for i in 0..120 {
            let body = if i % 17 == 0 {
                format!("export const hit{i} = \"BENCH_NEEDLE\";\nconst n = {i};\n")
            } else {
                format!("export const value{i} = {i};\nfunction keep{i}() {{\n  return {i};\n}}\n")
            };
            std::fs::write(root.join(format!("src/pkg/file{i}.ts")), body).unwrap();
        }
        root
    })
    .as_path()
}

fn workspace(c: &mut Criterion) {
    let root = workspace_root();
    c.bench_function("list_workspace_files", |b| {
        b.iter(|| black_box(list_workspace_files(root)));
    });
    c.bench_function("search_workspace", |b| {
        b.iter(|| black_box(search_workspace(root, "BENCH_NEEDLE")));
    });
}

fn find_in_file(c: &mut Criterion) {
    let mut group = c.benchmark_group("find_in_document");
    for lines in LINE_COUNTS {
        group.bench_function(format!("{lines}_lines"), |b| {
            let text = typescript_source(lines);
            b.iter(|| black_box(find_in_document(&text, "value", true)));
        });
    }
    group.finish();
}

fn git_status(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse_git_status");
    for count in [500usize, 5_000] {
        group.bench_function(format!("{count}_paths"), |b| {
            let mut output = Vec::with_capacity(count * 24);
            for i in 0..count {
                let status = if i % 5 == 0 { "??" } else { " M" };
                output.extend(format!("{status} src/pkg/file{i}.ts\0").into_bytes());
            }
            let root = Path::new("/repo");
            b.iter(|| black_box(parse_git_status(root, &output)));
        });
    }
    group.finish();
}

criterion_group! {
    name = editor;
    config = Criterion::default()
        .sample_size(10)
        .warm_up_time(Duration::from_millis(300))
        .measurement_time(Duration::from_secs(1));
    targets = keystroke, reparse, highlight, open_file, syntax_startup, file_finder, workspace, find_in_file, git_status
}
criterion_main!(editor);
