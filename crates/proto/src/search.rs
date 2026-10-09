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
//! # Why the cap is here
//!
//! [`SEARCH_MATCHES`] lives in `domain`, because that is where the match list is
//! held, and this crate clamps the request to it so that the one number appears
//! in one place. A limit written down in two crates is a limit that will be
//! changed in one of them.

use domain::message::MediaKind;

#[cfg(feature = "live")]
use crate::types::media_kind;
#[cfg(any(feature = "live", test))]
use domain::search::SEARCH_MATCHES;

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

    /// When the message was sent, as unix seconds.
    pub sent_at: i64,

    /// Whether the signed-in account sent the message.
    pub outgoing: bool,
}

/// The matches of a global search, newest first, and how many there are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalSearchResults {
    /// The hits, newest first, in the order Telegram answered them.
    pub hits: Vec<GlobalHit>,

    /// How many matches Telegram holds across every private conversation, which
    /// may exceed `hits`.
    pub total: usize,
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

/// Turns a framework global result into the domain's: widened, and still newest first.
#[cfg(feature = "live")]
fn to_global_results(
    results: telegram_framework::search::GlobalSearchResults,
) -> GlobalSearchResults {
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

    /// Searches every private conversation for messages matching `query`.
    ///
    /// One request and one page, bounded by [`SEARCH_MATCHES`] for the same
    /// reason as [`search`](Self::search). The hits are newest first, as Telegram
    /// answered them, each with the text the caller shows for it.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Framework`](crate::ProtoError::Framework) if Telegram
    /// rejects the request, or if the answer cannot be decoded. A query that
    /// matches nothing is an ordinary empty result, not an error.
    pub async fn search_global(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<GlobalSearchResults, crate::ProtoError> {
        let results = self
            .inner()
            .search_global(query.to_owned(), limit_of(limit))
            .await?;

        tracing::debug!(
            returned = results.hits.len(),
            total = results.total,
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
            sent_at: 1_700_000_000,
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

        let results = to_global_results(telegram_framework::search::GlobalSearchResults {
            hits: vec![photo, bare],
            total: 2,
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
        let results = to_global_results(telegram_framework::search::GlobalSearchResults {
            hits: vec![
                framework_hit(1, 30, "c"),
                framework_hit(2, 20, "b"),
                framework_hit(3, 10, "a"),
            ],
            total: 3,
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
        let results = to_global_results(telegram_framework::search::GlobalSearchResults {
            hits: vec![
                framework_hit(i64::MAX, i32::MAX, "x"),
                telegram_framework::search::GlobalHit {
                    sent_at: 1_700_000_001,
                    outgoing: true,
                    ..framework_hit(-5, 7, "y")
                },
            ],
            total: 2,
        });

        assert_eq!(results.hits[0].chat_id, i64::MAX);
        assert_eq!(results.hits[0].message_id, i64::from(i32::MAX));
        assert_eq!(results.hits[0].sent_at, 1_700_000_000);
        assert!(
            !results.hits[0].outgoing,
            "an incoming message stays incoming"
        );
        assert_eq!(results.hits[1].chat_id, -5);
        assert_eq!(results.hits[1].message_id, 7);
        assert_eq!(results.hits[1].sent_at, 1_700_000_001);
        assert!(
            results.hits[1].outgoing,
            "an outgoing message stays outgoing"
        );
    }

    #[test]
    fn a_global_total_survives_the_turn_unchanged() {
        let results = to_global_results(telegram_framework::search::GlobalSearchResults {
            hits: vec![framework_hit(1, 2, "a"), framework_hit(1, 1, "b")],
            total: 1_243,
        });

        assert_eq!(results.total, 1_243, "a page is not the count of matches");
        assert_eq!(results.hits.len(), 2);
    }

    #[test]
    fn an_empty_global_answer_is_an_empty_result() {
        let results = to_global_results(telegram_framework::search::GlobalSearchResults {
            hits: Vec::new(),
            total: 0,
        });

        assert!(results.hits.is_empty(), "nothing is not a failure");
        assert_eq!(results.total, 0);
    }
}
