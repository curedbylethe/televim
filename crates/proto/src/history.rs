//! One conversation's history, as `domain` types.
//!
//! The framework hands over pages of message descriptions it has already
//! narrowed to primitives and strings. All that is left here is to put each one
//! into the [`Message`] the domain applies — the same seam `stream` uses — and
//! to remember where the loaded part of a conversation ends, which is what lets
//! the next page be asked for.
//!
//! # Which conversation, and whose job it is to check
//!
//! A page is fetched for one peer at a time, so there is no chat list to filter
//! and nothing here re-applies
//! [`ChatKind::is_private`](domain::chat::ChatKind::is_private): the caller
//! names a conversation it already holds. The only conversations worth opening
//! are the ones `ProtoClient::fetch_private_chats` returned, and the application
//! only ever opens those — so a second check here would be a check on the
//! caller's own output.
//!
//! # Ordering
//!
//! Telegram answers newest first, and a window is read oldest first. The turn
//! happens once, here, so that every caller gets a page it can prepend or append
//! without sorting.

#[cfg(feature = "live")]
use telegram_framework::{HistoryArgs, MessageInfo};

use domain::message::Message;

/// Where the loaded part of a conversation ends.
///
/// The cursor is deliberately small — a peer identifier and four facts about the
/// page that was last loaded — so that a fetch can be handed a copy of it and
/// hand back an updated one. It is also deliberately pure: everything that
/// decides what the next request asks for is a calculation over those facts, and
/// it runs on every CI job rather than only against a datacenter.
///
/// `fetch_older` and `fetch_newer` keep it up to date themselves. A page that
/// *replaces* the window — the newest one, or a page around a message the reader
/// jumped to — is the caller's to report, with [`HistoryCursor::reset_to`],
/// because only the caller knows it replaced anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryCursor {
    /// Bare identifier of the conversation this cursor belongs to.
    peer_id: i64,

    /// Identifier of the oldest message that has been loaded.
    oldest_loaded_id: Option<i64>,

    /// Identifier of the newest message that has been loaded.
    newest_loaded_id: Option<i64>,

    /// Whether the conversation has no more messages in front of the oldest.
    exhausted_older: bool,

    /// Whether the conversation has no more messages behind the newest.
    exhausted_newer: bool,
}

impl HistoryCursor {
    /// A cursor for a conversation nothing has been loaded from yet.
    ///
    /// Both directions are open, because nothing has ruled either out, and both
    /// bounds are absent, which is what keeps a fetch from asking for a page
    /// relative to a message this client has never seen.
    #[must_use]
    pub fn new(peer_id: i64) -> Self {
        Self {
            peer_id,
            oldest_loaded_id: None,
            newest_loaded_id: None,
            exhausted_older: false,
            exhausted_newer: false,
        }
    }

    /// The conversation this cursor belongs to.
    #[must_use]
    pub fn peer_id(&self) -> i64 {
        self.peer_id
    }

    /// Identifier of the oldest loaded message, if any has been loaded.
    #[must_use]
    pub fn oldest_loaded_id(&self) -> Option<i64> {
        self.oldest_loaded_id
    }

    /// Identifier of the newest loaded message, if any has been loaded.
    #[must_use]
    pub fn newest_loaded_id(&self) -> Option<i64> {
        self.newest_loaded_id
    }

    /// Whether there is nothing older left to fetch.
    #[must_use]
    pub fn exhausted_older(&self) -> bool {
        self.exhausted_older
    }

    /// Whether there is nothing newer left to fetch.
    #[must_use]
    pub fn exhausted_newer(&self) -> bool {
        self.exhausted_newer
    }

    /// Records that the window was replaced by `messages`.
    ///
    /// Both directions open again, whatever the page before it said: a window
    /// that jumped somewhere else is surrounded by the unknown on both sides, so
    /// the only thing that survived the jump is where it landed.
    pub fn reset_to(&mut self, messages: &[Message]) {
        self.oldest_loaded_id = messages.iter().map(|message| message.id).min();
        self.newest_loaded_id = messages.iter().map(|message| message.id).max();
        self.exhausted_older = false;
        self.exhausted_newer = false;
    }

