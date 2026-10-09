//! Searching one conversation, described in this crate's own vocabulary.
//!
//! `grammers` stops at this module's private functions. A search hands back
//! **places** — message identifiers and a total — rather than the messages
//! themselves, so the only thing that ever leaves here is a `Vec<i32>` and a
//! number. There is no `MessageInfo` in this module, and that is the guarantee
//! rather than a habit: a caller that walks a search never holds the body of a
//! message it is not showing.
//!
//! # Why not `search_messages`
//!
//! `grammers`' own search builder exposes a query and an `offset_id`, and
//! nothing else: `add_offset`, `min_id` and `max_id` are not settable, and the
//! crate's own source carries a `TODO` to that effect. It also buffers whole
//! `grammers_client::types::Message` values, which is exactly the copy a search
//! does not need. This module therefore invokes `messages.search` itself, the
//! same argument and the same shape as `history`'s *"Why not `iter_messages`"*.
//!
//! # Ordering
//!
//! Telegram returns matches **newest first**. That is left as it arrives: the
//! caller owns the list the matches go into, and reversing it is the caller's
//! decision to make once rather than this module's on every call.
//!
//! # The total
//!
//! A sliced answer carries `count`, the total number of matches, which the
//! caller can use to say *"match 3 of 1,243"*. An unsliced answer — returned
//! when there are few enough matches to fit one page — has no `count` field at
//! all, so its total is the number of messages it holds. Reading `count`
//! unconditionally would report zero for exactly the small conversations a
//! search is most used in, which is why [`results_from`] reads whichever the
//! variant offers.

use crate::client::Client;
use crate::error::{FrameworkError, RequestError};
use crate::history::clamp_limit;
use crate::tl;

/// One message a global search matched: where it is, and what it says.
///
/// Unlike [`SearchResults`], a global hit carries its text. A global search has
/// no open conversation to draw the match from, so the identifiers alone cannot
/// be shown. `text` is passed through as Telegram sent it, which is empty for a
/// media-only message; choosing what to show in that case is the caller's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalHit {
    /// The bare identifier of the conversation the message is in.
    pub chat_id: i64,

    /// The message's identifier within that conversation.
    pub message_id: i32,

    /// The message text, untruncated.
    pub text: String,
}

/// The matches of a global search, and how many there are in all.
///
/// The same invariant as [`SearchResults`]: `total >= hits.len()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalSearchResults {
    /// The matched messages, in the order Telegram sent them (newest first).
    pub hits: Vec<GlobalHit>,

    /// How many matches Telegram holds across every private conversation.
    pub total: usize,
}

/// The largest page a search will return.
///
/// The same wire bound as [`HISTORY_LIMIT`](crate::HISTORY_LIMIT): Telegram
/// rejects a larger `limit` outright, so clamping is what lets a caller count in
/// whatever is convenient. The two are separate names for the same number
/// because a search is free to keep a different page size later.
pub const SEARCH_LIMIT: i32 = 100;

/// Which matches a search asks for.
///
/// The three fields only mean something together — `offset_id` anchors the page,
/// `limit` is how many to return, and `query` is what is being looked for — so
/// they are held in one value rather than passed as loose arguments. The same
/// argument `HistoryArgs` makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchArgs {
    /// What to look for. Passed through as typed: the query is the server's to
    /// interpret, and a rule invented here would be a second, worse one.
    pub query: String,

    /// The message the page is counted from. Zero means "the most recent match".
    pub offset_id: i32,

    /// How many matches to return.
    pub limit: i32,
}

impl SearchArgs {
    /// The newest `limit` matches.
    pub fn first(query: impl Into<String>, limit: usize) -> Self {
        Self {
            query: query.into(),
            offset_id: 0,
            limit: clamp_limit(limit),
        }
    }

    /// The `limit` matches older than `offset_id`.
    ///
    /// Not used by the client yet — a search holds one page — but the wire
    /// already expresses it, and the anchor is the whole of what a second page
    /// would need.
    pub fn after(query: impl Into<String>, offset_id: i32, limit: usize) -> Self {
        Self {
            query: query.into(),
            offset_id,
            limit: clamp_limit(limit),
        }
    }
}

/// Where a search matched, and how many matches there are in all.
///
/// **The shape is the design.** A search is a list of places, not a copy of the
/// messages: the only thing a match is ever used for is being gone to, and the
/// page is fetched there afterwards. Building a message for every match would
/// allocate and immediately drop every one of their bodies.
///
/// `total` is a plain `usize` rather than an `Option`, because every response
/// variant yields a number. The invariant is `total >= ids.len()`, and
/// `total > ids.len()` is exactly the truncation a caller reports as *"the first
/// hundred of these"*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResults {
    /// The matching message identifiers, in the order Telegram sent them.
    pub ids: Vec<i32>,

    /// How many matches the conversation holds in all.
    pub total: usize,
}

