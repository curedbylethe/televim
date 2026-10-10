//! Tokio runtime setup + TUI event loop.
//!
//! The loop is the one place the two halves of the program meet: the screen,
//! which knows nothing of the network, and the client, which knows nothing of
//! the screen. Everything either of them has to say arrives as an [`AppEvent`],
//! and every pass through the loop ends by asking the network what the screen is
//! about to need.

use std::collections::VecDeque;
use std::fs::File;
use std::io::Stdout;
use std::io::{Write, stdout};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use crossterm::event::{
    DisableFocusChange, EnableFocusChange, Event, KeyEventKind, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use time::{OffsetDateTime, UtcOffset};
use tokio::sync::mpsc;

use crate::config::Config;
use crate::draft_store::{DraftFile, drafts_acceptable};
use crate::history_store::{HISTORY_CACHE_PEERS, HistoryCache, HistoryFile, history_acceptable};
use crate::media_cache::MediaCache;
use crate::net;
use tui::app::App;
use tui::state::ui::{GraphicsMode, StickerMode};

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

    /// A viewer has exited, reported by the thread that waited for it.
    Viewer(ViewerDone),
}

/// What a viewer's wait reports: the file it was given, and how the viewer ended.
pub(crate) struct ViewerDone {
    path: PathBuf,
    outcome: std::io::Result<ExitStatus>,
}

/// Build a current-thread runtime (memory budget) and run the TUI.
///
/// `config_path` is where the log, the drafts file and the history file go,
/// and it is passed rather than derived so that they cannot disagree about
/// which run they belong to.
///
/// `initial_chat` is the `--chat` id, carried as launch state for STAGE-02 to
/// select on. Cli-only: it never enters `Config`, the file, or the environment.
pub fn run(cfg: &Config, config_path: &Path, initial_chat: Option<i64>) -> Result<()> {
    init_tracing(cfg, config_path);
    let drafts_path = config_path.with_extension("drafts.json");
    let history_path = config_path.with_extension("history.json");
    let media_path = cfg
        .media_cache_dir
        .clone()
        .unwrap_or_else(|| config_path.with_extension("media"));
    build_runtime()?.block_on(run_async(
        cfg,
        &drafts_path,
        &history_path,
        &media_path,
        initial_chat,
    ))
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
/// Only the first frame: the frame drawn before any network round trip, which
/// with no account is the **empty** screen saying it has nothing to connect
/// as. It is not the fetched chat list — no populated chat list is reachable
/// without credentials, and see `docs/memory.md` for why that figure is
/// therefore not measured here. A run with a warm history file draws the
/// cached list in this frame instead, still before any round trip.
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

async fn run_async(
    cfg: &Config,
    drafts_path: &Path,
    history_path: &Path,
    media_path: &Path,
    initial_chat: Option<i64>,
) -> Result<()> {
    enable_raw_mode().context("enabling raw mode")?;
    let mut screen = stdout();
    execute!(screen, EnterAlternateScreen).context("entering alternate screen")?;
    execute!(screen, EnableFocusChange).context("reporting focus changes")?;

    // Pushed before the reader thread starts, because a key arriving between the
    // two would be read without it — and popped by the guard when this function
    // returns, whatever it returns.
    let mut keys = EnhancedKeys::push(stdout()).context("asking for disambiguated keys")?;

    let backend = CrosstermBackend::new(screen);
    let mut terminal = Terminal::new(backend).context("creating terminal")?;

    let result = event_loop(
        cfg,
        &mut terminal,
        &mut keys,
        drafts_path,
        history_path,
        media_path,
        initial_chat,
    )
    .await;

    // Always restore the terminal, even if the loop errored.
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        DisableFocusChange,
        LeaveAlternateScreen
    );
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
///
/// **Suspending for a viewer is an explicit pop and re-push, not a second guard.**
/// The viewer must see the terminal as the shell left it, so the flags come off
/// with [`EnhancedKeys::suspend`] and go back on with [`EnhancedKeys::resume`].
/// `active` says which side of that the terminal is on, so the `Drop` pops only
/// what is actually pushed. Rebuilding the guard instead would have to move it out
/// of `run_async`, and the alternative of a bare escape pair would leave the
/// early-return guarantee above with nothing to hold it.
struct EnhancedKeys<W: Write> {
    out: W,
    active: bool,
}

const ENHANCED_KEY_FLAGS: KeyboardEnhancementFlags =
    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;

impl<W: Write> EnhancedKeys<W> {
    /// Asks the terminal for the flags, and returns the guard that gives them
    /// back.
    fn push(mut out: W) -> std::io::Result<Self> {
        execute!(out, PushKeyboardEnhancementFlags(ENHANCED_KEY_FLAGS))?;

        Ok(Self { out, active: true })
    }

    /// Gives the flags back for a viewer to read the keyboard without them.
    fn suspend(&mut self) -> std::io::Result<()> {
        if self.active {
            execute!(self.out, PopKeyboardEnhancementFlags)?;
            self.active = false;
        }
        Ok(())
    }

    /// Asks for the flags again once the viewer has exited.
    fn resume(&mut self) -> std::io::Result<()> {
        if !self.active {
            execute!(self.out, PushKeyboardEnhancementFlags(ENHANCED_KEY_FLAGS))?;
            self.active = true;
        }
        Ok(())
    }
}

impl<W: Write> Drop for EnhancedKeys<W> {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        // Nothing to report: the terminal has already been handed back by the
        // time this runs on every path, and a pop that failed is not something
        // the reader could act on.
        let _ = execute!(self.out, PopKeyboardEnhancementFlags);
    }
}

