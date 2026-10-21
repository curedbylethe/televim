//! Searching a conversation, as message identifiers.
//!
//! The framework hands over the identifiers a search matched and how many there
//! are in all. Two things are left to do here, and both are facts about the two
//! number spaces meeting in this crate rather than about the request:
//!
//! - **Widen** the wire's `i32` identifiers into the domain's `i64`, which is a
//!   conversion that cannot fail and so has no error of its own.
//! - **Turn the conversation's list around.** Telegram answers newest first, and a
//!   conversation's match list is walked oldest first: `n` means the next, newer
//!   match and `N` the previous, older one. The turn happens once, here, so that
//!   every caller gets a list it can walk without sorting — the same rule, and the
//!   same reasoning, as `history`'s page reversal. A global answer is not turned:
//!   its hits stay newest first, as Telegram sent them.
//!
//! # The cursor
//!
//! A global page hands back the cursor a later one starts from, in
//! [`GlobalCursor`], and takes it back in [`GlobalSearchRequest::after`]. The
//! cursor names the peer's kind as well as its identifier: a bare number cannot
//! be turned back into an address, so the kind travels with it rather than being
//! guessed here.
//!
//! # Why the cap is here
//!
//! [`SEARCH_MATCHES`] lives in `domain`, because that is where the match list is
//! held, and this crate clamps the request to it so that the one number appears
//! in one place. A limit written down in two crates is a limit that will be
//! changed in one of them.

use domain::global_search::MediaFilter;
use domain::message::MediaKind;

#[cfg(feature = "live")]
use crate::types::media_kind;
#[cfg(any(feature = "live", test))]
use domain::search::SEARCH_MATCHES;
#[cfg(feature = "live")]
use telegram_framework::search as framework;

/// Where a search matched, in the domain's identifier space.
///
/// The same shape the framework returns it in, and deliberately so: a search is
/// a list of places rather than a copy of the messages, and the identifiers are
/// oldest first so that a walk forward is always towards newer messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResults {
    /// The matching message identifiers, oldest first.
    pub ids: Vec<i64>,

    /// How many matches the conversation holds in all, which may exceed `ids`.
    pub total: usize,
}

/// One message a global search matched, in the domain's identifier space.
///
/// Carries its text, because a global hit has no open conversation to be read
/// from. `text` is what Telegram sent, so it is empty for a media-only message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalHit {
    /// The conversation the message is in.
    pub chat_id: i64,

    /// The message's identifier within that conversation.
    pub message_id: i64,

    /// The message text, as Telegram sent it.
    pub text: String,

    /// The attachment the message carries, if it carries one.
    pub media: Option<MediaKind>,

    /// When the message was sent, as unix seconds; `None` when Telegram sent no date.
    pub sent_at: Option<i64>,

    /// Whether the signed-in account sent the message.
    pub outgoing: bool,
}

/// The conversation a page of a global search stopped in, as the bare
/// identifier the rest of this crate reports.
///
/// The kind is part of the value rather than a separate field because a bare
/// identifier cannot be addressed again: Telegram needs to know whether the
/// number names a person, a group or a channel, and guessing would open the
/// wrong conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalPeer {
    /// A person.
    User(i64),

    /// A group.
    Chat(i64),

    /// A channel or supergroup.
    Channel(i64),
}

/// Where a page of a global search starts.
///
/// `rate` is the server's score for the message the page is counted from, and
/// `peer` and `message_id` name it. The three are one value because Telegram
/// takes them as one anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalCursor {
    /// The server's score for the message the next page starts after.
    pub rate: i64,

    /// The conversation that message is in.
    pub peer: GlobalPeer,

    /// The message's identifier within that conversation.
    pub message_id: i64,
}

/// Which matches a global search asks for.
///
/// The fields only mean something together, so they are one value rather than
/// loose arguments. `since` and `until` are unix seconds, both inclusive, and
/// `None` is no bound on that side; `after` is the cursor of the page to
/// continue from, or `None` for the first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalSearchRequest {
    /// What to look for, passed through as typed.
    pub query: String,

    /// The attachment kind to keep, if any.
    pub media: MediaFilter,

    /// The earliest send time to keep, as unix seconds, inclusive.
    pub since: Option<i64>,

    /// The latest send time to keep, as unix seconds, inclusive.
    pub until: Option<i64>,

    /// The page to start after, or `None` for the first page.
    pub after: Option<GlobalCursor>,

    /// How many matches to return, capped at [`SEARCH_MATCHES`].
    pub limit: usize,
}

