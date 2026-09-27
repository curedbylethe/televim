//! Sending, editing and deleting messages, described in this crate's own
//! vocabulary.
//!
//! `grammers` stops at this module's private functions. [`MessageInfo`] is built
//! from numbers and strings, so `proto` can turn the result of a send into a
//! `domain` type without ever naming a `grammers` type — which is what
//! `make boundary` asserts for the whole crate.
//!
//! # Why the typed API and nothing else
//!
//! Each of the three operations goes through `grammers`' typed method rather
//! than through [`Client::invoke`](crate::Client::invoke) and a hand-built
//! request. The typed layer is the only one that knows the parts the wire does
//! not make obvious: [`Client::send_message`] generates the `random_id` that
//! Telegram needs to deduplicate a retried send, so a caller who built
//! `SendMessage` by hand would have to invent one and would have no way to tell
//! a resend from a new message; `grammers` maps the reply onto
//! `InputReplyToMessage` for us; and [`Client::delete_messages`] picks the
//! per-channel request when the peer is a channel. [`Client::invoke`] remains
//! the escape hatch for the operations this crate has not wrapped, and reaching
//! for it here would trade all of that away to save a function call.
//!
//! # The one rule that is not the same for all three
//!
//! [`Client::edit_message`] is enforced by Telegram: an account cannot edit
//! another account's message, so the server refuses it and the caller's own
//! check is defence in depth. [`Client::delete_messages`] is **not** enforced
//! that way. Its `revoke` is not a permission — it chooses whether a deletion
//! applies to both sides or to this account alone — and, as `grammers` itself
//! warns, the request carries no peer for the messages' identifiers, so neither
//! Telegram nor this crate can check that the identifiers name messages in the
//! conversation the caller meant. Deleting another person's message from a
//! private chat is therefore something Telegram permits. A caller that reaches
//! this crate directly gets no protection from it: what stands between an
//! identifier and a deletion is whatever the caller decides to put there. In
//! the interface that is the `y/n` confirmation, which is the only guard.
//!
//! # Deletions are always for both sides
//!
//! There is no way to ask for deletion on this account alone. `grammers`'
//! `delete_messages` hard-codes `revoke: true`, and asking for the other scope
//! would mean the crate's first raw `tl` call — which the typed-API rule above
//! exists to avoid — together with a way to ask the reader which scope they
//! meant. The capability is deliberately absent rather than half-present: a
//! `revoke: false` hidden under the same confirmation every delete already uses
//! would make each one silently "for both".
//!
//! # Cancelling is out of scope
//!
//! A message that is still on its way cannot be cancelled here. Doing so needs
//! task-abort machinery, and the interface refuses to delete a message that has
//! not been acknowledged rather than pretending to.

use grammers_client::message::InputMessage;

use crate::client::Client;
use crate::error::{FrameworkError, RequestError};
use crate::updates::{MessageInfo, message_info};

/// The most characters Telegram accepts in one message.
///
/// Telegram counts characters rather than bytes, so a limit of 4096 permits a
/// message of 4096 emoji even though that is twice as many bytes. Counting
/// [`str::chars`] — rather than [`str::len`] — is what keeps the two in step.
pub const TEXT_LIMIT: usize = 4096;

/// Checks that `text` can be sent as a message.
///
/// Rejects text that is empty or nothing but whitespace, and text longer than
/// [`TEXT_LIMIT`] characters. Pure and free of a client, so the rule runs on
/// every CI job rather than only against a datacenter — and it runs before the
/// round trip, so a caller finds out that the text cannot be sent without
/// spending a request to be told.
///
/// Emptiness is checked first, and the order is load-bearing rather than
/// incidental: whitespace-only text is comfortably inside the length limit yet
/// trims to nothing, so reordering the two checks would report it as too long
/// instead of as empty.
///
/// # Errors
///
/// Returns [`FrameworkError::TextEmpty`] when `text` is empty or only
/// whitespace, and [`FrameworkError::TextTooLong`] when it is longer than
/// [`TEXT_LIMIT`] characters.
pub fn validate_text(text: &str) -> Result<(), FrameworkError> {
    if text.trim().is_empty() {
        return Err(FrameworkError::TextEmpty);
    }

    let chars = text.chars().count();
    if chars > TEXT_LIMIT {
        return Err(FrameworkError::TextTooLong {
            chars,
            limit: TEXT_LIMIT,
        });
    }

    Ok(())
}