/// Gives the terminal to a child process: leaves the alternate screen and raw
/// mode, pops the keyboard flags, and shows the cursor again.
///
/// The reverse of [`run_async`]'s setup, in reverse order. Every step is the
/// same one the shutdown takes, so a viewer and the shell see the same terminal.
fn suspend_tui(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    keys: &mut EnhancedKeys<Stdout>,
) -> std::io::Result<()> {
    execute!(
        terminal.backend_mut(),
        DisableFocusChange,
        LeaveAlternateScreen
    )?;
    disable_raw_mode()?;
    keys.suspend()?;
    terminal.show_cursor()
}

/// Takes the terminal back from a child process: the setup order of
/// [`run_async`], then a full redraw.
///
/// The clear is what makes the next frame a complete one: the child may have
/// written anywhere on the screen, and ratatui's diff would otherwise leave
/// those cells as they are.
fn resume_tui(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    keys: &mut EnhancedKeys<Stdout>,
) -> std::io::Result<()> {
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableFocusChange
    )?;
    enable_raw_mode()?;
    keys.resume()?;
    terminal.hide_cursor()?;
    terminal.clear()
}

/// Reacts to the terminal gaining or losing focus.
///
/// A blur can strand a half-typed key or reset the terminal's keyboard flags,
/// so every change clears the latches and a gain puts the flags back. Focus
/// events never reach the key chain.
fn focus_changed(
    app: &mut App,
    keys: &mut EnhancedKeys<Stdout>,
    gained: bool,
) -> std::io::Result<()> {
    app.reset_pending_input();
    if gained {
        keys.resume()?;
    }
    Ok(())
}

/// The platform's file opener, and the arguments that come before the path.
///
/// `open -W` on macOS waits for the application to quit; without `-W` it returns
/// as soon as the file is handed over, and the terminal would come back under a
/// running viewer. The Windows form is best-effort and untested: `start` returns
/// without waiting, so there the terminal comes back at once.
#[cfg(target_os = "macos")]
const OPENER: (&str, &[&str]) = ("open", &["-W"]);
#[cfg(all(unix, not(target_os = "macos")))]
const OPENER: (&str, &[&str]) = ("xdg-open", &[]);
#[cfg(windows)]
const OPENER: (&str, &[&str]) = ("cmd", &["/C", "start", ""]);

/// The command that opens `path` in the platform's viewer.
///
/// Inherits the terminal's stdio, which is what a viewer needs and what keeps the
/// program's own output off the screen: nothing is piped. `status()` is what
/// waits, and the caller holds the loop until it returns.
fn viewer_command(path: &Path) -> Command {
    let (program, args) = OPENER;
    let mut command = Command::new(program);
    command.args(args).arg(path);
    command
}

/// The three steps a viewer takes, so the order can be tested without a tty.
///
/// `spawn` starts the viewer and returns without waiting for it: the wait runs on
/// its own thread and reports to `done`, so the loop is never parked in it.
trait Viewer {
    fn suspend(&mut self) -> std::io::Result<()>;
    fn spawn(
        &mut self,
        path: PathBuf,
        done: mpsc::UnboundedSender<AppEvent>,
    ) -> std::io::Result<()>;
    fn resume(&mut self) -> std::io::Result<()>;
}

/// The terminal as the viewer sees it: the real suspend, spawn and resume.
struct TuiViewer<'a> {
    terminal: &'a mut Terminal<CrosstermBackend<Stdout>>,
    keys: &'a mut EnhancedKeys<Stdout>,
}

impl Viewer for TuiViewer<'_> {
    fn suspend(&mut self) -> std::io::Result<()> {
        suspend_tui(self.terminal, self.keys)
    }

    fn spawn(
        &mut self,
        path: PathBuf,
        done: mpsc::UnboundedSender<AppEvent>,
    ) -> std::io::Result<()> {
        std::thread::Builder::new()
            .name("viewer".to_owned())
            .spawn(move || {
                let outcome = viewer_command(&path).status();
                // Nothing waits for this once the loop has ended, so a closed
                // channel is not an error.
                let _ = done.send(AppEvent::Viewer(ViewerDone { path, outcome }));
            })
            .map(|_handle| ())
    }

    fn resume(&mut self) -> std::io::Result<()> {
        resume_tui(self.terminal, self.keys)
    }
}

/// The viewers the loop has been asked to open, one at a time.
///
/// Queued paths wait here, and `open` is the one whose viewer holds the terminal.
/// Nothing starts while one is open, so a second download settling mid-viewer
/// waits its turn rather than stacking a second viewer over the first.
#[derive(Default)]
struct Viewers {
    queued: VecDeque<PathBuf>,
    open: Option<PathBuf>,
}

impl Viewers {
    /// Whether a viewer holds the terminal. The loop draws nothing while it does.
    fn busy(&self) -> bool {
        self.open.is_some()
    }

    fn queue(&mut self, paths: Vec<PathBuf>) {
        self.queued.extend(paths);
    }

    /// Opens the next queued path, unless a viewer already holds the terminal.
    ///
    /// A failed suspend or spawn is a refusal, not an error: the terminal is taken
    /// back and the refusal says so on the status line. The outer `Result` is a
    /// failed restore, which ends the loop the way a failed draw does.
    fn start_next<V: Viewer>(
        &mut self,
        viewer: &mut V,
        app: &mut App,
        done: &mpsc::UnboundedSender<AppEvent>,
    ) -> Result<()> {
        if self.busy() {
            return Ok(());
        }
        let Some(path) = self.queued.pop_front() else {
            return Ok(());
        };
        app.set_status(format!("Opening {}…", path.display()));
        let launched = viewer
            .suspend()
            .and_then(|()| viewer.spawn(path.clone(), done.clone()));
        match launched {
            Ok(()) => self.open = Some(path),
            Err(error) => {
                viewer
                    .resume()
                    .context("taking the terminal back from the viewer")?;
                app.flash(viewer_sentence(&Err(error), &path));
            }
        }
        Ok(())
    }

