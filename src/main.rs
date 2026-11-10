use std::io::stdout;
use std::panic;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    EventStream,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;
use notify::Watcher;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

use r3vu::app::config::log_path;
use r3vu::app::state::HOVER_DWELL;
use r3vu::app::{self, App};
use r3vu::input;
use r3vu::language::{LanguageClient, LspCmd};
use r3vu::ui;
use r3vu::workspace::{WatchEvent, accept_watch_path, classify_watch_kind, watch_matcher};

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste,
        crossterm::cursor::Show
    );
}

fn setup_logging() {
    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok();
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    if let Some(file) = file {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(file)
            .with_ansi(false)
            .try_init();
    }
}

fn spawn_watcher(root: PathBuf) -> mpsc::UnboundedReceiver<WatchEvent> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let mut ignore = watch_matcher(&root);
        let tx2 = tx.clone();
        let watch_root = root.clone();
        let mut watcher =
            match notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                let Ok(event) = res else {
                    return;
                };
                let Some(kind) = classify_watch_kind(event.kind) else {
                    return;
                };
                for path in event.paths {
                    if !accept_watch_path(&watch_root, &path, &mut ignore) {
                        continue;
                    }
                    let _ = tx2.send(WatchEvent { path, kind });
                }
            }) {
                Ok(w) => w,
                Err(err) => {
                    tracing::warn!("file watcher unavailable: {err}");
                    return;
                }
            };
        if let Err(err) = watcher.watch(&root, notify::RecursiveMode::Recursive) {
            tracing::warn!("watch failed: {err}");
            return;
        }
        loop {
            std::thread::park();
        }
    });
    rx
}

async fn ready_in(delay: Option<Duration>) {
    match delay {
        Some(delay) => tokio::time::sleep(delay).await,
        None => std::future::pending().await,
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    setup_logging();
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let root = root.canonicalize().unwrap_or(root);

    let hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        restore_terminal();
        hook(info);
    }));

    enable_raw_mode().context("raw mode")?;
    execute!(
        stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste,
        crossterm::cursor::Show
    )?;
    let _guard = TerminalGuard;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    terminal.clear()?;

    let (lsp_event_tx, mut lsp_event_rx) = mpsc::unbounded_channel();
    let lsp = LanguageClient::spawn(lsp_event_tx);
    let mut watch_rx = spawn_watcher(root.clone());
    let mut app = App::new(root, lsp);
    let (search_tx, mut search_rx) = mpsc::unbounded_channel();
    app.search_tx = search_tx;
    let (git_status_tx, mut git_status_rx) = mpsc::unbounded_channel();
    app.git_status_tx = git_status_tx;
    let (diff_tx, mut diff_rx) = mpsc::unbounded_channel();
    app.diff_tx = diff_tx;
    let (git_write_tx, mut git_write_rx) = mpsc::unbounded_channel();
    app.git_write_tx = git_write_tx;
    let (file_diff_tx, mut file_diff_rx) = mpsc::unbounded_channel();
    app.file_diff_tx = file_diff_tx;
    let (agent_tx, mut agent_rx) = mpsc::unbounded_channel();
    app.agent_tx = agent_tx;
    app.refresh_git_status();
    if app.explorer_entries.iter().any(|e| e.name == "src") {
        // leave explorer as-is
    }

    let mut events = EventStream::new();
    while !app.should_quit {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        let hover_delay = app
            .hover_dwell
            .as_ref()
            .map(|dwell| HOVER_DWELL.saturating_sub(dwell.since.elapsed()));
        let git_delay = app.git_status_delay();
        tokio::select! {
            event = events.next() => {
                if let Some(Ok(event)) = event {
                    input::handle_event(&mut app, event);
                } else {
                    break;
                }
            }
            event = lsp_event_rx.recv() => {
                if let Some(event) = event {
                    app::events::handle_lsp(&mut app, event);
                }
            }
            results = search_rx.recv() => {
                if let Some(results) = results {
                    app::events::handle_search_results(&mut app, results);
                }
            }
            statuses = git_status_rx.recv() => {
                if let Some(statuses) = statuses {
                    app.receive_git_status(statuses);
                }
            }
            diff = diff_rx.recv() => {
                if let Some(diff) = diff {
                    app.receive_diff(diff);
                }
            }
            write = git_write_rx.recv() => {
                if let Some(write) = write {
                    app::commands::receive_git_write(&mut app, write);
                }
            }
            file_diff = file_diff_rx.recv() => {
                if let Some(file_diff) = file_diff {
                    app.receive_file_diff(file_diff);
                }
            }
            event = watch_rx.recv() => {
                if let Some(event) = event {
                    app::commands::handle_watch(&mut app, event);
                }
            }
            event = agent_rx.recv() => {
                if let Some(event) = event {
                    app::commands::handle_agent(&mut app, event);
                }
            }
            _ = ready_in(hover_delay) => {
                app::commands::poll_hover(&mut app);
            }
            _ = ready_in(git_delay) => {
                app.pump_git_status();
            }
        }
    }

    app::commands::stop_agent(&mut app);
    let _ = app.lsp.tx.send(LspCmd::Shutdown);
    restore_terminal();
    Ok(())
}
