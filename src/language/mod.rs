#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use crate::editor::{
    Document, DocumentChange, LanguageId, Position, position_to_utf16, utf16_to_column,
};

#[derive(Clone, Debug)]
pub enum LspStatus {
    Unavailable(String),
    Starting,
    Ready,
    Error(String),
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub path: PathBuf,
    pub range_start: Position,
    pub range_end: Position,
    pub severity: u8,
    pub message: String,
}

pub fn diagnostic_label(severity: u8) -> &'static str {
    match severity {
        2 => "Warning",
        3 => "Info",
        4 => "Hint",
        _ => "Error",
    }
}

pub fn diagnostic_status(item: &Diagnostic) -> String {
    let message = item.message.replace(['\n', '\r'], " ");
    format!("{} {message}", diagnostic_label(item.severity))
}

pub fn diagnostic_at<'a>(items: &'a [Diagnostic], pos: Position) -> Option<&'a Diagnostic> {
    items
        .iter()
        .filter(|item| diagnostic_contains(item, pos))
        .min_by_key(|item| severity_rank(item.severity))
}

fn severity_rank(severity: u8) -> u8 {
    match severity {
        1..=4 => severity,
        _ => 1,
    }
}

fn diagnostic_contains(item: &Diagnostic, pos: Position) -> bool {
    let start = item.range_start;
    let end = if item.range_end > start {
        item.range_end
    } else {
        Position::new(start.line, start.column.saturating_add(1))
    };
    pos >= start && pos < end
}

#[derive(Clone, Debug)]
pub struct CompletionEdit {
    pub start: Position,
    pub end: Position,
    pub new_text: String,
}

#[derive(Clone, Debug)]
pub struct CompletionItem {
    pub label: String,
    pub detail: Option<String>,
    pub insert: String,
    pub filter_text: String,
    pub sort_text: String,
    pub kind: Option<u8>,
    pub edit: Option<CompletionEdit>,
}

#[derive(Clone, Debug)]
pub struct Location {
    pub path: PathBuf,
    pub position: Position,
}

#[derive(Clone, Debug)]
pub struct TextEdit {
    pub path: PathBuf,
    pub start: Position,
    pub end: Position,
    pub new_text: String,
}

#[derive(Debug)]
pub enum LspCmd {
    Initialize {
        root: PathBuf,
    },
    DidOpen {
        path: PathBuf,
        language: LanguageId,
        text: String,
        version: i64,
    },
    DidChange {
        path: PathBuf,
        version: i64,
        change: DocumentChange,
        full_text: String,
        incremental: bool,
    },
    DidClose {
        path: PathBuf,
    },
    Completion {
        token: u64,
        path: PathBuf,
        position: Position,
    },
    Hover {
        token: u64,
        path: PathBuf,
        position: Position,
    },
    Signature {
        token: u64,
        path: PathBuf,
        position: Position,
    },
    Definition {
        token: u64,
        path: PathBuf,
        position: Position,
    },
    References {
        token: u64,
        path: PathBuf,
        position: Position,
    },
    Rename {
        token: u64,
        path: PathBuf,
        position: Position,
        new_name: String,
    },
    Restart {
        root: PathBuf,
    },
    Shutdown,
}

#[derive(Debug)]
pub enum LspEvent {
    Status(LspStatus),
    Diagnostics {
        path: PathBuf,
        items: Vec<Diagnostic>,
    },
    Completion {
        token: u64,
        items: Vec<CompletionItem>,
    },
    Hover {
        token: u64,
        text: String,
    },
    Signature {
        token: u64,
        text: String,
    },
    Locations {
        token: u64,
        items: Vec<Location>,
    },
    Edits {
        token: u64,
        items: Vec<TextEdit>,
    },
}

pub struct LanguageClient {
    pub tx: mpsc::UnboundedSender<LspCmd>,
}

impl LanguageClient {
    pub fn spawn(event_tx: mpsc::UnboundedSender<LspEvent>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(run_client(rx, event_tx));
        Self { tx }
    }
}

