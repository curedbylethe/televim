//! PTY-based E2E tests for the TUI.
//!
//! These are gated on `termlens`, which is not yet wired up. Once it is,
//! remove the `#[ignore]`s and add `termlens` as a dev-dependency of this
//! crate.
//!
//! Sending, editing and deleting are covered by unit tests and by the
//! `TestBackend` assertions in `tui`'s conversation panel; their keystroke
//! flows through a real terminal wait on `termlens` like the rest.

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

#[test]
#[ignore = "insert mode is unreachable without credentials or a chat; see report"]
fn insert_mode_echoes_typing() {
    let sandbox = common::sandbox();
    let mut t = common::spawn_offline(&sandbox);
    t.wait_until(|s| s.contains("NORMAL"))
        .expect("ready in NORMAL mode");

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
#[ignore = "requires termlens"]
fn scrolling_up_and_down_moves_the_visible_slice() {
    // TODO: with a conversation open, send `Ctrl+u`; assert the panel's first
    // visible message changed and the newest one is no longer on screen. Then
    // send `Ctrl+d`; assert the newest message is back.
}

/// A view pinned to the newest message is the state a conversation opens in,
/// and an arrival is supposed to move it. The flag transitions are unit-tested;
/// what a screen adds is that the viewport actually follows.
#[test]
#[ignore = "requires termlens"]
fn an_arrival_scrolls_a_pinned_view() {
    // TODO: with a conversation open and pinned to the bottom, deliver an
    // arrival for that conversation; assert the new message is the last row.
}

/// The counterpart: a reader who has scrolled away must not be dragged back by
/// an arrival. This is the case the anchor exists for, and it is invisible
/// until something arrives.
#[test]
#[ignore = "requires termlens"]
fn an_arrival_leaves_a_scrolled_back_view_alone() {
    // TODO: send `Ctrl+u`, note the first visible row, deliver an arrival for
    // the same conversation; assert the first visible row is unchanged and the
    // cursor is still on the message it was on.
}

/// Fetching is off the render loop, so a page in flight has to be visible as
/// such rather than as a frozen panel.
#[test]
#[ignore = "requires termlens"]
fn a_fetch_in_flight_is_shown_at_the_edge_it_is_coming_from() {
    // TODO: with the older page in flight, assert "Loading older…" is the first
    // row; with the newer page in flight, assert it is the last.
}
