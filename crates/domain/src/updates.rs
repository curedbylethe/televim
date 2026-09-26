//! Applying the live update feed to what the client displays.
//!
//! Two kinds of state are observable here: the conversations in the list, and
//! the messages the client has seen in them. This module is where an event from
//! the feed is folded into both, and it is pure — no clock, no IO, no runtime —
//! so all of it runs on every test run.
//!
//! [`ChatList::apply_update`] reports whether anything moved. That report is
//! what a caller redraws on: the feed carries events for conversations this
//! client does not hold, and repainting for those would be wasted work.
//!
//! # What is applied
//!
//! A new message joins the window, raises its conversation's unread count when
//! it came from someone else, and becomes the preview when it is the most
//! recent one. An edit replaces text, and reaches the preview when it is the
//! message on show — the conversation records which message that is, so the
//! match is exact rather than a guess. A deletion removes messages from the
//! window.
//!
//! Two things are deliberately left alone. Deleting a message does not lower
//! its conversation's unread count, because the window does not record whether
//! the message was one of the unread ones — a conversation holding three
//! unread messages and an older read one would lose a mark for deleting the
//! read one. And a deletion does not disturb a preview, because a preview
//! carries no identifier to match the deletion against. Fetching the chat list
//! again is what reconciles both.

use std::borrow::Cow;
use std::collections::{HashSet, VecDeque};

use crate::chat::Chat;
use crate::message::Message;

/// Something that happened to a conversation.
///
/// The variants name what changed, not where it sits: an event arrives against
/// a list the client may have replaced since, so positions would be stale
/// before they were read.
#[derive(Debug, Clone)]
pub enum UpdateEvent {
    /// A message arrived.
    ///
    /// The message names its own conversation, so the event does not repeat it.
    NewMessage(Message),

    /// A message's text changed.
    MessageEdited {
        /// The conversation the message belongs to.
        chat_id: i64,

        /// Identifier of the message within that conversation.
        message_id: i64,

        /// The text it now reads as.
        new_text: Cow<'static, str>,
    },

    /// Messages were deleted.
    ///
    /// The event names no conversation, and the update it comes from has no
    /// field that could: `updateDeleteMessages` carries the identifiers, a
    /// `pts` and a `pts_count`, and nothing else. Deletions from a channel
    /// arrive as a different update that does name its channel, and the
    /// framework discards those because a channel is not displayed.
    ///
    /// The identifiers therefore come from the single sequence private
    /// conversations share, which is what makes them usable with no
    /// conversation attached — and what makes a flat window the right shape to
    /// look them up in. That assumption is read off the wire format rather than
    /// from a live account; if it ever stopped holding, a deletion would take
    /// one conversation's message out of another's.
    MessagesDeleted {
        /// Identifiers of the deleted messages.
        message_ids: Vec<i64>,
    },
}

/// How many messages [`ChatList`] holds before the oldest is dropped.
///
/// The window exists so that an edit or a deletion can be matched against the
/// messages this client has seen, and a few hundred covers far more than any
/// screenful of conversation. The number matters because it is a ceiling on
/// memory, not a preference: without one the window would grow with every
/// message the account ever receives, which is the one thing this project's
/// memory budget cannot absorb.
pub const MESSAGE_WINDOW: usize = 500;

/// The conversations the client holds, and the messages it has seen.
///
/// This is observable state, separate from [`Session`](crate::session::Session)
/// — that models the login state machine, which knows nothing about chats.
#[derive(Debug, Default, Clone)]
pub struct ChatList {
    /// The conversations, as they were last fetched.
    pub chats: Vec<Chat>,

    /// The messages the client has seen since it started, newest first.
    ///
    /// Flat rather than one list per conversation, because every lookup here is
    /// a scan of a list that stays short in practice and a conversation's own
    /// history is a view over it. It is a `VecDeque` so that the newest message
    /// can be pushed onto the front without moving the rest, and it is capped
    /// at [`MESSAGE_WINDOW`]: an edit or a deletion for a message that has
    /// scrolled out is simply not applied, which is the same answer the list
    /// gives for a conversation it does not hold.
    pub messages: VecDeque<Message>,
}

impl ChatList {
    /// A list holding `chats`, with nothing in the message window yet.
    #[must_use]
    pub fn with_chats(chats: Vec<Chat>) -> Self {
        Self {
            chats,
            messages: VecDeque::new(),
        }
    }

