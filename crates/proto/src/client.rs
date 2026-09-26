//! The client, wrapping `telegram_framework::Client`. No `grammers` types leak
//! out.

use std::sync::atomic::{AtomicBool, Ordering};

use domain::chat::Chat;

use crate::error::ProtoError;
use crate::stream::UpdateStream;
use crate::types::ProtoChat;

/// A Telegram client, as the rest of the workspace sees one.
///
/// Every operation returns `domain` types or this crate's own error, so a
/// caller needs neither a `grammers` dependency nor any knowledge of
/// Telegram's peer taxonomy. It is built from a framework client that has
/// already logged in; the login flow belongs to the framework and is not
/// repeated here.
///
/// # Fetch before subscribing
///
/// [`ProtoClient::fetch_private_chats`] should be called before
/// [`ProtoClient::subscribe_updates`]. Resolving what arrived while the client
/// was offline reads a peer's `access_hash` back out of the session, and
/// iterating the chat list is what puts it there. Subscribing first is not an
/// error — a client resumed from a warm session store is legitimately already
/// in that state — but it is logged, because on a client that has just logged
/// in it means the gap is resolved against an empty cache.
#[derive(Debug)]
pub struct ProtoClient {
    inner: telegram_framework::Client,
    warm: Warmup,
}

impl ProtoClient {
    /// Wraps an authenticated framework client.
    #[must_use]
    pub fn new(inner: telegram_framework::Client) -> Self {
        Self {
            inner,
            warm: Warmup::default(),
        }
    }

    /// Fetches the private conversations, newest first.
    ///
    /// Only people: groups, channels and bots are fetched and then dropped. The
    /// framework deliberately does not filter them, and this does not either —
    /// it hands the whole list to [`domain::chat::filter_private`], so the
    /// question of what televim displays has one answer in one place.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Framework`] if Telegram rejects a request, if the
    /// connection fails, or if a page of the list cannot be decoded.
    pub async fn fetch_private_chats(&self) -> Result<Vec<Chat>, ProtoError> {
        let dialogs = self.inner.fetch_dialogs().await?;

        // Only a fetch that got all the way through warms the session, which is
        // why this is recorded after the await and not before it.
        self.warm.note_fetched();

        Ok(domain::chat::filter_private(
            dialogs
                .into_iter()
                .map(|dialog| Chat::from(ProtoChat::from(dialog))),
        ))
    }

    /// Subscribes to the updates Telegram sends for this account.
    ///
    /// The feed is already narrowed to private conversations, so everything it
    /// yields belongs to a chat [`ProtoClient::fetch_private_chats`] could have
    /// returned. An account has one ordered update sequence, so there is one
    /// feed: this can be called once, and a second call fails.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Framework`] if the feed has already been taken.
    pub fn subscribe_updates(&self) -> Result<UpdateStream, ProtoError> {
        if !self.warm.is_warm() {
            tracing::warn!(
                "subscribing to updates before the chat list was fetched; messages \
                 that arrived while offline may be missing"
            );
        }

        Ok(UpdateStream::new(self.inner.subscribe_updates()?))
    }
}

/// Whether the chat list has been fetched since this client was built.
///
/// The framework resolves a gap in the feed out of the session, and iterating
/// the chat list is what writes the peer access hashes that resolution reads.
/// A client resumed from a warm session store is already in that state; one
/// that has just logged in is not. This tracks which of the two this client is,
/// so that subscribing in the wrong order is visible in the log rather than
/// silently losing events.
///
/// A latch rather than a lock: it carries no data, and there is nothing that
/// could poison it. Relaxed ordering is enough for the same reason — the flag
/// only ever goes one way, and the fetch that sets it is the only thing that
/// can. A read that misses the store prints a warning that need not have been
/// printed; it cannot suppress one that should have been.
#[derive(Debug, Default)]
struct Warmup(AtomicBool);

impl Warmup {
    /// Records that the chat list was fetched.
    fn note_fetched(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether the chat list has been fetched.
    fn is_warm(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_is_cold_until_the_chat_list_is_fetched() {
        let warm = Warmup::default();

        assert!(
            !warm.is_warm(),
            "nothing has been fetched, so the session holds no peers"
        );

        warm.note_fetched();

        assert!(warm.is_warm());
    }

    #[test]
    fn fetching_again_leaves_a_client_warm() {
        let warm = Warmup::default();

        warm.note_fetched();
        warm.note_fetched();

        assert!(
            warm.is_warm(),
            "the flag is a latch; a second fetch has nothing left to change"
        );
    }
}