impl Client {
    /// Searches a conversation for messages matching `args`.
    ///
    /// `peer_id` is the conversation's *bare* identifier — the same number a
    /// [`DialogInfo`](crate::DialogInfo) reports and the same one a
    /// [`MessageInfo`](crate::MessageInfo) names as its chat — so the result can
    /// be matched against the chat list without a lookup table.
    ///
    /// The identifiers come back in Telegram's order, **newest first**: the
    /// caller owns the list they go into and decides how to read it.
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
    /// Returns [`FrameworkError::UnknownPeer`] when the conversation is not in
    /// the session's peer cache, and [`FrameworkError::Request`] when Telegram
    /// rejects the request, when the connection fails, or when the answer cannot
    /// be decoded.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use telegram_framework::session::MemoryStore;
    /// use telegram_framework::{ClientBuilder, SearchArgs};
    ///
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = ClientBuilder::new(1234, "api-hash")
    ///     .session_store(Box::new(MemoryStore::new()))
    ///     .build()
    ///     .await?;
    ///
    /// for dialog in client.fetch_dialogs().await? {
    ///     let found = client
    ///         .search_messages(dialog.peer_id, SearchArgs::first("televim", 50))
    ///         .await?;
    ///     println!("{}: {} of {} match(es)", dialog.title, found.ids.len(), found.total);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn search_messages(
        &self,
        peer_id: i64,
        args: SearchArgs,
    ) -> Result<SearchResults, FrameworkError> {
        let Some(peer) = self.peer_ref(peer_id) else {
            tracing::warn!(
                peer_id,
                "a search was requested for a conversation that is not in the peer cache"
            );
            return Err(FrameworkError::UnknownPeer(peer_id));
        };

        let SearchArgs {
            query,
            offset_id,
            limit,
        } = args;

        let request = tl::functions::messages::Search {
            peer: peer.into(),
            q: query,
            from_id: None,
            saved_peer_id: None,
            saved_reaction: None,
            top_msg_id: None,
            filter: tl::enums::MessagesFilter::InputMessagesFilterEmpty,
            min_date: 0,
            max_date: 0,
            offset_id,
            add_offset: 0,
            limit,
            max_id: 0,
            min_id: 0,
            hash: 0,
        };

        let response = self
            .inner()
            .invoke(&request)
            .await
            .map_err(|error| FrameworkError::from(RequestError::from_invocation(&error)))?;

        let results = results_from(response);

        // A search can cache a peer or move the datacenter, and that only
        // reaches the store if it is written back here.
        self.flush_session();

        tracing::debug!(
            peer_id,
            returned = results.ids.len(),
            total = results.total,
            "searched a conversation"
        );

        Ok(results)
    }

    /// Searches every private conversation for messages matching `query`.
    ///
    /// One request, and the result is its first page: `limit` is clamped into
    /// what Telegram accepts, and the matches are not continued past it. The
    /// request sets `users_only`, so Telegram restricts the search to one-to-one
    /// chats; groups and channels are not searched at all.
    ///
    /// The hits come back in Telegram's order, **newest first**.
    ///
    /// Peers in the response are not added to the peer cache, so a hit whose
    /// conversation is not already known cannot be opened by this client; the
    /// caller decides what to do with such a hit.
    ///
    /// # Errors
    ///
    /// Returns [`FrameworkError::Request`] when Telegram rejects the request,
    /// when the connection fails, or when the answer cannot be decoded. A query
    /// that matches nothing is an ordinary empty result.
    pub async fn search_global(
        &self,
        query: String,
        limit: usize,
    ) -> Result<GlobalSearchResults, FrameworkError> {
        let request = tl::functions::messages::SearchGlobal {
            broadcasts_only: false,
            groups_only: false,
            users_only: true,
            folder_id: None,
            q: query,
            filter: tl::enums::MessagesFilter::InputMessagesFilterEmpty,
            min_date: 0,
            max_date: 0,
            // The first page: no anchor peer and no anchor message.
            offset_rate: 0,
            offset_peer: tl::enums::InputPeer::Empty,
            offset_id: 0,
            limit: clamp_limit(limit),
        };

        let response = self
            .inner()
            .invoke(&request)
            .await
            .map_err(|error| FrameworkError::from(RequestError::from_invocation(&error)))?;

        let results = global_hits_from(response);

        self.flush_session();

        tracing::debug!(
            returned = results.hits.len(),
            total = results.total,
            "searched every private conversation"
        );

        Ok(results)
    }
}

