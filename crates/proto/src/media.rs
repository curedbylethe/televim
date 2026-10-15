//! A message's attachment, fetched or streamed.
//!
//! The framework does the work: it re-reads the message, finds the file the
//! media names, and transfers it. Nothing here builds a request or names a
//! `grammers` type — see `proto`'s module documentation for why that rule
//! exists, and why the wrapper belongs on the other side of it.
//!
//! # Why the identifier is narrowed here
//!
//! Telegram numbers messages with an `i32` and the workspace counts in `i64`,
//! so an identifier outside the wire's range cannot name a message. That is a
//! fact about the two number spaces meeting in this crate, which is why it is
//! [`ProtoError::MessageIdOutOfRange`] here and not something the framework
//! reports — and it is answered before the request, not after.

#[cfg(feature = "live")]
use crate::ProtoError;

/// Narrows a message identifier to the range Telegram numbers messages in.
///
/// Reported rather than converted: saturating would name the newest or oldest
/// message the account holds, and a download of the wrong message is worse than
/// no download at all.
#[cfg(feature = "live")]
fn narrow(chat_id: i64, message_id: i64) -> Result<i32, ProtoError> {
    let narrowed = i32::try_from(message_id);

    if narrowed.is_err() {
        tracing::warn!(
            chat_id,
            message_id,
            "a download was asked for a message identifier telegram could not have numbered"
        );
    }

    narrowed.map_err(|_| ProtoError::MessageIdOutOfRange {
        peer_id: chat_id,
        id: message_id,
    })
}

#[cfg(feature = "live")]
impl crate::ProtoClient {
    /// Streams the media a message carries to `sink`, one chunk at a time.
    ///
    /// `chat_id` is the bare identifier of the conversation, the same number the
    /// history fetch takes, and the message must be one that conversation's peer
    /// cache can address.
    ///
    /// `sink` is called with each chunk, the total bytes so far, and the size
    /// Telegram declared when it declared one; returning `false` stops the
    /// transfer. The bytes are never collected here, so a caller can stream an
    /// attachment far larger than the program's memory budget.
    ///
    /// # Errors
    ///
    /// - [`ProtoError::MessageIdOutOfRange`] — the identifier could not have
    ///   been numbered by Telegram, so it names no message.
    /// - [`ProtoError::Framework`] — the conversation is not in the peer cache,
    ///   the message carries nothing that can be fetched
    ///   (`FrameworkError::MediaUnavailable`), the sink stopped the transfer
    ///   (`FrameworkError::DownloadAborted`), or a request failed.
    pub async fn download_media_to(
        &self,
        chat_id: i64,
        message_id: i64,
        sink: impl FnMut(&[u8], usize, Option<usize>) -> bool,
    ) -> Result<(), ProtoError> {
        let target = narrow(chat_id, message_id)?;

        self.inner()
            .download_media_to(chat_id, i64::from(target), sink)
            .await?;

        tracing::debug!(chat_id, message_id, "streamed a message's media");

        Ok(())
    }

    /// Fetches the bytes of the media a message carries in one piece.
    ///
    /// For a caller with nowhere to stream to — a sticker decoded in memory once
    /// it is whole. The file path uses
    /// [`download_media_to`](Self::download_media_to), so a large attachment is
    /// never held whole.
    ///
    /// # Errors
    ///
    /// - [`ProtoError::MessageIdOutOfRange`] — the identifier could not have been
    ///   numbered by Telegram, so it names no message.
    /// - [`ProtoError::Framework`] — the conversation is not in the peer cache,
    ///   the message carries nothing that can be fetched
    ///   (`FrameworkError::MediaUnavailable`), or a request failed.
    pub async fn download_media(
        &self,
        chat_id: i64,
        message_id: i64,
    ) -> Result<Vec<u8>, ProtoError> {
        let target = narrow(chat_id, message_id)?;

        let bytes = self
            .inner()
            .download_media(chat_id, i64::from(target))
            .await?;

        tracing::debug!(
            chat_id,
            message_id,
            bytes = bytes.len(),
            "downloaded a message's media"
        );

        Ok(bytes)
    }
}

#[cfg(all(test, feature = "live"))]
mod live_tests {
    use super::*;

    /// The transfer itself needs a datacenter, so what runs on CI is the one
    /// decision this crate makes: an identifier outside the wire's range names
    /// no message, and is refused before a request rather than after one.
    #[test]
    fn an_identifier_outside_telegrams_range_names_no_message() {
        assert_eq!(narrow(42, 7).ok(), Some(7));
        assert_eq!(narrow(42, i64::from(i32::MAX)).ok(), Some(i32::MAX));

        assert!(
            matches!(
                narrow(42, i64::from(i32::MAX) + 1),
                Err(ProtoError::MessageIdOutOfRange { peer_id: 42, id })
                    if id == i64::from(i32::MAX) + 1
            ),
            "one past the wire's range cannot be narrowed, and both numbers travel \
             with the refusal — an identifier that cannot exist is only actionable \
             together with the conversation it was addressed to"
        );
    }
}
