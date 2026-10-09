//! PTY end-to-end tests: the real `televim` binary runs in a pseudo-terminal
//! driven by `termlens`, and each test asserts on the screen it draws.
//!
//! Run policy: these are ordinary tests with no `#[ignore]`. `make test` and
//! CI's `cargo test --all --all-features` run them on every job. They need a
//! pty and nothing else: no network, no credentials.
//!
//! Harness (`common/mod.rs`): `sandbox()` makes a temporary config directory,
//! `seed_history()` writes a `history.json` beside it so cached chats are on
//! screen before the first frame, and `spawn_offline()` launches the binary
//! from `CARGO_BIN_EXE_televim`. Assert with `wait_until` on positive
//! predicates, or `snapshot_after` when the screen text is needed.
//!
//! Deferred: AC5–AC7 (an arrival moving a pinned view, an arrival leaving a
//! scrolled-back view alone, a fetch in flight shown at its edge) stay
//! `#[ignore]`d with reasons. Nothing offline can inject an arrival or start a
//! fetch; see `docs/known-gaps.md`.
//!
//! Sending, editing and deleting are covered by unit tests and by the
//! `TestBackend` assertions in `tui`'s conversation panel.

mod common;

use termlens::Key;

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
#[ignore = "needs a prod seam to inject arrivals; deferred per O4, see GAPS.md"]
fn an_arrival_scrolls_a_pinned_view() {
    // TODO: with a conversation open and pinned to the bottom, deliver an
    // arrival for that conversation; assert the new message is the last row.
}

/// The counterpart: a reader who has scrolled away must not be dragged back by
/// an arrival. This is the case the anchor exists for, and it is invisible
/// until something arrives.
#[test]
#[ignore = "needs a prod seam to inject arrivals; deferred per O4, see GAPS.md"]
fn an_arrival_leaves_a_scrolled_back_view_alone() {
    // TODO: send `Ctrl+u`, note the first visible row, deliver an arrival for
    // the same conversation; assert the first visible row is unchanged and the
    // cursor is still on the message it was on.
}

/// Fetching is off the render loop, so a page in flight has to be visible as
/// such rather than as a frozen panel.
#[test]
#[ignore = "needs a prod seam to inject arrivals; deferred per O4, see GAPS.md"]
fn a_fetch_in_flight_is_shown_at_the_edge_it_is_coming_from() {
    // TODO: with the older page in flight, assert "Loading older…" is the first
    // row; with the newer page in flight, assert it is the last.
}
