//! Tokio runtime setup + TUI event loop.
//!
//! The loop is the one place the two halves of the program meet: the screen,
//! which knows nothing of the network, and the client, which knows nothing of
//! the screen. Everything either of them has to say arrives as an [`AppEvent`],
//! and every pass through the loop ends by asking the network what the screen is
//! about to need.

use std::fs::File;
use std::io::Stdout;
use std::io::{Write, stdout};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use crossterm::event::{
    Event, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
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
    build_runtime()?.block_on(run_async(cfg))
}

/// Records the instant the program was asked to start.
///
/// The clock starts here rather than inside the loop, because the loop's own
/// clock cannot see process start-up, configuration loading or tracing setup —
/// which is most of what a launch spends its first half-millisecond on. Called
/// from `main` before anything else, so the first-frame figure covers them.
pub fn note_launch() {
    if measuring() {
        let _ = LAUNCHED.set(Instant::now());
    }
}

/// Whether this run is being measured, and so should record timings.
///
/// Off unless `TELEVIM_MEASURE` is set, which only `make measure` does. It is
/// an environment variable rather than a flag so that adding it cannot add a
/// user-visible surface, and the cost when it is unset is one `var_os` per
/// event plus a load that is already `None`.
fn measuring() -> bool {
    std::env::var_os("TELEVIM_MEASURE").is_some()
}

/// Set by [`note_launch`], read by the first-frame probe.
static LAUNCHED: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Records the first drawn frame's age, once.
///
/// To the log, never the terminal: a `tracing` line written mid-frame lands on
/// the screen this program is in the middle of drawing (see [`init_tracing`]).
///
/// Only the first frame, and it is the **empty** one: the frame drawn before
/// any network round trip, which with no account is the screen saying it has
/// nothing to connect as. It is not the chat list — no populated chat list is
/// reachable without credentials, and see `docs/memory.md` for why that figure
/// is therefore not measured here.
///
/// Every later frame is the loop doing its ordinary work, and reporting those
/// would fill the log with numbers nobody is measuring.
fn probe_first_frame() {
    // `LAUNCHED` is the gate, and it is empty unless `note_launch` ran while
    // `TELEVIM_MEASURE` was set — so an ordinary run reaches this line, finds
    // `None`, and returns on one atomic load, with no environment lookup and
    // nothing to re-arm. A measured run sets it, and sets it once.
    let Some(launched) = LAUNCHED.get() else {
        return;
    };
    if FIRST_FRAME_REPORTED.swap(true, Ordering::Relaxed) {
        return;
    }
    tracing::info!(
        first_frame_ms = launched.elapsed().as_secs_f64() * 1000.0,
        "first frame drawn"
    );
}

/// Set once the first frame's age has been reported.
static FIRST_FRAME_REPORTED: AtomicBool = AtomicBool::new(false);

/// Records how long a keypress took to reach the screen.
///
/// The interval is from the loop taking the key to the completion of the draw
/// that shows its effect — **not** end to end. Two real costs sit outside it
/// and are therefore not measured by this number or by the harness: the reader
/// thread's blocking `crossterm::event::read`, which is where a keystroke waits
/// for the terminal to deliver it, and the terminal's own paint, which happens
/// after `Terminal::draw` has returned. What this measures is the part the loop
/// is answerable for. To the log, like every other probe here.
fn probe_input_latency(pressed: Instant) {
    if !measuring() {
        return;
    }
    tracing::info!(
        input_latency_ms = pressed.elapsed().as_secs_f64() * 1000.0,
        "keypress drawn"
    );
}

/// The one runtime builder in the program.
///
/// Current-thread, explicitly, whatever `tokio`'s features enable: a TUI has
/// one event loop, so a work-stealing runtime is overhead with nothing to
/// schedule. Both entry paths build theirs here rather than each writing a
/// builder, because the memory budget is a property of the runtime and a second
/// builder is a second thing that can disagree with it.
pub fn build_runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")
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
    let mut screen = stdout();
    execute!(screen, EnterAlternateScreen).context("entering alternate screen")?;

    // Pushed before the reader thread starts, because a key arriving between the
    // two would be read without it — and popped by the guard when this function
    // returns, whatever it returns.
    let keys = EnhancedKeys::push(stdout()).context("asking for disambiguated keys")?;

    let backend = CrosstermBackend::new(screen);
    let mut terminal = Terminal::new(backend).context("creating terminal")?;

    let result = event_loop(cfg, &mut terminal).await;

    // Always restore the terminal, even if the loop errored.
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    drop(keys);

    result
}

