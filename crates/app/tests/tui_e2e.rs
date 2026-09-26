//! PTY-based E2E tests for the TUI.
//!
//! These are gated on `termlens`, which is not yet wired up. Once it is,
//! remove the `#[ignore]`s and add `termlens` as a dev-dependency of this
//! crate.

#[test]
#[ignore = "requires termlens"]
fn launches_to_chat_list_in_normal_mode() {
    // TODO: spawn target/release/televim in a PTY; assert screen contains
    // "Chats", "Conversation", and "NORMAL".
}

#[test]
#[ignore = "requires termlens"]
fn insert_mode_echoes_typing() {
    // TODO: send `i`, `h`, `i`, `Esc`; assert screen contains "hi".
}

#[test]
#[ignore = "requires termlens"]
fn command_prompt_quits() {
    // TODO: send `:`, `q`, `Enter`; assert process exits.
}
