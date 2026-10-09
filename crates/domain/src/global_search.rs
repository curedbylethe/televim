//! Searching every private conversation at once.
//!
//! `:search` asks the server for matches across all peers, so this module is
//! server-authoritative: there is no local pass, because the history cache
//! holds at most a few peers' windows and a scan over it would look like an
//! answer while missing most of the archive. The state therefore moves from
//! in-flight to either a server answer or a failure, and nothing in between.
//!
//! # Grouping
//!
//! Hits arrive oldest first, as [`crate::search`] keeps them. Grouping keeps
//! that order and splits it into contiguous runs of one chat: a chat that the
//! server interleaves with another appears as two groups, not one. There is
//! deliberately no per-chat cap; the total cap is the only limit.
//!
//! # Why the list is bounded
//!
//! Like [`SEARCH_MATCHES`] for a single conversation, the cap is a round-trip
//! bound, not a memory one: the first page is all that is requested, and the
//! true total travels beside the list so truncation is stated, not hidden. A
//! hit is a few small fields, so the memory is negligible either way.

use crate::message::MediaKind;
use crate::search::SEARCH_MATCHES;

/// One message the server matched, with what is needed to show it.
///
/// Global hits live in no open window, so an identifier alone cannot be
/// rendered: the text travels with the hit. `text` is the caption or message
/// body; `media` names the attachment when there is one. Neither is a payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalHit {
    /// The chat the message belongs to.
    pub chat_id: i64,

    /// The message's identifier within that chat.
    pub message_id: i64,

    /// The message's text, or its caption; empty for a bare attachment.
    pub text: String,

    /// The attachment the message carries, if it carries one.
    pub media: Option<MediaKind>,
}

impl GlobalHit {
    /// The body to show: the text, or the placeholder for the attachment when
    /// there is no text.
    ///
    /// The same rule as [`crate::message::Message::display_body`], so a hit
    /// reads the way the message reads in its own conversation.
    #[must_use]
    pub fn display_body(&self) -> &str {
        if self.text.is_empty() {
            self.media.map_or("", MediaKind::label)
        } else {
            &self.text
        }
    }
}

/// A run of contiguous hits that share a chat, borrowed from the state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatGroup<'a> {
    /// The chat every hit in the run belongs to.
    pub chat_id: i64,

    /// The position of the run's first hit in the flattened list, so a caller
    /// can map the selection onto the groups.
    pub first_index: usize,

    /// The hits of the run, in server order.
    pub hits: &'a [GlobalHit],
}

/// The results of the last global search, and where the reader is among them.
///
/// Pure state: no clock, no client. Like [`crate::user::UserSearchState`], it
/// owns the query, so an answer to a query the reader has since replaced is
/// refused here rather than by every caller.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GlobalSearchState {
    /// The query, or empty when there has been no search.
    query: String,

    /// The matches, oldest first, capped at [`SEARCH_MATCHES`].
    hits: Vec<GlobalHit>,

    /// The selected hit, an index into `hits`, or zero on an empty list.
    selected: usize,

    /// How many matches the server holds in all, which may exceed `hits`.
    total: usize,

    /// Whether an answer is still on its way.
    in_flight: bool,

    /// Why the search failed, if it did.
    failure: Option<String>,
}

impl GlobalSearchState {
    /// Starts a search: records the query, clears the old hits, and marks the
    /// answer as pending.
    ///
    /// An empty query is no search at all, so it clears the state rather than
    /// leaving an active search with nothing to ask for.
    pub fn begin(&mut self, query: &str) {
        if query.is_empty() {
            self.clear();
            return;
        }

        query.clone_into(&mut self.query);
        self.hits.clear();
        self.selected = 0;
        self.total = 0;
        self.in_flight = true;
        self.failure = None;
    }

    /// Replaces the hits with the server's answer to `query`, if still wanted.
    ///
    /// Returns `false` when `query` is not the one being searched for, so an
    /// answer to a replaced query cannot overwrite the current one. The list is
    /// capped at [`SEARCH_MATCHES`] keeping the oldest, and the total is never
    /// less than what is held.
    pub fn adopt(&mut self, query: &str, mut hits: Vec<GlobalHit>, total: usize) -> bool {
        if !self.is_for(query) {
            return false;
        }

        hits.truncate(SEARCH_MATCHES);
        self.hits = hits;
        self.total = total.max(self.hits.len());
        self.selected = 0;
        self.in_flight = false;
        self.failure = None;

        true
    }

    /// Records that the search for `query` failed.
    ///
    /// Returns `false` when `query` is stale, as [`GlobalSearchState::adopt`]
    /// does. A failure leaves nothing to show, so the hits are cleared with it.
    pub fn fail(&mut self, query: &str, reason: impl Into<String>) -> bool {
        if !self.is_for(query) {
            return false;
        }

        self.hits.clear();
        self.total = 0;
        self.selected = 0;
        self.in_flight = false;
        self.failure = Some(reason.into());

        true
    }