/// Reads the messages and the total off a search response.
///
/// Shared by the per-conversation and the global answer: the four response
/// variants differ in where the total comes from, and the unsliced one is the
/// case that would otherwise read as zero — or panic — on a conversation small
/// enough to fit one page.
fn page_of(response: tl::enums::messages::Messages) -> (Vec<tl::enums::Message>, usize) {
    use tl::enums::messages::Messages;

    match response {
        // Unsliced: every match fits one page, and carries no `count` field.
        Messages::Messages(page) => {
            let total = page.messages.len();
            (page.messages, total)
        }
        Messages::Slice(page) => (page.messages, count_of(page.count)),
        Messages::ChannelMessages(page) => (page.messages, count_of(page.count)),
        // Only reachable if Telegram answers "nothing changed", which takes a
        // non-zero `hash` — this request always sends zero. The release profile
        // aborts on a panic, so an answer this build cannot read yields an empty
        // list and the count it did carry rather than crashing the process.
        Messages::NotModified(page) => (Vec::new(), count_of(page.count)),
    }
}

/// Reads the identifiers and the total off a per-conversation search response.
fn results_from(response: tl::enums::messages::Messages) -> SearchResults {
    let (raw, total) = page_of(response);

    let ids: Vec<i32> = raw.into_iter().map(|message| message.id()).collect();
    let total = total.max(ids.len());

    SearchResults { ids, total }
}

/// Reads the hits and the total off a global search response.
fn global_hits_from(response: tl::enums::messages::Messages) -> GlobalSearchResults {
    let (raw, total) = page_of(response);

    let hits: Vec<GlobalHit> = raw.into_iter().filter_map(hit_of).collect();
    let total = total.max(hits.len());

    GlobalSearchResults { hits, total }
}

/// Describes a message as a hit, if it has text to show.
///
/// Empty and service messages are not hits: neither carries text. They still
/// count in the total, because Telegram counted them.
fn hit_of(message: tl::enums::Message) -> Option<GlobalHit> {
    let tl::enums::Message::Message(message) = message else {
        return None;
    };

    Some(GlobalHit {
        chat_id: bare_id_of(&message.peer_id),
        message_id: message.id,
        text: message.message,
    })
}

/// The bare identifier of a peer, the same number `grammers` reports as one.
fn bare_id_of(peer: &tl::enums::Peer) -> i64 {
    match peer {
        tl::enums::Peer::User(peer) => peer.user_id,
        tl::enums::Peer::Chat(peer) => peer.chat_id,
        tl::enums::Peer::Channel(peer) => peer.channel_id,
    }
}

