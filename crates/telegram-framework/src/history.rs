//! One conversation's messages, described in this crate's own vocabulary.
//!
//! `grammers` stops at this module's private functions. [`MessageInfo`] is
//! already built from numbers and strings, so `proto` can turn a page of
//! history into `domain` types without ever naming a `grammers` type — which is
//! what `make boundary` asserts for the whole crate.
//!
//! # Why not `iter_messages`
//!
//! `grammers`' message iterator only walks backwards: it hard-codes
//! `add_offset = 0` and moves `offset_id` down as it pages. Scrolling *newer*,
//! and opening a conversation in the middle, both need a non-zero `add_offset`,
//! which the iterator never sends — so this module invokes `GetHistory` itself
//! and exposes the three arguments that express every direction.
//!
//! # Why the arguments are a value of their own
//!
//! `offset_id` and `add_offset` together say *which* messages to return, and
//! the two are easy to get wrong: the same `offset_id` means "older than this"
//! with `add_offset = 0` and "newer than this" with a negative one. Every case
//! televim needs is therefore named in [`HistoryArgs`], built over primitives
//! and tested here, and the async method below does nothing but send what it is
//! given.
//!
//! # Ordering
//!
//! Telegram returns a page **newest first**. That is left as it arrives: the
//! caller owns the window the page goes into, and reversing is its decision to
//! make once rather than the framework's on every call.

use crate::client::Client;
use crate::error::{FrameworkError, RequestError};
use crate::media::classify_raw;
use crate::tl;
use crate::updates::{MessageInfo, message_info};

/// The largest page Telegram will return from one history request.
///
/// Telegram rejects a larger `limit` outright, so a caller asking for more does
/// not get more — it gets an error. Clamping here is what lets a caller count in
/// whatever is convenient.
pub const HISTORY_LIMIT: i32 = 100;

/// Which messages a history request asks for.
///
/// The three fields are `GetHistory`'s own: `offset_id` anchors the page,
/// `add_offset` moves that anchor, and `limit` is how many to return. Their
/// meaning only exists together, so they are held in one value rather than
/// passed as three loose integers — see the module documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryArgs {
    /// The message the page is counted from. Zero means "the most recent".
    pub offset_id: i32,

    /// How far to move from `offset_id`. Negative counts *towards newer*.
    pub add_offset: i32,

    /// How many messages to return.
    pub limit: i32,
}

impl HistoryArgs {
    /// The newest `limit` messages.
    pub fn latest(limit: usize) -> Self {
        Self {
            offset_id: 0,
            add_offset: 0,
            limit: clamp_limit(limit),
        }
    }

    /// The `limit` messages older than `oldest_id`.
    ///
    /// This is how a conversation is scrolled upwards: the caller keeps the
    /// oldest identifier it holds and asks for the page in front of it.
    pub fn older(oldest_id: i32, limit: usize) -> Self {
        Self {
            offset_id: oldest_id,
            add_offset: 0,
            limit: clamp_limit(limit),
        }
    }

    /// The `limit` messages newer than `newest_id`.
    ///
    /// Moving past the anchor takes a negative `add_offset` as wide as the page,
    /// which is the one thing `grammers`' own iterator never sends.
    pub fn newer(newest_id: i32, limit: usize) -> Self {
        let limit = clamp_limit(limit);

        Self {
            offset_id: newest_id,
            // Negated after clamping, so the shift and the page size are the
            // same number: the page ends where the anchor is not.
            add_offset: -limit,
            limit,
        }
    }

    /// A page of `limit` messages centred on `target_id`.
    ///
    /// Half the page sits on each side of the anchor, which is what opening a
    /// conversation at a message the user jumped to needs.
    pub fn around(target_id: i32, limit: usize) -> Self {
        let limit = clamp_limit(limit);

        Self {
            offset_id: target_id,
            add_offset: -(limit / 2),
            limit,
        }
    }
}

/// Clamps a requested page size into what Telegram accepts.
///
/// Shared with the search module: a search page has the same wire bound as a
/// history page, and one clamp rule is one fewer place for the two to disagree.
pub(crate) fn clamp_limit(limit: usize) -> i32 {
    // Narrowed first, then clamped: a `usize` that does not fit the wire's
    // `i32` is already far past the largest page Telegram will return, so it
    // saturates instead of being converted.
    match i32::try_from(limit) {
        Ok(limit) => limit.clamp(1, HISTORY_LIMIT),
        Err(_) => HISTORY_LIMIT,
    }
}

