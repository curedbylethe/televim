//! Tokio runtime setup + TUI event loop.
//!
//! The loop is the one place the two halves of the program meet: the screen,
//! which knows nothing of the network, and the client, which knows nothing of
//! the screen. Everything either of them has to say arrives as an [`AppEvent`],
//! and every pass through the loop ends by asking the network what the screen is
//! about to need.

use std::fs::File;
use std::io::Stdout;
use std::io::stdout;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::net;
use tui::app::App;

/// Something for the loop to do.
///
/// The loop's own vocabulary, and the reason there is one queue rather than
/// two: whichever producer is ready is handled in the order it became ready.
/// The client pushes onto this from `net`, which is why it is not private to
/// this module.
pub(crate) enum AppEvent {
    /// A keystroke, from the thread reading the terminal.
    Input(Event),

    /// Something that happened away from the keyboard.
    Net(net::Event),
}

/// Build a current-thread runtime (memory budget) and run the TUI.
///
/// `config_path` is only where the log goes, and it is passed rather than
/// derived so that the two cannot disagree about which run they belong to.
pub fn run(cfg: &Config, config_path: &Path) -> Result<()> {
    init_tracing(cfg, config_path);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;
    rt.block_on(run_async(cfg))
}

/// Sends the diagnostics to a file beside the configuration.
///
/// Never the terminal. This program draws on it, and a `tracing` line written
/// mid-frame lands on top of the screen — which a client that reaches the
/// network produces as a matter of course, since `grammers` reports salts and
/// re-sends at `info`. A log that shares the screen with the display is a
/// display that comes apart on the first connection.
///
/// The destination is derived rather than configured, because a file beside the
/// configuration is one whose whereabouts the reader already knows. A directory
/// that cannot be written leaves the run with no log at all: worse than a log,
/// and better than a screen nothing can be read from — and saying so would go
/// to the one place the log cannot.
fn init_tracing(cfg: &Config, config_path: &Path) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&cfg.log_level));

    let Ok(file) = File::create(config_path.with_extension("log")) else {
        return;
    };

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        // A file is not a terminal, so escape sequences would be read as text.
        .with_ansi(false)
        .with_writer(Mutex::new(file))
        .try_init();
}

async fn run_async(cfg: &Config) -> Result<()> {
    enable_raw_mode().context("enabling raw mode")?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen).context("entering alternate screen")?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("creating terminal")?;

    let result = event_loop(cfg, &mut terminal).await;

    // Always restore the terminal, even if the loop errored.
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();

    result
}

/// Draw, wait for something to happen, apply it, then ask for what comes next.
async fn event_loop(cfg: &Config, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    let mut app = App::new();
    "connecting…".clone_into(&mut app.status);

    let mut network = net::State::default();

    // Reader thread -> channel. On a current-thread runtime, this is the
    // simplest way to bridge blocking crossterm reads into async code.
    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();
    spawn_reader(tx.clone());

    // Not awaited: the terminal is already up, and the first frame is worth
    // drawing before a round trip has finished. What it finds out arrives as an
    // event, like everything else.
    net::spawn_bring_up(cfg.clone(), tx.clone());

    loop {
        terminal
            .draw(|frame| app.render(frame))
            .context("drawing frame")?;

        if app.should_quit {
            break;
        }

        match tokio::time::timeout(Duration::from_millis(250), rx.recv()).await {
            Ok(Some(AppEvent::Input(Event::Key(key))))
                if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
            {
                app.handle_key(key);
            }
            Ok(Some(AppEvent::Input(_))) | Err(_) => {}
            Ok(Some(AppEvent::Net(event))) => net::apply(&mut app, &mut network, event),
            Ok(None) => break,
        }

        // Every pass rather than every keystroke: the tick is what turns a
        // reader who scrolled to the top and stopped into a page request, and
        // what starts the next page once one has landed.
        net::drive(&mut app, &mut network, &tx);
    }

    Ok(())
}

/// Bridges blocking terminal reads into the event loop.
///
/// A thread rather than a task, because `crossterm`'s `read` blocks and a task
/// on a current-thread runtime may not. The thread ends when the channel closes,
/// which is when the loop that owns the receiving end is gone.
fn spawn_reader(tx: mpsc::UnboundedSender<AppEvent>) {
    std::thread::spawn(move || {
        while let Ok(event) = crossterm::event::read() {
            if tx.send(AppEvent::Input(event)).is_err() {
                break;
            }
        }
    });
}
