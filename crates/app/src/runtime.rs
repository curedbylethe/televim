//! Tokio runtime setup + TUI event loop.
//!
//! The loop is the one place the two halves of the program meet: the screen,
//! which knows nothing of the network, and the client, which knows nothing of
//! the screen. Everything either of them has to say arrives as an [`AppEvent`],
//! and every pass through the loop ends by asking the network what the screen is
//! about to need.

use std::fs::File;
use std::io::Stdout;
use std::io::{Write as _, stdout};
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
        copy_if_asked(&mut app);
    }

    Ok(())
}

/// Hands the reader's last yank to the terminal's clipboard, if one is waiting.
///
/// Called every pass, so a yank is offered on the pass it was read on. Nothing
/// is reported either way: the terminal has already been written to by the time a
/// failure could be noticed, and there is no acknowledgement to wait for.
fn copy_if_asked(app: &mut App) {
    let Some(text) = app.take_clipboard() else {
        return;
    };

    if let Err(error) = copy_to_clipboard(&text) {
        tracing::debug!(%error, "the clipboard write did not go through");
    }
}

/// The most bytes of escape sequence this will write for a clipboard.
///
/// Terminals cap an OSC 52 payload — 74 kB is the figure most often quoted, and
/// some cap far less — and a sequence past the cap is refused outright rather than
/// shortened. So the budget is on the *encoded* payload: base64 turns three bytes
/// into four characters, and cutting the text at three quarters of the budget is
/// what keeps the sequence inside it.
///
/// The register keeps the whole text regardless, so a capped write costs the
/// reader the clipboard and not the yank.
const CLIPBOARD_BUDGET: usize = 74 * 1024;

/// Writes `text` to the terminal's clipboard, best-effort.
///
/// OSC 52 is the only way a terminal application can set the system clipboard
/// without a helper process, and it works in most terminals and not all — which
/// is why [`crate::app::Register`] is the load-bearing half of a yank and this is
/// the convenience. A refused or capped write is not a failure of the yank and is
/// not reported as one, because the terminal said nothing.
///
/// Written to a second handle on the same descriptor the terminal writes through,
/// which is safe here and only here: this runs on the loop's own thread, after the
/// frame has been drawn and flushed, with nothing else writing.
fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    use base64::Engine as _;

    let payload = base64::engine::general_purpose::STANDARD.encode(clipboard_text(text));
    let mut out = stdout();

    write!(out, "\x1b]52;c;{payload}\x07")?;
    out.flush()
}

/// The text to put in the escape sequence, cut to what a terminal will take.
///
/// A character boundary and not a byte: a sequence carrying half of a multibyte
/// character decodes to something the reader never yanked, which is worse than a
/// shorter yank.
fn clipboard_text(text: &str) -> String {
    let budget = CLIPBOARD_BUDGET / 4 * 3;
    if text.len() <= budget {
        return text.to_owned();
    }

    let cut = text
        .char_indices()
        .map(|(at, _)| at)
        .take_while(|at| *at <= budget)
        .last()
        .unwrap_or(0);

    text[..cut].to_owned()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The escape sequence is written to a real terminal, so the parts of it that
    /// can be checked without one are cut out into functions: what goes in it, and
    /// how much of it.
    #[test]
    fn a_yank_that_fits_is_written_whole() {
        let text = "one line\nand another";

        assert_eq!(clipboard_text(text), text);
    }

    /// A sequence past a terminal's cap is refused outright rather than shortened,
    /// so what is cut has to be the text — and cut at a character boundary, because
    /// a sequence carrying half of a multibyte character decodes to something the
    /// reader never yanked.
    #[test]
    fn a_yank_too_large_for_a_terminal_is_cut_to_the_budget() {
        let budget = CLIPBOARD_BUDGET / 4 * 3;
        // Mixed widths, so the budget does not happen to fall on a boundary.
        let too_large = "aé漢x".repeat(budget);

        let cut = clipboard_text(&too_large);

        assert!(
            cut.len() <= budget,
            "cut to {} bytes of a budget of {budget}",
            cut.len()
        );
        assert!(
            too_large.starts_with(&cut),
            "and it is the start of the yank"
        );
        assert!(
            cut.len() + 4 > budget,
            "and not a character short: {} bytes, and no character is over four",
            cut.len()
        );
    }

    /// Four base64 characters per three bytes: three quarters of the budget is the
    /// most text whose sequence fits inside it, which is the whole reason the
    /// budget is measured on the encoded payload.
    #[test]
    fn the_cut_leaves_a_sequence_inside_the_budget() {
        use base64::Engine as _;

        let budget = CLIPBOARD_BUDGET / 4 * 3;
        let too_large = "x".repeat(budget + 1);
        let encoded = base64::engine::general_purpose::STANDARD.encode(clipboard_text(&too_large));

        assert!(
            encoded.len() <= CLIPBOARD_BUDGET,
            "the sequence is {} bytes of a budget of {CLIPBOARD_BUDGET}",
            encoded.len()
        );
    }
}