struct Server {
    child: Child,
    writer: tokio::process::ChildStdin,
    incremental: bool,
    next_id: i64,
    pending: HashMap<i64, Pending>,
}

enum Pending {
    Completion(u64),
    Hover(u64),
    Signature(u64),
    Definition(u64),
    References(u64),
    Rename(u64),
    Initialize,
}

async fn run_client(
    mut rx: mpsc::UnboundedReceiver<LspCmd>,
    events: mpsc::UnboundedSender<LspEvent>,
) {
    let mut server: Option<Server> = None;
    let mut reader_task_alive = false;
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Value>();
    let mut restarts = 0u32;
    let mut last_root: Option<PathBuf> = None;
    let mut open_docs: HashMap<PathBuf, (LanguageId, String, i64)> = HashMap::new();

    loop {
        tokio::select! {
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { break; };
                match cmd {
                    LspCmd::Shutdown => {
                        if let Some(mut s) = server.take() {
                            let _ = write_rpc(&mut s.writer, json!({"jsonrpc":"2.0","id":s.next_id,"method":"shutdown","params":null})).await;
                            let _ = s.child.kill().await;
                        }
                        break;
                    }
                    LspCmd::Initialize { root } | LspCmd::Restart { root } => {
                        last_root = Some(root.clone());
                        if let Some(mut s) = server.take() {
                            let _ = s.child.kill().await;
                        }
                        match start_server(&root, &events, out_tx.clone()).await {
                            Ok(s) => {
                                reader_task_alive = true;
                                let _ = events.send(LspEvent::Status(LspStatus::Starting));
                                server = Some(s);
                                restarts = 0;
                            }
                            Err(err) => {
                                let _ = events.send(LspEvent::Status(LspStatus::Unavailable(err)));
                            }
                        }
                    }
                    other => {
                        if server.is_none() {
                            if matches!(other, LspCmd::DidOpen { .. }) {
                                if let LspCmd::DidOpen { path, language, text, version } = &other {
                                    open_docs.insert(path.clone(), (*language, text.clone(), *version));
                                }
                            }
                            continue;
                        }
                        if let Some(s) = server.as_mut() {
                            match handle_cmd(s, other, &mut open_docs).await {
                                Ok(()) => {}
                                Err(err) => {
                                    tracing::warn!("lsp command failed: {err}");
                                    let _ = s.child.kill().await;
                                    server = None;
                                    let _ = events.send(LspEvent::Status(LspStatus::Error(err)));
                                }
                            }
                        }
                    }
                }
            }
            msg = out_rx.recv() => {
                let Some(msg) = msg else { break; };
                if msg.get("method").and_then(|m| m.as_str()) == Some("exit") || msg.get("closed").is_some() {
                    let _ = events.send(LspEvent::Status(LspStatus::Error("language server exited".into())));
                    server = None;
                    if restarts < 5 {
                        if let Some(root) = last_root.clone() {
                            restarts += 1;
                            tokio::time::sleep(Duration::from_millis(300 * restarts as u64)).await;
                            if let Ok(s) = start_server(&root, &events, out_tx.clone()).await {
                                let _ = events.send(LspEvent::Status(LspStatus::Starting));
                                // replay docs after initialize response
                                server = Some(s);
                            }
                        }
                    }
                    continue;
                }
                if let Some(s) = server.as_mut() {
                    dispatch_message(s, msg, &events, &mut open_docs).await;
                }
            }
        }
        let _ = reader_task_alive;
    }
}