impl Client {
    /// Sends a message to a conversation.
    ///
    /// `peer_id` is the conversation's *bare* identifier — the same number a
    /// [`DialogInfo`](crate::DialogInfo) reports and the same one a
    /// [`MessageInfo`] names as its chat. When `reply_to` is `Some`, the message
    /// is sent as a reply to that message of the same conversation.
    ///
    /// The text is validated before the request, so nothing is sent that
    /// Telegram would refuse for its length. The returned [`MessageInfo`] is the
    /// message as it now exists on the server: its identifier is the real one
    /// Telegram assigned, not a local placeholder, and whether it has been read
    /// is not this crate's to report.
    ///
    /// # Peer cache
    ///
    /// Addressing a peer takes the `access_hash` Telegram handed out for it, and
    /// only [`Client::fetch_dialogs`](crate::Client::fetch_dialogs) discloses
    /// those, so the chat list has to have been fetched: a conversation this
    /// client has never seen is reported as [`FrameworkError::UnknownPeer`]
    /// rather than sent as a request Telegram would reject.
    ///
    /// # Errors
    ///
    /// Returns [`FrameworkError::TextEmpty`] or [`FrameworkError::TextTooLong`]
    /// when the text cannot be sent, [`FrameworkError::UnknownPeer`] when the
    /// conversation is not in the session's peer cache, and
    /// [`FrameworkError::Request`] when Telegram rejects the request or the
    /// connection fails.
    pub async fn send_message(
        &self,
        peer_id: i64,
        text: &str,
        reply_to: Option<i32>,
    ) -> Result<MessageInfo, FrameworkError> {
        validate_text(text)?;

        let Some(peer) = self.peer_ref(peer_id) else {
            tracing::warn!(
                peer_id,
                "a message was sent to a conversation that is not in the peer cache"
            );
            return Err(FrameworkError::UnknownPeer(peer_id));
        };

        // Boxed because the state this future has to carry is large — over
        // twenty kilobytes — and it is created on the stack of whatever asked for
        // the send. On a runtime this program's requests run on, that is a stack
        // that does not need the pressure. `edit_message` and `delete_messages`
        // below are not large enough to need the same treatment.
        let message = Box::pin(
            self.inner()
                .send_message(peer, InputMessage::new().text(text).reply_to(reply_to)),
        )
        .await
        .map_err(|error| FrameworkError::from(RequestError::from_invocation(&error)))?;

        // A send can cache a peer or move the datacenter, and that only reaches
        // the store through this call. `persist_if_dirty` skips the write when
        // nothing changed, so this is free in the ordinary case.
        self.flush_session();

        tracing::debug!(peer_id, message_id = message.id(), "sent a message");

        Ok(message_info(
            message.id(),
            peer_id,
            message.text(),
            message.date().timestamp(),
            message.outgoing(),
            message.reply_to_message_id(),
        ))
    }

    /// Replaces the text of a message the account wrote.
    ///
    /// Telegram enforces the direction: an account cannot edit another
    /// account's message, so a request that names one is refused rather than
    /// obeyed. The new text is validated exactly as a send's is.
    ///
    /// Nothing is returned for the message itself. `grammers`' own
    /// `edit_message` discards the `Updates` the request answers with, and an
    /// edited message arrives over the feed as
    /// [`UpdateKind::MessageEdited`](crate::UpdateKind::MessageEdited) — which
    /// is the only path by which any caller learns the new text.
    ///
    /// # Errors
    ///
    /// Returns the same text and peer errors as [`Client::send_message`], and
    /// [`FrameworkError::Request`] when Telegram rejects the request or the
    /// connection fails.
    pub async fn edit_message(
        &self,
        peer_id: i64,
        message_id: i32,
        text: &str,
    ) -> Result<(), FrameworkError> {
        validate_text(text)?;

        let Some(peer) = self.peer_ref(peer_id) else {
            tracing::warn!(
                peer_id,
                "a message was edited in a conversation that is not in the peer cache"
            );
            return Err(FrameworkError::UnknownPeer(peer_id));
        };

        self.inner()
            .edit_message(peer, message_id, InputMessage::new().text(text))
            .await
            .map_err(|error| FrameworkError::from(RequestError::from_invocation(&error)))?;

        self.flush_session();

        tracing::debug!(peer_id, message_id, "edited a message");

        Ok(())
    }

