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
//! # A deletion of many messages is several requests
//!
//! Telegram's own limit for `messages.deleteMessages` is
//! [`DELETE_BATCH`] identifiers, and the pinned `grammers` does not chunk: its
//! `delete_messages` passes the whole vector, so a larger selection is rejected
//! outright rather than split. So [`Client::delete_messages`] splits it itself,
//! and waits between the requests it makes.
//!
//! The wait is not politeness. Telegram rate-limits bulk deletions, and a burst of
//! back-to-back batches is how a five-hundred-message selection becomes a
//! `FLOOD_WAIT` and a conversation that is half deleted. Whether the later batches
//! go through is Telegram's to decide; the pause is what keeps the question from
//! being asked too many times at once.
//!
//! A batch that fails after an earlier one has landed is
//! [`FrameworkError::PartialDelete`], which says how much landed. "Deleted 200 of
//! 250" and "failed" are different events and a reader can act on only one of
//! them.
//!
//! # Cancelling is out of scope
//!
//! A message that is still on its way cannot be cancelled here. Doing so needs
//! task-abort machinery, and the interface refuses to delete a message that has
//! not been acknowledged rather than pretending to.

use grammers_client::message::InputMessage;

use crate::client::Client;
use crate::error::{FrameworkError, RequestError};
use crate::media::classify_typed;
use crate::updates::{MessageInfo, message_info};

/// The most characters Telegram accepts in one message.
///
/// Telegram counts characters rather than bytes, so a limit of 4096 permits a
/// message of 4096 emoji even though that is twice as many bytes. Counting
/// [`str::chars`] — rather than [`str::len`] — is what keeps the two in step.
pub const TEXT_LIMIT: usize = 4096;

/// The most identifiers one deletion request may name.
///
/// Telegram's own limit for `messages.deleteMessages`. The pinned `grammers` does
/// not batch, so a larger selection would be refused outright rather than split —
/// which is why [`delete_batches`] exists.
pub const DELETE_BATCH: usize = 100;

/// How long to wait between two deletion requests.
///
/// A floor rather than a backoff: there is nothing to be polite *to* between
/// requests this client makes itself, and the only thing a pause buys is that
/// Telegram's rate limiter is not asked twice in the same instant. A second is
/// long enough for that and short enough that a reader deleting a few hundred
/// messages is not left watching a progress bar.
pub const DELETE_BATCH_PAUSE: std::time::Duration = std::time::Duration::from_secs(1);

/// The most identifiers one forwarding request may name.
///
/// Telegram's limit for `messages.forwardMessages`, which the pinned `grammers`
/// does not split either, so [`forward_batches`] does. Kept apart from
/// [`DELETE_BATCH`] so each limit is named where it applies.
pub const FORWARD_BATCH: usize = 100;

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

/// The batches `ids` is deleted in, oldest first.
///
/// Pure, and free of a client, so the two things that can be wrong here — how many
/// requests a selection becomes, and how they are sized — are checked on every CI
/// job rather than only against a datacenter. Empty in, empty out: a deletion of
/// nothing is no requests, not one empty one.
#[must_use]
pub fn delete_batches(ids: &[i32]) -> Vec<&[i32]> {
    ids.chunks(DELETE_BATCH).collect()
}

/// The batches `ids` is forwarded in, oldest first.
///
/// The same shape as [`delete_batches`], and for the same reasons: pure, so the
/// splitting is checked without a datacenter, and empty in means no requests out.
#[must_use]
pub fn forward_batches(ids: &[i32]) -> Vec<&[i32]> {
    ids.chunks(FORWARD_BATCH).collect()
}