async fn start_server(
    root: &Path,
    events: &mpsc::UnboundedSender<LspEvent>,
    out_tx: mpsc::UnboundedSender<Value>,
) -> Result<Server, String> {
    let exe = which::which("typescript-language-server")
        .map_err(|_| "typescript-language-server not found".to_string())?;
    let mut child = Command::new(exe)
        .arg("--stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().ok_or("missing stdout")?;
    let stderr = child.stderr.take();
    let writer = child.stdin.take().ok_or("missing stdin")?;
    tokio::spawn(read_lsp(stdout, out_tx.clone()));
    if let Some(stderr) = stderr {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!("tsserver: {line}");
            }
        });
    }
    let mut server = Server {
        child,
        writer,
        incremental: true,
        next_id: 1,
        pending: HashMap::new(),
    };
    let uri = path_to_uri(root);
    let id = server.next_id;
    server.next_id += 1;
    server.pending.insert(id, Pending::Initialize);
    write_rpc(
        &mut server.writer,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "processId": std::process::id(),
                "rootUri": uri,
                "rootPath": root,
                "capabilities": {
                    "workspace": { "workspaceEdit": { "documentChanges": true } },
                    "textDocument": {
                        "synchronization": { "dynamicRegistration": false, "willSave": false, "didSave": true },
                        "completion": { "completionItem": { "snippetSupport": false } },
                        "hover": { "contentFormat": ["plaintext", "markdown"] },
                        "signatureHelp": {},
                        "definition": {},
                        "references": {},
                        "rename": {},
                        "publishDiagnostics": {}
                    }
                },
                "initializationOptions": { "preferences": { "includeCompletionsForModuleExports": true } },
                "workspaceFolders": [{ "uri": uri, "name": root.file_name().and_then(|n| n.to_str()).unwrap_or("workspace") }]
            }
        }),
    )
    .await?;
    let _ = events;
    Ok(server)
}

async fn read_lsp<R: tokio::io::AsyncRead + Unpin>(reader: R, tx: mpsc::UnboundedSender<Value>) {
    let mut reader = BufReader::new(reader);
    loop {
        let mut header = String::new();
        let mut content_length = None;
        loop {
            header.clear();
            match reader.read_line(&mut header).await {
                Ok(0) => {
                    let _ = tx.send(json!({"closed": true}));
                    return;
                }
                Ok(_) => {}
                Err(_) => {
                    let _ = tx.send(json!({"closed": true}));
                    return;
                }
            }
            let line = header.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(rest) = line.strip_prefix("Content-Length:") {
                content_length = rest.trim().parse().ok();
            }
        }
        let Some(len) = content_length else {
            continue;
        };
        let mut buf = vec![0u8; len];
        if reader.read_exact(&mut buf).await.is_err() {
            let _ = tx.send(json!({"closed": true}));
            return;
        }
        if let Ok(value) = serde_json::from_slice(&buf) {
            let _ = tx.send(value);
        }
    }
}

