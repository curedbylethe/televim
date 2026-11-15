//! The pinned list: one chat's pins, as `:pinned` draws them.
//!
//! The **identifiers** are the conversation's, not this list's: a pin is a
//! membership test on [`ConversationView`](domain::history::ConversationView),
//! the marker in the messages and the rows here read the same set, and this
//! holds the ordering to nobody — the view answers `pinned_newest_first`, so a
//! list that sorted again could disagree with the marker.
//!
//! What is here is the answer to the fetch: the whole messages, so a row the
//! window no longer holds still has a sender, a line and an age. Nothing is
//! persisted and nothing is shared: the list is a fetch the overlay owns, like
//! the global search's.

use domain::message::Message;

/// An open pinned list, and where the reader is among the pins.
#[derive(Debug, Default)]
pub struct PinnedListState {
    /// Whether the overlay is up. It stays up while the fetch is on its way and
    /// after one fails: the list is a place the reader asked to be, and a
    /// refusal answers *about* it rather than closing it.
    open: bool,

    /// The chat the rows are about.
    ///
    /// The pins belong to a conversation, and the reader can leave one while a
    /// fetch is in flight — an answer for a chat that is no longer the one on
    /// show is refused rather than drawn under another chat's title.
    chat_id: i64,

    /// The last answer, by identifier: what a row the window does not hold is
    /// drawn from.
    fetched: Vec<Message>,

    /// Whether a fetch is on its way. An empty list that is still being asked
    /// for is not a chat with no pins, so the empty sentence waits for it.
    in_flight: bool,

    selected: usize,
}

impl PinnedListState {
    /// Opens the list for `chat_id` and marks the fetch in flight.
    ///
    /// A refetch of the chat already on show keeps the answer it had: the round
    /// trip may be refused, and a list that emptied itself for it would tell the
    /// reader there are no pins. Another chat's answer is dropped, so a row is
    /// never drawn under a title it does not belong to.
    pub fn begin(&mut self, chat_id: i64) {
        if self.chat_id != chat_id {
            self.fetched.clear();
            self.selected = 0;
        }

        self.open = true;
        self.chat_id = chat_id;
        self.in_flight = true;
    }

    /// Takes the answer to the fetch that is in flight, for `chat_id`.
    ///
    /// Returns whether it was taken: an answer for a chat the reader has left,
    /// or for one whose list has been closed, is dropped rather than drawn.
    pub fn adopt(&mut self, chat_id: i64, messages: Vec<Message>) -> bool {
        if !self.wants(chat_id) {
            return false;
        }

        self.fetched = messages;
        self.in_flight = false;
        self.selected = 0;
        true
    }

    /// Records a fetch that failed, keeping the rows the reader already had.
    ///
    /// Returns whether the failure was this list's, so the caller can say why
    /// once rather than under another chat's overlay.
    pub fn fail(&mut self, chat_id: i64) -> bool {
        if !self.wants(chat_id) {
            return false;
        }

        self.in_flight = false;
        true
    }

    /// Puts the overlay away, keeping the answer for a reopen.
    pub fn close(&mut self) {
        self.open = false;
        self.in_flight = false;
    }

    /// Whether an answer for `chat_id` is the one this list is waiting for.
    fn wants(&self, chat_id: i64) -> bool {
        self.open && self.chat_id == chat_id
    }

    /// Whether the overlay is on show.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.open
    }

    /// Whether a fetch is still on its way.
    #[must_use]
    pub fn is_fetching(&self) -> bool {
        self.in_flight
    }

    /// The message the fetch answered for `id`, if it was in the answer.
    #[must_use]
    pub fn fetched(&self, id: i64) -> Option<&Message> {
        self.fetched.iter().find(|message| message.id == id)
    }

    /// Where the cursor is, or `0` when the list is empty.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The pin the cursor is on, out of the `ids` the conversation holds.
    ///
    /// The rows are the conversation's, so their number is its answer rather
    /// than this list's: `ids` is `pinned_newest_first`, which is the order the
    /// rows are drawn in.
    #[must_use]
    pub fn selected_id(&self, ids: &[i64]) -> Option<i64> {
        ids.get(self.selected).copied()
    }

    /// Moves the cursor by `delta`, stopping at either end of `len` pins.
    pub fn move_selection(&mut self, delta: isize, len: usize) {
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(len.saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::message::{MediaKind, MessageStatus};
    use std::borrow::Cow;

    fn pinned(id: i64) -> Message {
        Message {
            id,
            chat_id: 7,
            text: Cow::Borrowed("pinned"),
            timestamp: 1_730_000_000 + id,
            status: MessageStatus::Received,
            is_outgoing: false,
            reply_to: None,
            media: None::<MediaKind>,
            is_service: false,
            media_id: None,
        }
    }

    #[test]
    fn an_open_list_is_a_fetch_in_flight_until_its_answer() {
        let mut list = PinnedListState::default();
        list.begin(7);

        assert!(list.is_active());
        assert!(list.is_fetching(), "the empty sentence waits for this");

        assert!(list.adopt(7, vec![pinned(3)]));
        assert!(!list.is_fetching());
        assert_eq!(list.fetched(3).map(|message| message.id), Some(3));
    }

    /// An answer for a chat the reader has left is not this list's.
    #[test]
    fn another_chats_answer_is_refused() {
        let mut list = PinnedListState::default();
        list.begin(7);

        assert!(!list.adopt(9, vec![pinned(3)]));
        assert!(!list.fail(9));
        assert!(list.is_fetching(), "the list is still waiting for its own");
        assert!(list.fetched(3).is_none());
    }

    /// A closed list wants nothing: reopening another chat must not be answered
    /// by a fetch asked for before.
    #[test]
    fn a_closed_list_refuses_an_answer() {
        let mut list = PinnedListState::default();
        list.begin(7);
        list.close();

        assert!(!list.adopt(7, vec![pinned(3)]));
        assert!(list.fetched(3).is_none());
    }

    #[test]
    fn a_failed_refetch_keeps_the_answer_it_had() {
        let mut list = PinnedListState::default();
        list.begin(7);
        assert!(list.adopt(7, vec![pinned(3)]));

        list.begin(7);
        assert!(list.fail(7));

        assert_eq!(
            list.fetched(3).map(|message| message.id),
            Some(3),
            "the rows the reader already had are not taken away"
        );
    }

    #[test]
    fn the_cursor_stops_at_either_end_of_the_pins() {
        let mut list = PinnedListState::default();
        list.begin(7);

        list.move_selection(-1, 3);
        assert_eq!(list.selected(), 0, "not above the newest pin");
        list.move_selection(9, 3);
        assert_eq!(list.selected(), 2, "and not past the oldest");
        assert_eq!(list.selected_id(&[10, 11, 12]), Some(12));
        assert_eq!(
            list.selected_id(&[]),
            None,
            "an empty list has no row to act on"
        );
    }
}
