//! PTY end-to-end tests: the real `televim` binary runs in a pseudo-terminal
//! driven by `termlens`, and each test asserts on the screen it draws.
//!
//! Run policy: these are ordinary tests with no `#[ignore]` in a debug build.
//! `make test` and CI's `cargo test --all --all-features` run them on every job.
//! They need a pty and nothing else: no network, no credentials.
//!
//! Two states only the network can produce — an arrival, and a page in flight —
//! are replayed by a debug-only seam. `spawn_offline_with_scenario` writes a
//! script into the sandbox and hands its path to the binary in `TELEVIM_SCENARIO`;
//! the binary waits for the sibling `<path>.fire` and then sends each directive
//! through the same path the network's events take. `Scenario::fire` is the
//! trigger, so a test can put the screen in the state an arrival must not
//! disturb before releasing it. A release build carries no seam, which is why
//! those three cases name it in a `#[cfg_attr(not(debug_assertions), ignore)]`
//! rather than running there.
//!
//! Harness (`common/mod.rs`): `sandbox()` makes a temporary config directory,
//! `seed_history()` writes a `history.json` beside it so cached chats are on
//! screen before the first frame, and `spawn_offline()` launches the binary
//! from `CARGO_BIN_EXE_televim`. Assert with `wait_until` on positive
//! predicates, or `snapshot_after` when the screen text is needed.
//!
//! Sending, editing and deleting are covered by unit tests and by the
//! `TestBackend` assertions in `tui`'s conversation panel.

mod common;

use termlens::{Key, Screen};

/// The conversation panel's body on the 80x24 PTY: the chat list takes the
/// left 30%, the panel's border the top and bottom rows of the 20 rows above
/// the input bar and the status line. The scrollbar has a column of its own on
/// the right, which is why the body ends one column early.
const CONVERSATION_TOP: u16 = 1;
const CONVERSATION_BOTTOM: u16 = 18;
const CONVERSATION_FIRST_COL: u16 = 25;
const CONVERSATION_LAST_COL: u16 = 77;

/// One conversation row as text, without the scrollbar's column.
fn body_row(screen: &Screen, row: u16) -> String {
    screen
        .row_text(row)
        .chars()
        .skip(usize::from(CONVERSATION_FIRST_COL))
        .take(usize::from(
            CONVERSATION_LAST_COL - CONVERSATION_FIRST_COL + 1,
        ))
        .collect()
}

