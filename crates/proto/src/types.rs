//! Internal DTOs. Never expose `grammers` types from here.

use std::borrow::Cow;

use domain::chat::{Chat, ChatKind};
use domain::message::{Message, MessageStatus};

/// A conversation on its way from the framework into `domain`.
///
/// The fields line up with [`Chat`] one for one. The preview is owned because
/// it arrives over the network and there is nothing in this process to borrow
/// it from; `Chat` takes a `Cow` so that a caller building one from its own
/// constant can still avoid the copy.
#[derive(Debug, Clone)]
pub struct ProtoChat {
    pub id: i64,
    pub title: String,

    /// What the peer is.
    ///
    /// A [`ChatKind`] rather than a "is this private" flag, because Telegram
    /// has four answers to that question and a flag has two. A bot is neither a
    /// person nor a group, so folding it into either loses the distinction
    /// here — before the one layer that decides what televim displays has had a
    /// chance to look at it.
    pub kind: ChatKind,

    pub unread_count: u32,
    pub last_timestamp: Option<i64>,
    pub last_message: Option<String>,
}

/// A message on its way from the framework into `domain`.
///
/// There is no status field, because Telegram does not report one. A message
/// has either reached the account or it has not, so the only thing that
/// separates "sent" from "received" is who wrote it — that is
/// [`is_outgoing`](ProtoMessage::is_outgoing), and [`Message::status`] is
/// derived from it.
#[derive(Debug, Clone)]
pub struct ProtoMessage {
    pub id: i64,
    pub chat_id: i64,
    pub text: String,
    pub timestamp: i64,
    pub is_outgoing: bool,
}

impl From<ProtoChat> for Chat {
    fn from(chat: ProtoChat) -> Self {
        Self {
            id: chat.id,
            title: chat.title,
            kind: chat.kind,
            last_message: chat.last_message.map(Cow::Owned),
            unread_count: chat.unread_count,
            last_timestamp: chat.last_timestamp,
        }
    }
}

impl From<ProtoMessage> for Message {
    fn from(message: ProtoMessage) -> Self {
        Self {
            id: message.id,
            chat_id: message.chat_id,
            text: Cow::Owned(message.text),
            timestamp: message.timestamp,
            status: status_of(message.is_outgoing),
            is_outgoing: message.is_outgoing,
        }
    }
}

/// The status a message has once Telegram has delivered it.
///
/// [`MessageStatus::Sending`] and [`MessageStatus::Failed`] describe a local
/// attempt that has not been acknowledged, which is not a state anything
/// arriving over the feed can be in. So the direction is the whole of the
/// answer: a message the account wrote and that has come back is
/// [`MessageStatus::Sent`], and everything else came from someone else and is
/// [`MessageStatus::Received`].
fn status_of(is_outgoing: bool) -> MessageStatus {
    if is_outgoing {
        MessageStatus::Sent
    } else {
        MessageStatus::Received
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A conversation with everything but the fields under test filled in.
    fn proto_chat(id: i64, kind: ChatKind) -> ProtoChat {
        ProtoChat {
            id,
            title: format!("chat {id}"),
            kind,
            unread_count: 0,
            last_timestamp: None,
            last_message: None,
        }
    }

    /// A message with everything but the fields under test filled in.
    fn proto_message(is_outgoing: bool) -> ProtoMessage {
        ProtoMessage {
            id: 1,
            chat_id: 42,
            text: "hello".to_owned(),
            timestamp: 1_700_000_000,
            is_outgoing,
        }
    }

    #[test]
    fn every_kind_reaches_the_domain_unchanged() {
        for kind in [
            ChatKind::Private,
            ChatKind::Bot,
            ChatKind::Group,
            ChatKind::Channel,
        ] {
            let chat: Chat = proto_chat(1, kind).into();
            assert_eq!(
                chat.kind, kind,
                "a bot is not a group, and neither is a channel"
            );
        }
    }

    #[test]
    fn a_preview_becomes_owned_text_or_stays_absent() {
        let mut source = proto_chat(1, ChatKind::Private);

        source.last_message = Some("see you at six".to_owned());
        let chat: Chat = source.into();
        assert_eq!(chat.last_message.as_deref(), Some("see you at six"));

        let bare: Chat = proto_chat(2, ChatKind::Private).into();
        assert_eq!(bare.last_message, None, "a chat need not have a preview");
    }

    #[test]
    fn counts_and_timestamps_survive_the_trip() {
        let mut source = proto_chat(1, ChatKind::Private);
        source.unread_count = 3;
        source.last_timestamp = Some(1_700_000_000);

        let chat: Chat = source.into();
        assert_eq!(chat.id, 1);
        assert_eq!(chat.title, "chat 1");
        assert_eq!(chat.unread_count, 3);
        assert_eq!(chat.last_timestamp, Some(1_700_000_000));

        let dateless: Chat = proto_chat(2, ChatKind::Private).into();
        assert_eq!(
            dateless.last_timestamp, None,
            "a conversation with no messages has no timestamp, not the epoch"
        );
    }

    #[test]
    fn a_message_reports_its_direction_as_its_status() {
        let outgoing: Message = proto_message(true).into();
        assert!(
            matches!(outgoing.status, MessageStatus::Sent),
            "the account wrote it and telegram gave it back, so it went out"
        );
        assert!(outgoing.is_outgoing);

        let incoming: Message = proto_message(false).into();
        assert!(
            matches!(incoming.status, MessageStatus::Received),
            "nobody has to acknowledge a message someone else wrote"
        );
        assert!(!incoming.is_outgoing);
    }

    #[test]
    fn a_message_keeps_its_text_and_place() {
        let message: Message = proto_message(false).into();

        assert_eq!(message.id, 1);
        assert_eq!(message.chat_id, 42);
        assert_eq!(message.text, "hello");
        assert_eq!(message.timestamp, 1_700_000_000);
    }

    /// The two halves have to compose: a kind that reaches `domain` intact is
    /// what lets the filter drop a bot rather than mistake it for a person.
    #[test]
    fn only_people_survive_the_filter() {
        let chats: Vec<Chat> = vec![
            proto_chat(1, ChatKind::Private).into(),
            proto_chat(2, ChatKind::Bot).into(),
            proto_chat(3, ChatKind::Group).into(),
            proto_chat(4, ChatKind::Channel).into(),
            proto_chat(5, ChatKind::Private).into(),
        ];

        let visible = domain::chat::filter_private(chats);

        assert_eq!(
            visible.iter().map(|chat| chat.id).collect::<Vec<_>>(),
            vec![1, 5],
            "televim displays people and nobody else"
        );
    }
}
