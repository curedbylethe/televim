//! The chat list, described in this crate's own vocabulary.
//!
//! `grammers` stops at this module's private functions. Every public item is
//! built from numbers and strings, so `proto` can turn a dialog into a `domain`
//! type without ever naming a `grammers` type — which is what `make boundary`
//! asserts for the whole crate.
//!
//! # Why the mapping is split in two
//!
//! A `grammers::Dialog` can only be built from a raw response and a peer map,
//! so nothing that consumes one can be tested without a datacenter. That makes
//! the handful of functions which read a `grammers` value — `dialog_to_info`,
//! `peer_kind` and `peer_title` — untestable here. They are therefore kept as
//! thin as possible, and every decision inside them is pulled out into a free
//! function over primitives: `classify_user`, `join_title`, `unread_count`,
//! `message_timestamp`, `last_text` and `newest_first`. Those are the parts that
//! can be wrong, and they run on every CI job.

use std::cmp::Ordering;

use grammers_client::peer::{Dialog, Peer};

use crate::client::Client;
use crate::error::{FrameworkError, RequestError};
use crate::tl;

/// What kind of peer a conversation is with.
///
/// This mirrors `domain::ChatKind` one for one, deliberately. Telegram is the
/// only thing that knows the answer, so the framework reports it faithfully and
/// the layer that knows what `televim` displays decides what to keep — rather
/// than collapsing the distinction here, where a bot would silently become a
/// group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    /// A one-to-one conversation with a person.
    PrivateUser,

    /// A one-to-one conversation with a bot account.
    Bot,

    /// A small group chat, or a megagroup.
    Group,

    /// A broadcast channel.
    Channel,
}

/// A conversation as this crate describes it.
///
/// The fields are primitives and strings so that a `grammers` type never
/// escapes the crate. `peer_id` is the peer's *bare* identifier — the same
/// number a message reports as its chat, so an entry here can be matched against
/// an event without a lookup table.
#[derive(Debug, Clone)]
pub struct DialogInfo {
    /// Bare identifier of the peer the conversation is with.
    pub peer_id: i64,

    /// Display name. Never empty: see `join_title`.
    pub title: String,

    /// What the peer is.
    pub kind: DialogKind,

    /// How many messages are unread.
    pub unread_count: u32,

    /// Identifier of the most recent message, if there is one.
    ///
    /// The timestamp and the text say what a conversation last showed; this
    /// says *which* message that was, which is what lets an edit be matched to
    /// the preview exactly rather than by guessing from the timestamp.
    pub last_message_id: Option<i64>,

    /// Unix timestamp in seconds of the most recent message, if there is one.
    pub last_timestamp: Option<i64>,

    /// Text of the most recent message, if it had any.
    pub last_text: Option<String>,

    /// Whether the account has pinned this conversation to the top of its list.
    pub pinned: bool,
}

impl Client {
    /// Fetches every conversation the account has, newest first.
    ///
    /// The list is deliberately unfiltered: groups, channels and bots are
    /// returned too, and the caller decides what to display. Classifying them is
    /// the framework's job because Telegram is the only source of the answer.
    ///
    /// # Ordering and the session
    ///
    /// Iterating dialogs is what makes Telegram disclose a peer's `access_hash`,
    /// and for a channel or a megagroup it also discloses the persistent
    /// timestamp the update stream needs to detect a gap. Both are session
    /// state, so the session is written back before this returns — and a caller
    /// that intends to subscribe to updates should fetch first, because the
    /// stream resolves what it missed while offline out of that same state.
    /// This is what marks the client as fetched; see
    /// [`Client::has_fetched_dialogs`].
    ///
    /// # Errors
    ///
    /// Returns [`FrameworkError::Request`] if Telegram rejects a request, if the
    /// connection fails, or if a page of the list cannot be decoded.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use telegram_framework::session::MemoryStore;
    /// use telegram_framework::ClientBuilder;
    ///
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = ClientBuilder::new(1234, "api-hash")
    ///     .session_store(Box::new(MemoryStore::new()))
    ///     .build()
    ///     .await?;
    ///
    /// for chat in client.fetch_dialogs().await? {
    ///     println!("{} ({:?})", chat.title, chat.kind);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn fetch_dialogs(&self) -> Result<Vec<DialogInfo>, FrameworkError> {
        let mut pages = self.inner().iter_dialogs();
        let mut conversations = Vec::new();
        let mut folders = 0usize;

