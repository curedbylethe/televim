//! Chat entity and filtering rules.

use std::borrow::Cow;

/// The kind of a chat. `televim` displays **only** `Private`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChatKind {
    Private,
    Bot,
    Group,
    Channel,
}

/// A single chat as shown in the chat list.
#[derive(Debug, Clone)]
pub struct Chat {
    pub id: i64,
    pub title: String,
    pub kind: ChatKind,
    pub last_message: Option<Cow<'static, str>>,
    pub unread_count: u32,
    /// Unix timestamp (seconds) of the most recent message, if any.
    pub last_timestamp: Option<i64>,
}

impl Chat {
    #[must_use]
    pub fn is_private(&self) -> bool {
        matches!(self.kind, ChatKind::Private)
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
            last_timestamp: None,
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