async fn write_rpc(writer: &mut tokio::process::ChildStdin, value: Value) -> Result<(), String> {
    let body = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer
        .write_all(header.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    writer.write_all(&body).await.map_err(|e| e.to_string())?;
    writer.flush().await.map_err(|e| e.to_string())?;
    Ok(())
}

async fn handle_cmd(
    server: &mut Server,
    cmd: LspCmd,
    open_docs: &mut HashMap<PathBuf, (LanguageId, String, i64)>,
) -> Result<(), String> {
    match cmd {
        LspCmd::DidOpen {
            path,
            language,
            text,
            version,
        } => {
            open_docs.insert(path.clone(), (language, text.clone(), version));
            write_rpc(
                &mut server.writer,
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": path_to_uri(&path),
                            "languageId": language.lsp_language_id(),
                            "version": version,
                            "text": text
                        }
                    }
                }),
            )
            .await
        }
        LspCmd::DidChange {
            path,
            version,
            change,
            full_text,
            incremental,
        } => {
            if let Some(entry) = open_docs.get_mut(&path) {
                entry.1 = full_text.clone();
                entry.2 = version;
            }
            let content_change = if incremental && server.incremental {
                json!({
                    "range": {
                        "start": { "line": change.start_position.line, "character": change_to_utf16_start(&full_text, &change) },
                        "end": { "line": change.old_end_position.line, "character": change.old_end_position.column }
                    },
                    "text": change.inserted_text
                })
            } else {
                json!({ "text": full_text })
            };
            // Always send full text to avoid UTF-16 mismatch during v1.
            let _ = (content_change, incremental);
            write_rpc(
                &mut server.writer,
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didChange",
                    "params": {
                        "textDocument": { "uri": path_to_uri(&path), "version": version },
                        "contentChanges": [{ "text": full_text }]
                    }
                }),
            )
            .await
        }
        LspCmd::DidClose { path } => {
            open_docs.remove(&path);
            write_rpc(
                &mut server.writer,
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didClose",
                    "params": { "textDocument": { "uri": path_to_uri(&path) } }
                }),
            )
            .await
        }
        LspCmd::Completion {
            token,
            path,
            position,
        } => {
            request(
                server,
                "textDocument/completion",
                json!({
                    "textDocument": { "uri": path_to_uri(&path) },
                    "position": lsp_pos_raw(position),
                    "context": { "triggerKind": 1 }
                }),
                Pending::Completion(token),
            )
            .await
        }
        LspCmd::Hover {
            token,
            path,
            position,
        } => {
            request(
                server,
                "textDocument/hover",
                pos_params(&path, position),
                Pending::Hover(token),
            )
            .await
        }
        LspCmd::Signature {
            token,
            path,
            position,
        } => {
            request(
                server,
                "textDocument/signatureHelp",
                pos_params(&path, position),
                Pending::Signature(token),
            )
            .await
        }
        LspCmd::Definition {
            token,
            path,
            position,
        } => {
            request(
                server,
                "textDocument/definition",
                pos_params(&path, position),
                Pending::Definition(token),
            )
            .await
        }
        LspCmd::References {
            token,
            path,
            position,
        } => {
            request(
                server,
                "textDocument/references",
                json!({
                    "textDocument": { "uri": path_to_uri(&path) },
                    "position": lsp_pos_raw(position),
                    "context": { "includeDeclaration": true }
                }),
                Pending::References(token),
            )
            .await
        }
        LspCmd::Rename {
            token,
            path,
            position,
            new_name,
        } => {
            request(
                server,
                "textDocument/rename",
                json!({
                    "textDocument": { "uri": path_to_uri(&path) },
                    "position": lsp_pos_raw(position),
                    "newName": new_name
                }),
                Pending::Rename(token),
            )
            .await
        }
        LspCmd::Initialize { .. } | LspCmd::Restart { .. } | LspCmd::Shutdown => Ok(()),
    }
}

fn change_to_utf16_start(_text: &str, change: &DocumentChange) -> u32 {
    change.start_position.column as u32
}

fn pos_params(path: &Path, position: Position) -> Value {
    json!({
        "textDocument": { "uri": path_to_uri(path) },
        "position": lsp_pos_raw(position)
    })
}

fn lsp_pos_raw(position: Position) -> Value {
    json!({ "line": position.line, "character": position.column })
}

pub fn lsp_position(doc: &Document, pos: Position) -> Value {
    json!({
        "line": pos.line,
        "character": position_to_utf16(&doc.rope, pos)
    })
}

async fn request(
    server: &mut Server,
    method: &str,
    params: Value,
    pending: Pending,
) -> Result<(), String> {
    let id = server.next_id;
    server.next_id += 1;
    server.pending.insert(id, pending);
    write_rpc(
        &mut server.writer,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }),
    )
    .await
}