    /// Records a page that was prepended, and whether it was short.
    ///
    /// A page shorter than the one asked for means the conversation ran out, so
    /// the question is settled and no round trip is spent asking again.
    ///
    /// Only the fetches move a cursor, so this exists whenever they do — and
    /// whenever the tests that pin the rule do.
    #[cfg(any(feature = "live", test))]
    fn note_older(&mut self, page: &[Message], requested: usize) {
        if let Some(oldest) = page.iter().map(|message| message.id).min() {
            self.oldest_loaded_id = Some(oldest);
        }
        self.exhausted_older = page.len() < requested;
    }

    /// Records a page that was appended, and whether it was short.
    #[cfg(any(feature = "live", test))]
    fn note_newer(&mut self, page: &[Message], requested: usize) {
        if let Some(newest) = page.iter().map(|message| message.id).max() {
            self.newest_loaded_id = Some(newest);
        }
        self.exhausted_newer = page.len() < requested;
    }
}

/// Turns a page the framework described into the domain's messages.
///
/// The trip goes through `ProtoMessage` rather than straight across, and it has
/// to: neither crate can implement the conversion on its own. The middleware is
/// the one place the two can meet, which is why it is the seam the rest of this
/// crate uses too.
#[cfg(feature = "live")]
fn to_messages(peer_id: i64, page: Vec<MessageInfo>) -> Vec<Message> {
    let mut messages: Vec<Message> = page
        .into_iter()
        .map(|info| Message::from(crate::types::ProtoMessage::from(info)))
        // A page is fetched for one conversation, so a message naming another
        // cannot belong to it. Cheap, and it keeps a mixed page from being
        // spliced into one window.
        .filter(|message| message.chat_id == peer_id)
        .collect();

    // Telegram answers newest first; a window is read oldest first.
    messages.reverse();
    messages
}

/// How many messages `args` will actually ask Telegram for.
///
/// The framework clamps the page size, so "was the page short?" can only be
/// answered against the number that was really sent. Reading the clamped value
/// back off the arguments — rather than repeating the bound here — is what keeps
/// the two in step.
#[cfg(feature = "live")]
fn requested(args: HistoryArgs) -> usize {
    // Clamped into `1..=HISTORY_LIMIT` on the way in, so the saturated value is
    // unreachable rather than merely unlikely.
    usize::try_from(args.limit).unwrap_or(usize::MAX)
}

/// Narrows a message identifier to the range Telegram numbers messages with.
///
/// A message identifier is an `i32` on the wire, so a value outside that range
/// cannot have come from Telegram and there is no page to ask for. Reported
/// rather than converted: saturating would turn it into a request for a message
/// that does not exist, which is a worse answer than no answer at all.
#[cfg(feature = "live")]
fn narrow(id: i64, peer_id: i64) -> Option<i32> {
    let narrowed = i32::try_from(id).ok();

    if narrowed.is_none() {
        tracing::warn!(
            peer_id,
            message_id = id,
            "a message identifier outside telegram's range cannot be paged from"
        );
    }

    narrowed
}

/// The history operations, which need the framework's client.
///
/// Every one of them returns the domain's messages or this crate's error, so a
/// caller needs no knowledge of Telegram's paging arguments. A flood wait
/// arrives as a [`RequestError::Rpc`](telegram_framework::RequestError::Rpc)
/// carrying the delay in its `value`; backing off is the caller's job, because
/// only the caller knows what it was scrolling towards.
#[cfg(feature = "live")]
impl crate::ProtoClient {
    /// Fetches the newest `limit` messages of a conversation, oldest first.
    ///
    /// This is what opening a conversation asks for. The result replaces the
    /// window, so the caller's cursor has to be told —
    /// [`HistoryCursor::reset_to`] — or the next page will be asked for relative
    /// to a message that is no longer on screen.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Framework`](crate::ProtoError::Framework) if the
    /// conversation is not in the session's peer cache, if Telegram rejects the
    /// request, or if the page cannot be decoded.
    pub async fn fetch_latest(
        &self,
        peer_id: i64,
        limit: usize,
    ) -> Result<Vec<Message>, crate::ProtoError> {
        let args = HistoryArgs::latest(limit);
        let page = self.inner().fetch_history(peer_id, args).await?;

        tracing::debug!(
            peer_id,
            returned = page.len(),
            "fetched the newest page of a conversation"
        );

        Ok(to_messages(peer_id, page))
    }