    /// Forgets the search, which is what closing the overlay does.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Whether a search has been started and not cleared.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !self.query.is_empty()
    }

    /// The query being searched for, if there has been one.
    #[must_use]
    pub fn query(&self) -> Option<&str> {
        (!self.query.is_empty()).then_some(self.query.as_str())
    }

    /// Whether an answer is still on its way.
    #[must_use]
    pub fn in_flight(&self) -> bool {
        self.in_flight
    }

    /// The matches, oldest first, in server order.
    #[must_use]
    pub fn hits(&self) -> &[GlobalHit] {
        &self.hits
    }

    /// How many matches are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.hits.len()
    }

    /// Whether the search matched nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hits.is_empty()
    }

    /// How many matches the server holds in all.
    #[must_use]
    pub fn total(&self) -> usize {
        self.total
    }

    /// Where the selection stands among the held hits.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The hit the reader is on, if the list has one.
    #[must_use]
    pub fn selected_hit(&self) -> Option<&GlobalHit> {
        self.hits.get(self.selected)
    }

    /// Moves the selection `delta` hits, wrapping at both ends.
    ///
    /// Wrapping rather than clamping, as in the user search: a reader who has
    /// run off one end is reaching for the other. A move on an empty list does
    /// nothing.
    pub fn move_selection(&mut self, delta: isize) {
        let len = self.hits.len();
        if len == 0 {
            return;
        }

        let steps = delta.unsigned_abs() % len;
        self.selected = if delta >= 0 {
            (self.selected + steps) % len
        } else {
            (self.selected + len - steps) % len
        };
    }

    /// The held hits split into runs of one chat, in server order.
    #[must_use]
    pub fn groups(&self) -> Vec<ChatGroup<'_>> {
        let mut groups = Vec::new();
        let mut start = 0;

        for end in 1..=self.hits.len() {
            if end == self.hits.len() || self.hits[end].chat_id != self.hits[start].chat_id {
                groups.push(ChatGroup {
                    chat_id: self.hits[start].chat_id,
                    first_index: start,
                    hits: &self.hits[start..end],
                });
                start = end;
            }
        }

        groups
    }

    /// What the status line says about the search.
    ///
    /// One function owns every wording, in the shape of
    /// [`crate::search::SearchState::label`]: `/query — …`.
    #[must_use]
    pub fn label(&self) -> String {
        let Some(query) = self.query() else {
            return String::new();
        };

        let description = if self.in_flight {
            "searching…".to_owned()
        } else if let Some(reason) = &self.failure {
            format!("no matches (search failed: {reason})")
        } else if self.hits.is_empty() {
            "no matches".to_owned()
        } else {
            let chats = self.groups().len();
            let chat_noun = if chats == 1 { "chat" } else { "chats" };
            let shown = self.hits.len();

            if self.total > shown {
                format!("{shown} of {} results in {chats} {chat_noun}", self.total)
            } else {
                let noun = if shown == 1 { "result" } else { "results" };
                format!("{shown} {noun} in {chats} {chat_noun}")
            }
        };

        format!("/{query} — {description}")
    }

    /// Whether `query` is the search being waited on or shown.
    fn is_for(&self, query: &str) -> bool {
        !self.query.is_empty() && self.query == query
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A text hit in `chat_id`, with the message identifier as its text.
    fn hit(chat_id: i64, message_id: i64) -> GlobalHit {
        GlobalHit {
            chat_id,
            message_id,
            text: format!("m{message_id}"),
            media: None,
        }
    }

    /// `count` hits in chat 1, identified 1 to `count`.
    fn one_chat(count: usize) -> Vec<GlobalHit> {
        (1..=count)
            .map(|id| hit(1, i64::try_from(id).expect("test ids fit in an i64")))
            .collect()
    }

    /// A state with a search for `query` answered by `hits`.
    fn answered(query: &str, hits: Vec<GlobalHit>, total: usize) -> GlobalSearchState {
        let mut state = GlobalSearchState::default();
        state.begin(query);
        assert!(state.adopt(query, hits, total));
        state
    }

    #[test]
    fn hits_group_into_contiguous_runs_in_server_order() {
        let state = answered("x", vec![hit(1, 10), hit(1, 11), hit(2, 12), hit(1, 13)], 4);

        let groups = state.groups();
        let shape: Vec<(i64, usize, usize)> = groups
            .iter()
            .map(|group| (group.chat_id, group.first_index, group.hits.len()))
            .collect();

        // A chat the server interleaves with another is two groups, not one.
        assert_eq!(shape, vec![(1, 0, 2), (2, 2, 1), (1, 3, 1)]);
        assert_eq!(groups[2].hits[0].message_id, 13);
    }

    #[test]
    fn selection_wraps_at_both_ends() {
        let mut state = answered("x", vec![hit(1, 1), hit(1, 2), hit(2, 3)], 3);

        state.move_selection(1);
        assert_eq!(state.selected(), 1);

        state.move_selection(2);
        assert_eq!(state.selected(), 0, "forward past the end wraps to the top");

        state.move_selection(-1);
        assert_eq!(state.selected(), 2, "back past the top wraps to the end");

        state.move_selection(-7);
        assert_eq!(state.selected(), 1, "a move larger than the list wraps too");
        assert_eq!(state.selected_hit().map(|hit| hit.message_id), Some(2));
    }

    #[test]
    fn selection_on_an_empty_list_does_nothing() {
        let mut state = answered("x", Vec::new(), 0);
        state.move_selection(3);
        assert_eq!(state.selected(), 0);
        assert!(state.selected_hit().is_none());
    }

    #[test]
    fn a_stale_answer_is_refused() {
        let mut state = GlobalSearchState::default();
        state.begin("a");
        state.begin("b");

        assert!(!state.adopt("a", vec![hit(1, 1)], 1));
        assert!(!state.fail("a", "flood"));

        assert!(state.in_flight());
        assert!(state.hits().is_empty());
        assert_eq!(state.query(), Some("b"));
    }

    #[test]
    fn an_answer_arrives_with_nothing_searched_for_is_refused() {
        let mut state = GlobalSearchState::default();
        assert!(!state.adopt("a", vec![hit(1, 1)], 1));
        assert!(!state.fail("a", "flood"));
        assert!(!state.is_active());
    }

    #[test]
    fn a_later_answer_replaces_an_earlier_one() {
        let mut state = answered("q", vec![hit(1, 1)], 1);
        state.move_selection(0);

        assert!(state.adopt("q", vec![hit(2, 5), hit(2, 6)], 2));

        assert_eq!(state.hits().len(), 2);
        assert_eq!(state.hits()[0].chat_id, 2);
        assert_eq!(state.total(), 2);
        assert_eq!(state.selected(), 0);
        assert!(!state.in_flight());
    }

    #[test]
    fn the_list_is_capped_keeping_the_oldest_and_the_total_is_reported() {
        let state = answered("q", one_chat(SEARCH_MATCHES + 50), SEARCH_MATCHES + 50);

        assert_eq!(state.len(), SEARCH_MATCHES);
        assert_eq!(state.hits()[0].message_id, 1);
        assert_eq!(
            state.hits()[SEARCH_MATCHES - 1].message_id,
            i64::try_from(SEARCH_MATCHES).expect("the cap fits in an i64")
        );
        assert_eq!(state.total(), SEARCH_MATCHES + 50);
    }

    #[test]
    fn clearing_forgets_the_search() {
        let mut state = answered("q", vec![hit(1, 1)], 1);
        state.clear();

        assert!(!state.is_active());
        assert_eq!(state.query(), None);
        assert!(state.hits().is_empty());
        assert_eq!(state.label(), "");
    }

    #[test]
    fn an_empty_query_is_no_search() {
        let mut state = answered("q", vec![hit(1, 1)], 1);
        state.begin("");

        assert!(!state.is_active());
        assert_eq!(state.label(), "");
    }

    #[test]
    fn a_failure_clears_the_hits_and_says_why() {
        let mut state = answered("q", Vec::new(), 0);
        state.begin("q");
        assert!(state.fail("q", "flood wait"));

        assert!(!state.in_flight());
        assert_eq!(state.label(), "/q — no matches (search failed: flood wait)");
    }

    #[test]
    fn label_while_searching() {
        let mut state = GlobalSearchState::default();
        state.begin("hi");
        assert_eq!(state.label(), "/hi — searching…");
    }

    #[test]
    fn label_with_no_matches() {
        let state = answered("hi", Vec::new(), 0);
        assert_eq!(state.label(), "/hi — no matches");
    }

    #[test]
    fn label_with_one_result_in_one_chat() {
        let state = answered("hi", vec![hit(1, 1)], 1);
        assert_eq!(state.label(), "/hi — 1 result in 1 chat");
    }

    #[test]
    fn label_with_several_results_across_chats() {
        let state = answered("hi", vec![hit(1, 1), hit(1, 2), hit(2, 3)], 3);
        assert_eq!(state.label(), "/hi — 3 results in 2 chats");
    }

    #[test]
    fn label_when_the_server_holds_more_than_it_sends() {
        let state = answered("hi", one_chat(SEARCH_MATCHES), 240);
        assert_eq!(state.label(), "/hi — 100 of 240 results in 1 chat");
    }

    #[test]
    fn display_body_prefers_text_then_the_attachment_label() {
        let text = hit(1, 1);
        assert_eq!(text.display_body(), "m1");

        let photo = GlobalHit {
            chat_id: 1,
            message_id: 2,
            text: String::new(),
            media: Some(MediaKind::Photo),
        };
        assert_eq!(photo.display_body(), "[image]");

        let captioned = GlobalHit {
            text: "at the pier".to_owned(),
            ..photo.clone()
        };
        assert_eq!(captioned.display_body(), "at the pier");

        let bare = GlobalHit {
            media: None,
            ..photo
        };
        assert_eq!(bare.display_body(), "");
    }
}