async fn dispatch_message(
    server: &mut Server,
    msg: Value,
    events: &mpsc::UnboundedSender<LspEvent>,
    open_docs: &mut HashMap<PathBuf, (LanguageId, String, i64)>,
) {
    if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
        if method == "textDocument/publishDiagnostics" {
            if let Some(params) = msg.get("params") {
                let uri = params.get("uri").and_then(|u| u.as_str()).unwrap_or("");
                let path = uri_to_path(uri);
                let items = params
                    .get("diagnostics")
                    .and_then(|d| d.as_array())
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|d| parse_diagnostic(&path, d))
                    .collect();
                let _ = events.send(LspEvent::Diagnostics { path, items });
            }
        }
        return;
    }
    let Some(id) = msg.get("id").and_then(|id| id.as_i64()) else {
        return;
    };
    let Some(pending) = server.pending.remove(&id) else {
        return;
    };
    match pending {
        Pending::Initialize => {
            if msg.get("error").is_some() {
                let _ = events.send(LspEvent::Status(LspStatus::Error(
                    "initialize failed".into(),
                )));
                return;
            }
            let caps = msg.pointer("/result/capabilities/textDocumentSync");
            server.incremental = match caps {
                Some(Value::Number(n)) => n.as_u64() == Some(2),
                Some(Value::Object(o)) => o.get("change").and_then(|c| c.as_u64()) == Some(2),
                _ => true,
            };
            let _ = write_rpc(
                &mut server.writer,
                json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
            )
            .await;
            let _ = events.send(LspEvent::Status(LspStatus::Ready));
            let docs: Vec<_> = open_docs
                .iter()
                .map(|(p, (l, t, v))| (p.clone(), *l, t.clone(), *v))
                .collect();
            for (path, language, text, version) in docs {
                let _ = write_rpc(
                    &mut server.writer,
                    json!({
                        "jsonrpc": "2.0",
                        "method": "textDocument/didOpen",
                        "params": {
                            "textDocument": {
                                "uri": path_to_uri(&path),
                                "languageId": language.lsp_language_id(),
                                "version": version,
                                "text": text
                            }
                        }
                    }),
                )
                .await;
            }
        }
        Pending::Completion(token) => {
            let items = parse_completions(msg.get("result"));
            let _ = events.send(LspEvent::Completion { token, items });
        }
        Pending::Hover(token) => {
            let text = parse_hover(msg.get("result"));
            let _ = events.send(LspEvent::Hover { token, text });
        }
        Pending::Signature(token) => {
            let text = parse_signature(msg.get("result"));
            let _ = events.send(LspEvent::Signature { token, text });
        }
        Pending::Definition(token) | Pending::References(token) => {
            let items = parse_locations(msg.get("result"));
            let _ = events.send(LspEvent::Locations { token, items });
        }
        Pending::Rename(token) => {
            let items = parse_workspace_edit(msg.get("result"));
            let _ = events.send(LspEvent::Edits { token, items });
        }
    }
}

fn parse_diagnostic(path: &Path, d: Value) -> Option<Diagnostic> {
    let message = d.get("message")?.as_str()?.to_string();
    let severity = d.get("severity").and_then(|s| s.as_u64()).unwrap_or(1) as u8;
    let start = d.pointer("/range/start")?;
    let end = d.pointer("/range/end")?;
    Some(Diagnostic {
        path: path.to_path_buf(),
        range_start: Position::new(
            start.get("line")?.as_u64()? as usize,
            start.get("character")?.as_u64()? as usize,
        ),
        range_end: Position::new(
            end.get("line")?.as_u64()? as usize,
            end.get("character")?.as_u64()? as usize,
        ),
        severity,
        message,
    })
}

pub fn completion_kind_tag(kind: Option<u8>) -> &'static str {
    match kind {
        Some(2..=4) => "fn",
        Some(5 | 6 | 10 | 21) => "var",
        Some(7 | 8 | 13 | 22 | 25) => "type",
        Some(14) => "kw",
        Some(9 | 17) => "mod",
        Some(15) => "snip",
        _ => "•",
    }
}

pub fn completion_prefix(doc: &Document, cursor: Position) -> (Position, String) {
    let text = crate::editor::line_text(&doc.rope, cursor.line);
    let chars: Vec<char> = text.chars().collect();
    let end = cursor.column.min(chars.len());
    let mut start = end;
    while start > 0 && is_completion_char(chars[start - 1]) {
        start -= 1;
    }
    let prefix: String = chars[start..end].iter().collect();
    (Position::new(cursor.line, start), prefix)
}

fn is_completion_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '$'
}