/// What a failed deletion means, given how much had already landed.
///
/// The distinction is between "it did not work" and "most of it did", and a
/// caller that cannot tell them apart has nothing to tell the reader.
fn deletion_failed(deleted: usize, error: RequestError) -> FrameworkError {
    if deleted == 0 {
        return FrameworkError::Request(error);
    }

    FrameworkError::PartialDelete {
        deleted,
        source: Box::new(error),
    }
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

        // The text is validated as text above, so a message sent through here
        // carries no media — but it is read rather than assumed, so that the
        // description cannot drift from the message if that ever changes.
        Ok(message_info(
            message.id(),
            peer_id,
            message.text(),
            message.date().timestamp(),
            message.outgoing(),
            message.reply_to_message_id(),
            classify_typed(message.media().as_ref()),
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
    /// More identifiers than [`DELETE_BATCH`] is several requests, with a pause
    /// between them; see the module documentation for why both.
    ///
    /// Nothing is returned on success. `grammers` reports a `pts_count`, which
    /// counts updates rather than messages and would be a lie any caller read as
    /// "how many were deleted". A deletion arrives over the feed as
    /// [`UpdateKind::MessagesDeleted`](crate::UpdateKind::MessagesDeleted),
    /// which names every identifier irrespective of whether the request deleted
    /// anything for it.
    ///
    /// # Errors
    ///
    /// Returns [`FrameworkError::UnknownPeer`] when the conversation is not in
    /// the session's peer cache, and [`FrameworkError::Request`] when Telegram
    /// rejects the request or the connection fails. A batch that fails after an
    /// earlier one landed is [`FrameworkError::PartialDelete`], which says how
    /// much landed — the identifiers that did not go are in the request that
    /// failed, and retrying them is the caller's to arrange.
    pub async fn delete_messages(&self, peer_id: i64, ids: &[i32]) -> Result<(), FrameworkError> {
        let Some(peer) = self.peer_ref(peer_id) else {
            tracing::warn!(
                peer_id,
                "messages were deleted from a conversation that is not in the peer cache"
            );
            return Err(FrameworkError::UnknownPeer(peer_id));
        };

        let mut deleted = 0usize;

        for batch in delete_batches(ids) {
            // Between the batches, and never after the last one: the reader is
            // already waiting on that one.
            if deleted > 0 {
                tokio::time::sleep(DELETE_BATCH_PAUSE).await;
            }

            // The `usize` grammers returns is a count of updates, not of
            // deletions, and is deliberately dropped rather than surfaced as
            // either.
            if let Err(error) = self
                .inner()
                .delete_messages(peer, batch)
                .await
                .map_err(|error| RequestError::from_invocation(&error))
            {
                return Err(deletion_failed(deleted, error));
            }

            deleted += batch.len();
        }

        self.flush_session();

        tracing::debug!(peer_id, deleted, "deleted messages");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ids` from `0` up to but not including `up_to`, as Telegram would number
    /// them — the numbering does not matter, only how many there are.
    fn ids(up_to: usize) -> Vec<i32> {
        let mut all: Vec<i32> = Vec::with_capacity(up_to);
        for n in 0..up_to {
            all.push(i32::try_from(n).expect("a test's count fits telegram's range"));
        }
        all
    }

    // ---- batching --------------------------------------------------------

    /// Telegram caps one request at a hundred identifiers and `grammers` does not
    /// split them, so a longer selection is several requests or it is refused.
    #[test]
    fn a_selection_longer_than_one_request_is_split() {
        let all = ids(250);
        let batches = delete_batches(&all);

        assert_eq!(
            batches.iter().map(|batch| batch.len()).collect::<Vec<_>>(),
            vec![DELETE_BATCH, DELETE_BATCH, 50]
        );
    }

    /// Every identifier goes exactly once, in order, and none is dropped at a
    /// batch boundary — the failure a chunking bug produces is a silent omission.
    #[test]
    fn splitting_a_selection_keeps_every_identifier_in_order() {
        let all = ids(250);
        let batches = delete_batches(&all);

        assert_eq!(batches.concat(), all, "nothing dropped, nothing reordered");
    }

    #[test]
    fn a_selection_that_fits_in_one_request_is_not_split() {
        assert_eq!(delete_batches(&ids(1)).len(), 1);
        assert_eq!(delete_batches(&ids(DELETE_BATCH)).len(), 1);
        assert_eq!(delete_batches(&ids(DELETE_BATCH + 1)).len(), 2);
    }

    /// A deletion of nothing is no requests. An empty batch would be a request
    /// Telegram has no reason to answer.
    #[test]
    fn a_deletion_of_nothing_is_no_requests() {
        assert!(delete_batches(&[]).is_empty());
    }

    // ---- forwarding batches -------------------------------------------------

    #[test]
    fn a_forward_longer_than_one_request_is_split() {
        let all = ids(250);
        let batches = forward_batches(&all);

        assert_eq!(
            batches.iter().map(|batch| batch.len()).collect::<Vec<_>>(),
            vec![FORWARD_BATCH, FORWARD_BATCH, 50]
        );
    }

    #[test]
    fn splitting_a_forward_keeps_every_identifier_in_order() {
        let all = ids(250);
        let batches = forward_batches(&all);

        assert_eq!(batches.concat(), all, "nothing dropped, nothing reordered");
    }

    #[test]
    fn a_forward_that_fits_in_one_request_is_passed_through_whole() {
        let all = ids(FORWARD_BATCH);
        let batches = forward_batches(&all);

        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0], all.as_slice());
        assert_eq!(forward_batches(&ids(FORWARD_BATCH + 1)).len(), 2);
    }

    #[test]
    fn a_forward_of_nothing_is_no_requests() {
        assert!(forward_batches(&[]).is_empty());
    }

    // ---- what a failure means ---------------------------------------------

    #[test]
    fn a_failure_after_nothing_landed_is_an_ordinary_request_error() {
        let error = deletion_failed(0, RequestError::Network("reset".to_owned()));

        assert!(matches!(error, FrameworkError::Request(_)), "got {error:?}");
    }

    /// "Deleted 200 of 250" and "failed" are different events, and a reader can
    /// act on only one of them.
    #[test]
    fn a_failure_after_some_landed_says_how_many() {
        let error = deletion_failed(200, RequestError::Network("reset".to_owned()));

        assert!(
            error.to_string().contains("deleted 200"),
            "the wording says what happened: {error}"
        );

        let FrameworkError::PartialDelete { deleted, source } = error else {
            panic!("expected a partial deletion, got {error:?}");
        };
        assert_eq!(deleted, 200);
        assert!(
            matches!(*source, RequestError::Network(_)),
            "and the cause is kept"
        );
    }

    // ---- the text limit ----------------------------------------------------

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
