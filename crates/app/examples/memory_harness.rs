//! The synthetic half of the memory measurement harness (`make measure`).
//!
//! It builds an [`App`] holding a fixed load of chats and messages through the
//! crate's public API only — never `#[cfg(test)]` data, which no dependent crate
//! can see — renders frames into an in-memory terminal, and prints one JSON
//! object on stdout describing what it observed.
//!
//! It is an `[[example]]` rather than a second binary or a test because
//! `cargo build --release` does not build examples, so nothing here can end up
//! in the shipped artifact. `scripts/memory/measure.py` is what runs it, times
//! the release binary, and aggregates several runs into one report.
//!
//! What it measures: resident set size at idle after the load settles, the
//! time to a populated screen, and the time from a synthetic keypress to the
//! frame that shows its effect.
//!
//! What it does not measure, and no part of this workspace does: allocation
//! counts, heap fragmentation, cache behaviour, or anything inside `grammers`.

use std::borrow::Cow;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use domain::chat::{Chat, ChatKind};
use domain::message::{Message, MessageStatus};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use tui::app::App;

/// How many chats the load holds.
///
/// Above the 50 the budget is written against: the process-lifetime
/// allocations — `Chat.title`, the read-receipt map — scale with the number of
/// chats, so a fixture with fewer cannot show them.
const CHATS: usize = 60;

/// How many messages the fixture hands to the open conversation.
///
/// More than `CONVERSATION_WINDOW` (200), so the window is asked to drop the
/// far end: the cap is part of the budget, and a fixture that never reaches it
/// measures something the program cannot do.
const MESSAGES: usize = 250;

/// A terminal big enough that the row layout is doing real work, and small
/// enough to be in-memory.
const WIDTH: u16 = 120;
const HEIGHT: u16 = 40;

/// Frames drawn before anything is timed, so the first touches (lazily built
/// emoji tables, the first pass over the window) are not what is recorded.
const WARMUP_FRAMES: usize = 120;

/// Keypresses measured for input latency.
const KEYPRESSES: usize = 50;

/// How long the load is left alone before RSS is sampled, in milliseconds.
///
/// The "idle" the report claims: no keypress pending, no frame in flight, and
/// this much quiet time after the last load so that anything that would grow
/// the process has had the chance to.
const SETTLE_MS: u64 = 2_000;

/// RSS samples taken after settling, and the gap between them.
const RSS_SAMPLES: usize = 5;
const SAMPLE_GAP_MS: u64 = 200;

/// A fixed clock, so two runs render the same frame.
const FIXED_NOW: i64 = 1_730_000_000;

/// The key the latency is measured on. A motion, so the frame differs and the
/// draw cannot be skipped.
const PROBE_KEY: KeyEvent = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);

fn main() {
    let started = Instant::now();

    let mut app = App::new();
    app.record_now(FIXED_NOW);
    "measuring".clone_into(&mut app.ui.status);
    app.set_chats(fixture_chats());
    app.select_chat(0);

    let applied = app.apply_latest(fixture_messages());
    assert!(
        applied,
        "the fixture page belongs to the chat it was opened for"
    );

    let window = app.conversation.conversation.window.len();
    assert!(
        window >= 50,
        "the window holds {window} messages, so the conversation is not loaded"
    );

    let mut terminal =
        Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("the test backend builds");

    // The first drawn frame is the populated screen: everything above it is the
    // load, and this is the moment the load is on show.
    draw(&mut terminal, &app);
    let populated_load_us = micros(started.elapsed());

    let frame_us = steady_frame_us(&mut terminal, &app);
    let latency_us = input_latency_us(&mut terminal, &mut app);
    let latency_median = median(&latency_us).expect("the pass pressed keys");

    std::thread::sleep(Duration::from_millis(SETTLE_MS));
    let rss = idle_rss();
    let rss_median = median(&rss);

    println!(
        "{{\n  \"schema\": \"televim.harness-run/1\",\n  \
         \"harness\": \"synthetic-app\",\n  \
         \"chats\": {CHATS},\n  \
         \"messages_supplied\": {MESSAGES},\n  \
         \"messages_in_window\": {window},\n  \
         \"terminal\": {{\"width\": {WIDTH}, \"height\": {HEIGHT}}},\n  \
         \"settle_ms\": {SETTLE_MS},\n  \
         \"warmup_frames\": {WARMUP_FRAMES},\n  \
         \"startup_populated_load_us\": {populated_load_us},\n  \
         \"frame_us_median\": {frame_us},\n  \
         \"input_latency_us_median\": {latency_median},\n  \
         \"input_latency_us_samples\": {latency_samples},\n  \
         \"rss_idle_bytes\": {rss_median},\n  \
         \"rss_idle_bytes_samples\": {rss_samples}\n}}",
        latency_samples = number_list(&latency_us),
        rss_median = json_number(rss_median),
        rss_samples = number_list(&rss),
    );
}

