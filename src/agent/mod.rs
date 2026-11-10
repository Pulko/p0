use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use uuid::Uuid;

pub const TURN_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const STDERR_TAIL: usize = 2_000;

#[derive(Clone, Debug)]
pub struct AgentRun {
    cancel: Arc<AtomicBool>,
    pid: Arc<AtomicU32>,
}

impl AgentRun {
    pub fn new() -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            pid: Arc::new(AtomicU32::new(0)),
        }
    }

    pub fn stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        kill_group(self.pid.load(Ordering::SeqCst));
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentEvent {
    Text { text: String, block: bool },
    Tool { label: String },
    Session,
    Failure { text: String },
    Result { text: String },
    Finished { success: bool, stderr: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptStyle {
    Chat,
    Edit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub snippet: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    User,
    Reply,
    Tool,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptLine {
    pub kind: LineKind,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct Thread {
    pub session_id: String,
    pub live: bool,
    pub running: bool,
    pub saw_session: bool,
    pub failure: Option<String>,
    pub spinner: usize,
    pub lines: Vec<TranscriptLine>,
    pub compose: String,
    pub scroll: usize,
    pub anchor: Option<Anchor>,
    pub style: PromptStyle,
    pub prior: Vec<(String, String)>,
    pub pending_user: String,
    pub reply_buf: String,
}

impl Thread {
    pub fn new() -> Self {
        Self {
            session_id: Uuid::new_v4().to_string(),
            live: false,
            running: false,
            saw_session: false,
            failure: None,
            spinner: 0,
            lines: Vec::new(),
            compose: String::new(),
            scroll: 0,
            anchor: None,
            style: PromptStyle::Chat,
            prior: Vec::new(),
            pending_user: String::new(),
            reply_buf: String::new(),
        }
    }
}

pub fn claude_argv(bin: &str, session_id: &str, resume: bool, prompt: &str) -> Vec<String> {
    let mut args = vec![
        bin.to_string(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--permission-mode".into(),
        "acceptEdits".into(),
    ];
    if resume {
        args.push("--resume".into());
    } else {
        args.push("--session-id".into());
    }
    args.push(session_id.to_string());
    args.push("-p".into());
    args.push(prompt.to_string());
    args
}

pub fn compose_prompt(
    style: PromptStyle,
    anchor: Option<&Anchor>,
    prior: &[(String, String)],
    message: &str,
    replay: bool,
) -> String {
    let mut out = String::new();
    let introduce = prior.is_empty() || replay;
    if introduce {
        match style {
            PromptStyle::Edit => {
                out.push_str("Carry out the edit request below by editing files in place. ");
                out.push_str("Change only what it asks for; you may touch other files if the change needs it. ");
                out.push_str("When done, reply in a sentence or two saying what you changed and in which files. ");
                out.push_str("The user may follow up in this conversation.\n\n");
            }
            PromptStyle::Chat => {
                out.push_str(
                    "You are helping with a conversation about the code in this workspace. ",
                );
                out.push_str("You may read and edit files here. ");
                out.push_str("When you finish, reply with a short summary of what you found or changed, and in which files.\n\n");
            }
        }
        if let Some(anchor) = anchor {
            let span = if anchor.start_line == anchor.end_line {
                format!("line {}", anchor.start_line)
            } else {
                format!("lines {}-{}", anchor.start_line, anchor.end_line)
            };
            let ext = Path::new(&anchor.path)
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("");
            out.push_str(&format!(
                "The conversation started at @{path} {span} (the file may have changed since):\n```{ext}\n{snippet}\n```\n\n",
                path = anchor.path,
                snippet = anchor.snippet,
            ));
        }
    }
    if replay && !prior.is_empty() {
        out.push_str("### Earlier in this conversation\n");
        for (user, assistant) in prior {
            out.push_str(&format!("User: {user}\n\nAssistant: {assistant}\n\n"));
        }
        out.push_str("### Current request\n");
    }
    if matches!(style, PromptStyle::Edit) && prior.is_empty() {
        out.push_str("### Instruction\n");
    }
    out.push_str(message);
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaudeLine {
    Session,
    Text { text: String, block: bool },
    Tool { label: String },
    Error { text: String },
    Result { text: String },
}

pub fn parse_claude_line(line: &str) -> Vec<ClaudeLine> {
    let line = line.trim();
    if !line.starts_with('{') {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if value
        .get("session_id")
        .and_then(|id| id.as_str())
        .is_some_and(|id| !id.is_empty())
    {
        out.push(ClaudeLine::Session);
    }
    match value.get("type").and_then(|kind| kind.as_str()) {
        Some("assistant") => {
            let content = value
                .pointer("/message/content")
                .and_then(|content| content.as_array());
            if let Some(content) = content {
                for block in content {
                    match block.get("type").and_then(|kind| kind.as_str()) {
                        Some("text") => {
                            if let Some(text) = block.get("text").and_then(|text| text.as_str())
                                && !text.trim().is_empty()
                            {
                                out.push(ClaudeLine::Text {
                                    text: text.to_string(),
                                    block: true,
                                });
                            }
                        }
                        Some("tool_use") => {
                            let name = block
                                .get("name")
                                .and_then(|name| name.as_str())
                                .unwrap_or("tool");
                            let input = block.get("input").cloned().unwrap_or(Value::Null);
                            out.push(ClaudeLine::Tool {
                                label: tool_label(name, &input),
                            });
                        }
                        _ => {}
                    }
                }
            }
        }
        Some("result") => {
            let result = value
                .get("result")
                .and_then(|text| text.as_str())
                .unwrap_or("");
            if value
                .get("is_error")
                .and_then(|flag| flag.as_bool())
                .unwrap_or(false)
            {
                let text = if result.trim().is_empty() {
                    "the harness reported an error".to_string()
                } else {
                    result.trim().to_string()
                };
                out.push(ClaudeLine::Error { text });
            } else if !result.trim().is_empty() {
                out.push(ClaudeLine::Result {
                    text: result.to_string(),
                });
            }
        }
        _ => {}
    }
    out
}

fn tool_label(name: &str, input: &Value) -> String {
    let object = input.as_object();
    for key in ["file_path", "path", "command"] {
        let Some(value) = object
            .and_then(|object| object.get(key))
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
        let value = truncate_chars(&value, 120);
        return format!("{name} {value}");
    }
    name.to_string()
}

fn truncate_chars(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

pub fn spawn_turn(
    root: PathBuf,
    prompt: String,
    session_id: String,
    resume: bool,
    tx: mpsc::UnboundedSender<AgentEvent>,
    run: AgentRun,
) {
    tokio::spawn(async move {
        let finished = execute_turn(root, prompt, session_id, resume, tx.clone(), run).await;
        let _ = tx.send(finished);
    });
}

async fn execute_turn(
    root: PathBuf,
    prompt: String,
    session_id: String,
    resume: bool,
    tx: mpsc::UnboundedSender<AgentEvent>,
    run: AgentRun,
) -> AgentEvent {
    let bin = match which::which("claude") {
        Ok(bin) => bin,
        Err(_) => {
            return AgentEvent::Finished {
                success: false,
                stderr: "claude is not installed".into(),
            };
        }
    };
    let args = claude_argv(&bin.to_string_lossy(), &session_id, resume, &prompt);
    let mut command = Command::new(&args[0]);
    command
        .args(&args[1..])
        .current_dir(&root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT");
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return AgentEvent::Finished {
                success: false,
                stderr: error.to_string(),
            };
        }
    };
    if let Some(pid) = child.id() {
        run.pid.store(pid, Ordering::SeqCst);
    }
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stderr_task = tokio::spawn(async move {
        match stderr {
            Some(stderr) => read_tail(stderr).await,
            None => String::new(),
        }
    });
    let mut timed_out = false;
    let mut stopped = false;
    if let Some(stdout) = stdout {
        let mut lines = BufReader::new(stdout).lines();
        let sleep = tokio::time::sleep(TURN_TIMEOUT);
        tokio::pin!(sleep);
        'read: loop {
            if run.cancelled() {
                stopped = true;
                kill_group(run.pid.load(Ordering::SeqCst));
                break;
            }
            tokio::select! {
                _ = &mut sleep => {
                    timed_out = true;
                    stopped = true;
                    kill_group(run.pid.load(Ordering::SeqCst));
                    break;
                }
                line = lines.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            for event in parse_claude_line(&line) {
                                let outgoing = match event {
                                    ClaudeLine::Session => AgentEvent::Session,
                                    ClaudeLine::Text { text, block } => AgentEvent::Text { text, block },
                                    ClaudeLine::Tool { label } => AgentEvent::Tool { label },
                                    ClaudeLine::Error { text } => AgentEvent::Failure { text },
                                    ClaudeLine::Result { text } => AgentEvent::Result { text },
                                };
                                if tx.send(outgoing).is_err() {
                                    stopped = true;
                                    kill_group(run.pid.load(Ordering::SeqCst));
                                    break 'read;
                                }
                            }
                        }
                        _ => break,
                    }
                }
            }
        }
    }
    if stopped {
        let _ = child.start_kill();
    }
    let status = child.wait().await.ok();
    let mut stderr = stderr_task.await.unwrap_or_default();
    if timed_out {
        stderr = format!("gave up after 30 minutes\n{stderr}");
    }
    let success = !stopped && status.is_some_and(|status| status.success());
    AgentEvent::Finished { success, stderr }
}

async fn read_tail(stderr: impl tokio::io::AsyncRead + Unpin) -> String {
    let mut reader = BufReader::new(stderr);
    let mut tail = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                tail.extend_from_slice(&buf[..n]);
                if tail.len() > STDERR_TAIL * 2 {
                    tail.drain(..tail.len() - STDERR_TAIL);
                }
            }
        }
    }
    if tail.len() > STDERR_TAIL {
        tail.drain(..tail.len() - STDERR_TAIL);
    }
    String::from_utf8_lossy(&tail).into_owned()
}