/// The matches of a global search, newest first, and how many there are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalSearchResults {
    /// The hits, newest first, in the order Telegram answered them.
    pub hits: Vec<GlobalHit>,

    /// How many matches Telegram holds across every private conversation, which
    /// may exceed `hits`.
    pub total: usize,

    /// Where the next page starts, or `None` when Telegram sent every match.
    pub next: Option<GlobalCursor>,
}

/// The page size a search asks for, capped at what the domain will hold.
#[cfg(any(feature = "live", test))]
fn limit_of(limit: usize) -> usize {
    limit.min(SEARCH_MATCHES)
}

/// Turns a framework result into the domain's list: widened, and oldest first.
#[cfg(feature = "live")]
fn to_results(results: telegram_framework::SearchResults) -> SearchResults {
    let mut ids: Vec<i64> = results.ids.into_iter().map(i64::from).collect();

    // Telegram answers newest first; a match list is walked oldest first.
    ids.reverse();

    SearchResults {
        ids,
        total: results.total,
    }
}

/// The narrowing a request asks for, in the framework's vocabulary.
///
/// Total and exhaustive on purpose: a kind added to `domain` stops this
/// compiling rather than arriving at the server as no narrowing at all.
#[cfg(feature = "live")]
fn filter_of(media: MediaFilter) -> framework::GlobalFilter {
    match media {
        MediaFilter::None => framework::GlobalFilter::None,
        MediaFilter::PhotoVideo => framework::GlobalFilter::PhotoVideo,
        MediaFilter::Document => framework::GlobalFilter::Document,
        MediaFilter::Music => framework::GlobalFilter::Music,
        MediaFilter::Voice => framework::GlobalFilter::Voice,
        MediaFilter::Url => framework::GlobalFilter::Url,
    }
}

/// Turns a request into the framework's arguments: narrowed, dated, continued.
///
/// `None` is the wire's zero on both date bounds, and the limit is clamped once
/// here so that the one page size lives in one crate.
#[cfg(feature = "live")]
fn to_global_args(request: &GlobalSearchRequest) -> framework::GlobalSearchArgs {
    framework::GlobalSearchArgs {
        query: request.query.clone(),
        filter: filter_of(request.media),
        min_date: request.since.unwrap_or(0),
        max_date: request.until.unwrap_or(0),
        after: request.after.map(offset_of),
        limit: limit_of(request.limit),
    }
}

/// Turns the framework's cursor into this crate's: widened, and keeping the
/// peer's kind.
#[cfg(feature = "live")]
fn cursor_of(offset: framework::GlobalOffset) -> GlobalCursor {
    let peer = match offset.peer {
        framework::GlobalPeer::User(id) => GlobalPeer::User(id),
        framework::GlobalPeer::Chat(id) => GlobalPeer::Chat(id),
        framework::GlobalPeer::Channel(id) => GlobalPeer::Channel(id),
    };

    GlobalCursor {
        rate: i64::from(offset.rate),
        peer,
        message_id: i64::from(offset.message_id),
    }
}

/// Turns a cursor back into the anchor the request continues from.
#[cfg(feature = "live")]
fn offset_of(cursor: GlobalCursor) -> framework::GlobalOffset {
    let peer = match cursor.peer {
        GlobalPeer::User(id) => framework::GlobalPeer::User(id),
        GlobalPeer::Chat(id) => framework::GlobalPeer::Chat(id),
        GlobalPeer::Channel(id) => framework::GlobalPeer::Channel(id),
    };

    framework::GlobalOffset {
        rate: narrow(cursor.rate),
        peer,
        message_id: narrow(cursor.message_id),
    }
}

/// Narrows a widened number back into the wire's signed 32 bits.
///
/// A cursor came from the wire, so its numbers always fit; a caller that hands
/// back something else gets the nearest bound rather than a wrapped value.
#[cfg(feature = "live")]
fn narrow(value: i64) -> i32 {
    i32::try_from(value).unwrap_or(if value < 0 { i32::MIN } else { i32::MAX })
}