/// The conversation's body as one piece of text, without the scrollbar.
fn conversation(screen: &Screen) -> String {
    (CONVERSATION_TOP..=CONVERSATION_BOTTOM)
        .map(|row| body_row(screen, row))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The row the cursor's `REVERSED` selection is drawn on.
fn cursor_row(screen: &Screen) -> u16 {
    (CONVERSATION_TOP..=CONVERSATION_BOTTOM)
        .find(|&row| {
            (CONVERSATION_FIRST_COL..=CONVERSATION_LAST_COL).any(|col| {
                screen
                    .cell(row, col)
                    .is_some_and(|cell| cell.style().reverse)
            })
        })
        .expect("the conversation draws its cursor on one of its rows")
}

/// The prelude the conversation tests share: wait for the first frame, then
/// clear the sign-in card so the conversation takes keys.
fn open_conversation(t: &mut termlens::Terminal) {
    t.wait_until(|s| s.contains("NORMAL"))
        .expect("ready in NORMAL mode");
    t.send_str(":").expect("open command line");
    t.send(Key::Esc).expect("close command line");
    t.wait_until(|s| !s.contains("Sign in to Telegram"))
        .expect("sign-in card cleared");
}

/// Forty seeded messages in chat 1, named `msg01` upward.
fn seeded_chat(sandbox: &common::Sandbox) {
    let texts: Vec<String> = (1..=40).map(|n| format!("msg{n:02}")).collect();
    let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
    common::seed_history(sandbox, &[(1, "Ada")], &[(1, texts)]);
}

#[test]
fn launches_to_chat_list_in_normal_mode() {
    let sandbox = common::sandbox();
    let mut t = common::spawn_offline(&sandbox);

    // The empty sandbox has no chats, so the title is "Chats (0)". The
    // no-credentials sentence is the deterministic offline screen.
    t.wait_until(|s| {
        s.contains("Chats") && s.contains("NORMAL") && s.contains("no application credentials")
    })
    .expect("chat list in NORMAL mode");
}

/// The warm-start cache seeds the chat list before the first frame, so a
/// launch with a history file shows its chats with no network.
#[test]
fn seeded_history_fills_the_chat_list_offline() {
    let sandbox = common::sandbox();
    common::seed_history(
        &sandbox,
        &[(1, "Ada"), (2, "Grace"), (3, "Linus")],
        &[(1, vec!["hi", "are you there", "call me"])],
    );
    let mut t = common::spawn_offline(&sandbox);

    t.wait_until(|s| s.contains("Chats (3)") && s.contains("no application credentials"))
        .expect("seeded chat list drawn");
}

#[test]
fn insert_mode_echoes_typing() {
    let sandbox = common::sandbox();
    common::seed_history(&sandbox, &[(1, "Ada")], &[]);
    let mut t = common::spawn_offline(&sandbox);
    t.wait_until(|s| s.contains("NORMAL"))
        .expect("ready in NORMAL mode");

    // With no credentials the sign-in sentence answers only `q` and `:`, so
    // `i` does nothing until a command line has cleared it.
    t.send_str(":").expect("open command line");
    t.send(Key::Esc).expect("close command line");
    t.wait_until(|s| !s.contains("Sign in to Telegram"))
        .expect("sign-in card cleared");

    t.send(Key::Char('i')).expect("enter insert mode");
    t.wait_until(|s| s.contains("INSERT"))
        .expect("INSERT mode shown");
    t.send_str("hi").expect("type hi");
    t.wait_until(|s| s.contains("hi"))
        .expect("typed text echoed");
    t.send(Key::Esc).expect("leave insert mode");
    t.wait_until(|s| s.contains("NORMAL"))
        .expect("back in NORMAL mode");
}

/// `i` on the open draft's row resumes the draft in one key: the words typed
/// and left are back in the line, INSERT is shown, and a key typed there lands
/// after them.
#[test]
fn i_on_the_draft_row_resumes_the_draft() {
    let sandbox = common::sandbox();
    common::seed_history(&sandbox, &[(1, "Ada")], &[(1, vec!["hi", "are you there"])]);
    let mut t = common::spawn_offline(&sandbox);
    t.wait_until(|s| s.contains("NORMAL"))
        .expect("ready in NORMAL mode");

    // The G7 prelude: clear the sign-in card so the conversation takes keys.
    t.send_str(":").expect("open command line");
    t.send(Key::Esc).expect("close command line");
    t.wait_until(|s| !s.contains("Sign in to Telegram"))
        .expect("sign-in card cleared");

    t.send(Key::Char('i')).expect("enter insert mode");
    t.wait_until(|s| s.contains("INSERT"))
        .expect("INSERT mode shown");
    t.send_str("see you there").expect("type the draft");
    t.wait_until(|s| s.contains("see you there"))
        .expect("draft echoed");
    t.send(Key::Esc).expect("leave insert mode");
    t.wait_until(|s| s.contains("NORMAL"))
        .expect("back in NORMAL mode");
    t.send(Key::Esc)
        .expect("leave the line for the conversation");
    // Wait for the line to be parked: an `Esc` sent straight before `j` can be
    // read as one Alt+`j` chord, and `j` could land before the focus moved.
    t.wait_until(|s| s.contains("┌ draft"))
        .expect("the line is parked as a draft");

    // `j` from the newest message rests the cursor on the draft's row.
    t.send(Key::Char('j')).expect("move onto the draft row");

    t.send(Key::Char('i')).expect("resume the draft");
    t.wait_until(|s| s.contains("INSERT"))
        .expect("INSERT mode shown on the draft");
    t.send_str("!").expect("type after the draft");
    t.wait_until(|s| s.contains("see you there!"))
        .expect("the draft is back in the line, and the key lands after it");
}

#[test]
fn command_prompt_quits() {
    let sandbox = common::sandbox();
    let mut t = common::spawn_offline(&sandbox);
    t.wait_until(|s| s.contains("NORMAL"))
        .expect("ready in NORMAL mode");

    t.send_str(":q").expect("type :q");
    t.send(Key::Enter).expect("submit command");
    // `:q` asks for confirmation before quitting.
    t.wait_until(|s| s.contains("Quit televim?"))
        .expect("quit confirmation shown");
    t.send(Key::Char('y')).expect("confirm quit");
    assert!(
        t.wait_exit().expect("process exits").success(),
        "televim exits cleanly on :q"
    );
}

/// The directory is a network question, so offline it is refused by name rather
/// than opening an empty panel — which would read as an account with no contacts.
#[test]
fn contacts_offline_is_refused_by_name() {
    let sandbox = common::sandbox();
    let mut t = common::spawn_offline(&sandbox);
    t.wait_until(|s| s.contains("NORMAL"))
        .expect("ready in NORMAL mode");

    t.send_str(":contacts").expect("type :contacts");
    t.send(Key::Enter).expect("submit command");
    t.wait_until(|s| s.contains("not connected yet"))
        .expect("the refusal names the missing client");
}

/// The cursor is what tells the panel which slice to draw, so a scroll that
/// moves the cursor but not the slice — or the reverse — is a rendering bug
/// that only shows on a screen.
#[test]
fn scrolling_up_and_down_moves_the_visible_slice() {
    let sandbox = common::sandbox();
    let texts: Vec<String> = (1..=40).map(|n| format!("msg{n:02}")).collect();
    let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
    common::seed_history(&sandbox, &[(1, "Ada")], &[(1, texts)]);
    let mut t = common::spawn_offline(&sandbox);
    t.wait_until(|s| s.contains("NORMAL"))
        .expect("ready in NORMAL mode");

    // The G7 prelude: clear the sign-in card so the conversation takes keys.
    t.send_str(":").expect("open command line");
    t.send(Key::Esc).expect("close command line");
    t.wait_until(|s| !s.contains("Sign in to Telegram"))
        .expect("sign-in card cleared");

    // Scrolling up from the newest message moves the slice to the older end.
    t.send(Key::Ctrl('u')).expect("scroll up");
    let up = t
        .snapshot_after(|s| s.contains("Conversation (22/40)"))
        .expect("scrolled up to 22/40");
    assert!(
        up.contains("msg13") && up.contains("msg30"),
        "older slice drawn: {}",
        up.full_text()
    );
    assert!(
        !up.contains("msg40"),
        "newest message scrolled off: {}",
        up.full_text()
    );

    // Scrolling back down brings the newest message back into view.
    t.send(Key::Ctrl('d')).expect("scroll down");
    let down = t
        .snapshot_after(|s| s.contains("Conversation (40/40)"))
        .expect("scrolled back to 40/40");
    assert!(
        down.contains("msg23") && down.contains("msg40"),
        "newest slice drawn again: {}",
        down.full_text()
    );
}

/// A view pinned to the newest message is the state a conversation opens in,
/// and an arrival is supposed to move it. The flag transitions are unit-tested;
/// what a screen adds is that the viewport actually follows.
#[test]
#[cfg_attr(not(debug_assertions), ignore = "the scenario seam is debug-only")]
fn an_arrival_scrolls_a_pinned_view() {
    let sandbox = common::sandbox();
    seeded_chat(&sandbox);
    let mut t = common::spawn_offline_with_scenario(&sandbox, "arrival 1 41 msg41");
    open_conversation(&mut t);
    t.wait_until(|s| s.contains("Conversation (40/40)"))
        .expect("the conversation opens pinned to the newest message");

    t.fire();

    let after = t
        .snapshot_after(|s| s.contains("Conversation (41/41)"))
        .expect("the pinned view followed the arrival to the new newest message");
    let body = conversation(&after);
    assert!(
        body.contains("msg41"),
        "the arrival is on screen: {}",
        after.full_text()
    );
    assert!(
        body.contains("msg40"),
        "the viewport followed rather than jumped to it: {}",
        after.full_text()
    );
}

/// The counterpart: a reader who has scrolled away must not be dragged back by
/// an arrival. This is the case the anchor exists for, and it is invisible
/// until something arrives.
#[test]
#[cfg_attr(not(debug_assertions), ignore = "the scenario seam is debug-only")]
fn an_arrival_leaves_a_scrolled_back_view_alone() {
    let sandbox = common::sandbox();
    seeded_chat(&sandbox);
    let mut t = common::spawn_offline_with_scenario(&sandbox, "arrival 1 41 msg41");
    open_conversation(&mut t);

    t.send(Key::Ctrl('u')).expect("scroll up");
    let before = t
        .snapshot_after(|s| s.contains("Conversation (22/40)"))
        .expect("scrolled back to 22/40");
    let first = body_row(&before, CONVERSATION_TOP);
    let cursor = cursor_row(&before);
    let on_cursor = body_row(&before, cursor);

    t.fire();

    let after = t
        .snapshot_after(|s| s.contains("Conversation (22/41)"))
        .expect("the arrival joined the window without moving the reader");
    assert_eq!(
        body_row(&after, CONVERSATION_TOP),
        first,
        "the first conversation row is unchanged: {}",
        after.full_text()
    );
    assert_eq!(
        body_row(&after, cursor),
        on_cursor,
        "the cursor is still on the message it was on: {}",
        after.full_text()
    );
    assert!(
        !conversation(&after).contains("msg41"),
        "the arrival is below the viewport, not forced into it: {}",
        after.full_text()
    );
}

/// Fetching is off the render loop, so a page in flight has to be visible as
/// such rather than as a frozen panel.
#[test]
#[cfg_attr(not(debug_assertions), ignore = "the scenario seam is debug-only")]
fn a_fetch_in_flight_is_shown_at_the_edge_it_is_coming_from() {
    let sandbox = common::sandbox();
    seeded_chat(&sandbox);
    let mut t = common::spawn_offline_with_scenario(&sandbox, "fetch older\nfetch newer");
    open_conversation(&mut t);
    t.wait_until(|s| s.contains("Conversation (40/40)"))
        .expect("the seeded window on show");

    t.fire();

    let after = t
        .snapshot_after(|s| s.contains("Loading older…") && s.contains("Loading newer…"))
        .expect("both pages are in flight");
    assert!(
        body_row(&after, CONVERSATION_TOP).contains("Loading older…"),
        "the older page is announced at the top, where it will be joined: {}",
        after.full_text()
    );
    assert!(
        body_row(&after, CONVERSATION_BOTTOM).contains("Loading newer…"),
        "the newer page is announced at the bottom: {}",
        after.full_text()
    );
}