/// Draws one frame. The draw is the frame's end, which is where every latency
/// in this harness is measured from.
fn draw(terminal: &mut Terminal<TestBackend>, app: &App) {
    terminal
        .draw(|frame| app.render(frame))
        .expect("the frame draws");
}

/// A whole warm-up pass's worth of frames, and the median of a second one.
///
/// The two passes are kept apart so that lazy one-off costs — an emoji table
/// built on first use, a growth the first draw leaves behind — are paid before
/// anything is recorded.
fn steady_frame_us(terminal: &mut Terminal<TestBackend>, app: &App) -> u64 {
    for _ in 0..WARMUP_FRAMES {
        draw(terminal, app);
    }

    let mut samples = Vec::with_capacity(WARMUP_FRAMES);
    for _ in 0..WARMUP_FRAMES {
        let at = Instant::now();
        draw(terminal, app);
        samples.push(micros(at.elapsed()));
    }

    median(&samples).expect("the pass drew frames")
}

/// Keypress to the frame that shows it, one sample per keypress.
///
/// This is the harness's answer to the README's "input latency" row: the same
/// interval the real binary's probe measures, taken over a whole pass rather
/// than one key.
fn input_latency_us(terminal: &mut Terminal<TestBackend>, app: &mut App) -> Vec<u64> {
    let mut samples = Vec::with_capacity(KEYPRESSES);
    for _ in 0..KEYPRESSES {
        let at = Instant::now();
        app.handle_key(PROBE_KEY);
        draw(terminal, app);
        samples.push(micros(at.elapsed()));
    }
    samples
}

/// The chat list the load is made of.
fn fixture_chats() -> Vec<Chat> {
    const TITLES: [&str; 6] = [
        "Ada Lovelace",
        "Grace Hopper",
        "Alan Turing",
        "Barbara Liskov",
        "Edsger Dijkstra",
        "Katherine Johnson",
    ];

    (0..CHATS)
        .map(|n| {
            let title = TITLES[n % TITLES.len()];
            Chat {
                id: to_id(n + 1),
                title: format!("{title} #{n}"),
                kind: if n % 3 == 0 {
                    ChatKind::Group
                } else {
                    ChatKind::Private
                },
                last_message: Some(Cow::Borrowed("a synthetic message for the load")),
                unread_count: u32::try_from(n % 5).unwrap_or(u32::MAX),
                last_message_id: Some(to_id(n + 1)),
                last_timestamp: Some(FIXED_NOW - to_id(n) * 60),
                pinned: false,
            }
        })
        .collect()
}

/// The open conversation's page. One chat, and the full window behind it.
fn fixture_messages() -> Vec<Message> {
    (0..MESSAGES)
        .map(|n| Message {
            id: to_id(n + 1),
            chat_id: 1,
            text: Cow::Owned(format!(
                "message {n}: the harness measures what a reader's feed costs"
            )),
            timestamp: FIXED_NOW - to_id(MESSAGES - n) * 60,
            status: if n % 4 == 0 {
                MessageStatus::Sent
            } else {
                MessageStatus::Received
            },
            is_outgoing: n % 4 == 0,
            reply_to: None,
            media: None,
        })
        .collect()
}