/// Narrows a wire count into a length.
///
/// A count Telegram would not send as a negative is floored at zero rather than
/// wrapping to an enormous `usize`.
fn count_of(count: i32) -> usize {
    usize::try_from(count).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty message with the given identifier.
    fn empty(id: i32) -> tl::enums::Message {
        tl::enums::Message::Empty(tl::types::MessageEmpty { id, peer_id: None })
    }

    /// An unsliced page: every match fits one response, and there is no count.
    fn unsliced(ids: &[i32]) -> tl::enums::messages::Messages {
        tl::enums::messages::Messages::Messages(tl::types::messages::Messages {
            messages: ids.iter().copied().map(empty).collect(),
            topics: Vec::new(),
            chats: Vec::new(),
            users: Vec::new(),
        })
    }

    /// A sliced page: the ids are a page of `count` matches in all.
    fn sliced(ids: &[i32], count: i32) -> tl::enums::messages::Messages {
        tl::enums::messages::Messages::Slice(tl::types::messages::MessagesSlice {
            inexact: false,
            count,
            next_rate: None,
            offset_id_offset: None,
            search_flood: None,
            messages: ids.iter().copied().map(empty).collect(),
            topics: Vec::new(),
            chats: Vec::new(),
            users: Vec::new(),
        })
    }

    /// A channel's sliced page, which carries its count in the same place.
    fn channel_sliced(ids: &[i32], count: i32) -> tl::enums::messages::Messages {
        tl::enums::messages::Messages::ChannelMessages(tl::types::messages::ChannelMessages {
            inexact: false,
            pts: 0,
            count,
            offset_id_offset: None,
            messages: ids.iter().copied().map(empty).collect(),
            topics: Vec::new(),
            chats: Vec::new(),
            users: Vec::new(),
        })
    }

    #[test]
    fn the_first_page_starts_from_nowhere_in_particular() {
        let args = SearchArgs::first("hello", 50);

        assert_eq!(args.query, "hello");
        assert_eq!(args.offset_id, 0, "zero means 'the most recent match'");
        assert_eq!(args.limit, 50);
    }

    #[test]
    fn a_later_page_is_anchored_on_a_message() {
        let args = SearchArgs::after("hello", 4_096, 25);

        assert_eq!(args.query, "hello");
        assert_eq!(args.offset_id, 4_096);
        assert_eq!(args.limit, 25);
    }

    #[test]
    fn a_page_size_is_clamped_into_what_telegram_accepts() {
        assert_eq!(
            SearchArgs::first("q", 0).limit,
            1,
            "an empty page is rejected"
        );
        assert_eq!(SearchArgs::first("q", 1).limit, 1);
        assert_eq!(SearchArgs::first("q", 100).limit, 100);
        assert_eq!(
            SearchArgs::first("q", 101).limit,
            SEARCH_LIMIT,
            "asking for one more than telegram returns does not return more"
        );
        assert_eq!(
            SearchArgs::first("q", usize::MAX).limit,
            SEARCH_LIMIT,
            "and a size that does not fit the wire saturates rather than converting"
        );
    }

    /// The small conversation: every match fits one unsliced page, which has no
    /// count field, so the total is what came back and not a zero.
    #[test]
    fn an_unsliced_page_totals_its_own_messages() {
        let results = results_from(unsliced(&[30, 20, 10]));

        assert_eq!(results.ids, vec![30, 20, 10], "Telegram's order, untouched");
        assert_eq!(results.total, 3);
    }

    /// The truncation case: the answer is a page of a much larger set, and the
    /// count is what says so.
    #[test]
    fn a_sliced_page_carries_the_total_that_exceeds_it() {
        let results = results_from(sliced(&[30, 20, 10], 1_243));

        assert_eq!(results.ids, vec![30, 20, 10]);
        assert_eq!(
            results.total, 1_243,
            "the caller reports 'the first three of 1,243', not 'three'"
        );
    }

    #[test]
    fn a_channel_page_carries_its_count_the_same_way() {
        let results = results_from(channel_sliced(&[7, 3], 99));

        assert_eq!(results.ids, vec![7, 3]);
        assert_eq!(results.total, 99);
    }

    #[test]
    fn a_not_modified_answer_is_an_empty_result() {
        let results = results_from(tl::enums::messages::Messages::NotModified(
            tl::types::messages::MessagesNotModified { count: 5 },
        ));

        assert!(results.ids.is_empty(), "there are no messages to name");
        assert_eq!(results.total, 5);
    }

    /// The invariant the caller relies on: a list never exceeds the total it is
    /// a page of, so truncation is stated as `total > ids.len()` and never the
    /// other way round.
    #[test]
    fn the_total_is_never_below_the_list() {
        assert_eq!(results_from(unsliced(&[1, 2, 3])).total, 3);
        assert_eq!(
            results_from(sliced(&[1, 2, 3], 1)).total,
            3,
            "a count that disagreed with the page is raised to the page"
        );
        assert_eq!(results_from(sliced(&[], 0)).total, 0);
    }

    #[test]
    fn a_global_hit_names_the_conversation_of_each_peer_kind() {
        assert_eq!(
            bare_id_of(&tl::enums::Peer::User(tl::types::PeerUser { user_id: 42 })),
            42
        );
        assert_eq!(
            bare_id_of(&tl::enums::Peer::Chat(tl::types::PeerChat { chat_id: 7 })),
            7
        );
        assert_eq!(
            bare_id_of(&tl::enums::Peer::Channel(tl::types::PeerChannel {
                channel_id: 9
            })),
            9
        );
    }

    /// An empty message carries no text, so it is counted but is not a hit.
    #[test]
    fn a_message_without_text_is_counted_but_not_a_hit() {
        let response = tl::enums::messages::Messages::Slice(tl::types::messages::MessagesSlice {
            inexact: false,
            count: 4,
            next_rate: None,
            offset_id_offset: None,
            search_flood: None,
            messages: vec![empty(3), empty(2)],
            topics: Vec::new(),
            chats: Vec::new(),
            users: Vec::new(),
        });

        let results = global_hits_from(response);

        assert!(results.hits.is_empty(), "nothing to show");
        assert_eq!(results.total, 4, "Telegram counted them, so the total does");
    }

    #[test]
    fn an_empty_global_answer_is_an_empty_result() {
        let results = global_hits_from(unsliced(&[]));

        assert!(results.hits.is_empty());
        assert_eq!(results.total, 0);
    }
}