/// Turns a framework global result into the domain's: widened, and still newest first.
#[cfg(feature = "live")]
fn to_global_results(results: framework::GlobalSearchPage) -> GlobalSearchResults {
    let hits: Vec<GlobalHit> = results
        .hits
        .into_iter()
        .map(|hit| GlobalHit {
            chat_id: hit.chat_id,
            message_id: i64::from(hit.message_id),
            text: hit.text,
            media: hit.media.map(media_kind),
            sent_at: hit.sent_at,
            outgoing: hit.outgoing,
        })
        .collect();

    GlobalSearchResults {
        hits,
        total: results.total,
        next: results.next.map(cursor_of),
    }
}

/// The search operation, which needs the framework's client.
///
/// It returns this crate's [`SearchResults`] or [`ProtoError`](crate::ProtoError),
/// so a caller needs no knowledge of Telegram's paging arguments. A flood wait
/// arrives as a `RequestError::Rpc` carrying the delay in its `value`; deciding
/// what to do about it is the caller's, because only the caller knows what the
/// reader asked for.
#[cfg(feature = "live")]
impl crate::ProtoClient {
    /// Searches a conversation for messages matching `query`.
    ///
    /// One request per call, and the result is one page: the match list is
    /// bounded by [`SEARCH_MATCHES`], so asking for more would only delay the
    /// answer. `total` reports what the conversation holds beyond that page, so
    /// the caller can say what is not being shown.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Framework`](crate::ProtoError::Framework) if the
    /// conversation is not in the session's peer cache, if Telegram rejects the
    /// request, or if the answer cannot be decoded. A search that matches
    /// nothing is an ordinary empty result, not an error.
    pub async fn search(
        &self,
        peer_id: i64,
        query: &str,
        limit: usize,
    ) -> Result<SearchResults, crate::ProtoError> {
        let args = telegram_framework::SearchArgs::first(query, limit_of(limit));
        let results = self.inner().search_messages(peer_id, args).await?;

        tracing::debug!(
            peer_id,
            returned = results.ids.len(),
            total = results.total,
            "searched a conversation"
        );

        Ok(to_results(results))
    }

    /// Searches every private conversation for the matches `request` asks for.
    ///
    /// One request and one page: the query, the attachment filter, the two date
    /// bounds and the limit all ride in `request`, capped at [`SEARCH_MATCHES`]
    /// for the same reason as [`search`](Self::search). The hits are newest
    /// first, as Telegram answered them, each with the text the caller shows for
    /// it. `next` is the cursor a later page continues from — following it is the
    /// caller's decision, because only the caller knows what the reader asked
    /// for.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Framework`](crate::ProtoError::Framework) if Telegram
    /// rejects the request, or if the answer cannot be decoded. A query that
    /// matches nothing is an ordinary empty result, not an error.
    pub async fn search_global(
        &self,
        request: &GlobalSearchRequest,
    ) -> Result<GlobalSearchResults, crate::ProtoError> {
        let results = self.inner().search_global(to_global_args(request)).await?;

        tracing::debug!(
            returned = results.hits.len(),
            total = results.total,
            next = results.next.is_some(),
            "searched every private conversation"
        );

        Ok(to_global_results(results))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_never_asks_for_more_than_the_list_holds() {
        assert_eq!(limit_of(10), 10);
        assert_eq!(limit_of(SEARCH_MATCHES), SEARCH_MATCHES);
        assert_eq!(
            limit_of(SEARCH_MATCHES + 1),
            SEARCH_MATCHES,
            "asking for more than the match list holds only delays the answer"
        );
        assert_eq!(limit_of(usize::MAX), SEARCH_MATCHES);
    }

    #[test]
    fn limit_is_still_clamped_to_the_page_size() {
        let request = GlobalSearchRequest {
            query: "q".to_owned(),
            media: MediaFilter::None,
            since: None,
            until: None,
            after: None,
            limit: SEARCH_MATCHES * 2,
        };

        assert_eq!(
            limit_of(request.limit),
            SEARCH_MATCHES,
            "a global request is one page, capped where the conversation search is"
        );
    }
}

/// The translation from the framework's result.
///
/// Gated with the type it converts from. The cap rule needs neither a feature
/// nor a datacenter and runs on every job; these run whenever the client does.
#[cfg(all(test, feature = "live"))]
mod live_tests {
    use super::*;