/// Resident set size of this process, right now, in bytes.
///
/// Where the figure comes from differs by platform and the difference is worth
/// knowing before comparing two hosts: Linux is read out of `/proc/self/status`,
/// macOS through `proc_pidinfo`. Both are the kernel's own number for this
/// process, not a profiler's estimate.
fn idle_rss() -> Vec<u64> {
    let mut samples = Vec::with_capacity(RSS_SAMPLES);
    for n in 0..RSS_SAMPLES {
        if n > 0 {
            std::thread::sleep(Duration::from_millis(SAMPLE_GAP_MS));
        }
        if let Some(bytes) = resident_bytes() {
            samples.push(bytes);
        }
    }
    samples
}

#[cfg(target_os = "linux")]
fn resident_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    // `VmRSS` is reported in kibibytes, so no page size has to be assumed.
    status.lines().find_map(|line| {
        let rest = line.strip_prefix("VmRSS:")?;
        rest.split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()
            .map(|kb| kb * 1024)
    })
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::c_void;
    use std::mem::size_of;

    /// `PROC_PIDTASKINFO`: the flavour that fills a `proc_taskinfo`.
    const FLAVOUR: i32 = 4;

    /// The kernel's `struct proc_taskinfo`.
    ///
    /// Its second field is the resident size. The layout is kernel ABI, stable
    /// across releases, and declaring it is cheaper than a crate for one field —
    /// but a wrong declaration here would be a wrong number in a report rather
    /// than a compile error, so the size is asserted rather than assumed.
    #[repr(C)]
    #[derive(Default)]
    struct ProcTaskInfo {
        virtual_size: u64,
        resident_size: u64,
        total_user: u64,
        total_system: u64,
        threads_user: u64,
        threads_system: u64,
        policy: i32,
        faults: i32,
        pageins: i32,
        cow_faults: i32,
        messages_sent: i32,
        messages_received: i32,
        syscalls_mach: i32,
        syscalls_unix: i32,
        context_switches: i32,
        threads: i32,
        numrunning: i32,
        priority: i32,
    }

    const _: () = assert!(size_of::<ProcTaskInfo>() == 96);

    unsafe extern "C" {
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            arg: u64,
            buffer: *mut c_void,
            buffersize: i32,
        ) -> i32;
    }

    /// This process's resident set size, or `None` if the kernel declined.
    ///
    /// # Safety
    ///
    /// The caller must hand `proc_pidinfo` a buffer of at least
    /// `size_of::<ProcTaskInfo>()` bytes.
    unsafe fn proc_taskinfo() -> Option<ProcTaskInfo> {
        let mut info = ProcTaskInfo::default();
        // SAFETY: the buffer is this struct's own size, which is what is passed
        // as `buffersize`, so the kernel cannot write past it. The return value
        // says how much it wrote, and a short write leaves `resident_size` at
        // zero, which is checked rather than reported.
        let written = unsafe {
            proc_pidinfo(
                i32::try_from(std::process::id()).ok()?,
                FLAVOUR,
                0,
                std::ptr::from_mut(&mut info).cast(),
                i32::try_from(size_of::<ProcTaskInfo>()).ok()?,
            )
        };

        let expected = i32::try_from(size_of::<ProcTaskInfo>()).ok()?;
        (written == expected).then_some(info)
    }

    /// The resident set size of this process.
    pub(super) fn resident_bytes() -> Option<u64> {
        // SAFETY: `proc_taskinfo` allocates and sizes the buffer it writes into.
        let info = unsafe { proc_taskinfo() }?;
        (info.resident_size > 0).then_some(info.resident_size)
    }
}

#[cfg(target_os = "macos")]
fn resident_bytes() -> Option<u64> {
    macos::resident_bytes()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn resident_bytes() -> Option<u64> {
    // No portable way to ask. Reported as absent rather than guessed at, so a
    // Windows figure is "not measured" and not a number of the wrong kind.
    None
}

fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn to_id(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn median(samples: &[u64]) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    Some(sorted[sorted.len() / 2])
}

/// The samples themselves, so a report can be read rather than trusted.
fn number_list(samples: &[u64]) -> String {
    let parts: Vec<String> = samples.iter().map(u64::to_string).collect();
    format!("[{}]", parts.join(", "))
}

/// A number as JSON, or `null` when there is none.
///
/// `null` rather than `0`: an RSS this host cannot report has to be absent from
/// the report, because a zero reads as a measurement.
fn json_number(value: Option<u64>) -> String {
    value.map_or_else(|| "null".to_owned(), |v| v.to_string())
}