/// The [kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/),
/// asked for on the way in and given back on the way out.
///
/// `DISAMBIGUATE_ESCAPE_CODES` is what makes a shifted `Enter` arrive as
/// `Enter` with `SHIFT` set, and a bare `Enter` arrive without it — the
/// difference between "send this" and "new line here" in the input bar. It also
/// makes `Esc` itself unambiguous, which removes a class of "the escape did not
/// register".
///
/// **The protocol is not universal.** In a terminal that does not support it —
/// `xterm` among them — a shifted `Enter` arrives as a bare `Enter`, and under
/// the input line's rule that *sends the message* rather than inserting a
/// newline. There is no way to detect that from inside `crossterm`: the terminal
/// says nothing about whether it honoured the request. Which is why `Ctrl+J`,
/// which needs no protocol at all, is the newline key the bar names first.
///
/// **The pop matters, and it is the part that is easy to get wrong.** Leaving a
/// terminal in an enhanced mode changes how the user's *shell* reads their
/// keyboard after this program exits, and a bare `execute!` at the bottom of a
/// function is skipped on every early return and every `?`. So the pop is a
/// [`Drop`], and dropping the guard is what undoes it.
struct EnhancedKeys<W: Write> {
    out: W,
}

impl<W: Write> EnhancedKeys<W> {
    /// Asks the terminal for the flags, and returns the guard that gives them
    /// back.
    fn push(mut out: W) -> std::io::Result<Self> {
        execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;

        Ok(Self { out })
    }
}

impl<W: Write> Drop for EnhancedKeys<W> {
    fn drop(&mut self) {
        // Nothing to report: the terminal has already been handed back by the
        // time this runs on every path, and a pop that failed is not something
        // the reader could act on.
        let _ = execute!(self.out, PopKeyboardEnhancementFlags);
    }
}

