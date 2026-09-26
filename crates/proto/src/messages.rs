//! Sending, editing and deleting messages, as `domain` types.
//!
//! `telegram-framework` hands over the result of an operation already narrowed
//! to primitives and strings. All that is left is to put a sent message into the
//! [`Message`] the domain applies — the same seam `stream` and `history` use —
//! and to narrow the message identifiers a caller names to the range Telegram
//! numbers messages in.
//!
//! The client half of this module only exists under the `live` feature, which is
//! why [`ProtoClient`](crate::ProtoClient) is named rather than linked: a link
//! would dangle in a default-feature build. The narrowing helper does not touch
//! the framework at all, so it and its tests do not need the feature.
//!
//! # Why narrowing is an error here and a dropped page there
//!
//! `fetch_around` narrows the message a page is centred on and treats a value
//! outside the wire's range as a page it cannot ask for: dropping it is right,
//! because the reader is left where they were and nothing is lost. An edit or a
//! deletion names one message on purpose, and there is no page to leave out — a
//! request that silently skipped it would report success for an operation that
//! never happened, which is worse than an error. So the same conversion reports
//! the failure instead of dropping it.
//!
//! # Why the text is not checked here
//!
//! Empty text is refused by the interface, which has somewhere to say so, and by
//! the framework, which owns the contract for every caller. A third check here
//! would have no caller that benefits from it.

#[cfg(any(feature = "live", test))]
use crate::error::ProtoError;

#[cfg(feature = "live")]
use crate::types::ProtoMessage;

#[cfg(feature = "live")]
use domain::message::Message;

/// Narrows a message identifier to the range Telegram numbers messages with.
///
/// A message identifier is an `i32` on the wire, so a value outside that range
/// cannot have come from Telegram and cannot be named in a request. Reported
/// rather than converted: saturating would turn it into a request for a message
/// that does not exist, which is a worse answer than no answer at all.
///
/// `peer_id` travels with the error as well as into the log line: which
/// conversation the impossible identifier was addressed to is the part that
/// makes the failure actionable, and an error that named only the identifier
/// would send the reader back to the log to find the peer.
///
/// # Errors
///
/// Returns [`ProtoError::MessageIdOutOfRange`] when `id` does not fit an `i32`.
#[cfg(any(feature = "live", test))]
pub(crate) fn narrow_id(id: i64, peer_id: i64) -> Result<i32, ProtoError> {
    i32::try_from(id).map_err(|_| {
        tracing::warn!(
            peer_id,
            message_id = id,
            "a message identifier outside telegram's range cannot be named in a request"
        );
        ProtoError::MessageIdOutOfRange { peer_id, id }
    })
}

/// The message operations, which need the framework's client.
///
/// Every one of them returns `domain` types or this crate's error, so a caller
/// needs no knowledge of Telegram's identifiers. A flood wait arrives as a
/// [`RequestError::Rpc`](telegram_framework::RequestError::Rpc) carrying the
/// delay in its `value`; backing off is the caller's job.
#[cfg(feature = "live")]
impl crate::ProtoClient {
    /// Sends a message to a conversation, optionally as a reply.
    ///
    /// `reply_to` names another message of the same conversation, or is `None`
    /// for an ordinary message. The returned [`Message`] is the message as the
    /// server now has it, translated through the same seam the feed and history
    /// use, so a message just sent and the same message arriving over the feed
    /// are described identically.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Framework`](crate::ProtoError::Framework) when the
    /// conversation is not in the session's peer cache, when the text cannot be
    /// sent, or when Telegram rejects the request; and
    /// [`ProtoError::MessageIdOutOfRange`](crate::ProtoError::MessageIdOutOfRange)
    /// when `reply_to` names a message Telegram could not have numbered.
    pub async fn send_message(
        &self,
        peer_id: i64,
        text: &str,
        reply_to: Option<i64>,
    ) -> Result<Message, ProtoError> {
        let reply_to = reply_to.map(|id| narrow_id(id, peer_id)).transpose()?;

        let sent = self.inner().send_message(peer_id, text, reply_to).await?;

        Ok(Message::from(ProtoMessage::from(sent)))
    }

    /// Replaces the text of a message the account wrote.
    ///
    /// Nothing comes back for the message. `grammers` discards the updates an
    /// edit answers with, so the new text reaches a caller as a
    /// [`MessageEdited`](domain::updates::UpdateEvent::MessageEdited) event and
    /// nowhere else.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Framework`](crate::ProtoError::Framework) when the
    /// conversation is not in the session's peer cache, when the text cannot be
    /// sent, or when Telegram rejects the request; and
    /// [`ProtoError::MessageIdOutOfRange`](crate::ProtoError::MessageIdOutOfRange)
    /// when `message_id` is outside Telegram's range.
    pub async fn edit_message(
        &self,
        peer_id: i64,
        message_id: i64,
        text: &str,
    ) -> Result<(), ProtoError> {
        let message_id = narrow_id(message_id, peer_id)?;

        self.inner().edit_message(peer_id, message_id, text).await?;

        Ok(())
    }

    /// Deletes messages, for both sides.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Framework`](crate::ProtoError::Framework) when the
    /// conversation is not in the session's peer cache, or when Telegram rejects
    /// the request; and
    /// [`ProtoError::MessageIdOutOfRange`](crate::ProtoError::MessageIdOutOfRange)
    /// when any identifier is outside Telegram's range. Nothing is deleted when
    /// one is: the identifiers are narrowed before the request is sent.
    pub async fn delete_messages(&self, peer_id: i64, ids: &[i64]) -> Result<(), ProtoError> {
        let ids = ids
            .iter()
            .map(|id| narrow_id(*id, peer_id))
            .collect::<Result<Vec<i32>, _>>()?;

        self.inner().delete_messages(peer_id, &ids).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identifier_inside_telegram_s_range_is_narrowed_unchanged() {
        assert_eq!(narrow_id(1, 42).ok(), Some(1));
        assert_eq!(narrow_id(i64::from(i32::MIN), 42).ok(), Some(i32::MIN));
        assert_eq!(narrow_id(i64::from(i32::MAX), 42).ok(), Some(i32::MAX));
        assert_eq!(
            narrow_id(-1, 42).ok(),
            Some(-1),
            "the check is about the wire's range, not about which values telegram would use"
        );
    }

    #[test]
    fn an_identifier_outside_telegram_s_range_is_reported() {
        for id in [
            i64::from(i32::MAX) + 1,
            i64::from(i32::MIN) - 1,
            i64::MAX,
            i64::MIN,
        ] {
            assert!(
                matches!(
                    narrow_id(id, 42),
                    Err(ProtoError::MessageIdOutOfRange {
                        peer_id: 42,
                        id: got,
                    }) if got == id
                ),
                "{id} cannot name a message, and the error has to name the peer it was for"
            );
        }
    }
}