    /// Takes the terminal back once the viewer's wait has reported, and says how it went.
    fn finish<V: Viewer>(
        &mut self,
        viewer: &mut V,
        app: &mut App,
        done: &ViewerDone,
    ) -> Result<()> {
        self.open = None;
        viewer
            .resume()
            .context("taking the terminal back from the viewer")?;
        app.flash(viewer_sentence(&done.outcome, &done.path));
        Ok(())
    }
}

/// The status sentence for a viewer's outcome.
///
/// A missing opener and a spawn failure are refusals, and the path is named in
/// both so the reader can open the file by hand: the file stays on disk.
fn viewer_sentence(outcome: &std::io::Result<ExitStatus>, path: &Path) -> String {
    match outcome {
        Ok(status) => match status.code() {
            Some(code) => format!("Viewer exited (code {code})"),
            None => "Viewer exited (no exit code)".to_owned(),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => format!(
            "Cannot open media: {} is not installed; the file is at {}",
            OPENER.0,
            path.display()
        ),
        Err(error) => format!(
            "Cannot open media: {error}; the file is at {}",
            path.display()
        ),
    }
}

/// Drops what the channel buffered while the viewer had the terminal.
///
/// Input is discarded: a key typed into a viewer that the reader thread picked up
/// and queued is not one the reader meant for the program, and replaying it on
/// resume would act on it twice. This is the Q1 decision; the byte the reader
/// thread captured mid-suspend is the residual race, documented in
/// `docs/known-gaps.md`. Network events are applied, not dropped, because a page
/// that landed during the viewer is still a page.
fn drain_while_suspended(
    rx: &mut mpsc::UnboundedReceiver<AppEvent>,
    app: &mut App,
    network: &mut net::State,
) {
    while let Ok(event) = rx.try_recv() {
        if let AppEvent::Net(event) = event {
            net::apply(app, network, event);
        }
    }
}

/// Draw, wait for something to happen, apply it, then ask for what comes next.
///
/// `initial_chat` is the `--chat` id, recorded below and selected from the
/// cached chat list when it holds the id (see `net::open_from_cache`), or
/// otherwise once the first fetched list lands (see
/// `net::apply_ready_to_screen`).
///
/// `drafts_path` is the drafts file beside the configuration: loaded here
/// once, re-synced every pass, and cleared on sign-out.
///
/// `history_path` is the history file beside it: loaded here once, handed to
/// the network state with what it held, written behind every page that
/// changes it, and removed on sign-out.
async fn event_loop(
    cfg: &Config,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    keys: &mut EnhancedKeys<Stdout>,
    drafts_path: &Path,
    history_path: &Path,
    media_path: &Path,
    initial_chat: Option<i64>,
) -> Result<()> {
    // When a key was taken on the previous pass, so the next frame can be timed
    // against it. Nothing to report in an ordinary run, hence the `Option`.
    let mut keypress_to_probe: Option<Instant> = None;

    // The bidi mode is read once, here, and is fixed for the life of the
    // application: it is an input to the layout, so a mode that could change
    // while the window is open would make the same conversation two different
    // heights depending on when it was asked.
    let mut app = App::new()
        .with_bidi(cfg.bidi_mode())
        .with_stickers(cfg.sticker_mode())
        .with_graphics(cfg.graphics_mode(|key| std::env::var(key).ok()));
    if let Some(id) = initial_chat {
        app.set_initial_chat(id);
    }
    "connecting…".clone_into(&mut app.ui.status);

    // The drafts file beside the configuration, loaded once at launch. A file
    // tagged for another account never seeds this one: a peer id can be
    // reused across accounts, and inheriting a stranger's words is worse
    // than losing one's own. Either side unnamed accepts the file — a false
    // clear loses words, a false accept is corrected by park/resume.
    let draft_file = DraftFile::new(drafts_path.to_path_buf());
    let loaded = draft_file.load();
    if drafts_acceptable(loaded.account.as_deref(), cfg.phone.as_deref()) {
        app.drafts.restore(loaded.drafts);
        app.drafts.restore_read_marks(loaded.reads);
    } else {
        tracing::warn!("drafts stored for another account were discarded");
    }
    // Nothing synced yet, so the first pass always writes: a discarded file
    // is overwritten rather than left behind, and a launch with nothing
    // stored settles the file (or its absence) at once.
    let mut last_synced: Option<SyncedDrafts> = None;

    // The history file beside it, loaded once at launch too, under the
    // stricter rule: only a file saved under exactly this account seeds the
    // cache, because a false accept paints a stranger's conversation and a
    // false clear costs one fetch. A file that may not seed is removed rather
    // than left for the first write to overwrite, so another account's
    // messages do not wait on disk for a page that may never come.
    let history_file = HistoryFile::new(history_path.to_path_buf());
    let loaded_history = history_file.load();
    let history = if history_acceptable(loaded_history.account.as_deref(), cfg.phone.as_deref()) {
        loaded_history.cache
    } else {
        if loaded_history.cache != HistoryCache::default() {
            tracing::warn!("history stored for another account was discarded");
        }
        history_file.clear();
        HistoryCache::default()
    };

    // What the configuration carries goes into the sign-in flow as pre-fills,
    // and nothing more: the flow is where a phone number, a code and a password
    // are read, and these are what a launch with a reader in a hurry saves them
    // typing. `credentials_configured` is the flag that decides whether there is
    // a flow to put them in at all.
    app.session.phone = cfg.phone.clone().unwrap_or_default();
    app.session.code_prefill = cfg.code.clone().unwrap_or_default();
    app.session.password_prefill = cfg.password.clone().unwrap_or_default();
    app.session.credentials_configured = cfg.credentials().is_some();

    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();
    spawn_reader(tx.clone());

    // Holds the configuration and this channel, because a sign-out has to rebuild
    // the client and `apply` cannot be handed either.
    let mut network = net::State::new(cfg.clone(), tx.clone());
    // And the drafts file, so signing out can remove it.
    network.set_draft_file(draft_file.clone());
    // Opened beside the history file. A directory written for another account,
    // or for none, is cleared on the way in: the history's strict rule.
    network.set_media_cache(MediaCache::open(
        media_path.to_path_buf(),
        cfg.phone.as_deref(),
        cfg.media_cache_max_bytes,
    ));
    // And the history file and what it held: every fetched page is merged
    // into the cache and written behind, and the messages go with the account
    // that is leaving.
    network.set_history_file(history_file);
    network.restore_history(history);
    // Before the first frame: a warm cache draws the reader's list and their
    // conversation now, under `connecting…`, rather than after the round trip
    // the bring-up below is about to start.
    net::open_from_cache(&mut app, &mut network);

    // Not awaited: the terminal is already up, and the first frame is worth
    // drawing before a round trip has finished. What it finds out arrives as an
    // event, like everything else.
    net::spawn_bring_up(cfg.clone(), tx.clone());

    let mut viewers = Viewers::default();

    loop {
        // Nothing is drawn while a viewer holds the terminal: the screen is the
        // viewer's until it hands it back, and a frame written now lands on the
        // shell's. The loop still runs the rest of the pass.
        if !viewers.busy() {
            draw_frame(terminal, &app)?;
        }

        // The key was taken on the previous pass; what arrives here is the
        // frame that shows what it did.
        if let Some(pressed) = keypress_to_probe {
            probe_input_latency(pressed);
            keypress_to_probe = None;
        }

        if app.ui.should_quit && !viewers.busy() {
            break;
        }

        match tokio::time::timeout(Duration::from_millis(250), rx.recv()).await {
            // Input the reader took while the viewer had the terminal is dropped,
            // as it always was: it was not typed into the program.
            Ok(Some(AppEvent::Input(_))) if viewers.busy() => {}
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
            Ok(Some(AppEvent::Input(Event::FocusGained))) => focus_changed(&mut app, keys, true)?,
            Ok(Some(AppEvent::Input(Event::FocusLost))) => focus_changed(&mut app, keys, false)?,
            Ok(Some(AppEvent::Input(_))) | Err(_) => {}
            Ok(Some(AppEvent::Net(event))) => net::apply(&mut app, &mut network, event),
            Ok(Some(AppEvent::Viewer(done))) => {
                viewers.finish(&mut TuiViewer { terminal, keys }, &mut app, &done)?;
                drain_while_suspended(&mut rx, &mut app, &mut network);
            }
            Ok(None) => break,
        }

        // Every pass rather than every keystroke: the tick is what turns a
        // reader who scrolled to the top and stopped into a page request, and
        // what starts the next page once one has landed.
        net::drive(&mut app, &mut network, &tx);
        // Stickers the panel asked for while drawing are taken off the queue
        // here and downloaded in a task, so the tick never waits on the network:
        // each answer comes back as an event and settles on the next pass. Flag
        // off never reaches the spawn, so no download traffic runs.
        if let Some(client) = network.client() {
            let batch = sticker_batch(app.sticker_mode(), &mut app);
            let download = move |chat_id: i64, message_id: i64| {
                let client = std::sync::Arc::clone(&client);
                async move { client.download_media(chat_id, message_id).await }
            };
            spawn_sticker_drain(batch, download, tx.clone());
        }
        // The clipboard is written to the terminal, so it waits for the viewer
        // to hand the terminal back: the yank is kept until then.
        if !viewers.busy() {
            copy_if_asked(&mut app);
        }
        // Queued paths open one viewer at a time, and the wait runs off this
        // thread: the loop carries on while one is open, and the next starts when
        // it reports (see `Viewers::start_next` and `Viewers::finish`).
        viewers.queue(network.take_media());
        viewers.start_next(&mut TuiViewer { terminal, keys }, &mut app, &tx)?;
        sync_drafts(&app, &draft_file, &mut last_synced, cfg.phone.as_deref());
        // Unlike the drafts, written off the loop's thread: the file is up to
        // a window per conversation, and it is a write behind the pages that
        // already reached the screen, so nothing waits on it. The chat list
        // goes with it, read off the screen, where every change to it has
        // already landed.
        network.persist_history(app.chats());
    }

    network.finish_history(app.chats()).await;
    Ok(())
}

/// Draws one frame, and places its pictures when the terminal takes them.
fn draw_frame(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &App) -> Result<()> {
    // Before the draw, and every pass: what a day is called — `Today` rather than
    // a date — depends on when the frame is being read, and the screen owns no
    // clock of its own.
    record_clock(app);

    terminal
        .draw(|frame| app.render(frame))
        .context("drawing frame")?;
    if app.graphics() == GraphicsMode::Kitty {
        place_pictures(app);
    }

    probe_first_frame();
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

/// Records what the reader's clock says and the zone it is read in, for one pass.
/// The instant and its offset are read together, so they always name one moment.
fn record_clock(app: &App) {
    let now = unix_seconds();
    app.record_now(now, local_offset(now));
}

/// The reader's UTC offset at `now`, in seconds east of UTC.
///
/// Read on every pass beside `unix_seconds`, so a DST change during a long
/// session is picked up on the next frame rather than baked in at launch. UTC
/// when no local zone can be read: `time` returns an error rather than a guess
/// when the platform will not say, and UTC is what every label showed before
/// the zone was read at all.
fn local_offset(now: i64) -> i64 {
    OffsetDateTime::from_unix_timestamp(now)
        .ok()
        .and_then(|at| UtcOffset::local_offset_at(at).ok())
        .map_or(0, |offset| i64::from(offset.whole_seconds()))
}

/// Takes one tick's sticker requests off the queue, to be downloaded.
///
/// With [`StickerMode::Token`] the requests are dropped, never downloaded:
/// flag off means zero fetch traffic, and the cache stays empty so geometry
/// and draw take the token path on their own.
fn sticker_batch(mode: StickerMode, app: &mut App) -> Vec<(i64, i64)> {
    let pending = app.conversation.stickers.take_pending();
    if mode != StickerMode::Inline {
        return Vec::new();
    }
    pending
}

/// Downloads a batch of `(chat_id, message_id)` pairs in a spawned task, and
/// sends each outcome back as a [`net::Event::StickerSettled`].
///
/// The loop calls this and returns to its channel at once: no download is
/// awaited on the loop's thread. Within the batch the downloads run in series,
/// so one task per batch rather than one per request keeps task churn down on
/// a sticker wall. A failed download is sent back as a settle, not retried
/// here: settling releases the in-flight mark, and the next miss re-requests.
///
/// The downloader is a parameter rather than the client, so the plumbing is
/// testable without a datacenter: production passes a closure over the
/// client's download, and a test passes a counter.
fn spawn_sticker_drain<F, Fut, E>(
    batch: Vec<(i64, i64)>,
    download: F,
    tx: mpsc::UnboundedSender<AppEvent>,
) where
    F: Fn(i64, i64) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<Vec<u8>, E>> + Send,
    E: std::fmt::Display,
{
    if batch.is_empty() {
        return;
    }

    tokio::spawn(async move {
        for (chat_id, message_id) in batch {
            let fetched = download(chat_id, message_id)
                .await
                .map_err(|error| error.to_string());
            let settled = net::Event::StickerSettled {
                chat_id,
                message_id,
                fetched,
            };
            if tx.send(AppEvent::Net(settled)).is_err() {
                break;
            }
        }
    });
}

/// Writes the kitty graphics bytes for the frame just drawn, over its sticker
/// blocks.
///
/// Best-effort, like the clipboard: the frame is already on the screen, and a
/// picture that did not land is a block of blank cells until the next pass
/// places it again.
fn place_pictures(app: &App) {
    let bytes = tui::graphics::frame(app);
    let mut out = stdout();
    if let Err(error) = out.write_all(bytes.as_bytes()).and_then(|()| out.flush()) {
        tracing::debug!(%error, "the picture placement did not go through");
    }
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

/// Writes the drafts file when the snapshot changed, and removes it when the
/// snapshot is empty.
///
/// Every pass rather than on park or send: `resume_draft` moves the entry out
/// of the map into the live line, so a park-only file would go stale the
/// moment the reader returns to the chat — the snapshot is the parked map
/// *plus* the live line, compared as a sorted vec with no dirty flag.
/// Change-gated, so a quiet loop costs one small vec compare per 250 ms tick
/// and no IO; the file sees at most one atomic rename a tick.
///
/// A kill inside the pass between a send and this sync resurrects the sent
/// text as a draft: the window is one tick wide, the alternative puts IO on
/// the keystroke path for no measurable gain, and a resurrected sent line is
/// visible and deletable rather than silent loss.
///
/// An empty snapshot removes the file rather than writing an empty payload:
/// no words means nothing to keep, and logout's `clear` then survives the
/// passes that follow it instead of being rewritten by them.
fn sync_drafts(
    app: &App,
    file: &DraftFile,
    last: &mut Option<SyncedDrafts>,
    account: Option<&str>,
) {
    let chat_id = app.conversation.conversation.window.chat_id;
    let drafts = app.drafts.snapshot(Some((chat_id, &app.input.line)));
    let reads = app.drafts.recent_read_marks(HISTORY_CACHE_PEERS);
    if let Some((last_drafts, last_reads)) = last.as_ref()
        && *last_drafts == drafts
        && *last_reads == reads
    {
        return;
    }
    if drafts.is_empty() && reads.is_empty() {
        file.clear();
    } else {
        file.save(&drafts, &reads, account);
    }
    *last = Some((drafts, reads));
}

/// What [`sync_drafts`] last wrote: the parked words and the read marks.
type SyncedDrafts = (Vec<(i64, String)>, Vec<(i64, i64)>);

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
    use tui::state::ui::StickerMode;

    /// The download, as a request counter: records every pair it is asked
    /// for, and answers with bytes that will not decode — so settling, not
    /// picturing, is what these tests pin down.
    struct Counter {
        calls: std::sync::Mutex<Vec<(i64, i64)>>,
    }

    impl Counter {
        fn new() -> Self {
            Self {
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<(i64, i64)> {
            self.calls
                .lock()
                .expect("the counter is not poisoned")
                .clone()
        }

        fn download(
            &self,
            chat_id: i64,
            message_id: i64,
        ) -> std::future::Ready<Result<Vec<u8>, String>> {
            self.calls
                .lock()
                .expect("the counter is not poisoned")
                .push((chat_id, message_id));
            std::future::ready(Ok(vec![0xDE, 0xAD]))
        }
    }

    // ---- the drafts sync --------------------------------------------------

    /// A loop pass writes the snapshot on change, skips a quiet pass, and
    /// removes the file when nothing is left — the whole per-tick contract
    /// without a terminal or a tick.
    #[test]
    fn the_sync_writes_on_change_skips_quiet_passes_and_removes_when_empty() {
        use std::borrow::Cow;

        use domain::chat::{Chat, ChatKind};
        use domain::message::{Message, MessageStatus};

        let dir = tempfile::tempdir().expect("a scratch directory");
        let file = DraftFile::new(dir.path().join("televim.drafts.json"));

        let mut app = App::new();
        app.set_chats(vec![Chat {
            read_outbox_max_id: None,
            id: 7,
            title: "seven".to_owned(),
            kind: ChatKind::Private,
            last_message: None,
            unread_count: 0,
            last_message_id: None,
            last_timestamp: None,
            pinned: false,
            presence: None,
            deleted: false,
        }]);
        app.select_chat(0);
        app.apply_latest(vec![Message {
            id: 1,
            chat_id: 7,
            text: Cow::Borrowed("first"),
            timestamp: 1,
            status: MessageStatus::Received,
            is_outgoing: false,
            reply_to: None,
            media: None,
            media_id: None,
        }]);
        app.drafts.restore(vec![(7, "unsent".to_owned())]);

        let mut last: Option<SyncedDrafts> = None;
        sync_drafts(&app, &file, &mut last, Some("+1555"));
        assert_eq!(
            file.load().drafts,
            vec![(7, "unsent".to_owned())],
            "the first pass writes what the launch loaded"
        );

        let before = std::fs::read(dir.path().join("televim.drafts.json"))
            .expect("the file the first pass wrote");
        sync_drafts(&app, &file, &mut last, Some("+1555"));
        let after =
            std::fs::read(dir.path().join("televim.drafts.json")).expect("the file is still there");
        assert_eq!(before, after, "a quiet pass writes nothing");

        // Empty the map the way a send does: the entry leaves, the line is
        // already empty, and the snapshot is nothing.
        let mut empty = App::new();
        empty.set_chats(vec![Chat {
            read_outbox_max_id: None,
            id: 7,
            title: "seven".to_owned(),
            kind: ChatKind::Private,
            last_message: None,
            unread_count: 0,
            last_message_id: None,
            last_timestamp: None,
            pinned: false,
            presence: None,
            deleted: false,
        }]);
        empty.select_chat(0);
        sync_drafts(&empty, &file, &mut last, Some("+1555"));
        assert!(
            !dir.path().join("televim.drafts.json").exists(),
            "no words means no file, so logout's clear survives later passes"
        );
    }

    /// The read marks ride the same file: a restored mark is written by the
    /// first pass, reloads for the same account, and is removed with the file
    /// when a sign-out forgets it.
    #[test]
    fn the_sync_persists_read_marks_until_a_sign_out_forgets_them() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let file = DraftFile::new(dir.path().join("televim.drafts.json"));
        let mut app = App::new();

        app.drafts.restore_read_marks(vec![(7, 9)]);
        let mut last: Option<SyncedDrafts> = None;
        sync_drafts(&app, &file, &mut last, Some("+1555"));
        assert_eq!(file.load().reads, vec![(7, 9)], "the mark is on disk");

        app.drafts.clear_read_marks();
        sync_drafts(&app, &file, &mut last, Some("+1555"));
        assert!(
            !dir.path().join("televim.drafts.json").exists(),
            "forgotten marks leave no file behind"
        );
    }

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

    /// Spawning the drain returns before any download runs: the loop hands the
    /// batch off and goes back to its channel, and the fetch happens only once
    /// the task is driven.
    #[tokio::test]
    async fn spawning_the_drain_downloads_nothing_on_the_loop() {
        let downloads = std::sync::Arc::new(Counter::new());
        let mut app = App::new();
        app.conversation.stickers.request(42, 7);
        let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();

        let batch = sticker_batch(StickerMode::Inline, &mut app);
        let counted = std::sync::Arc::clone(&downloads);
        spawn_sticker_drain(batch, move |c, m| counted.download(c, m), tx);

        assert!(
            downloads.calls().is_empty(),
            "the call that spawns the drain returned before the task ran"
        );
        assert!(
            matches!(
                rx.recv().await,
                Some(AppEvent::Net(net::Event::StickerSettled { .. }))
            ),
            "the answer comes back as a settle event"
        );
        assert_eq!(downloads.calls(), vec![(42, 7)]);
    }

    /// Inline, the drain downloads every requested pair exactly once, in
    /// order, and each answer settles through the net loop — here a refusal,
    /// which releases the pair rather than caching anything.
    #[tokio::test]
    async fn the_drain_downloads_every_request_and_settles_it() {
        let downloads = std::sync::Arc::new(Counter::new());
        let mut app = App::new();
        let mut state = net::State::default();
        app.conversation.stickers.request(42, 7);
        app.conversation.stickers.request(42, 8);
        app.conversation.stickers.request(42, 7);
        let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();

        let batch = sticker_batch(StickerMode::Inline, &mut app);
        let counted = std::sync::Arc::clone(&downloads);
        spawn_sticker_drain(batch, move |c, m| counted.download(c, m), tx);

        for _ in 0..2 {
            let Some(AppEvent::Net(event)) = rx.recv().await else {
                panic!("the drain answers each pair with a settle event");
            };
            net::apply(&mut app, &mut state, event);
        }

        assert_eq!(
            downloads.calls(),
            vec![(42, 7), (42, 8)],
            "one download per message, in order"
        );
        assert!(
            app.conversation.stickers.take_pending().is_empty(),
            "settled pairs are not asked for again"
        );
        assert!(
            app.conversation.stickers.get(7).is_none(),
            "an undecodable answer caches nothing"
        );
    }

    /// Flag off, the drain never downloads: the requests are dropped, nothing
    /// is spawned, and the cache stays empty — so every sticker message draws
    /// `[sticker]` with zero fetch traffic.
    #[tokio::test]
    async fn flag_off_means_zero_fetch_traffic() {
        let downloads = std::sync::Arc::new(Counter::new());
        let mut app = App::new();
        app.conversation.stickers.request(42, 7);
        let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();

        let batch = sticker_batch(StickerMode::Token, &mut app);
        let counted = std::sync::Arc::clone(&downloads);
        spawn_sticker_drain(batch, move |c, m| counted.download(c, m), tx);
        tokio::task::yield_now().await;

        assert!(
            downloads.calls().is_empty(),
            "no download ran for a dropped request"
        );
        assert!(rx.try_recv().is_err(), "and no settle came back");
        assert!(
            app.conversation.stickers.take_pending().is_empty(),
            "and the dropped requests do not pile up"
        );
        assert!(
            app.conversation.stickers.get(7).is_none(),
            "so the token path is all there is to draw"
        );
    }

    /// The viewer gets the keyboard back as the shell left it, and the reader
    /// gets it back after: suspend pops once, a second suspend writes nothing,
    /// and resume pushes again.
    #[test]
    fn suspend_and_resume_pop_and_push_the_flags_once_each() {
        let out = Shared::default();
        let mut keys = EnhancedKeys::push(out.clone()).expect("a cell takes the sequence");

        keys.suspend().expect("a cell takes the sequence");
        let after_suspend = out.written();
        keys.suspend().expect("a cell takes the sequence");
        assert_eq!(out.written(), after_suspend, "a second suspend is a no-op");

        keys.resume().expect("a cell takes the sequence");
        keys.resume().expect("a cell takes the sequence");
        assert_eq!(
            out.written().matches("\x1b[>1u").count(),
            2,
            "one push at push and one at resume: {:?}",
            out.written()
        );
        assert_eq!(
            out.written().matches("\x1b[<1u").count(),
            1,
            "one pop at suspend, none from the drop while active: {:?}",
            out.written()
        );
    }

    /// Suspended, the guard pops nothing on drop: the flags are already off.
    #[test]
    fn dropping_a_suspended_guard_does_not_pop_twice() {
        let out = Shared::default();
        {
            let mut keys = EnhancedKeys::push(out.clone()).expect("a cell takes the sequence");
            keys.suspend().expect("a cell takes the sequence");
        }

        assert_eq!(
            out.written().matches("\x1b[<1u").count(),
            1,
            "{:?}",
            out.written()
        );
    }

    /// The path is the last argument, so the opener never reads a flag from it.
    #[test]
    fn the_viewer_is_the_platform_opener_on_the_file() {
        let path = Path::new("/tmp/televim-1-2-3.jpg");
        let command = viewer_command(path);

        assert_eq!(command.get_program(), OPENER.0);
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args.last().copied(), Some(path.as_os_str()));
        assert_eq!(args.len(), OPENER.1.len() + 1);
    }

    /// The sentences the reader sees for each outcome: an exit code, an exit
    /// with no code, and the two refusals, which both keep the path so the file
    /// can be opened by hand.
    #[cfg(unix)]
    #[test]
    fn a_viewer_outcome_is_one_status_sentence() {
        use std::os::unix::process::ExitStatusExt as _;

        let path = Path::new("/tmp/televim-1-2-3.jpg");

        let exited = std::process::ExitStatus::from_raw(3 << 8);
        assert_eq!(viewer_sentence(&Ok(exited), path), "Viewer exited (code 3)");

        let signalled = std::process::ExitStatus::from_raw(9);
        assert_eq!(
            viewer_sentence(&Ok(signalled), path),
            "Viewer exited (no exit code)"
        );

        let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
        let sentence = viewer_sentence(&Err(missing), path);
        assert!(sentence.starts_with("Cannot open media: "), "{sentence}");
        assert!(
            sentence.ends_with("the file is at /tmp/televim-1-2-3.jpg"),
            "{sentence}"
        );

        let refused = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let sentence = viewer_sentence(&Err(refused), path);
        assert!(sentence.starts_with("Cannot open media: "), "{sentence}");
        assert!(
            sentence.ends_with("the file is at /tmp/televim-1-2-3.jpg"),
            "{sentence}"
        );
    }

    /// Records the steps in order, and fails the ones asked to. The wait does not
    /// run here: a successful spawn reports its outcome on the channel, as the
    /// real one does from its thread, and the test takes that event the way the
    /// loop would.
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt as _;

    #[derive(Default)]
    struct FakeViewer {
        log: Vec<String>,
        fail_suspend: bool,
        fail_spawn: bool,
    }

    impl Viewer for FakeViewer {
        fn suspend(&mut self) -> std::io::Result<()> {
            self.log.push("suspend".to_owned());
            if self.fail_suspend {
                return Err(std::io::Error::other("no tty"));
            }
            Ok(())
        }

        fn spawn(
            &mut self,
            path: PathBuf,
            done: mpsc::UnboundedSender<AppEvent>,
        ) -> std::io::Result<()> {
            self.log.push(format!("spawn {}", path.display()));
            if self.fail_spawn {
                return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
            }
            let _ = done.send(AppEvent::Viewer(ViewerDone {
                path,
                outcome: Ok(ExitStatus::from_raw(0)),
            }));
            Ok(())
        }

        fn resume(&mut self) -> std::io::Result<()> {
            self.log.push("resume".to_owned());
            Ok(())
        }
    }

    fn chat(id: i64) -> domain::chat::Chat {
        domain::chat::Chat {
            read_outbox_max_id: None,
            id,
            title: format!("chat-{id}"),
            kind: domain::chat::ChatKind::Private,
            last_message: None,
            unread_count: 0,
            last_message_id: None,
            last_timestamp: None,
            pinned: false,
            presence: None,
            deleted: false,
        }
    }

    /// The event the wait reported, which the loop would receive next.
    fn next_done(rx: &mut mpsc::UnboundedReceiver<AppEvent>) -> ViewerDone {
        match rx.try_recv() {
            Ok(AppEvent::Viewer(done)) => done,
            _ => panic!("the wait reports its outcome before the loop asks"),
        }
    }

    /// The terminal is given away, the viewer runs on the file, and the terminal
    /// comes back once after it, in that order.
    #[cfg(unix)]
    #[test]
    fn the_viewer_runs_between_suspend_and_one_resume() {
        let path = PathBuf::from("/tmp/televim-1-2-3.jpg");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new();
        let mut viewers = Viewers::default();
        let mut viewer = FakeViewer::default();
        viewers.queue(vec![path]);

        viewers
            .start_next(&mut viewer, &mut app, &tx)
            .expect("the start goes through");
        assert!(viewers.busy());
        let done = next_done(&mut rx);
        viewers
            .finish(&mut viewer, &mut app, &done)
            .expect("resume succeeds");

        assert!(!viewers.busy());
        assert_eq!(
            viewer.log,
            ["suspend", "spawn /tmp/televim-1-2-3.jpg", "resume"]
        );
    }

    /// A spawn that fails still takes the terminal back, once, and the failure is
    /// a refusal on the status line rather than an error that ends the loop.
    #[cfg(unix)]
    #[test]
    fn the_terminal_comes_back_when_the_spawn_fails() {
        let path = PathBuf::from("/tmp/televim-1-2-3.jpg");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new();
        let mut viewers = Viewers::default();
        let mut viewer = FakeViewer {
            fail_spawn: true,
            ..FakeViewer::default()
        };
        viewers.queue(vec![path]);

        viewers
            .start_next(&mut viewer, &mut app, &tx)
            .expect("resume succeeds");

        assert!(!viewers.busy(), "a refused viewer holds nothing");
        assert!(rx.try_recv().is_err(), "nothing is left to wait for");
        assert_eq!(
            viewer.log,
            ["suspend", "spawn /tmp/televim-1-2-3.jpg", "resume"]
        );
        assert!(app.status_text().contains("/tmp/televim-1-2-3.jpg"));
    }

    /// A suspend that fails never spawns the viewer, and the terminal is still
    /// taken back once.
    #[cfg(unix)]
    #[test]
    fn a_failed_suspend_spawns_nothing_and_still_resumes_once() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new();
        let mut viewers = Viewers::default();
        let mut viewer = FakeViewer {
            fail_suspend: true,
            ..FakeViewer::default()
        };
        viewers.queue(vec![PathBuf::from("/tmp/televim-1-2-3.jpg")]);

        viewers
            .start_next(&mut viewer, &mut app, &tx)
            .expect("resume succeeds");

        assert!(!viewers.busy());
        assert_eq!(viewer.log, ["suspend", "resume"]);
    }

    /// GAP 4 and GAP 5: while a viewer is open the loop is not parked. The wait
    /// is reported later, and the same conversation and cursor are on the screen
    /// when the terminal comes back: no restart, one resume.
    #[cfg(unix)]
    #[test]
    fn a_completed_viewer_returns_to_the_same_conversation_and_cursor() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new();
        app.set_chats(vec![chat(7)]);
        app.select_chat(0);
        app.conversation.vim.set_total(5);
        app.conversation.vim.set_cursor(3);
        let mut viewers = Viewers::default();
        let mut viewer = FakeViewer::default();
        viewers.queue(vec![PathBuf::from("/tmp/televim-1-2-3.jpg")]);

        viewers
            .start_next(&mut viewer, &mut app, &tx)
            .expect("the start goes through");
        // The loop keeps running while the viewer is open: another pass starts
        // nothing, and the wait has not yet been taken.
        viewers
            .start_next(&mut viewer, &mut app, &tx)
            .expect("the start goes through");
        assert!(viewers.busy());
        assert_eq!(viewer.log, ["suspend", "spawn /tmp/televim-1-2-3.jpg"]);

        let done = next_done(&mut rx);
        viewers
            .finish(&mut viewer, &mut app, &done)
            .expect("resume succeeds");
        viewers
            .start_next(&mut viewer, &mut app, &tx)
            .expect("the start goes through");

        assert_eq!(app.current_chat_id(), 7);
        assert_eq!(app.conversation.conversation.window.chat_id, 7);
        assert_eq!(app.conversation.vim.cursor(), 3);
        assert_eq!(
            viewer.log,
            ["suspend", "spawn /tmp/televim-1-2-3.jpg", "resume"],
            "one resume, and no second viewer for the same file"
        );
    }

    /// GAP 7: queued paths open one viewer at a time. The second opens only once
    /// the first has reported and the terminal is back.
    #[cfg(unix)]
    #[test]
    fn queued_media_opens_one_viewer_at_a_time() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new();
        let mut viewers = Viewers::default();
        let mut viewer = FakeViewer::default();
        viewers.queue(vec![
            PathBuf::from("/tmp/a.jpg"),
            PathBuf::from("/tmp/b.jpg"),
        ]);

        viewers
            .start_next(&mut viewer, &mut app, &tx)
            .expect("the start goes through");
        viewers
            .start_next(&mut viewer, &mut app, &tx)
            .expect("the start goes through");
        assert_eq!(viewer.log, ["suspend", "spawn /tmp/a.jpg"]);

        let done = next_done(&mut rx);
        viewers
            .finish(&mut viewer, &mut app, &done)
            .expect("resume succeeds");
        viewers
            .start_next(&mut viewer, &mut app, &tx)
            .expect("the start goes through");
        let done = next_done(&mut rx);
        viewers
            .finish(&mut viewer, &mut app, &done)
            .expect("resume succeeds");

        assert_eq!(
            viewer.log,
            [
                "suspend",
                "spawn /tmp/a.jpg",
                "resume",
                "suspend",
                "spawn /tmp/b.jpg",
                "resume",
            ]
        );
    }
}