    /// Deletes messages, for both sides.
    ///
    /// `peer_id` is the conversation the identifiers belong to, and is used to
    /// pick the request: a channel takes a channel-scoped one and everything
    /// else the account-wide one. Only one of the two carries the peer, which is
    /// why this takes it even though the account-wide request cannot use it —
    /// see the module documentation for what that means for the caller.
    ///
    /// Nothing is returned. `grammers` reports a `pts_count`, which counts
    /// updates rather than messages and would be a lie any caller read as "how
    /// many were deleted". A deletion arrives over the feed as
    /// [`UpdateKind::MessagesDeleted`](crate::UpdateKind::MessagesDeleted),
    /// which names every identifier irrespective of whether the request deleted
    /// anything for it.
    ///
    /// # Errors
    ///
    /// Returns [`FrameworkError::UnknownPeer`] when the conversation is not in
    /// the session's peer cache, and [`FrameworkError::Request`] when Telegram
    /// rejects the request or the connection fails.
    pub async fn delete_messages(&self, peer_id: i64, ids: &[i32]) -> Result<(), FrameworkError> {
        let Some(peer) = self.peer_ref(peer_id) else {
            tracing::warn!(
                peer_id,
                "messages were deleted from a conversation that is not in the peer cache"
            );
            return Err(FrameworkError::UnknownPeer(peer_id));
        };

        // The `usize` grammers returns is a count of updates, not of deletions,
        // and is deliberately dropped rather than surfaced as either.
        let _ = self
            .inner()
            .delete_messages(peer, ids)
            .await
            .map_err(|error| FrameworkError::from(RequestError::from_invocation(&error)))?;

        self.flush_session();

        tracing::debug!(peer_id, deleted = ids.len(), "deleted messages");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_within_the_limit_is_accepted() {
        assert!(validate_text("hello").is_ok());
        assert!(validate_text("x").is_ok());

        let at_the_limit = "x".repeat(TEXT_LIMIT);
        assert!(
            validate_text(&at_the_limit).is_ok(),
            "{TEXT_LIMIT} characters is exactly what telegram accepts"
        );
    }

    #[test]
    fn empty_text_is_refused() {
        assert!(matches!(validate_text(""), Err(FrameworkError::TextEmpty)));
    }

    #[test]
    fn whitespace_only_text_is_refused() {
        for text in [" ", "\n", "\t", " \n\t "] {
            assert!(
                matches!(validate_text(text), Err(FrameworkError::TextEmpty)),
                "{text:?} is not a message"
            );
        }
        assert!(
            validate_text(" hi ").is_ok(),
            "whitespace around real text is real text"
        );
    }

    #[test]
    fn text_one_character_past_the_limit_is_refused() {
        let too_long = "x".repeat(TEXT_LIMIT + 1);

        let error = validate_text(&too_long).expect_err("one past the limit is past it");
        assert!(
            matches!(
                error,
                FrameworkError::TextTooLong {
                    chars,
                    limit: TEXT_LIMIT
                } if chars == TEXT_LIMIT + 1
            ),
            "got {error:?}"
        );
    }

    /// The limit is counted in characters, not bytes: 4096 four-byte emoji are
    /// twice as many bytes and still a message Telegram accepts.
    #[test]
    fn the_limit_counts_characters_not_bytes() {
        let emoji = "😀".repeat(TEXT_LIMIT);
        assert!(
            emoji.len() > TEXT_LIMIT,
            "the fixture has to be over the limit in bytes for this to mean anything"
        );

        assert!(
            validate_text(&emoji).is_ok(),
            "four bytes per character does not make it too long"
        );

        let one_too_many = "😀".repeat(TEXT_LIMIT + 1);
        assert!(matches!(
            validate_text(&one_too_many),
            Err(FrameworkError::TextTooLong { .. })
        ));
    }
}