    /// Applies an event, reporting whether it changed anything.
    ///
    /// `false` means the event named a conversation or a message this client
    /// does not hold — a chat that is not in the list, or a message that has
    /// scrolled out of the window — so nothing observable moved.
    #[must_use]
    pub fn apply_update(&mut self, event: UpdateEvent) -> bool {
        match event {
            UpdateEvent::NewMessage(message) => self.apply_new_message(message),
            UpdateEvent::MessageEdited {
                chat_id,
                message_id,
                new_text,
            } => self.apply_edit(chat_id, message_id, new_text),
            UpdateEvent::MessagesDeleted { message_ids } => self.apply_deletion(&message_ids),
        }
    }

    /// Adds a message to its conversation.
    fn apply_new_message(&mut self, message: Message) -> bool {
        let Some(chat) = self.chat_mut(message.chat_id) else {
            return false;
        };

        if !message.is_outgoing {
            chat.unread_count = chat.unread_count.saturating_add(1);
        }

        // The list previews the most recent message, so one at least as recent
        // as the message on show becomes the message on show. An older arrival
        // leaves it alone: a feed that is replayed out of order must not walk
        // the preview backwards.
        if chat
            .last_timestamp
            .is_none_or(|current| message.timestamp >= current)
        {
            chat.last_message_id = Some(message.id);
            chat.last_timestamp = Some(message.timestamp);
            chat.last_message = Some(message.text.clone());
        }

        // Front first, then trimmed from the back: the window keeps the newest
        // messages and cannot outgrow its ceiling however long the client runs.
        self.messages.push_front(message);
        self.messages.truncate(MESSAGE_WINDOW);

        true
    }

    /// Replaces a message's text.
    fn apply_edit(&mut self, chat_id: i64, message_id: i64, new_text: Cow<'static, str>) -> bool {
        let Some(index) = self
            .messages
            .iter()
            .position(|message| message.chat_id == chat_id && message.id == message_id)
        else {
            return false;
        };

        if self.messages[index].text == new_text {
            return false;
        }

        self.messages[index].text = new_text;

        // An edit to the message on show has to reach the preview, or the list
        // would keep quoting text the conversation no longer contains. The
        // conversation records which message the preview came from, so this is
        // exact — a timestamp would not be, because two messages can share one.
        let text = self.messages[index].text.clone();

        if let Some(chat) = self.chat_mut(chat_id)
            && chat.last_message_id == Some(message_id)
        {
            chat.last_message = Some(text);
        }

        true
    }

    /// Removes deleted messages from the window.
    ///
    /// Every conversation is searched, because the event names none: an
    /// identifier is enough to find the message wherever it lives.
    fn apply_deletion(&mut self, message_ids: &[i64]) -> bool {
        if message_ids.is_empty() {
            return false;
        }

        // A set rather than a scan of the slice per message: clearing a
        // conversation's history sends every identifier it held, and looking
        // each one up in turn would make that quadratic.
        let deleted: HashSet<i64> = message_ids.iter().copied().collect();
        let before = self.messages.len();

        self.messages
            .retain(|message| !deleted.contains(&message.id));

        self.messages.len() != before
    }