pub fn filter_completions(items: &[CompletionItem], prefix: &str) -> Vec<CompletionItem> {
    let needle = prefix.to_lowercase();
    let mut matched: Vec<CompletionItem> = items
        .iter()
        .filter(|item| needle.is_empty() || item.filter_text.to_lowercase().starts_with(&needle))
        .cloned()
        .collect();
    matched.sort_by(|a, b| {
        a.sort_text
            .cmp(&b.sort_text)
            .then_with(|| a.label.cmp(&b.label))
    });
    matched.truncate(80);
    matched
}

pub fn strip_snippets(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '$' && index + 1 < chars.len() {
            if chars[index + 1] == '{' {
                if let Some(end) = matching_brace(&chars, index + 1) {
                    let inner: String = chars[index + 2..end].iter().collect();
                    out.push_str(&snippet_default(&inner));
                    index = end + 1;
                    continue;
                }
            } else if chars[index + 1].is_ascii_digit() {
                index += 2;
                while index < chars.len() && chars[index].is_ascii_digit() {
                    index += 1;
                }
                continue;
            }
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

fn matching_brace(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (idx, ch) in chars.iter().enumerate().skip(open) {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
    }
    None
}

fn snippet_default(inner: &str) -> String {
    let rest = inner.trim_start_matches(|ch: char| ch.is_ascii_digit());
    if let Some(body) = rest.strip_prefix(':') {
        strip_snippets(body)
    } else if let Some(body) = rest.strip_prefix('|') {
        let body = body.strip_suffix('|').unwrap_or(body);
        body.split(',').next().unwrap_or("").to_string()
    } else {
        String::new()
    }
}

fn parse_completions(result: Option<&Value>) -> Vec<CompletionItem> {
    let items = match result {
        Some(Value::Array(a)) => a.clone(),
        Some(Value::Object(o)) => o
            .get("items")
            .and_then(|i| i.as_array())
            .cloned()
            .unwrap_or_default(),
        _ => return Vec::new(),
    };
    items
        .into_iter()
        .filter_map(|item| {
            let label = item.get("label")?.as_str()?.to_string();
            let edit = parse_completion_edit(&item);
            let insert = edit
                .as_ref()
                .map(|edit| edit.new_text.clone())
                .or_else(|| {
                    item.get("insertText")
                        .and_then(|text| text.as_str())
                        .map(|text| text.to_string())
                })
                .unwrap_or_else(|| label.clone());
            let filter_text = item
                .get("filterText")
                .and_then(|text| text.as_str())
                .unwrap_or(&label)
                .to_string();
            let sort_text = item
                .get("sortText")
                .and_then(|text| text.as_str())
                .unwrap_or(&filter_text)
                .to_string();
            Some(CompletionItem {
                detail: item
                    .get("detail")
                    .and_then(|d| d.as_str())
                    .map(|s| s.to_string()),
                filter_text,
                sort_text,
                kind: item.get("kind").and_then(|k| k.as_u64()).map(|k| k as u8),
                label,
                insert,
                edit,
            })
        })
        .collect()
}

fn parse_completion_edit(item: &Value) -> Option<CompletionEdit> {
    let edit = item.get("textEdit")?;
    let new_text = edit.get("newText")?.as_str()?.to_string();
    let range = edit.get("range").or_else(|| edit.get("insert"))?;
    let start = range.get("start")?;
    let end = range.get("end")?;
    Some(CompletionEdit {
        start: Position::new(
            start.get("line")?.as_u64()? as usize,
            start.get("character")?.as_u64()? as usize,
        ),
        end: Position::new(
            end.get("line")?.as_u64()? as usize,
            end.get("character")?.as_u64()? as usize,
        ),
        new_text,
    })
}

fn parse_hover(result: Option<&Value>) -> String {
    let Some(result) = result else {
        return String::new();
    };
    match result.get("contents") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(o)) => o
            .get("value")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(s.clone()),
                Value::Object(o) => o
                    .get("value")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn parse_signature(result: Option<&Value>) -> String {
    let Some(result) = result else {
        return String::new();
    };
    let sigs = result.get("signatures").and_then(|s| s.as_array());
    let Some(sigs) = sigs else {
        return String::new();
    };
    sigs.iter()
        .filter_map(|s| {
            s.get("label")
                .and_then(|l| l.as_str())
                .map(|s| s.to_string())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_locations(result: Option<&Value>) -> Vec<Location> {
    let values = match result {
        Some(Value::Array(a)) => a.clone(),
        Some(obj) if obj.get("uri").is_some() => vec![obj.clone()],
        _ => return Vec::new(),
    };
    values
        .into_iter()
        .filter_map(|v| {
            let uri = v
                .get("uri")
                .or_else(|| v.pointer("/targetUri"))
                .and_then(|u| u.as_str())?;
            let pos = v
                .pointer("/range/start")
                .or_else(|| v.pointer("/targetSelectionRange/start"))?;
            Some(Location {
                path: uri_to_path(uri),
                position: Position::new(
                    pos.get("line")?.as_u64()? as usize,
                    pos.get("character")?.as_u64()? as usize,
                ),
            })
        })
        .collect()
}

fn parse_workspace_edit(result: Option<&Value>) -> Vec<TextEdit> {
    let Some(result) = result else {
        return Vec::new();
    };
    let mut edits = Vec::new();
    if let Some(changes) = result.get("changes").and_then(|c| c.as_object()) {
        for (uri, list) in changes {
            let path = uri_to_path(uri);
            if let Some(list) = list.as_array() {
                for edit in list {
                    if let Some(parsed) = parse_text_edit(&path, edit) {
                        edits.push(parsed);
                    }
                }
            }
        }
    }
    if let Some(docs) = result.get("documentChanges").and_then(|c| c.as_array()) {
        for doc in docs {
            let uri = doc
                .pointer("/textDocument/uri")
                .and_then(|u| u.as_str())
                .unwrap_or("");
            let path = uri_to_path(uri);
            if let Some(list) = doc.get("edits").and_then(|e| e.as_array()) {
                for edit in list {
                    if let Some(parsed) = parse_text_edit(&path, edit) {
                        edits.push(parsed);
                    }
                }
            }
        }
    }
    edits
}

fn parse_text_edit(path: &Path, edit: &Value) -> Option<TextEdit> {
    let start = edit.pointer("/range/start")?;
    let end = edit.pointer("/range/end")?;
    Some(TextEdit {
        path: path.to_path_buf(),
        start: Position::new(
            start.get("line")?.as_u64()? as usize,
            start.get("character")?.as_u64()? as usize,
        ),
        end: Position::new(
            end.get("line")?.as_u64()? as usize,
            end.get("character")?.as_u64()? as usize,
        ),
        new_text: edit.get("newText")?.as_str()?.to_string(),
    })
}

pub fn path_to_uri(path: &Path) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    format!("file://{}", path.display())
}

pub fn uri_to_path(uri: &str) -> PathBuf {
    let stripped = uri.strip_prefix("file://").unwrap_or(uri);
    PathBuf::from(stripped)
}

pub fn utf16_position_to_doc(doc: &Document, line: usize, character: u32) -> Position {
    Position::new(
        line.min(doc.rope.len_lines().saturating_sub(1)),
        utf16_to_column(&doc.rope, line, character),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample(label: &str, filter: &str, sort: &str) -> CompletionItem {
        CompletionItem {
            label: label.into(),
            detail: None,
            insert: label.into(),
            filter_text: filter.into(),
            sort_text: sort.into(),
            kind: Some(3),
            edit: None,
        }
    }

    #[test]
    fn filters_and_sorts_by_prefix() {
        let items = vec![
            sample("zebra", "zebra", "b"),
            sample("console", "console", "a"),
            sample("const", "Const", "0"),
            sample("log", "log", "c"),
        ];
        let labels: Vec<_> = filter_completions(&items, "con")
            .into_iter()
            .map(|item| item.label)
            .collect();
        assert_eq!(labels, vec!["const".to_string(), "console".to_string()]);
    }

    #[test]
    fn empty_prefix_keeps_the_sorted_list() {
        let items = vec![
            sample("zebra", "zebra", "b"),
            sample("console", "console", "a"),
            sample("const", "const", "0"),
            sample("log", "log", "c"),
        ];
        let labels: Vec<_> = filter_completions(&items, "")
            .into_iter()
            .map(|item| item.label)
            .collect();
        assert_eq!(
            labels,
            vec![
                "const".to_string(),
                "console".to_string(),
                "zebra".to_string(),
                "log".to_string()
            ]
        );
    }

    #[test]
    fn strips_snippet_placeholders() {
        assert_eq!(strip_snippets("foo$0"), "foo");
        assert_eq!(strip_snippets("a${1}b$2"), "ab");
        assert_eq!(strip_snippets("${1:name}"), "name");
        assert_eq!(strip_snippets("fn ${1:name}($2)"), "fn name()");
    }

    #[test]
    fn parses_filter_sort_and_text_edit() {
        let result = json!({
            "items": [{
                "label": "console",
                "filterText": "con",
                "sortText": "1",
                "kind": 3,
                "insertText": "console.log($0)",
                "textEdit": {
                    "range": {
                        "start": {"line": 1, "character": 2},
                        "end": {"line": 1, "character": 5}
                    },
                    "newText": "console"
                }
            }]
        });
        let items = parse_completions(Some(&result));
        assert_eq!(items[0].filter_text, "con");
        assert_eq!(items[0].sort_text, "1");
        assert_eq!(items[0].kind, Some(3));
        assert_eq!(items[0].insert, "console");
        let edit = items[0].edit.as_ref().unwrap();
        assert_eq!(edit.start, Position::new(1, 2));
        assert_eq!(edit.end, Position::new(1, 5));
        assert_eq!(edit.new_text, "console");
        assert_eq!(completion_kind_tag(Some(3)), "fn");
        assert_eq!(completion_kind_tag(Some(6)), "var");
        assert_eq!(completion_kind_tag(Some(7)), "type");
        assert_eq!(completion_kind_tag(Some(14)), "kw");
    }

    fn diag(start: (usize, usize), end: (usize, usize), severity: u8, message: &str) -> Diagnostic {
        Diagnostic {
            path: PathBuf::from("a.ts"),
            range_start: Position::new(start.0, start.1),
            range_end: Position::new(end.0, end.1),
            severity,
            message: message.into(),
        }
    }

    #[test]
    fn diagnostic_at_covers_start_middle_and_end_lines() {
        let items = vec![diag((1, 4), (3, 2), 1, "broken")];
        assert_eq!(
            diagnostic_at(&items, Position::new(1, 4)).map(|item| item.message.as_str()),
            Some("broken")
        );
        assert_eq!(
            diagnostic_at(&items, Position::new(2, 0)).map(|item| item.message.as_str()),
            Some("broken")
        );
        assert_eq!(
            diagnostic_at(&items, Position::new(3, 1)).map(|item| item.message.as_str()),
            Some("broken")
        );
        assert!(diagnostic_at(&items, Position::new(1, 3)).is_none());
        assert!(diagnostic_at(&items, Position::new(3, 2)).is_none());
        assert!(diagnostic_at(&items, Position::new(4, 0)).is_none());
    }

    #[test]
    fn diagnostic_at_prefers_the_error_when_ranges_overlap() {
        let items = vec![
            diag((0, 0), (0, 8), 2, "careful"),
            diag((0, 2), (0, 6), 1, "broken"),
        ];
        let hit = diagnostic_at(&items, Position::new(0, 3)).unwrap();
        assert_eq!(hit.message, "broken");
        assert_eq!(diagnostic_label(hit.severity), "Error");
        assert_eq!(diagnostic_status(&items[0]), "Warning careful");
    }
}