/// Draw, wait for something to happen, apply it, then ask for what comes next.
async fn event_loop(cfg: &Config, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    // When a key was taken on the previous pass, so the next frame can be timed
    // against it. Nothing to report in an ordinary run, hence the `Option`.
    let mut keypress_to_probe: Option<Instant> = None;

    let mut app = App::new();
    "connecting…".clone_into(&mut app.status);

    // What the configuration carries goes into the sign-in flow as pre-fills,
    // and nothing more: the flow is where a phone number, a code and a password
    // are read, and these are what a launch with a reader in a hurry saves them
    // typing. `credentials_configured` is the flag that decides whether there is
    // a flow to put them in at all.
    app.phone = cfg.phone.clone().unwrap_or_default();
    app.code_prefill = cfg.code.clone().unwrap_or_default();
    app.password_prefill = cfg.password.clone().unwrap_or_default();
    app.credentials_configured = cfg.credentials().is_some();

    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();
    spawn_reader(tx.clone());

    // Holds the configuration and this channel, because a sign-out has to rebuild
    // the client and `apply` cannot be handed either.
    let mut network = net::State::new(cfg.clone(), tx.clone());

    // Not awaited: the terminal is already up, and the first frame is worth
    // drawing before a round trip has finished. What it finds out arrives as an
    // event, like everything else.
    net::spawn_bring_up(cfg.clone(), tx.clone());

    loop {
        // Before the draw, and every pass: what a day is called — `Today`
        // rather than a date — depends on when the frame is being read, and the
        // screen owns no clock of its own.
        app.record_now(unix_seconds());

        terminal
            .draw(|frame| app.render(frame))
            .context("drawing frame")?;

        probe_first_frame();

        // The key was taken on the previous pass; what arrives here is the
        // frame that shows what it did.
        if let Some(pressed) = keypress_to_probe {
            probe_input_latency(pressed);
            keypress_to_probe = None;
        }

        if app.should_quit {
            break;
        }

        match tokio::time::timeout(Duration::from_millis(250), rx.recv()).await {
            Ok(Some(AppEvent::Input(Event::Key(key))))
                if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
            {
                app.handle_key(key);
                // The clock only starts for a measured run: an ordinary run
                // leaves the `None` it has, and never reads the environment.
                if keypress_to_probe.is_some() || measuring() {
                    keypress_to_probe = Some(Instant::now());
                }
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

/// What the reader's clock says, in unix seconds.
///
/// The one place wall-clock time is read: the loop's own `Instant`s measure
/// intervals and cannot say what day it is. A clock before the epoch — a
/// machine whose date is set before 1970 — reads as zero rather than as a
/// number that would label every day wrong.
fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
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

    /// Line yanks carry whatever the reader typed, which is multi-byte text as
    /// often as not — conversation yanks are mostly ASCII. The cut is at a
    /// character boundary, so what is encoded must decode to the start of the
    /// yank rather than to half a character.
    #[test]
    fn a_multibyte_yank_encodes_to_its_own_start() {
        use base64::Engine as _;

        let budget = CLIPBOARD_BUDGET / 4 * 3;
        let too_large = "héllo wörld 😀".repeat(budget);

        let cut = clipboard_text(&too_large);
        let encoded = base64::engine::general_purpose::STANDARD.encode(&cut);
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&encoded)
            .expect("it just encoded");

        assert!(
            encoded.len() <= CLIPBOARD_BUDGET,
            "the sequence is {} bytes of a budget of {CLIPBOARD_BUDGET}",
            encoded.len()
        );
        assert_eq!(
            String::from_utf8(decoded).expect("a boundary cut decodes to text"),
            cut,
            "and it is the yank's start, whole characters only"
        );
        assert!(
            too_large.starts_with(&cut),
            "cut from the front, not the middle"
        );
    }

    // ---- the keyboard protocol ------------------------------------------

    /// A writer that hands what it is given to whoever is holding the cell, so a
    /// test can read what the guard wrote *while* the guard still holds it.
    ///
    /// The guard borrows its writer for as long as it lives, which is the whole
    /// of what makes it a guard — so a `&mut Vec<u8>` cannot be looked at until
    /// the guard is gone, and the push is exactly what has to be looked at
    /// before then.
    #[derive(Clone, Default)]
    struct Shared(std::rc::Rc<std::cell::RefCell<Vec<u8>>>);

    impl Shared {
        fn written(&self) -> String {
            String::from_utf8_lossy(&self.0.borrow()).into_owned()
        }
    }

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// The escape sequences are written to a real terminal, so what is asserted
    /// here is the one thing that can be checked without one: that both halves
    /// happen, and that the second one is the guard's rather than a line at the
    /// bottom of a function.
    #[test]
    fn the_keyboard_flags_are_pushed_and_given_back() {
        let out = Shared::default();

        {
            let _keys = EnhancedKeys::push(out.clone()).expect("a cell takes the sequence");
            assert!(
                out.written().contains("\x1b[>1u"),
                "the push is written, and it is the disambiguation flag: {:?}",
                out.written()
            );
        }

        assert!(
            out.written().contains("\x1b[<1u"),
            "and dropping the guard gives them back: {:?}",
            out.written()
        );
    }

    /// The whole reason the pop is a `Drop` and not a statement: every way out
    /// of a function skips the statements after it, and this one is a change to
    /// the terminal the *user's shell* inherits.
    #[test]
    fn the_flags_are_given_back_even_when_the_loop_panics() {
        let out = Shared::default();

        let unwound = std::panic::catch_unwind({
            let out = out.clone();
            std::panic::AssertUnwindSafe(move || {
                let _keys = EnhancedKeys::push(out).expect("a cell takes the sequence");
                panic!("the event loop fell over");
            })
        });

        assert!(unwound.is_err(), "and the panic was not swallowed");
        assert!(
            out.written().contains("\x1b[<1u"),
            "but the terminal was still handed back: {:?}",
            out.written()
        );
    }
}
