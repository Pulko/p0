mod app;
mod editor;
mod input;
mod language;
mod syntax;
mod ui;
mod workspace;

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

use app::App;
use app::config::log_path;
use language::{LanguageClient, LspCmd};

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

fn spawn_watcher(root: PathBuf) -> mpsc::UnboundedReceiver<PathBuf> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let tx2 = tx.clone();
        let mut watcher =
            match notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    for path in event.paths {
                        let _ = tx2.send(path);
                    }
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
    app.refresh_git_status();
    if app.explorer_entries.iter().any(|e| e.name == "src") {
        // leave explorer as-is
    }

    let mut events = EventStream::new();
    while !app.should_quit {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
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
                    app.git_status = statuses;
                }
            }
            diff = diff_rx.recv() => {
                if let Some(diff) = diff {
                    app.receive_diff(diff);
                }
            }
            path = watch_rx.recv() => {
                if let Some(path) = path {
                    app::commands::handle_watch(&mut app, path);
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(80)) => {
                app::commands::poll_hover(&mut app);
            }
        }
    }

    let _ = app.lsp.tx.send(LspCmd::Shutdown);
    restore_terminal();
    Ok(())
}