        while let Some(dialog) = pages
            .next()
            .await
            .map_err(|error| RequestError::from_invocation(&error))
            .map_err(FrameworkError::from)?
        {
            match dialog_to_info(&dialog) {
                Some(info) => conversations.push(info),
                None => folders += 1,
            }
        }

        conversations.sort_by(pinned_then_newest);
        // Nothing else writes the peers and per-channel timestamps that dialog
        // iteration just learned, and the update stream reads them from the
        // store rather than from memory.
        //
        // The fetch is recorded on the client as well: a feed started before
        // this point has no peers to resolve a gap against, and the client is
        // where that question gets one answer for every caller.
        self.note_dialogs_fetched();
        self.flush_session();

        tracing::debug!(
            conversations = conversations.len(),
            folders,
            "fetched the dialog list"
        );

        Ok(conversations)
    }
}

/// Maps a dialog `grammers` built into this crate's own description.
///
/// `None` means the entry is a folder. Telegram returns folders next to
/// conversations and `grammers` models both as a `Dialog`, but a folder is a
/// navigation container: it has no peer to talk to and no single unread count —
/// Telegram splits one across its muted and unmuted halves — so it is not a
/// conversation and does not belong in a chat list. Matching on the raw variant
/// once, here, is what rules folders out and what makes the unread count below
/// total: there is no second place left for a folder to be handled wrongly.
fn dialog_to_info(dialog: &Dialog) -> Option<DialogInfo> {
    let tl::enums::Dialog::Dialog(raw) = &dialog.raw else {
        return None;
    };

    let peer = dialog.peer();
    // `grammers` reports no bare identifier for a peer that is the account
    // itself, and there is no number to substitute: the account's real user
    // identifier is only ever disclosed by asking Telegram for the account's own
    // user, which this crate does not do. A conversation is therefore skipped
    // rather than filed under an identifier that would address nothing. See
    // `every_real_user_keeps_its_identifier`, which is what makes this
    // unreachable for a real conversation.
    let peer_id = peer.id().bare_id()?;
    let last_message = dialog.last_message.as_ref();

    Some(DialogInfo {
        peer_id,
        title: peer_title(peer_id, peer),
        kind: peer_kind(peer),
        unread_count: unread_count(raw.unread_count),
        last_message_id: last_message.map(|message| i64::from(message.id())),
        last_timestamp: last_message
            .and_then(|message| message_timestamp(message.date().timestamp())),
        last_text: last_message.and_then(|message| last_text(message.text())),
        pinned: raw.pinned,
    })
}

/// Classifies a peer into the kinds a chat list distinguishes.
fn peer_kind(peer: &Peer) -> DialogKind {
    match peer {
        Peer::User(user) => classify_user(user.is_bot()),
        Peer::Group(_) => DialogKind::Group,
        Peer::Channel(_) => DialogKind::Channel,
    }
}

/// Classifies a user-shaped peer.
///
/// A bot *is* a user as far as Telegram's peer identifiers are concerned; only
/// the account flag tells the two apart. The distinction is worth keeping
/// because a chat list renders them differently, and because collapsing it here
/// would leave the caller unable to tell a bot from a person.
fn classify_user(is_bot: bool) -> DialogKind {
    if is_bot {
        DialogKind::Bot
    } else {
        DialogKind::PrivateUser
    }
}

