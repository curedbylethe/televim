//! Wraps `telegram_framework::Client`. No `grammers` types leak out.

use domain::chat::Chat;
use domain::message::Message;

use crate::types::{ProtoChat, ProtoMessage};

#[derive(Debug)]
pub struct ProtoClient {
    // TODO: hold a `telegram_framework::Client`.
    _private: (),
}

impl ProtoClient {
    #[must_use]
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// TODO: fetch the private-chat list.
    pub fn fetch_chats(&self) -> Result<Vec<Chat>, ProtoError> {
        Ok(vec![])
    }

    /// TODO: fetch a slice of a conversation.
    pub fn fetch_messages(&self, _chat_id: i64) -> Result<Vec<Message>, ProtoError> {
        Ok(vec![])
    }

    // Kept to silence dead-code lint and to document intent.
    #[allow(dead_code)]
    fn _internal_convert(p: ProtoChat, m: ProtoMessage) -> (Chat, Message) {
        (p.into(), m.into())
    }
}

impl Default for ProtoClient {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("not implemented")]
    NotImplemented,
    #[error("framework error: {0}")]
    Framework(#[from] telegram_framework::Error),
}
