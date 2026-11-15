//! The edit-history overlay: the revisions one message's text went through.
//!
//! Nothing is fetched. Telegram keeps no edit history this client can ask for, so
//! the revisions are the ones the conversation journalled as their edits arrived —
//! [`ConversationView::revisions`](domain::history::ConversationView::revisions) —
//! and this holds only where the reader is among them. Keeping no copy here is
//! also what keeps the overlay current: an edit that lands while it is open is a
//! block it draws on the next frame.

use domain::history::ConversationView;

/// An open edit-history overlay, and which block the reader is on.
#[derive(Debug, Default)]
pub struct EditHistoryState {
    /// Whether the overlay is up. A message with no journal is still up: "no
    /// revision was seen" is an answer, and a key that opened nothing would read
    /// as a key that failed.
    open: bool,

    /// The message whose revisions are listed.
    ///
    /// Taken when the overlay opens, like the reaction line's subject: the rows
    /// can move under it, and a list that retitled itself as the cursor travelled
    /// would be listing a message the reader never asked about.
    message_id: Option<i64>,

    /// Which block is selected, counted from the newest — the current text is
    /// block zero.
    selected: usize,
}

impl EditHistoryState {
    /// Opens the overlay on `message_id`, with the newest block selected.
    pub fn begin(&mut self, message_id: i64) {
        self.open = true;
        self.message_id = Some(message_id);
        self.selected = 0;
    }

    /// Puts the overlay away.
    pub fn close(&mut self) {
        self.open = false;
        self.message_id = None;
        self.selected = 0;
    }

    /// Whether the overlay is on show.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.open && self.message_id.is_some()
    }

    /// The message whose revisions are listed.
    #[must_use]
    pub fn message_id(&self) -> Option<i64> {
        self.message_id.filter(|_| self.open)
    }

    /// Where the cursor is, or `0` when there is nothing to be on.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// How many blocks the overlay draws: the text the message shows now, then
    /// one per revision the journal holds for it.
    ///
    /// One answer for the widget and for the keys, so a `j` that walked past the
    /// last block and a list that drew one more than it could reach cannot
    /// disagree. Zero when no revision was recorded — the panel says so in a
    /// sentence rather than drawing the current text on its own, which is not a
    /// history of anything.
    #[must_use]
    pub fn len(&self, view: &ConversationView) -> usize {
        let Some(id) = self.message_id() else {
            return 0;
        };

        let revisions = view.revisions(id);
        if revisions.is_empty() {
            return 0;
        }

        usize::from(view.message(id).is_some()) + revisions.len()
    }

    /// Moves the cursor by `delta`, stopping at either end of `len` blocks.
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
    use domain::history::REVISIONS_PER_MESSAGE;
    use domain::message::{Message, MessageStatus};
    use domain::updates::UpdateEvent;
    use std::borrow::Cow;

    const CHAT: i64 = 42;

    fn message(id: i64, text: &'static str) -> Message {
        Message {
            id,
            chat_id: CHAT,
            text: Cow::Borrowed(text),
            timestamp: 1_700_000_000,
            status: MessageStatus::Received,
            is_outgoing: false,
            reply_to: None,
            media: None,
            is_service: false,
            media_id: None,
        }
    }

    fn edited(message_id: i64, text: &'static str) -> UpdateEvent {
        UpdateEvent::MessageEdited {
            chat_id: CHAT,
            message_id,
            new_text: Cow::Borrowed(text),
        }
    }

    /// A view holding one message that has been edited `edits` times.
    fn view(edits: usize) -> ConversationView {
        let mut view = ConversationView::new(CHAT);
        view.window.replace([message(5, "text")]);
        for n in 0..edits {
            let text = ["one", "two", "three", "four", "five", "six"][n.min(5)];
            let _ = view.apply_event(&edited(5, text));
        }
        view
    }

    #[test]
    fn the_blocks_are_the_current_text_and_every_recorded_revision() {
        let mut state = EditHistoryState::default();
        state.begin(5);

        assert_eq!(
            state.len(&view(0)),
            0,
            "the current text alone is no blocks"
        );
        assert_eq!(state.len(&view(1)), 2, "and the text it replaced");
        assert_eq!(
            state.len(&view(REVISIONS_PER_MESSAGE + 2)),
            1 + REVISIONS_PER_MESSAGE,
            "and no more than the journal's own bound"
        );
    }

    #[test]
    fn a_closed_overlay_has_nothing_to_list_and_nothing_to_be_on() {
        let mut state = EditHistoryState::default();
        state.begin(5);
        state.move_selection(2, 3);
        assert_eq!(state.selected(), 2);

        state.close();

        assert!(!state.is_active());
        assert_eq!(state.message_id(), None);
        assert_eq!(state.selected(), 0);
        assert_eq!(state.len(&view(3)), 0);
    }

    #[test]
    fn the_cursor_stops_at_either_end_of_the_blocks() {
        let mut state = EditHistoryState::default();
        state.begin(5);

        state.move_selection(-1, 3);
        assert_eq!(state.selected(), 0, "not above the current text");
        state.move_selection(9, 3);
        assert_eq!(state.selected(), 2, "and not past the original");
    }

    /// The message is taken when the overlay opens: the rows can move under it,
    /// and a list that followed the cursor would list another message's revisions.
    #[test]
    fn the_overlay_belongs_to_the_message_it_opened_on() {
        let mut state = EditHistoryState::default();
        state.begin(5);

        let mut view = view(2);
        view.window.replace([message(6, "a different message")]);

        assert_eq!(state.message_id(), Some(5));
        assert_eq!(
            state.len(&view),
            2,
            "the blocks are message 5's revisions, not the message now in the window"
        );
        assert!(view.revisions(6).is_empty(), "and the window's has none");
    }
}