/// Builds the name a chat list shows for a peer.
///
/// Telegram discloses a name in pieces and any of them can be missing: a deleted
/// account has neither a name nor a username, and a user who never set a
/// username has only the two halves of their name. `join_title` is what falls
/// back to the peer identifier, so that a conversation is never rendered as an
/// empty row.
fn peer_title(peer_id: i64, peer: &Peer) -> String {
    let (first, last) = match peer {
        Peer::User(user) => (user.first_name(), user.last_name()),
        // A group's and a channel's whole name is a single field, so it takes
        // the first half of the chain and the same username fallback.
        Peer::Group(_) | Peer::Channel(_) => (peer.name(), None),
    };

    join_title(first, last, peer.username(), peer_id)
}

/// Joins the pieces of a name, falling back to a username and then to a
/// placeholder built from the peer identifier.
///
/// Blank and whitespace-only pieces count as missing, because Telegram sends
/// them for accounts that have been emptied out. The placeholder is built here
/// rather than passed in so that the common case — a peer that has a name, and
/// so never reaches the fallback — does not allocate a string it would discard.
fn join_title(
    first: Option<&str>,
    last: Option<&str>,
    username: Option<&str>,
    peer_id: i64,
) -> String {
    let parts = [first, last]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|part| !part.is_empty());

    let mut name = String::new();
    for part in parts {
        if !name.is_empty() {
            name.push(' ');
        }
        name.push_str(part);
    }
    if !name.is_empty() {
        return name;
    }

    match username.map(str::trim).filter(|name| !name.is_empty()) {
        Some(username) => format!("@{username}"),
        None => format!("chat {peer_id}"),
    }
}

/// Normalises the unread count Telegram reports.
///
/// Telegram uses a negative count to mean "more unread than this client is
/// allowed to know", which is not something a badge can render.
fn unread_count(raw: i32) -> u32 {
    u32::try_from(raw).unwrap_or(0)
}

/// Normalises the timestamp `grammers` reports for a message.
///
/// An empty or service message carries no date, which `grammers` reports as
/// zero — indistinguishable from 1970 without this.
fn message_timestamp(raw: i64) -> Option<i64> {
    (raw > 0).then_some(raw)
}

/// Normalises the text of a message that may have none.
///
/// A photo, a sticker and a service message all carry no text, which `grammers`
/// reports as an empty string. That is the same thing a chat list shows for
/// "there is nothing to preview", so an empty text is not a preview.
fn last_text(text: &str) -> Option<String> {
    if text.is_empty() {
        None
    } else {
        Some(text.to_owned())
    }
}

/// Orders conversations the way a chat list shows them: newest first, with the
/// ones that have no messages at all last.
///
/// Comparing the two timestamps in reverse is enough. `Some` sorts above `None`
/// for the same type, and reversing turns "above" into "first", so the ordering
/// puts `None` at the end without needing a second rule for it.
fn newest_first(left: &DialogInfo, right: &DialogInfo) -> Ordering {
    right.last_timestamp.cmp(&left.last_timestamp)
}

