//! Chat entity and filtering rules.

use std::borrow::Cow;

use crate::presence::Presence;

/// The kind of a chat. `televim` displays **only** `Private`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChatKind {
    Private,
    Bot,
    Group,
    Channel,
}

impl ChatKind {
    /// Whether `televim` displays conversations of this kind.
    ///
    /// Only people. A bot, a group and a channel are all classified faithfully
    /// on the way in and then dropped here, because this is the one place that
    /// decides what the client renders — so the fetch and the filter agree by
    /// construction rather than by both spelling the rule out.
    #[must_use]
    pub fn is_private(self) -> bool {
        matches!(self, Self::Private)
    }
}

/// A single chat as shown in the chat list.
#[derive(Debug, Clone)]
pub struct Chat {
    pub id: i64,
    pub title: String,
    pub kind: ChatKind,
    pub last_message: Option<Cow<'static, str>>,
    pub unread_count: u32,
    /// Identifier of the message `last_message` was taken from, if any.
    ///
    /// The preview's text and timestamp say what the conversation last showed;
    /// this says *which* message it was, so an edit can be matched to the
    /// preview exactly. Two messages can share a second, and then the timestamp
    /// alone cannot tell the one on show from its neighbour.
    pub last_message_id: Option<i64>,
    /// Unix timestamp (seconds) of the most recent message, if any.
    pub last_timestamp: Option<i64>,
    /// Whether the account has pinned this chat to the top of the list.
    pub pinned: bool,
    /// The peer's last reported presence, if one has been seen. Sticky until the
    /// next update for this peer; nothing here expires it.
    pub presence: Option<Presence>,
    /// Whether the peer is a deleted account. The chat list still shows it;
    /// the forward picker leaves it out, since a message to it cannot land.
    pub deleted: bool,
}

impl Chat {
    #[must_use]
    pub fn is_private(&self) -> bool {
        self.kind.is_private()
    }

    #[must_use]
    pub fn has_unread(&self) -> bool {
        self.unread_count > 0
    }
}

/// Filter an iterator of `Chat`s down to the ones `televim` displays.
pub fn filter_private(chats: impl IntoIterator<Item = Chat>) -> Vec<Chat> {
    chats.into_iter().filter(Chat::is_private).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(id: i64, kind: ChatKind) -> Chat {
        Chat {
            id,
            title: format!("chat-{id}"),
            kind,
            last_message: None,
            unread_count: 0,
            last_message_id: None,
            last_timestamp: None,
            pinned: false,
            presence: None,
            deleted: false,
        }
    }

    #[test]
    fn only_people_are_displayed() {
        assert!(ChatKind::Private.is_private());
        assert!(!ChatKind::Bot.is_private(), "a bot is not a person");
        assert!(!ChatKind::Group.is_private());
        assert!(!ChatKind::Channel.is_private());
    }

    /// The chat and its kind have to answer the same question the same way; the
    /// filter is expressed through the kind, so a divergence would silently
    /// display something the rest of the crate treats as hidden.
    #[test]
    fn a_chat_answers_about_itself_as_its_kind_does() {
        for kind in [
            ChatKind::Private,
            ChatKind::Bot,
            ChatKind::Group,
            ChatKind::Channel,
        ] {
            assert_eq!(chat(1, kind).is_private(), kind.is_private(), "{kind:?}");
        }
    }

    #[test]
    fn filter_private_keeps_only_private() {
        let input = vec![
            chat(1, ChatKind::Private),
            chat(2, ChatKind::Bot),
            chat(3, ChatKind::Group),
            chat(4, ChatKind::Channel),
            chat(5, ChatKind::Private),
        ];
        let out = filter_private(input);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, 1);
        assert_eq!(out[1].id, 5);
    }

    #[test]
    fn unread_flag_reflects_count() {
        let mut c = chat(1, ChatKind::Private);
        assert!(!c.has_unread());
        c.unread_count = 3;
        assert!(c.has_unread());
    }
}
