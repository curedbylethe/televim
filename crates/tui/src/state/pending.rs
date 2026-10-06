//! Half-typed keys and the requests a keystroke defers.

use std::time::Instant;

use crate::app::{CHAT_SWITCH_DELAY, ChatChoice, Find, Jump};

/// Half-typed keys and the requests a keystroke defers.
///
/// The `pending_` prefix stays: call sites are `self.pending.pending_g`.
#[allow(clippy::struct_field_names)]
pub struct Pending {
    /// The jump the reader has asked for and no page has answered yet.
    ///
    /// Set when `gg` cannot be answered from what is loaded, and cleared when the
    /// page arrives — or fails, or comes back empty, because a jump that went
    /// wrong must not wedge the key. It is what makes the key idempotent: a
    /// second `gg` produces the same intent, which the caller recognises as one
    /// already on its way.
    pub(crate) pending_jump: Option<Jump>,

    /// The conversation the highlight has moved onto but has not been taken to.
    ///
    /// The same hand-over as [`App::pending_jump`] — recorded here because
    /// `tui` cannot reach the network — and for the same reason it carries a
    /// time: a reader holding `j` would otherwise fetch every conversation they
    /// scrolled past, and one page per chat as fast as a key repeats is how a
    /// scroll through the list becomes a flood wait.
    pub(crate) pending_chat: Option<ChatChoice>,

    /// Whether a `g` was just pressed in the chat list and a second one would
    /// take the reader to the top of it.
    ///
    /// The same latch as `dd` and for the same reason: `gg` is two presses in
    /// Vim, and a key held down is not two of them.
    pub(crate) pending_g: bool,

    /// A `f`, `t`, `F` or `T` waiting for the character to look for.
    ///
    /// Two keys rather than one, as in Vim, and a latch for the same reason `dd`
    /// has one: the key after `f` is the character, not a motion. The character
    /// itself is not recorded, because it has not been typed yet.
    pub(crate) pending_find: Option<Find>,
}

impl Pending {
    /// No key half-typed and no request waiting.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            pending_jump: None,
            pending_chat: None,
            pending_g: false,
            pending_find: None,
        }
    }

    /// Records the jump the reader asked for, or clears it.
    ///
    /// `None` ends the wait however it ended: a page that failed, or came back
    /// empty, has to end it exactly as a page that landed does.
    pub(crate) fn set_jump(&mut self, jump: Option<Jump>) {
        self.pending_jump = jump;
    }

    /// Ends the reader's wait for a jump, reporting whether it was still the one
    /// being waited on.
    ///
    /// A page that failed, or came back empty, has to end it exactly as a page
    /// that landed does. The reader stays where they were either way; what this
    /// is for is that the key is free again — a jump nothing releases is a key
    /// that never works again.
    pub(crate) fn clear_jump(&mut self, target_id: i64) -> bool {
        if self.pending_jump.map(|jump| jump.target_id) != Some(target_id) {
            return false;
        }

        self.pending_jump = None;
        true
    }

    /// Records the conversation the highlight moved onto.
    ///
    /// The open is recorded rather than made, because a reader who holds `j`
    /// would otherwise have every conversation they passed fetched.
    pub(crate) fn set_pending_chat(&mut self, choice: Option<ChatChoice>) {
        self.pending_chat = choice;
    }

    /// The conversation the reader has stopped on, once they have stopped.
    ///
    /// Nothing while they are still moving, so a held key opens the chat they
    /// land on rather than every one between here and there. Idempotent in the
    /// way [`Self::pending_jump`] is: once handed over it is forgotten, so a
    /// caller that asks twice gets one conversation.
    pub(crate) fn take_pending_chat(&mut self, now: Instant) -> Option<usize> {
        let choice = self.pending_chat?;
        if now.saturating_duration_since(choice.at) < CHAT_SWITCH_DELAY {
            return None;
        }

        self.pending_chat = None;
        Some(choice.index)
    }

    /// Records whether a `g` was just pressed.
    pub(crate) fn set_g(&mut self, armed: bool) {
        self.pending_g = armed;
    }

    /// Records the `f`/`t`/`F`/`T` waiting for its character, or clears it.
    pub(crate) fn set_find(&mut self, find: Option<Find>) {
        self.pending_find = find;
    }
}
