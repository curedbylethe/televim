//! The client, wrapping `telegram_framework::Client`. No `grammers` types leak
//! out.

use domain::chat::Chat;
use telegram_framework::DialogInfo;

use crate::error::ProtoError;
use crate::stream::UpdateStream;
use crate::types::{ProtoChat, chat_kind};

/// The conversations televim displays, built from the dialogs Telegram returned.
///
/// Every dialog is filtered here before anything is built for it: only a
/// private user becomes a [`Chat`]. [`chat_kind`] is the same mapping
/// [`ProtoChat`] applies, so what the list keeps and the kind a chat claims
/// cannot drift apart.
fn private_chats(dialogs: Vec<DialogInfo>) -> Vec<Chat> {
    dialogs
        .into_iter()
        .filter(|dialog| chat_kind(dialog.kind).is_private())
        .map(|dialog| Chat::from(ProtoChat::from(dialog)))
        .collect()
}

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
///
/// The framework records which side of that line a client is on, so the two
/// wrappers cannot disagree about it and the warning needs no state of its own.
#[derive(Debug)]
pub struct ProtoClient {
    inner: telegram_framework::Client,
}

impl ProtoClient {
    /// Wraps an authenticated framework client.
    #[must_use]
    pub fn new(inner: telegram_framework::Client) -> Self {
        Self { inner }
    }

    /// Borrows the framework client, for the operations that reach Telegram.
    pub(crate) fn inner(&self) -> &telegram_framework::Client {
        &self.inner
    }

    /// Fetches the private conversations, newest first.
    ///
    /// Only people: groups, channels and bots are dropped, and dropped before
    /// anything is built for them. The framework classifies every dialog
    /// faithfully — Telegram is the only thing that knows a peer's kind — and
    /// the decision about what televim displays stays the domain's, applied
    /// here through [`ChatKind::is_private`](domain::chat::ChatKind::is_private)
    /// so that the fetch and the rule cannot drift apart.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Framework`] if Telegram rejects a request, if the
    /// connection fails, or if a page of the list cannot be decoded.
    pub async fn fetch_private_chats(&self) -> Result<Vec<Chat>, ProtoError> {
        let dialogs = self.inner.fetch_dialogs().await?;

        Ok(private_chats(dialogs))
    }

    /// Pins or unpins a chat on the account's own list.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Framework`] if the chat is not known to this
    /// session or Telegram refuses the change.
    pub async fn toggle_pin(&self, chat_id: i64, pinned: bool) -> Result<(), ProtoError> {
        self.inner.toggle_pin(chat_id, pinned).await?;

        Ok(())
    }

    /// Fetches the private conversations in the archive folder.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Framework`] when Telegram refuses or the answer
    /// cannot be decoded.
    pub async fn fetch_archived_chats(&self) -> Result<Vec<Chat>, ProtoError> {
        let dialogs = self.inner.fetch_archived_dialogs().await?;

        Ok(private_chats(dialogs))
    }

    /// Moves a chat into or out of the archive folder.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Framework`] when the chat is not known to this
    /// session or Telegram refuses the change.
    pub async fn set_archived(&self, chat_id: i64, archived: bool) -> Result<(), ProtoError> {
        self.inner.set_archived(chat_id, archived).await?;

        Ok(())
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
    pub async fn subscribe_updates(&self) -> Result<UpdateStream, ProtoError> {
        if !self.inner.has_fetched_dialogs() {
            tracing::warn!(
                "subscribing to updates before the chat list was fetched; messages \
                 that arrived while offline may be missing"
            );
        }

        Ok(UpdateStream::new(self.inner.subscribe_updates().await?))
    }

    /// Signs the account out and forgets the persisted session.
    ///
    /// Every other method here reads from a session or fills it in; this is the
    /// one that ends it. The stored session is cleared, so a next launch has
    /// nothing to restore and comes up with the sign-in surface rather than a
    /// client it believes in.
    ///
    /// The client is not spent by it. It stays usable and reads as
    /// unauthorised afterwards — `is_authorized` on the framework client reports
    /// `false` — so the sign-in flow can run again on this same client instead of
    /// the reader restarting the program.
    ///
    /// Only the local clear can fail. Asking Telegram to revoke the key is best
    /// effort — a reader who has asked to log out must end up logged out whether
    /// or not that request got through — so a failure there is logged by the
    /// framework rather than returned here, and what a caller shows before making
    /// this call, and what it does with the result, is still its own decision.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Framework`] if the persisted session could not be
    /// cleared.
    pub async fn logout(&self) -> Result<(), ProtoError> {
        self.inner.logout().await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::chat::ChatKind;
    use telegram_framework::DialogKind;

    /// A dialog with everything but the fields under test filled in.
    fn dialog(peer_id: i64, kind: DialogKind) -> DialogInfo {
        DialogInfo {
            peer_id,
            title: format!("dialog {peer_id}"),
            kind,
            deleted: false,
            unread_count: 0,
            last_message_id: None,
            last_timestamp: None,
            last_text: None,
            pinned: false,
            read_outbox_max_id: 0,
        }
    }

    /// Protects the history ADR's explicit-switch exception — the history cache
    /// tag still governs *content*, but a peer id may be carried across accounts
    /// (`docs/decisions.md`, "Why the history cache is tagged with the signed-in
    /// account and checked after `Ready`"). That is only safe while the chat
    /// list holds nobody but private users, because a Telegram user id is
    /// global: a group or channel id is not, so a carried one could name a
    /// different account's conversation. If this test fails, the exception is
    /// no longer sound.
    #[test]
    fn a_group_or_a_channel_never_becomes_a_chat() {
        let chats = private_chats(vec![
            dialog(10, DialogKind::PrivateUser),
            dialog(-20, DialogKind::Group),
            dialog(-30, DialogKind::Channel),
            dialog(40, DialogKind::Bot),
        ]);

        let ids: Vec<i64> = chats.iter().map(|chat| chat.id).collect();
        assert_eq!(ids, vec![10], "only the person survives the fetch");
        assert!(
            chats.iter().all(|chat| chat.kind.is_private()),
            "the list holds private users and nothing else"
        );

        // The DTO maps a peer's kind faithfully, so nothing downstream drops a
        // group: `private_chats` is the only thing keeping it out.
        let group: Chat = ProtoChat::from(dialog(-20, DialogKind::Group)).into();
        assert_eq!(group.kind, ChatKind::Group);
    }
}