    /// Fetches the `limit` messages in front of the oldest one loaded.
    ///
    /// Short-circuits to an empty page when the cursor already knows there is
    /// nothing older, so scrolling at the top of a conversation costs no round
    /// trip — and when nothing has been loaded at all, because a page can only
    /// be counted from a message this client holds.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Framework`](crate::ProtoError::Framework) on the
    /// same failures as `fetch_latest`.
    pub async fn fetch_older(
        &self,
        cursor: &mut HistoryCursor,
        limit: usize,
    ) -> Result<Vec<Message>, crate::ProtoError> {
        if cursor.exhausted_older {
            return Ok(Vec::new());
        }

        let Some(oldest) = cursor.oldest_loaded_id else {
            tracing::debug!(
                peer_id = cursor.peer_id(),
                "older history was asked for before anything was loaded"
            );
            return Ok(Vec::new());
        };
        let Some(oldest) = narrow(oldest, cursor.peer_id()) else {
            return Ok(Vec::new());
        };

        let args = HistoryArgs::older(oldest, limit);
        let page = self.inner().fetch_history(cursor.peer_id(), args).await?;
        let messages = to_messages(cursor.peer_id(), page);
        cursor.note_older(&messages, requested(args));

        tracing::debug!(
            peer_id = cursor.peer_id(),
            returned = messages.len(),
            exhausted = cursor.exhausted_older(),
            "fetched an older page of a conversation"
        );

        Ok(messages)
    }

    /// Fetches the `limit` messages behind the newest one loaded.
    ///
    /// This is how a conversation catches up after the reader has scrolled away
    /// from the bottom: the same short-circuit as `fetch_older`, in the other
    /// direction.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Framework`](crate::ProtoError::Framework) on the
    /// same failures as `fetch_latest`.
    pub async fn fetch_newer(
        &self,
        cursor: &mut HistoryCursor,
        limit: usize,
    ) -> Result<Vec<Message>, crate::ProtoError> {
        if cursor.exhausted_newer {
            return Ok(Vec::new());
        }

        let Some(newest) = cursor.newest_loaded_id else {
            tracing::debug!(
                peer_id = cursor.peer_id(),
                "newer history was asked for before anything was loaded"
            );
            return Ok(Vec::new());
        };
        let Some(newest) = narrow(newest, cursor.peer_id()) else {
            return Ok(Vec::new());
        };

        let args = HistoryArgs::newer(newest, limit);
        let page = self.inner().fetch_history(cursor.peer_id(), args).await?;
        let messages = to_messages(cursor.peer_id(), page);
        cursor.note_newer(&messages, requested(args));

        tracing::debug!(
            peer_id = cursor.peer_id(),
            returned = messages.len(),
            exhausted = cursor.exhausted_newer(),
            "fetched a newer page of a conversation"
        );

        Ok(messages)
    }

