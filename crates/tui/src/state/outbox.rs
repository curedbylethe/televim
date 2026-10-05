//! Queued actions, in-flight fetches, and the clipboard the driver takes.

use std::collections::VecDeque;

use crate::app::{Action, Fetching};

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
}
