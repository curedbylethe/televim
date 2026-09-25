//! Message entity.

use std::borrow::Cow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageStatus {
    Sending,
    Sent,
    Failed,
    Received,
}

/// A message in a conversation.
///
/// `text` uses `Cow<'static, str>` so callers can borrow when possible and
/// own only when necessary ("Zero-Copy Where Possible").
#[derive(Debug, Clone)]
pub struct Message {
    pub id: i64,
    pub chat_id: i64,
    pub text: Cow<'static, str>,
    pub timestamp: i64,
    pub status: MessageStatus,
    pub is_outgoing: bool,
}

impl Message {
    #[must_use]
    pub fn is_from_self(&self) -> bool {
        self.is_outgoing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outgoing_flag() {
        let m = Message {
            id: 1,
            chat_id: 1,
            text: Cow::Borrowed("hi"),
            timestamp: 0,
            status: MessageStatus::Sent,
            is_outgoing: true,
        };
        assert!(m.is_from_self());
    }
}