    /// The conversation with this identifier, if the client holds it.
    fn chat_mut(&mut self, chat_id: i64) -> Option<&mut Chat> {
        self.chats.iter_mut().find(|chat| chat.id == chat_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::ChatKind;
    use crate::message::MessageStatus;

    /// A conversation whose preview already reads `earlier`.
    fn chat(id: i64) -> Chat {
        Chat {
            id,
            title: format!("chat-{id}"),
            kind: ChatKind::Private,
            last_message: Some(Cow::Borrowed("earlier")),
            unread_count: 0,
            last_message_id: Some(9),
            last_timestamp: Some(1_000),
        }
    }

    /// A conversation with nothing in it yet.
    fn empty_chat(id: i64) -> Chat {
        Chat {
            last_message: None,
            last_message_id: None,
            last_timestamp: None,
            ..chat(id)
        }
    }

    fn message(
        chat_id: i64,
        id: i64,
        text: &'static str,
        timestamp: i64,
        is_outgoing: bool,
    ) -> Message {
        Message {
            id,
            chat_id,
            text: Cow::Borrowed(text),
            timestamp,
            status: if is_outgoing {
                MessageStatus::Sent
            } else {
                MessageStatus::Received
            },
            is_outgoing,
            reply_to: None,
        }
    }

    fn list() -> ChatList {
        ChatList::with_chats(vec![chat(1), empty_chat(2)])
    }

    fn arrival(chat_id: i64, id: i64, text: &'static str, timestamp: i64) -> UpdateEvent {
        UpdateEvent::NewMessage(message(chat_id, id, text, timestamp, false))
    }

    fn edit(chat_id: i64, message_id: i64, new_text: &'static str) -> UpdateEvent {
        UpdateEvent::MessageEdited {
            chat_id,
            message_id,
            new_text: Cow::Borrowed(new_text),
        }
    }

    fn deletion(message_ids: Vec<i64>) -> UpdateEvent {
        UpdateEvent::MessagesDeleted { message_ids }
    }

    /// Applies an event the test expects to land.
    ///
    /// Asserting the report here rather than discarding it keeps the setup
    /// honest: a fixture that silently stopped taking effect would otherwise
    /// turn the assertions that follow into tests of nothing.
    fn applied(list: &mut ChatList, event: UpdateEvent) {
        assert!(
            list.apply_update(event),
            "the event was expected to change the list"
        );
    }

    #[test]
    fn a_new_message_joins_the_window_and_counts_as_unread() {
        let mut list = list();

        applied(&mut list, arrival(1, 10, "hello", 2_000));

        assert_eq!(list.messages.len(), 1);
        assert_eq!(list.messages[0].id, 10);
        assert_eq!(list.messages[0].text, "hello");
        assert_eq!(list.chats[0].unread_count, 1);
    }

    #[test]
    fn a_message_the_account_sent_is_not_unread() {
        let mut list = list();

        applied(
            &mut list,
            UpdateEvent::NewMessage(message(1, 10, "mine", 2_000, true)),
        );

        assert_eq!(list.chats[0].unread_count, 0);
    }

    #[test]
    fn the_window_holds_the_newest_message_at_the_front() {
        let mut list = list();

        applied(&mut list, arrival(1, 10, "first", 2_000));
        applied(&mut list, arrival(1, 11, "second", 3_000));

        assert_eq!(list.messages[0].id, 11);
        assert_eq!(list.messages[1].id, 10);
    }

    #[test]
    fn the_newest_message_becomes_the_preview() {
        let mut list = list();

        applied(&mut list, arrival(1, 10, "newest", 2_000));

        assert_eq!(list.chats[0].last_timestamp, Some(2_000));
        assert_eq!(list.chats[0].last_message.as_deref(), Some("newest"));
        assert_eq!(
            list.chats[0].last_message_id,
            Some(10),
            "the preview has to record which message it came from"
        );
    }

    #[test]
    fn a_message_as_recent_as_the_preview_takes_its_place() {
        let mut list = list();

        applied(&mut list, arrival(1, 10, "first", 1_000));
        // The same second as the message on show. Nothing else separates them,
        // so the newer arrival has to win on its own.
        applied(&mut list, arrival(1, 11, "second", 1_000));

        assert_eq!(list.chats[0].last_timestamp, Some(1_000));
        assert_eq!(list.chats[0].last_message.as_deref(), Some("second"));
        assert_eq!(list.chats[0].last_message_id, Some(11));
    }

    #[test]
    fn the_window_holds_only_the_newest_messages() {
        let mut list = list();
        let overflow = MESSAGE_WINDOW + 5;

        for index in 0..overflow {
            let id = i64::try_from(index).expect("the index fits in an identifier");
            applied(&mut list, arrival(1, id, "text", 2_000 + id));
        }

        assert_eq!(
            list.messages.len(),
            MESSAGE_WINDOW,
            "the window is a ceiling on memory, not a preference"
        );
        assert_eq!(
            list.messages[0].id,
            i64::try_from(overflow - 1).expect("the index fits"),
            "the newest message is still at the front"
        );
        assert_eq!(
            list.messages[MESSAGE_WINDOW - 1].id,
            i64::try_from(overflow - MESSAGE_WINDOW).expect("the index fits"),
            "the oldest arrivals are the ones that were dropped"
        );
    }

    #[test]
    fn a_conversation_with_no_preview_takes_the_first_message() {
        let mut list = list();

        applied(&mut list, arrival(2, 20, "opening", 500));

        assert_eq!(list.chats[1].last_timestamp, Some(500));
        assert_eq!(list.chats[1].last_message.as_deref(), Some("opening"));
    }

    #[test]
    fn an_older_message_does_not_walk_the_preview_back() {
        let mut list = list();

        // The message still joins the window, so this is a change even though
        // the preview stays where it was.
        applied(&mut list, arrival(1, 10, "replayed", 500));

        assert_eq!(
            list.chats[0].last_timestamp,
            Some(1_000),
            "the preview was newer than the arrival"
        );
        assert_eq!(list.chats[0].last_message.as_deref(), Some("earlier"));
    }

    #[test]
    fn a_message_for_an_unknown_conversation_changes_nothing() {
        let mut list = list();

        assert!(!list.apply_update(arrival(99, 10, "hello", 2_000)));
        assert!(list.messages.is_empty());
        assert_eq!(list.chats[0].unread_count, 0);
    }

    #[test]
    fn an_edit_replaces_the_text() {
        let mut list = list();
        applied(&mut list, arrival(1, 10, "before", 2_000));

        applied(&mut list, edit(1, 10, "after"));

        assert_eq!(list.messages[0].text, "after");
    }

    #[test]
    fn an_edit_to_the_message_on_show_reaches_the_preview() {
        let mut list = list();
        applied(&mut list, arrival(1, 10, "before", 2_000));

        applied(&mut list, edit(1, 10, "after"));

        assert_eq!(list.chats[0].last_message.as_deref(), Some("after"));
    }

    #[test]
    fn an_edit_to_an_older_message_leaves_the_preview_alone() {
        let mut list = list();
        applied(&mut list, arrival(1, 10, "old", 2_000));
        applied(&mut list, arrival(1, 11, "new", 3_000));

        applied(&mut list, edit(1, 10, "old, edited"));

        assert_eq!(list.messages[1].text, "old, edited");
        assert_eq!(list.chats[0].last_message.as_deref(), Some("new"));
    }

    /// The case a timestamp cannot decide: both messages are in the same
    /// second, so "most recent" does not tell them apart. Only the identifier
    /// does, and it has to do so in both directions.
    #[test]
    fn an_edit_in_the_preview_s_second_reaches_only_the_preview() {
        let mut list = list();
        applied(&mut list, arrival(1, 10, "older", 2_000));
        applied(&mut list, arrival(1, 11, "newer", 2_000));
        assert_eq!(list.chats[0].last_message_id, Some(11));

        applied(&mut list, edit(1, 10, "older, edited"));

        assert_eq!(list.messages[1].text, "older, edited");
        assert_eq!(
            list.chats[0].last_message.as_deref(),
            Some("newer"),
            "the edited message shares the preview's timestamp but is not the preview"
        );

        applied(&mut list, edit(1, 11, "newer, edited"));

        assert_eq!(list.chats[0].last_message.as_deref(), Some("newer, edited"));
    }

    #[test]
    fn an_edit_to_a_message_the_client_does_not_hold_changes_nothing() {
        let mut list = list();

        assert!(!list.apply_update(edit(1, 10, "after")));
    }

    #[test]
    fn an_edit_that_changes_nothing_is_not_a_change() {
        let mut list = list();
        applied(&mut list, arrival(1, 10, "same", 2_000));

        assert!(
            !list.apply_update(edit(1, 10, "same")),
            "a redraw is not owed for an edit that reads the same"
        );
    }

    #[test]
    fn an_edit_only_matches_within_its_conversation() {
        let mut list = list();
        applied(&mut list, arrival(1, 10, "one", 2_000));
        applied(&mut list, arrival(2, 10, "two", 2_000));

        applied(&mut list, edit(2, 10, "two, edited"));

        assert_eq!(list.messages[0].text, "two, edited");
        assert_eq!(list.messages[1].text, "one");
    }

    #[test]
    fn a_deletion_removes_the_named_messages() {
        let mut list = list();
        applied(&mut list, arrival(1, 10, "a", 2_000));
        applied(&mut list, arrival(1, 11, "b", 3_000));

        applied(&mut list, deletion(vec![10]));

        assert_eq!(list.messages.len(), 1);
        assert_eq!(list.messages[0].id, 11);
    }

    #[test]
    fn a_deletion_searches_every_conversation() {
        let mut list = list();
        applied(&mut list, arrival(1, 10, "one", 2_000));
        applied(&mut list, arrival(2, 20, "two", 2_000));

        applied(&mut list, deletion(vec![10, 20]));

        assert!(
            list.messages.is_empty(),
            "the event names no conversation, so both are searched"
        );
    }

    #[test]
    fn a_deletion_of_messages_the_client_does_not_hold_changes_nothing() {
        let mut list = list();
        applied(&mut list, arrival(1, 10, "kept", 2_000));

        assert!(!list.apply_update(deletion(vec![99])));
        assert!(!list.apply_update(deletion(Vec::new())));
        assert_eq!(list.messages.len(), 1);
    }

    #[test]
    fn a_deletion_leaves_the_preview_and_the_unread_count_where_they_were() {
        let mut list = list();
        applied(&mut list, arrival(1, 10, "gone soon", 2_000));

        applied(&mut list, deletion(vec![10]));

        assert_eq!(list.chats[0].last_message.as_deref(), Some("gone soon"));
        assert_eq!(
            list.chats[0].unread_count, 1,
            "the window does not record whether this was the unread message"
        );
    }
}
