//! Queued actions, in-flight fetches, and the clipboard the driver takes.

use std::collections::VecDeque;

use crate::app::{Action, FetchDirection, Fetching};

/// Queued actions, in-flight fetches, and the clipboard the driver takes.
pub struct Outbox {
    /// The text a yank asked to be copied to the system clipboard, if one is
    /// waiting to be written.
    ///
    /// Recorded rather than written, because `tui` does not hold stdout and a
    /// widget that writes to the terminal behind the renderer's back is a race.
    /// The caller that owns the terminal takes it with
    /// [`App::take_clipboard`], which it does on the same pass of its loop that
    /// the yank was read on — so this cannot outlive a conversation change by more
    /// than a frame, and clearing it here would lose a yank rather than a stale
    /// one.
    pub(crate) clipboard: Option<String>,

    /// The operations the reader asked for, waiting to be taken by the caller.
    ///
    /// The outbound half of the [`Jump`] pattern: recorded here because `tui`
    /// cannot reach the network, and taken once by the caller that can. A queue
    /// rather than a single slot, because two requests made inside one tick are
    /// two requests — a send followed by `/` has to perform both, not lose the
    /// send to the key that came after it.
    pub(crate) actions: VecDeque<Action>,

    /// The pages on their way from the network.
    pub(crate) fetching: Fetching,
}

impl Outbox {
    /// Nothing queued, nothing in flight, and no yank waiting to be written.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            clipboard: None,
            actions: VecDeque::new(),
            fetching: Fetching::default(),
        }
    }

    /// Records that a fetch for `direction` has been asked for.
    ///
    /// One fetch per direction at a time: this is what a trigger checks before
    /// it fires, so holding a key down cannot turn into a stream of requests.
    pub(crate) fn begin_fetch(&mut self, direction: FetchDirection) {
        self.fetching.set(direction, true);
    }

    /// Records that the fetch for `direction` is over, however it ended.
    ///
    /// A failed fetch releases the direction as surely as a successful one: the
    /// alternative is a conversation that can never be paged again because one
    /// request went wrong.
    pub(crate) fn end_fetch(&mut self, direction: FetchDirection) {
        self.fetching.set(direction, false);
    }

    /// Takes the operation the reader asked for, if there is one.
    ///
    /// Idempotent in the same way every other hand-over here is: once taken it
    /// is cleared, and a caller that asks twice gets one action. The request is
    /// handed over rather than made here because the network is the caller's.
    pub(crate) fn take_action(&mut self) -> Option<Action> {
        self.actions.pop_front()
    }

    /// Takes the text the reader asked to copy to the system clipboard.
    ///
    /// Drained by the caller that owns the terminal, on the same pass it read
    /// the yank on. Idempotent in the way every other hand-over here is: once
    /// taken it is forgotten, so a caller that asks twice gets one copy and not
    /// two.
    ///
    /// A yank is also offered to the clipboard, which is a convenience rather
    /// than the point: whether the terminal honours OSC 52 at all is not this
    /// crate's to know, so the register — which always works — is what a yank
    /// can be relied on for.
    pub(crate) fn take_clipboard(&mut self) -> Option<String> {
        self.clipboard.take()
    }

    /// Holds `text` for the caller that owns the terminal to write.
    ///
    /// The same slot the conversation's yanks go through as the line's: one
    /// seam, two producers.
    pub(crate) fn store_clipboard(&mut self, text: String) {
        self.clipboard = Some(text);
    }
}