    #[test]
    fn a_newest_first_answer_comes_out_oldest_first() {
        let results = to_results(telegram_framework::SearchResults {
            ids: vec![30, 20, 10],
            total: 3,
        });

        assert_eq!(
            results.ids,
            vec![10, 20, 30],
            "the walk's forward direction has to mean 'newer' whatever order \
             telegram answered in"
        );
    }

    #[test]
    fn every_identifier_is_widened_without_changing() {
        // Descending, so the turn leaves the list in the order the assertion
        // reads it in.
        let results = to_results(telegram_framework::SearchResults {
            ids: vec![i32::MAX, 7, i32::MIN],
            total: 3,
        });

        assert_eq!(
            results.ids,
            vec![i64::from(i32::MIN), 7, i64::from(i32::MAX)]
        );
    }

    /// The total is a count of matches, not of the list, and the turn must not
    /// touch it: rearranging a page does not change how many there are.
    #[test]
    fn the_total_survives_the_turn_unchanged() {
        let results = to_results(telegram_framework::SearchResults {
            ids: vec![30, 20, 10],
            total: 1_243,
        });

        assert_eq!(results.total, 1_243);
        assert_eq!(results.ids.len(), 3);
    }

    #[test]
    fn a_search_that_matched_nothing_is_an_empty_result() {
        let results = to_results(telegram_framework::SearchResults {
            ids: Vec::new(),
            total: 0,
        });

        assert!(results.ids.is_empty(), "nothing is not a failure");
        assert_eq!(results.total, 0);
    }

    fn framework_hit(
        chat_id: i64,
        message_id: i32,
        text: &str,
    ) -> telegram_framework::search::GlobalHit {
        telegram_framework::search::GlobalHit {
            chat_id,
            message_id,
            text: text.to_owned(),
            media: None,
            sent_at: Some(1_700_000_000),
            outgoing: false,
        }
    }

    #[test]
    fn a_global_hit_keeps_its_media_kind_in_the_domain_vocabulary() {
        let photo = telegram_framework::search::GlobalHit {
            media: Some(telegram_framework::media::MediaKind::Photo),
            ..framework_hit(1, 1, "")
        };
        let bare = framework_hit(1, 2, "words");

        let results = to_global_results(framework::GlobalSearchPage {
            hits: vec![photo, bare],
            total: 2,
            next: None,
        });

        // Kept in Telegram's order: the photo is the newer of the two, first.
        assert_eq!(results.hits[0].media, Some(MediaKind::Photo));
        assert!(
            results.hits[0].text.is_empty(),
            "a bare attachment has no text"
        );
        assert_eq!(results.hits[1].media, None);
    }

    #[test]
    fn a_newest_first_global_answer_stays_newest_first() {
        let results = to_global_results(framework::GlobalSearchPage {
            hits: vec![
                framework_hit(1, 30, "c"),
                framework_hit(2, 20, "b"),
                framework_hit(3, 10, "a"),
            ],
            total: 3,
            next: None,
        });

        let message_ids: Vec<i64> = results.hits.iter().map(|hit| hit.message_id).collect();
        assert_eq!(message_ids, vec![30, 20, 10]);
        let texts: Vec<&str> = results.hits.iter().map(|hit| hit.text.as_str()).collect();
        assert_eq!(
            texts,
            vec!["c", "b", "a"],
            "each text stays with its message"
        );
    }

    #[test]
    fn a_global_hit_keeps_its_chat_and_widens_its_message_identifier() {
        let results = to_global_results(framework::GlobalSearchPage {
            hits: vec![
                framework_hit(i64::MAX, i32::MAX, "x"),
                telegram_framework::search::GlobalHit {
                    sent_at: Some(1_700_000_001),
                    outgoing: true,
                    ..framework_hit(-5, 7, "y")
                },
            ],
            total: 2,
            next: None,
        });

        assert_eq!(results.hits[0].chat_id, i64::MAX);
        assert_eq!(results.hits[0].message_id, i64::from(i32::MAX));
        assert_eq!(results.hits[0].sent_at, Some(1_700_000_000));
        assert!(
            !results.hits[0].outgoing,
            "an incoming message stays incoming"
        );
        assert_eq!(results.hits[1].chat_id, -5);
        assert_eq!(results.hits[1].message_id, 7);
        assert_eq!(results.hits[1].sent_at, Some(1_700_000_001));
        assert!(
            results.hits[1].outgoing,
            "an outgoing message stays outgoing"
        );
    }

