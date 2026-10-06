//! Per-peer parked drafts and read receipts.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::line::LineEditor;

/// Per-peer parked drafts and read receipts.
pub struct DraftStore {
    /// The drafts of the conversations the reader is not in.
    ///
    /// The open conversation's draft is [`InputState`](super::input::InputState)'s
    /// `line`; this holds the rest,
    /// parked under their peer id, so a reader who looks away and comes back
    /// finds the sentence they had started. The same seam as
    /// [`DraftStore::read_receipts`]: the view is replaced on every switch, and what
    /// belongs to the conversation rather than to the page on show is kept here.
    ///
    /// Only a plain message draft is stored — [`App::park_draft`] forgets the
    /// reply or edit subject on the way in — and a peer's entry is dropped when
    /// its draft is empty, so the map holds only peers with words in them.
    ///
    /// Not written to disk, for the same reason as [`DraftStore::read_receipts`]: a
    /// launch starts empty, and an account change clears it ([`App::set_chats`])
    /// so no words cross an account boundary.
    pub(crate) drafts: HashMap<i64, LineEditor>,

    /// How far each conversation this client has been told about has been read.
    ///
    /// One number per conversation, kept here rather than on
    /// [`ConversationView`] because the view is replaced on every chat switch:
    /// a reader who looks away and comes back must find the reading of that
    /// conversation as it was, not as a fresh view believes it. The view holds
    /// the one on show; this holds the rest.
    ///
    /// Monotone per conversation, because the wire says so: `max_id` is a
    /// watermark, and a read that has already been shown cannot be taken back by
    /// a later, lower one. Grows with the conversations the client is told about,
    /// which is no more than the chat list already holds.
    ///
    /// Not written to disk: a launch starts with nothing recorded, so a receipt
    /// the reader has not been shown is never drawn from a previous session's
    /// memory of it. That is the "never claim more than was received" rule at the
    /// storage layer.
    pub(crate) read_receipts: RefCell<HashMap<i64, i64>>,
}

impl DraftStore {
    /// No parked drafts, and nothing recorded as read.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            drafts: HashMap::new(),
            read_receipts: RefCell::new(HashMap::new()),
        }
    }
}