fn kill_group(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(unix)]
    unsafe {
        // The child is the leader of its own process group, so the negative
        // pid reaches the helpers Claude starts as well as Claude itself.
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "11111111-1111-4111-8111-111111111111";

    #[test]
    fn first_turn_uses_session_id_and_later_turns_resume() {
        let first = claude_argv("claude", ID, false, "explain this");
        assert_eq!(
            first,
            vec![
                "claude",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                "acceptEdits",
                "--session-id",
                ID,
                "-p",
                "explain this",
            ]
        );
        let next = claude_argv("claude", ID, true, "continue");
        assert!(next.windows(2).any(|pair| pair == ["--resume", ID]));
        assert!(!next.windows(2).any(|pair| pair == ["--session-id", ID]));
        assert_eq!(next.last().map(String::as_str), Some("continue"));
        let joined = first.join(" ");
        assert!(!joined.contains("include-partial-messages"));
        assert!(!joined.contains("dangerously-skip-permissions"));
    }

    #[test]
    fn parser_keeps_text_tools_and_errors() {
        let line = r#"{"type":"assistant","session_id":"abc","message":{"content":[{"type":"text","text":"Done."},{"type":"tool_use","name":"Edit","input":{"file_path":"src/main.rs"}}]}}"#;
        assert_eq!(
            parse_claude_line(line),
            vec![
                ClaudeLine::Session,
                ClaudeLine::Text {
                    text: "Done.".into(),
                    block: true
                },
                ClaudeLine::Tool {
                    label: "Edit src/main.rs".into()
                },
            ]
        );
        assert_eq!(
            parse_claude_line(r#"{"type":"result","is_error":true,"result":"no login"}"#),
            vec![ClaudeLine::Error {
                text: "no login".into()
            }]
        );
        assert!(parse_claude_line("not json").is_empty());
        assert_eq!(
            parse_claude_line(
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"go test ./..."}}]}}"#
            ),
            vec![ClaudeLine::Tool {
                label: "Bash go test ./...".into()
            }]
        );
    }

    #[test]
    fn live_follow_up_sends_only_the_new_message() {
        let anchor = Anchor {
            path: "src/main.rs".into(),
            start_line: 4,
            end_line: 6,
            snippet: "fn main() {}".into(),
        };
        let first = compose_prompt(
            PromptStyle::Chat,
            Some(&anchor),
            &[],
            "what is this?",
            false,
        );
        assert!(first.contains("You are helping with a conversation"));
        assert!(first.contains("@src/main.rs lines 4-6"));
        assert!(first.contains("fn main() {}"));
        assert!(first.ends_with("what is this?"));

        let prior = vec![("what is this?".into(), "A main function.".into())];
        let follow = compose_prompt(PromptStyle::Chat, Some(&anchor), &prior, "rename it", false);
        assert_eq!(follow, "rename it");

        let replay = compose_prompt(PromptStyle::Edit, Some(&anchor), &prior, "try again", true);
        assert!(replay.contains("Change only what it asks for"));
        assert!(replay.contains("User: what is this?"));
        assert!(replay.contains("Assistant: A main function."));
        assert!(replay.contains("### Current request\ntry again"));
    }
}