    #[test]
    fn a_global_total_survives_the_turn_unchanged() {
        let results = to_global_results(framework::GlobalSearchPage {
            hits: vec![framework_hit(1, 2, "a"), framework_hit(1, 1, "b")],
            total: 1_243,
            next: None,
        });

        assert_eq!(results.total, 1_243, "a page is not the count of matches");
        assert_eq!(results.hits.len(), 2);
    }

    #[test]
    fn an_empty_global_answer_is_an_empty_result() {
        let results = to_global_results(framework::GlobalSearchPage {
            hits: Vec::new(),
            total: 0,
            next: None,
        });

        assert!(results.hits.is_empty(), "nothing is not a failure");
        assert_eq!(results.total, 0);
    }

    /// A first-page request for "hello", with no narrowing of any kind.
    fn plain_request() -> GlobalSearchRequest {
        GlobalSearchRequest {
            query: "hello".to_owned(),
            media: MediaFilter::None,
            since: None,
            until: None,
            after: None,
            limit: 50,
        }
    }

    #[test]
    fn a_cursor_survives_the_round_trip() {
        let cursors = [
            GlobalCursor {
                rate: 1_700_000_000,
                peer: GlobalPeer::User(42),
                message_id: 4_096,
            },
            GlobalCursor {
                rate: 1_700_000_001,
                peer: GlobalPeer::Chat(7),
                message_id: 1,
            },
            GlobalCursor {
                rate: 1_700_000_002,
                peer: GlobalPeer::Channel(99),
                message_id: i64::from(i32::MAX),
            },
        ];

        for cursor in cursors {
            let offset = offset_of(cursor);

            assert_eq!(
                offset.peer,
                match cursor.peer {
                    GlobalPeer::User(id) => framework::GlobalPeer::User(id),
                    GlobalPeer::Chat(id) => framework::GlobalPeer::Chat(id),
                    GlobalPeer::Channel(id) => framework::GlobalPeer::Channel(id),
                },
                "the peer's kind reaches the wire: {:?}",
                cursor.peer
            );
            assert_eq!(i64::from(offset.rate), cursor.rate);
            assert_eq!(i64::from(offset.message_id), cursor.message_id);
            assert_eq!(cursor_of(offset), cursor, "{:?}", cursor.peer);
        }
    }

    #[test]
    fn an_empty_page_has_no_cursor() {
        let results = to_global_results(framework::GlobalSearchPage {
            hits: Vec::new(),
            total: 0,
            next: None,
        });

        assert_eq!(
            results.next, None,
            "a page with nothing on it has nowhere to continue from"
        );
    }

    #[test]
    fn the_date_bounds_are_passed_through() {
        let bounded = GlobalSearchRequest {
            since: Some(1_600_000_000),
            until: Some(1_700_000_000),
            ..plain_request()
        };
        let args = to_global_args(&bounded);

        assert_eq!(args.min_date, 1_600_000_000, "an inclusive lower bound");
        assert_eq!(args.max_date, 1_700_000_000, "an inclusive upper bound");

        let unbounded = to_global_args(&plain_request());
        assert_eq!(unbounded.min_date, 0, "zero is the wire's 'no bound'");
        assert_eq!(unbounded.max_date, 0);
    }

    #[test]
    fn every_media_filter_reaches_the_wire() {
        let cases = [
            (MediaFilter::None, framework::GlobalFilter::None),
            (MediaFilter::PhotoVideo, framework::GlobalFilter::PhotoVideo),
            (MediaFilter::Document, framework::GlobalFilter::Document),
            (MediaFilter::Music, framework::GlobalFilter::Music),
            (MediaFilter::Voice, framework::GlobalFilter::Voice),
            (MediaFilter::Url, framework::GlobalFilter::Url),
        ];

        for (media, expected) in cases {
            let request = GlobalSearchRequest {
                media,
                ..plain_request()
            };

            assert_eq!(
                to_global_args(&request).filter,
                expected,
                "{media:?} is a narrowing Telegram understands"
            );
        }
    }
}
