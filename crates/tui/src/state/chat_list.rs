//! The chat list and the selection cursor.

use domain::updates::ChatList;

/// The chat list and the selection cursor.
pub struct ChatListState {
    /// The conversations, and the messages the client has seen in them.
    ///
    /// One value rather than a list beside a window: an event from the feed
    /// moves both, and keeping them apart would leave the preview and the
    /// unread count somewhere the event never reached. [`App::apply_update`] is
    /// the one place either is folded in.
    pub(crate) list: ChatList,

    pub selected_chat: usize,
}

impl ChatListState {
    /// Nothing fetched, and the highlight on the first row.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            list: ChatList::default(),
            selected_chat: 0,
        }
    }
}
