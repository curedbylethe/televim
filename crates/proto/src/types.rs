//! Internal DTOs. Never expose `grammers` types from here.

use domain::chat::{Chat, ChatKind};
use domain::message::{Message, MessageStatus};

#[derive(Debug, Clone)]
pub struct ProtoChat {
    pub id: i64,
    pub title: String,
    pub is_private: bool,
    pub unread_count: u32,
    pub last_timestamp: Option<i64>,
    pub last_message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProtoMessage {
    pub id: i64,
    pub chat_id: i64,
    pub text: String,
    pub timestamp: i64,
    pub is_outgoing: bool,
}

impl From<ProtoChat> for Chat {
    fn from(p: ProtoChat) -> Self {
        Chat {
            id: p.id,
            title: p.title,
            kind: if p.is_private {
                ChatKind::Private
            } else {
                ChatKind::Group // placeholder; PR 3 will refine
            },
            last_message: p.last_message.map(std::borrow::Cow::Owned),
            unread_count: p.unread_count,
            last_timestamp: p.last_timestamp,
        }
    }
}

impl From<ProtoMessage> for Message {
    fn from(p: ProtoMessage) -> Self {
        Message {
            id: p.id,
            chat_id: p.chat_id,
            text: std::borrow::Cow::Owned(p.text),
            timestamp: p.timestamp,
            status: MessageStatus::Received,
            is_outgoing: p.is_outgoing,
        }
    }
}