    /// Fetches a page of `limit` messages centred on `message_id`.
    ///
    /// This is what jumping to a message asks for — the first unread one, say —
    /// where neither "older" nor "newer" from the bottom of the conversation
    /// would land on it. Like `fetch_latest`, the result replaces the window, so
    /// the caller's cursor has to be told.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Framework`](crate::ProtoError::Framework) on the
    /// same failures as `fetch_latest`.
    pub async fn fetch_around(
        &self,
        peer_id: i64,
        message_id: i64,
        limit: usize,
    ) -> Result<Vec<Message>, crate::ProtoError> {
        let Some(target) = narrow(message_id, peer_id) else {
            return Ok(Vec::new());
        };

        let args = HistoryArgs::around(target, limit);
        let page = self.inner().fetch_history(peer_id, args).await?;

        tracing::debug!(
            peer_id,
            message_id,
            returned = page.len(),
            "fetched a page of a conversation around a message"
        );

        Ok(to_messages(peer_id, page))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A message with nothing but its identifier filled in.
    fn message(id: i64) -> Message {
        Message {
            id,
            chat_id: 42,
            text: std::borrow::Cow::Borrowed("text"),
            timestamp: 1_700_000_000,
            status: domain::message::MessageStatus::Received,
            is_outgoing: false,
            reply_to: None,
            media: None,
        }
    }

    fn page(ids: &[i64]) -> Vec<Message> {
        ids.iter().copied().map(message).collect()
    }

    #[test]
    fn a_fresh_cursor_has_nothing_to_count_from() {
        let cursor = HistoryCursor::new(42);

        assert_eq!(cursor.peer_id(), 42);
        assert_eq!(cursor.oldest_loaded_id(), None);
        assert_eq!(cursor.newest_loaded_id(), None);
        assert!(
            !cursor.exhausted_older() && !cursor.exhausted_newer(),
            "nothing has ruled either direction out yet"
        );
    }

    #[test]
    fn a_replaced_window_opens_both_directions_again() {
        let mut cursor = HistoryCursor::new(42);
        cursor.note_newer(&page(&[13, 14]), 3);
        assert!(
            cursor.exhausted_newer(),
            "two of three asked for settles the direction"
        );

        cursor.reset_to(&page(&[10, 11, 12]));

        assert_eq!(cursor.oldest_loaded_id(), Some(10));
        assert_eq!(cursor.newest_loaded_id(), Some(12));
        assert!(
            !cursor.exhausted_older() && !cursor.exhausted_newer(),
            "a window that jumped somewhere else is surrounded by the unknown"
        );
    }

    #[test]
    fn an_empty_replacement_leaves_nothing_to_count_from() {
        let mut cursor = HistoryCursor::new(42);
        cursor.reset_to(&page(&[1, 2, 3]));

        cursor.reset_to(&[]);

        assert_eq!(cursor.oldest_loaded_id(), None);
        assert_eq!(cursor.newest_loaded_id(), None);
    }

    /// The short page is what settles a direction: without it, scrolling at the
    /// end of a conversation would keep asking Telegram for a page it has
    /// already said does not exist.
    #[test]
    fn a_short_page_settles_the_direction_it_was_asked_for() {
        let mut cursor = HistoryCursor::new(42);
        cursor.reset_to(&page(&[10, 11, 12]));

        cursor.note_older(&page(&[7, 8, 9]), 3);
        assert!(
            !cursor.exhausted_older(),
            "a full page means there may be more"
        );
        assert_eq!(cursor.oldest_loaded_id(), Some(7));

        cursor.note_older(&page(&[4, 5]), 3);
        assert!(
            cursor.exhausted_older(),
            "two of three asked for means the end"
        );
        assert_eq!(cursor.oldest_loaded_id(), Some(4));
        assert_eq!(
            cursor.newest_loaded_id(),
            Some(12),
            "paging backwards must not move the newest bound"
        );
    }

    #[test]
    fn a_newer_page_moves_only_the_newest_bound() {
        let mut cursor = HistoryCursor::new(42);
        cursor.reset_to(&page(&[10, 11, 12]));

        cursor.note_newer(&page(&[13, 14]), 2);

        assert_eq!(cursor.newest_loaded_id(), Some(14));
        assert_eq!(cursor.oldest_loaded_id(), Some(10));
        assert!(!cursor.exhausted_newer());
    }

    /// An empty page is the clearest "the conversation ran out" there is.
    #[test]
    fn an_empty_page_settles_its_direction() {
        let mut cursor = HistoryCursor::new(42);
        cursor.reset_to(&page(&[10, 11, 12]));

        cursor.note_older(&[], 3);
        assert!(cursor.exhausted_older());
        assert_eq!(
            cursor.oldest_loaded_id(),
            Some(10),
            "the bound stays where it was; nothing older arrived"
        );

        cursor.note_newer(&[], 3);
        assert!(cursor.exhausted_newer());
    }

    /// A page that arrives out of order must not walk a bound backwards: the
    /// cursor records what has been loaded, not what arrived last.
    #[test]
    fn a_page_out_of_order_does_not_walk_a_bound_backwards() {
        let mut cursor = HistoryCursor::new(42);
        cursor.reset_to(&page(&[10, 11, 12]));

        cursor.note_older(&page(&[9, 8, 7]), 3);
        assert_eq!(
            cursor.oldest_loaded_id(),
            Some(7),
            "the oldest of the page, not its first element"
        );

        cursor.note_newer(&page(&[14, 13]), 2);
        assert_eq!(cursor.newest_loaded_id(), Some(14));
    }

    /// The page-keeping rule, on the shape the fetches use it in: extend in one
    /// direction, then jump, then extend again.
    #[test]
    fn a_cursor_survives_a_scroll_and_a_jump() {
        let mut cursor = HistoryCursor::new(42);
        cursor.reset_to(&page(&[10, 11, 12]));

        cursor.note_older(&page(&[7, 8, 9]), 3);
        cursor.note_older(&page(&[4, 5, 6]), 3);
        assert_eq!(cursor.oldest_loaded_id(), Some(4));

        cursor.reset_to(&page(&[100, 101]));
        cursor.note_newer(&page(&[102, 103]), 2);

        assert_eq!(cursor.oldest_loaded_id(), Some(100));
        assert_eq!(cursor.newest_loaded_id(), Some(103));
    }

    /// A fetch is handed a copy and hands back an updated one, so the two have
    /// to be able to diverge without either of them being shared.
    #[test]
    fn a_cursor_is_a_value_a_fetch_can_own() {
        let mut cursor = HistoryCursor::new(42);
        cursor.reset_to(&page(&[10, 11, 12]));

        let copy = cursor;
        assert_eq!(copy, cursor, "a copy carries the same question to answer");

        cursor.note_older(&page(&[7, 8, 9]), 3);
        assert_ne!(
            copy, cursor,
            "and the two diverge once one of them is answered"
        );
    }
}

/// The translation from the framework's descriptions.
///
/// Gated with the type it converts from. The cursor's rules need neither a
/// feature nor a datacenter and run on every job; these run whenever the client
/// does.
#[cfg(all(test, feature = "live"))]
mod live_tests {
    use super::*;
    use domain::message::MessageStatus;
    use telegram_framework::MessageInfo;