impl Client {
    /// Fetches one page of a conversation's messages, newest first.
    ///
    /// `peer_id` is the conversation's *bare* identifier — the same number a
    /// [`DialogInfo`](crate::DialogInfo) reports and the same one a
    /// [`MessageInfo`] names as its chat — so a page can be matched against the
    /// chat list without a lookup table.
    ///
    /// # Peer cache
    ///
    /// Addressing a peer takes the `access_hash` Telegram handed out for it, and
    /// only [`Client::fetch_dialogs`] discloses those, so the chat list has to
    /// have been fetched: a conversation this client has never seen is reported
    /// as [`FrameworkError::UnknownPeer`] rather than sent as a request
    /// Telegram would reject.
    ///
    /// # Errors
    ///
    /// Returns [`FrameworkError::UnknownPeer`] when the conversation is not in
    /// the session's peer cache, and [`FrameworkError::Request`] when Telegram
    /// rejects the request, when the connection fails, or when the page cannot
    /// be decoded.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use telegram_framework::session::MemoryStore;
    /// use telegram_framework::{ClientBuilder, HistoryArgs};
    ///
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = ClientBuilder::new(1234, "api-hash")
    ///     .session_store(Box::new(MemoryStore::new()))
    ///     .build()
    ///     .await?;
    ///
    /// for dialog in client.fetch_dialogs().await? {
    ///     let page = client
    ///         .fetch_history(dialog.peer_id, HistoryArgs::latest(50))
    ///         .await?;
    ///     println!("{}: {} message(s)", dialog.title, page.len());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn fetch_history(
        &self,
        peer_id: i64,
        args: HistoryArgs,
    ) -> Result<Vec<MessageInfo>, FrameworkError> {
        let Some(peer) = self.peer_ref(peer_id) else {
            tracing::warn!(
                peer_id,
                "the history was requested for a conversation that is not in the peer cache"
            );
            return Err(FrameworkError::UnknownPeer(peer_id));
        };

        let request = tl::functions::messages::GetHistory {
            peer: peer.into(),
            offset_id: args.offset_id,
            offset_date: 0,
            add_offset: args.add_offset,
            limit: args.limit,
            max_id: 0,
            min_id: 0,
            hash: 0,
        };

        let response = self
            .inner()
            .invoke(&request)
            .await
            .map_err(|error| FrameworkError::from(RequestError::from_invocation(&error)))?;

        // The peers the page carries are not read. Every field a `MessageInfo` is
        // built from lives on the message itself, so the peers would only matter
        // for resolving who sent it — and the conversation this page belongs to
        // is one the chat list already vetted. See [`message_from_raw`] for why
        // the raw message is read directly.
        let (raw, _users, _chats) = match response {
            tl::enums::messages::Messages::Messages(page) => {
                (page.messages, page.users, page.chats)
            }
            tl::enums::messages::Messages::Slice(page) => (page.messages, page.users, page.chats),
            tl::enums::messages::Messages::ChannelMessages(page) => {
                (page.messages, page.users, page.chats)
            }
            // Only reachable if Telegram answers "nothing changed", which takes
            // a non-zero `hash` — this request always sends zero. The release
            // profile aborts on a panic, so an answer this build cannot read is
            // reported rather than crashing the process.
            tl::enums::messages::Messages::NotModified(_) => {
                return Err(FrameworkError::Request(RequestError::Deserialize(
                    "telegram answered a history request with NotModified".to_owned(),
                )));
            }
        };

        let messages: Vec<MessageInfo> = raw
            .iter()
            .map(|raw| message_from_raw(raw, peer_id))
            .collect();

        // Reading history can migrate the datacenter or cache a peer, and that
        // only reaches the store if it is written back here.
        self.flush_session();

        tracing::debug!(
            peer_id,
            offset_id = args.offset_id,
            add_offset = args.add_offset,
            limit = args.limit,
            returned = messages.len(),
            "fetched a page of history"
        );

        Ok(messages)
    }
}

