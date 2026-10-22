//! The chat list and the selection cursor.

use domain::chat::Chat;
use domain::updates::ChatList;

/// The chat list and the selection cursor.
pub struct ChatListState {
    /// The conversations, and the messages the client has seen in them.
    ///
    /// One value rather than a list beside a window: an event from the feed
    /// moves both, and keeping them apart would leave the preview and the
    /// unread count somewhere the event never reached. [`ChatListState`] and
    /// [`ConversationState`](super::conversation::ConversationState) are folded
    /// in together, in the one place that takes the feed's events.
    pub(crate) list: ChatList,

    pub selected_chat: usize,

    /// The conversations in Telegram's archive folder.
    ///
    /// Kept beside the visible list because the archive is a place, not a flag
    /// on a chat: the visible list is exactly what the reader sees.
    pub(crate) archived: Vec<Chat>,

    /// Whether the archived list is drawn over the chat column.
    pub(crate) archived_open: bool,

    /// The cursor in that list.
    pub(crate) archived_selected: usize,
}

impl ChatListState {
    /// Nothing fetched, and the highlight on the first row.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            list: ChatList::default(),
            selected_chat: 0,
            archived: Vec::new(),
            archived_open: false,
            archived_selected: 0,
        }
    }

    /// The chats a forward can land in: every chat the list shows, except a
    /// deleted account, which has no one left to receive it.
    ///
    /// The forward picker's cursor indexes this list, so the picker and its
    /// confirm both read it rather than the chat list itself.
    pub(crate) fn forward_destinations(&self) -> Vec<&Chat> {
        self.list
            .chats
            .iter()
            .filter(|chat| !chat.deleted)
            .collect()
    }

    /// Moves `chat_id` to where its pin puts it. The highlight stays on the
    /// chat it was on, which may itself have moved; nothing changes when the
    /// list does not hold the chat.
    pub(crate) fn set_pinned(&mut self, chat_id: i64, pinned: bool) {
        let highlighted = self.list.chats.get(self.selected_chat).map(|chat| chat.id);
        if !self.list.set_pinned(chat_id, pinned) {
            return;
        }
        if let Some(index) =
            highlighted.and_then(|id| self.list.chats.iter().position(|chat| chat.id == id))
        {
            self.selected_chat = index;
        }
    }

    /// Installs a freshly fetched chat list.
    ///
    /// The list is replaced wholesale, and the selection is clamped rather
    /// than reset, because the reader's place is a position in a list that may
    /// have become shorter. An empty list leaves nothing selected, which is
    /// what closes the conversation: there is no chat for the window to belong
    /// to.
    pub(crate) fn install(&mut self, chats: Vec<Chat>) {
        self.list = ChatList::with_chats(chats);

        let last = self.list.chats.len().saturating_sub(1);
        self.selected_chat = self.selected_chat.min(last);
    }

    /// Installs a freshly fetched chat list while keeping the open conversation.
    ///
    /// The highlight is restored by the open conversation's own id, never by
    /// index: the list comes back ordered by recency, and an index means a
    /// different conversation on either side of the fetch. An id the new list
    /// does not hold falls back to the top and reports `false`, so the caller
    /// can say where the reader landed.
    pub(crate) fn reinstall(&mut self, chats: Vec<Chat>, open: i64) -> bool {
        self.list = ChatList::with_chats(chats);

        if let Some(index) = self.list.chats.iter().position(|chat| chat.id == open) {
            self.selected_chat = index;
            true
        } else {
            self.selected_chat = 0;
            false
        }
    }

    /// Replaces the archived conversations, leaving the visible list alone.
    pub(crate) fn install_archived(&mut self, archived: Vec<Chat>) {
        self.archived = archived;
        self.clamp_archived();
    }

    /// Opens the archived list over the chat column.
    pub(crate) fn open_archived(&mut self) {
        self.archived_open = true;
        self.clamp_archived();
    }

    /// Puts the archived list away.
    pub(crate) fn close_archived(&mut self) {
        self.archived_open = false;
    }

    /// Whether the archived list is on show.
    #[must_use]
    pub(crate) fn archived_is_open(&self) -> bool {
        self.archived_open
    }

    /// The archived conversations, newest first.
    #[must_use]
    pub(crate) fn archived(&self) -> &[Chat] {
        &self.archived
    }

    /// Where the cursor is in the archived list.
    #[must_use]
    pub(crate) fn archived_cursor(&self) -> usize {
        self.archived_selected
    }

    /// Moves the archived cursor by `delta`, wrapping at either end.
    pub(crate) fn move_archived(&mut self, delta: isize) {
        if self.archived.is_empty() {
            return;
        }
        let len = i64::try_from(self.archived.len()).unwrap_or(i64::MAX);
        let next = (i64::try_from(self.archived_selected).unwrap_or(0)
            + i64::try_from(delta).unwrap_or(0))
        .rem_euclid(len);
        self.archived_selected = usize::try_from(next).unwrap_or(0);
    }

    /// Keeps the archived cursor on a row, and closes an empty list.
    pub(crate) fn clamp_archived(&mut self) {
        if self.archived.is_empty() {
            self.archived_open = false;
            self.archived_selected = 0;
        } else {
            self.archived_selected = self.archived_selected.min(self.archived.len() - 1);
        }
    }

    /// Moves `chat_id` out of the visible list and into the archive.
    ///
    /// Returns the chat's name when it moved, so the caller can say so.
    pub(crate) fn archive(&mut self, chat_id: i64) -> Option<String> {
        let chat = self.list.remove(chat_id)?;
        let name = chat.title.clone();
        self.archived.retain(|present| present.id != chat_id);
        self.archived.push(chat);
        self.selected_chat = self
            .selected_chat
            .min(self.list.chats.len().saturating_sub(1));
        Some(name)
    }

    /// Puts an archived chat back at its ordered place in the visible list.
    pub(crate) fn unarchive(&mut self, chat_id: i64) -> Option<String> {
        let at = self.archived.iter().position(|chat| chat.id == chat_id)?;
        let chat = self.archived.remove(at);
        let name = chat.title.clone();
        let highlighted = self.list.chats.get(self.selected_chat).map(|chat| chat.id);
        self.list.insert(chat);
        // The insert can land above the highlight, which would leave the same
        // index naming another conversation.
        if let Some(index) =
            highlighted.and_then(|id| self.list.chats.iter().position(|chat| chat.id == id))
        {
            self.selected_chat = index;
        }
        self.clamp_archived();
        Some(name)
    }

    /// Moves the highlight to `index` in the chat list.
    ///
    /// The caller has checked the index names a chat: an index outside the
    /// list moves nothing, and that check stays where the list is read.
    pub(crate) fn select(&mut self, index: usize) {
        self.selected_chat = index;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::chat::ChatKind;

    fn chat(id: i64, deleted: bool) -> Chat {
        Chat {
            read_outbox_max_id: None,
            id,
            title: format!("chat-{id}"),
            kind: ChatKind::Private,
            last_message: None,
            unread_count: 0,
            last_message_id: None,
            last_timestamp: None,
            pinned: false,
            presence: None,
            deleted,
        }
    }

    #[test]
    fn a_deleted_account_is_listed_but_is_no_forward_destination() {
        let mut state = ChatListState::new();
        state.install(vec![chat(1, false), chat(2, true), chat(3, false)]);

        let ids: Vec<i64> = state.forward_destinations().iter().map(|c| c.id).collect();

        assert_eq!(ids, vec![1, 3]);
        assert_eq!(state.list.chats.len(), 3, "the chat list still shows it");
    }

    #[test]
    fn unarchiving_keeps_the_highlight_on_the_chat_it_was_on() {
        let mut state = ChatListState::new();
        let dated = |id: i64, at: i64| Chat {
            last_timestamp: Some(at),
            ..chat(id, false)
        };
        state.install(vec![dated(1, 300), dated(2, 200), dated(3, 100)]);
        state.select(2);
        state.archived.push(dated(4, 250));

        state.unarchive(4);

        let ids: Vec<i64> = state.list.chats.iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![1, 4, 2, 3]);
        assert_eq!(state.list.chats[state.selected_chat].id, 3);
    }

}