    /// A message as the framework describes one.
    fn info(id: i64, chat_peer_id: i64, is_outgoing: bool) -> MessageInfo {
        MessageInfo {
            id,
            chat_peer_id,
            text: format!("message {id}"),
            timestamp: 1_700_000_000 + id,
            is_outgoing,
            reply_to_msg_id: None,
            media: None,
        }
    }

    /// Telegram answers newest first; a page has to come out oldest first, or a
    /// window would be filled back to front.
    #[test]
    fn a_page_comes_out_oldest_first() {
        let page = vec![info(3, 42, false), info(2, 42, false), info(1, 42, false)];

        let messages = to_messages(42, page);

        assert_eq!(
            messages
                .iter()
                .map(|message| message.id)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(
            messages
                .iter()
                .map(|message| message.timestamp)
                .collect::<Vec<_>>(),
            vec![1_700_000_001, 1_700_000_002, 1_700_000_003],
            "the timestamp travels with the message it belongs to"
        );
    }

    #[test]
    fn a_page_is_translated_field_for_field() {
        let messages = to_messages(42, vec![info(7, 42, true)]);

        let message = &messages[0];
        assert_eq!(message.id, 7);
        assert_eq!(message.chat_id, 42);
        assert_eq!(message.text, "message 7");
        assert!(message.is_outgoing);
        assert!(
            matches!(message.status, MessageStatus::Sent),
            "the account wrote it and telegram gave it back"
        );
    }

    /// A page is fetched for one conversation, so a message naming another is
    /// not part of it — and splicing one in would put a stranger's message in
    /// the window on show.
    #[test]
    fn a_message_naming_another_conversation_is_not_part_of_the_page() {
        let messages = to_messages(42, vec![info(1, 42, false), info(2, 99, false)]);

        assert_eq!(
            messages
                .iter()
                .map(|message| message.id)
                .collect::<Vec<_>>(),
            vec![1],
            "the page belongs to conversation 42"
        );
    }

    /// The page size the framework really sent is what "was the page short?" is
    /// answered against, and it is not always what the caller asked for.
    #[test]
    fn a_requested_page_size_is_the_clamped_one() {
        let clamped = requested(HistoryArgs::latest(usize::MAX));

        assert_eq!(
            clamped,
            usize::try_from(telegram_framework::HISTORY_LIMIT).expect("the limit is positive"),
            "a caller asking for more than telegram returns gets the clamped page"
        );
        assert_eq!(requested(HistoryArgs::latest(0)), 1);
    }

    /// An identifier that cannot have come from Telegram has no page to ask for.
    #[test]
    fn a_message_identifier_outside_telegram_s_range_is_rejected() {
        assert_eq!(narrow(7, 42), Some(7));
        assert_eq!(narrow(i64::from(i32::MAX), 42), Some(i32::MAX));
        assert_eq!(narrow(i64::from(i32::MAX) + 1, 42), None);
        assert_eq!(
            narrow(-1, 42),
            Some(-1),
            "the check is about the wire's range, not about whether telegram would use the value"
        );
    }
}