/// Reads a raw history message into the description this crate publishes.
///
/// # Why the raw message is read here
///
/// `grammers`' `Message` is not used for a hand-made request. It is built
/// through `Message::from_raw`, which needs a `PeerMap`, and a `PeerMap` can
/// only be obtained from a `grammers` response — it has no public constructor,
/// and `Client` exposes no way to make one. This module builds its own
/// `GetHistory` request (see the module docs), so there is nothing to take one
/// from. This is the one place where a `grammers` type is read field by field
/// rather than through an accessor.
///
/// Nothing is lost by it, because every field read here is the same read
/// `grammers` makes, off the same raw value:
///
/// - an empty or service message carries no text and no media, and
/// - an empty message is not outgoing, while a service one is whatever
///   `out` says.
///
/// The media is read the same way, and the same is true of it: a message that
/// carries media says so through `media`, and [`classify_raw`] is what
/// `grammers`' own accessor reduces that field to. Reading the raw value is
/// what lets a kind this build does not model still arrive as a kind rather
/// than as nothing — `grammers` drops some variants before anything can look
/// at them.
///
/// A message read out of a conversation and the same message arriving over the
/// feed are therefore still described identically, which is what lets the two
/// be deduplicated against each other.
///
/// The date is widened straight from the raw `i32`. `grammers` would route it
/// through a `DateTime` and take the timestamp back out, which is the same
/// number for every value that conversion accepts — and does not abort on a
/// value it does not.
fn message_from_raw(raw: &tl::enums::Message, chat_peer_id: i64) -> MessageInfo {
    let (id, text, date, is_outgoing, reply_to_msg_id, media) = match raw {
        tl::enums::Message::Empty(message) => (message.id, "", 0, false, None, None),
        tl::enums::Message::Message(message) => (
            message.id,
            message.message.as_str(),
            message.date,
            message.out,
            match &message.reply_to {
                Some(tl::enums::MessageReplyHeader::Header(header)) => header.reply_to_msg_id,
                _ => None,
            },
            classify_raw(message.media.as_ref()),
        ),
        tl::enums::Message::Service(message) => {
            (message.id, "", message.date, message.out, None, None)
        }
    };

    message_info(
        id,
        chat_peer_id,
        text,
        i64::from(date),
        is_outgoing,
        reply_to_msg_id,
        media,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::MediaKind;
    use crate::testing::{media_empty, media_photo, media_unmodelled, raw_message};

    #[test]
    fn the_newest_page_starts_from_nowhere_in_particular() {
        let args = HistoryArgs::latest(50);

        assert_eq!(args.offset_id, 0, "zero means 'the most recent message'");
        assert_eq!(args.add_offset, 0);
        assert_eq!(args.limit, 50);
    }

    #[test]
    fn an_older_page_is_anchored_and_does_not_move() {
        let args = HistoryArgs::older(4_096, 25);

        assert_eq!(args.offset_id, 4_096);
        assert_eq!(
            args.add_offset, 0,
            "an anchor with no shift returns what is in front of it"
        );
        assert_eq!(args.limit, 25);
    }

    /// The case `grammers`' own iterator cannot express, and the reason this
    /// module invokes `GetHistory` itself.
    #[test]
    fn a_newer_page_shifts_past_its_anchor_by_the_page_size() {
        let args = HistoryArgs::newer(4_096, 25);

        assert_eq!(args.offset_id, 4_096);
        assert_eq!(
            args.add_offset, -25,
            "the shift has to be negative to count towards newer messages"
        );
        assert_eq!(args.limit, 25);
    }

    #[test]
    fn a_page_around_a_message_is_centred_on_it() {
        let args = HistoryArgs::around(4_096, 50);

        assert_eq!(args.offset_id, 4_096);
        assert_eq!(args.add_offset, -25, "half the page sits on each side");
        assert_eq!(args.limit, 50);

        // An odd page still centres: the remainder goes to the newer side,
        // which is the side a reader jumping to a message has just come from.
        let odd = HistoryArgs::around(4_096, 51);
        assert_eq!(odd.add_offset, -25);
        assert_eq!(odd.limit, 51);
    }

    #[test]
    fn a_page_size_is_clamped_into_what_telegram_accepts() {
        assert_eq!(
            HistoryArgs::latest(0).limit,
            1,
            "an empty page is a request telegram would reject"
        );
        let limit = usize::try_from(HISTORY_LIMIT).expect("the limit is positive");
        assert_eq!(HistoryArgs::latest(limit).limit, HISTORY_LIMIT);
        assert_eq!(
            HistoryArgs::latest(limit + 400).limit,
            HISTORY_LIMIT,
            "asking for more does not return more"
        );
        assert_eq!(HistoryArgs::latest(usize::MAX).limit, HISTORY_LIMIT);
    }

    /// The shift and the page size have to agree, or a "newer" page would skip
    /// messages or repeat the anchor.
    #[test]
    fn a_newer_page_shifts_by_its_clamped_size_not_its_requested_one() {
        let args = HistoryArgs::newer(4_096, usize::MAX);

        assert_eq!(args.limit, HISTORY_LIMIT);
        assert_eq!(args.add_offset, -HISTORY_LIMIT);
    }

    /// The raw path is the only one of the two that sees a media kind
    /// `grammers` would not build, so the description it produces is the one
    /// that must not lose it.
    #[test]
    fn a_raw_message_reports_the_media_it_carries() {
        let photo = message_from_raw(&raw_message(Some(media_photo())), 42);

        assert_eq!(
            photo.media,
            Some(MediaKind::Photo),
            "the message carries a photo, so it is described as one"
        );
        assert_eq!(photo.chat_peer_id, 42);
        assert_eq!(photo.id, 1);

        let unknown = message_from_raw(&raw_message(Some(media_unmodelled())), 42);

        assert_eq!(
            unknown.media,
            Some(MediaKind::File),
            "a kind this build does not model is still something the message carries"
        );

        let bare = message_from_raw(&raw_message(Some(media_empty())), 42);

        assert_eq!(
            bare.media, None,
            "and a message that carries nothing is the only one that says so"
        );
    }
}