/// Orders conversations with the pinned ones first, each section newest first.
///
/// `false < true`, so comparing the pins in reverse puts the pinned section
/// ahead; ties fall through to `newest_first` within each section.
fn pinned_then_newest(left: &DialogInfo, right: &DialogInfo) -> Ordering {
    right
        .pinned
        .cmp(&left.pinned)
        .then_with(|| newest_first(left, right))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dialog with everything but the fields under test filled in.
    fn dialog(peer_id: i64, last_timestamp: Option<i64>) -> DialogInfo {
        DialogInfo {
            peer_id,
            title: format!("chat {peer_id}"),
            kind: DialogKind::PrivateUser,
            unread_count: 0,
            last_message_id: None,
            last_timestamp,
            last_text: None,
            pinned: false,
        }
    }

    fn peer_ids(dialogs: &[DialogInfo]) -> Vec<i64> {
        dialogs.iter().map(|dialog| dialog.peer_id).collect()
    }

    #[test]
    fn a_bot_is_classified_apart_from_a_person() {
        assert_eq!(classify_user(false), DialogKind::PrivateUser);
        assert_eq!(
            classify_user(true),
            DialogKind::Bot,
            "a bot is a user to telegram; only the account flag tells them apart"
        );
    }

    #[test]
    fn a_title_prefers_the_full_name() {
        let cases = [
            (Some("Ada"), Some("Lovelace"), Some("ada"), "Ada Lovelace"),
            (Some("Ada"), None, Some("ada"), "Ada"),
            (Some("Ada"), Some(""), Some("ada"), "Ada"),
            (Some("Ada"), Some("   "), Some("ada"), "Ada"),
            (Some("  Ada  "), None, None, "Ada"),
            (Some("Ada"), Some("  Lovelace"), None, "Ada Lovelace"),
        ];

        for (first, last, username, expected) in cases {
            assert_eq!(
                join_title(first, last, username, 1),
                expected,
                "first={first:?} last={last:?} username={username:?}"
            );
        }
    }

    #[test]
    fn a_title_falls_back_to_the_username() {
        let cases = [(None, None), (Some(""), Some("")), (Some("  "), None)];

        for (first, last) in cases {
            assert_eq!(
                join_title(first, last, Some("ada"), 1),
                "@ada",
                "first={first:?} last={last:?}"
            );
        }
    }

    #[test]
    fn a_nameless_peer_gets_the_placeholder() {
        let cases = [
            (None, None, None),
            (Some(""), Some(""), Some("")),
            (Some("  "), Some(" "), Some("  ")),
        ];

        for (first, last, username) in cases {
            let title = join_title(first, last, username, 42);
            assert_eq!(
                title, "chat 42",
                "the fallback is built from the peer identifier, the one thing \
                 always present"
            );
            assert!(!title.is_empty(), "a row must never be nameless");
        }
    }

    #[test]
    fn an_unread_count_is_never_negative() {
        assert_eq!(unread_count(0), 0);
        assert_eq!(unread_count(7), 7);
        assert_eq!(
            unread_count(-1),
            0,
            "telegram marks a count it will not disclose with a negative value"
        );
        assert_eq!(unread_count(i32::MIN), 0, "and it must not wrap around");
    }

    #[test]
    fn a_message_without_a_date_has_no_timestamp() {
        assert_eq!(message_timestamp(0), None, "zero is grammers' 'no date'");
        assert_eq!(message_timestamp(-5), None);
        assert_eq!(message_timestamp(1_700_000_000), Some(1_700_000_000));
    }

    #[test]
    fn a_message_without_text_has_no_preview() {
        assert_eq!(last_text(""), None, "a photo and a sticker have no text");
        assert_eq!(last_text("hi"), Some("hi".to_owned()));
        assert_eq!(
            last_text(" "),
            Some(" ".to_owned()),
            "whitespace is a preview a user typed; only nothing at all is nothing"
        );
    }

    #[test]
    fn dialogs_are_ordered_newest_first_with_the_dateless_last() {
        let mut dialogs = vec![
            dialog(1, None),
            dialog(2, Some(100)),
            dialog(3, Some(300)),
            dialog(4, Some(200)),
            dialog(5, None),
        ];

        dialogs.sort_by(newest_first);

        assert_eq!(peer_ids(&dialogs), vec![3, 4, 2, 1, 5]);
    }

    #[test]
    fn dialogs_with_the_same_timestamp_keep_their_relative_order() {
        let mut dialogs = vec![dialog(1, Some(100)), dialog(2, Some(100))];

        dialogs.sort_by(newest_first);

        assert_eq!(peer_ids(&dialogs), vec![1, 2]);
    }

    #[test]
    fn pinned_dialogs_sort_above_newer_unpinned_ones_newest_first_within_each() {
        let mut pinned_old = dialog(1, Some(100));
        pinned_old.pinned = true;
        let mut pinned_new = dialog(2, Some(300));
        pinned_new.pinned = true;
        let mut dialogs = vec![
            dialog(3, Some(900)),
            pinned_old,
            dialog(4, Some(500)),
            pinned_new,
        ];

        dialogs.sort_by(pinned_then_newest);

        assert_eq!(peer_ids(&dialogs), vec![2, 1, 3, 4]);
    }
}
