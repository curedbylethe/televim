//! Searching the conversation on show.
//!
//! `/` is answered in two phases, and this module holds the state both phases
//! write into. The window on show is scanned immediately — a word-prefix match
//! over at most a couple of hundred messages — so the reader can press `n` in
//! the same frame. That first answer is **provisional**: the local predicate
//! approximates what the server does, and it can only see what the client has
//! loaded. The server's answer, when it arrives, replaces it.
//!
//! # Two searches, and why they are not merged
//!
//! The local scan and `messages.search` do not mean the same thing by "match".
//! The server's search is case-insensitive and fuzzy — a word may also match its
//! plural, for instance — and it sees the whole conversation. [`word_prefix_match`]
//! is a close approximation of its *shape*: a whole word, case-insensitively.
//!
//! The two lists are therefore kept apart rather than unioned. A union would
//! hold matches the authoritative search did not confirm, and `n` would walk the
//! reader to a message with no highlight and nothing to say for itself. The
//! server list **replaces** the local one, and [`SearchSource`] is what lets the
//! label say which one is on screen.
//!
//! # Ordering
//!
//! Match identifiers are **oldest first, always**. Telegram answers a search
//! newest first and `proto` turns it around, so that `n` means "the next, newer
//! match" and `N` means "the previous, older one" no matter which phase supplied
//! the list.
//!
//! # Why the list is bounded
//!
//! [`SEARCH_MATCHES`] is not a memory bound — a hundred identifiers is eight
//! hundred bytes — but a **round-trip** bound: a search page is a hundred, and
//! every match beyond that is another request before `n` has an authoritative
//! list. The total is still reported, so the reader knows what they are not
//! being shown.

use crate::history::ConversationWindow;

/// How many matches a search holds.
///
/// A round-trip bound rather than a memory one: one page is what one request
/// returns, and holding more would mean another request before the reader can
/// walk the list. The true total travels beside the list, so truncation can be
/// stated rather than hidden.
pub const SEARCH_MATCHES: usize = 100;

/// Whether `text` holds a word that begins with `query`.
///
/// The local half of a search, and deliberately an approximation of the
/// server's rather than a different thing with the same name. Telegram matches
/// word *prefixes*, case-insensitively, so this scans for word starts — a
/// character that is alphanumeric and follows a non-alphanumeric one, or the
/// start of the text — and compares the query against each with ASCII
/// case-folding. `/ell` therefore does not match `"hello"` here or there, and
/// `/hell` matches in both places.
///
/// It allocates nothing: a per-message `to_lowercase` would allocate once per
/// message, and comparing per position would be a hand-rolled scan. This does
/// neither.
///
/// # What it cannot know
///
/// This is **not a subset** of what the server returns, in either direction.
/// Unicode case folding, diacritics (`cafe` against `café`), and Telegram's own
/// tokenisation are all outside what a scan over one string can decide. That is
/// why the local list is shown as provisional and is replaced by the server's
/// answer rather than merged with it.
pub fn word_prefix_match(text: &str, query: &str) -> bool {
    if query.is_empty() {
        return false;
    }

    let mut at_word_start = true;
    for (index, character) in text.char_indices() {
        if at_word_start && character.is_alphanumeric() && prefix_matches(&text[index..], query) {
            return true;
        }
        at_word_start = !character.is_alphanumeric();
    }

    false
}

/// Whether `query` is a case-insensitive prefix of `rest`.
fn prefix_matches(rest: &str, query: &str) -> bool {
    let mut rest = rest.chars();

    query.chars().all(|wanted| {
        rest.next()
            .is_some_and(|found| found.eq_ignore_ascii_case(&wanted))
    })
}

/// Where a search's matches came from.
///
/// The label turns on this, because a count from the loaded window and a count
/// from the conversation are not the same claim about the same thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchSource {
    /// The window's own text, through [`word_prefix_match`]: provisional.
    #[default]
    Local,

    /// The conversation's history, as Telegram searched it: authoritative.
    Server,
}

/// The matches of the last search, and where the reader is among them.
///
/// Pure state: no clock, no network, no window. The window is passed in where it
/// is needed — [`SearchState::resolve`] and nothing else — so the arithmetic can
/// be tested without one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchState {
    /// The query, or empty when there has been no search.
    query: String,

    /// The matching identifiers, oldest first, capped at [`SEARCH_MATCHES`].
    ids: Vec<i64>,

    /// Where the walk stands among `ids`, if it has moved at all.
    index: Option<usize>,

    /// How many matches the search had in all, which may exceed `ids`.
    total: usize,

    /// Which phase supplied the list.
    source: SearchSource,

    /// Whether a better answer is still on its way.
    in_flight: bool,

    /// Why the server pass failed, if it did.
    ///
    /// The local list stands after a failure; this is what lets the label say so
    /// rather than leaving the reader to guess why the count never changed.
    failure: Option<String>,

    /// A wrap-around announcement, shown until the next search action.
    notice: Option<String>,
}

impl SearchState {
    /// The query being searched for, if there has been one.
    ///
    /// `None` rather than an empty string, so that "no search" is a state a
    /// caller has to handle rather than one an empty query silently becomes.
    #[must_use]
    pub fn query(&self) -> Option<&str> {
        (!self.query.is_empty()).then_some(self.query.as_str())
    }

    /// Whether a search has been run and not cleared.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !self.query.is_empty()
    }

    /// The matching identifiers, oldest first.
    #[must_use]
    pub fn ids(&self) -> &[i64] {
        &self.ids
    }

    /// How many matches are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// Whether the search matched nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Where the walk stands among the matches.
    #[must_use]
    pub fn index(&self) -> Option<usize> {
        self.index
    }

    /// How many matches the conversation holds in all.
    #[must_use]
    pub fn total(&self) -> usize {
        self.total
    }

    /// Which phase supplied the list.
    #[must_use]
    pub fn source(&self) -> SearchSource {
        self.source
    }

    /// Whether a better answer is still on its way.
    #[must_use]
    pub fn in_flight(&self) -> bool {
        self.in_flight
    }

    /// Forgets the search, which is what opening another conversation does.
    ///
    /// As opening another buffer clears the highlight: the matches were places
    /// in a window that is no longer on screen, and none of them names a message
    /// in the new one.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Answers a search from the window, provisionally.
    ///
    /// The list is capped at [`SEARCH_MATCHES`] and the total is what remains
    /// after that cap: there is no point reporting a total for a list that has
    /// not been asked for yet. The walk starts unplaced, so the caller's first
    /// [`SearchState::next`] lands on the oldest match.
    pub fn begin_local(&mut self, query: &str, ids: Vec<i64>) {
        query.clone_into(&mut self.query);
        self.ids = capped(ids);
        self.total = self.ids.len();
        self.index = None;
        self.source = SearchSource::Local;
        self.in_flight = true;
        self.failure = None;
        self.notice = None;
    }

    /// Declares the local list the whole answer, with no request behind it.
    ///
    /// This is the skip for a conversation the window already holds in full: the
    /// server cannot see anything more, so a round trip would be pure latency.
    pub fn finish_local(&mut self) {
        self.in_flight = false;
    }

    /// Records that the server pass failed, keeping the local list.
    ///
    /// The local list is deliberately never emptied: the reader asked a question
    /// and the window answered it, and a failed request says nothing about that
    /// answer. The reason travels with the state so the label can say the list
    /// is local and why.
    pub fn fail(&mut self, reason: impl Into<String>) {
        self.in_flight = false;
        self.failure = Some(reason.into());
    }

    /// Replaces the local list with the server's, if it is still wanted.
    ///
    /// Returns `false` when `query` is not the one being searched for: the
    /// reader has typed a new search, and the answer to the old one must not
    /// overwrite it. That check is here rather than only at the caller because
    /// this type owns the query the answer has to match.
    ///
    /// On success the walk is placed at `cursor_id` — the match the cursor is
    /// on, or the nearest match in front of it, or the first — so that the next
    /// `n` moves on from where the reader is rather than restarting.
    pub fn adopt_server(
        &mut self,
        query: &str,
        ids: Vec<i64>,
        total: usize,
        cursor_id: Option<i64>,
    ) -> bool {
        if self.query.is_empty() || query != self.query {
            return false;
        }

        self.ids = capped(ids);
        self.total = total.max(self.ids.len());
        self.source = SearchSource::Server;
        self.in_flight = false;
        self.failure = None;
        self.notice = None;
        self.index = placement(&self.ids, cursor_id);

        true
    }

    /// The match the cursor should be moved onto after an adopt.
    ///
    /// `None` when the cursor is already on a match — the common case, and one
    /// that should not move the reader. Otherwise the nearest **preceding**
    /// match, which keeps the reader's sense of place and makes the next `n`
    /// move forward from where they are; or the first match, when the cursor is
    /// older than every one of them.
    #[must_use]
    pub fn landing(&self, cursor_id: Option<i64>) -> Option<i64> {
        let Some(cursor) = cursor_id else {
            return self.ids.first().copied();
        };

        if self.is_match(cursor) {
            return None;
        }

        self.ids
            .iter()
            .rev()
            .find(|id| **id <= cursor)
            .or_else(|| self.ids.first())
            .copied()
    }

    /// Walks forward to the next match, wrapping within the list.
    ///
    /// Not `Iterator::next`: the walk is cyclic, and the state is a cursor rather
    /// than a source of items. Implementing the trait would say this yields the
    /// matches once, which it does not.
    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn next(&mut self) -> Option<i64> {
        self.step(true)
    }

    /// Walks back to the previous match, wrapping within the list.
    #[must_use]
    pub fn prev(&mut self) -> Option<i64> {
        self.step(false)
    }

    fn step(&mut self, forward: bool) -> Option<i64> {
        if self.ids.is_empty() {
            self.index = None;
            return None;
        }

        let next = if forward {
            match self.index {
                Some(index) => (index + 1) % self.ids.len(),
                None => 0,
            }
        } else {
            match self.index {
                Some(0) | None => self.ids.len() - 1,
                Some(index) => index - 1,
            }
        };

        self.index = Some(next);
        self.ids.get(next).copied()
    }

    /// The match the walk is standing on, if it has moved.
    #[must_use]
    pub fn current(&self) -> Option<i64> {
        self.index.and_then(|index| self.ids.get(index).copied())
    }

    /// Where the current match sits in `window`, if it is loaded.
    ///
    /// The join between a match, which is a place in a conversation, and a row,
    /// which is a place in a window.
    #[must_use]
    pub fn resolve(&self, window: &ConversationWindow) -> Option<usize> {
        self.current().and_then(|id| window.position_of(id))
    }

    /// Whether `id` is one of the matches, for the highlight.
    #[must_use]
    pub fn is_match(&self, id: i64) -> bool {
        self.ids.binary_search(&id).is_ok()
    }

    /// Records a wrap-around so the label can announce it.
    ///
    /// `forward` is the direction of the walk that wrapped: [`SearchState::next`]
    /// running off the newest match continues at the oldest, and the other way
    /// round.
    pub fn note_wrap(&mut self, forward: bool) {
        self.notice = Some(if forward {
            "search hit BOTTOM, continuing at TOP".to_owned()
        } else {
            "search hit TOP, continuing at BOTTOM".to_owned()
        });
    }

    /// Forgets a wrap announcement.
    ///
    /// A step that did not wrap supersedes the announcement the last one made,
    /// so that the label describes where the reader is now rather than where
    /// they once were.
    pub fn clear_notice(&mut self) {
        self.notice = None;
    }

    /// What the status line says about the search.
    ///
    /// One function owns every wording, so the status line cannot drift from the
    /// state it describes. The local count is never presented as a final one: it
    /// is either *"searching…"* while an answer is coming, or explicitly a local
    /// list once one is not.
    #[must_use]
    pub fn label(&self) -> String {
        if !self.is_active() {
            return String::new();
        }

        let mut parts: Vec<String> = Vec::new();

        parts.push(match self.source {
            SearchSource::Server => self.server_position(),
            SearchSource::Local => self.local_position(),
        });

        if let Some(notice) = &self.notice {
            parts.push(notice.clone());
        }

        format!("/{} — {}", self.query, parts.join(" — "))
    }

    /// What a server list says about where the walk is and how much there is.
    fn server_position(&self) -> String {
        if self.ids.is_empty() {
            return "no matches".to_owned();
        }

        let position = match self.index {
            Some(index) => format!("match {} of {}", index + 1, self.total),
            None => format!("{} match(es)", self.total),
        };

        if self.total > self.ids.len() {
            format!("{position} (first {})", self.ids.len())
        } else {
            position
        }
    }

    /// What a local list says, which is never an answer.
    fn local_position(&self) -> String {
        let count = self.ids.len();

        if self.in_flight {
            return format!("{count} loaded — searching…");
        }

        match &self.failure {
            Some(reason) => format!("{count} loaded (search failed: {reason})"),
            None => format!("{count} loaded"),
        }
    }
}

/// Caps a list of matches at [`SEARCH_MATCHES`], keeping the oldest.
fn capped(mut ids: Vec<i64>) -> Vec<i64> {
    ids.truncate(SEARCH_MATCHES);
    ids
}

/// Where the walk starts in `ids` so that the first step continues from the
/// reader.
///
/// The index of the match the cursor is on, or of the nearest one in front of
/// it, or of the first — the same choice [`SearchState::landing`] makes for the
/// cursor, so the walk and the cursor agree about where "here" is.
fn placement(ids: &[i64], cursor_id: Option<i64>) -> Option<usize> {
    let Some(cursor) = cursor_id else {
        return (!ids.is_empty()).then_some(0);
    };

    let target = if ids.binary_search(&cursor).is_ok() {
        cursor
    } else {
        ids.iter()
            .rev()
            .find(|id| **id <= cursor)
            .or_else(|| ids.first())
            .copied()?
    };

    ids.iter().position(|id| *id == target)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A search answered from the window.
    fn local(query: &str, ids: &[i64]) -> SearchState {
        let mut state = SearchState::default();
        state.begin_local(query, ids.to_vec());
        state
    }

    /// A search answered by the server.
    fn server(query: &str, ids: &[i64], total: usize, cursor: Option<i64>) -> SearchState {
        let mut state = local(query, &[]);
        assert!(
            state.adopt_server(query, ids.to_vec(), total, cursor),
            "the server answer matches the query in flight"
        );
        state
    }

    // ---- the local predicate --------------------------------------------

    /// The absurdity the feature exists to remove: a substring that is not a
    /// word start does not match, here or on the server.
    #[test]
    fn a_query_does_not_match_mid_word() {
        assert!(
            !word_prefix_match("hello", "ell"),
            "`/ell` must not match `hello`, or the label would flip when the \
             server's answer arrives"
        );
        assert!(word_prefix_match("hello", "hell"));
        assert!(word_prefix_match("hello", "hello"));
        assert!(word_prefix_match("hello world", "wor"));
    }

    #[test]
    fn case_does_not_matter() {
        assert!(word_prefix_match("Hello", "hello"));
        assert!(word_prefix_match("hello", "HELL"));
        assert!(word_prefix_match("HELLO", "HeLL"));
    }

    #[test]
    fn a_word_start_is_after_anything_that_is_not_alphanumeric() {
        assert!(word_prefix_match("hello,world", "wor"));
        assert!(word_prefix_match("foo-bar", "bar"));
        assert!(word_prefix_match("  spaced", "spa"));
        assert!(
            !word_prefix_match("concatenate", "cat"),
            "`cat` is not a prefix of `concatenate`, whatever letters it holds"
        );
    }

    #[test]
    fn an_empty_query_matches_nothing() {
        assert!(!word_prefix_match("anything", ""));
        assert!(!word_prefix_match("", ""));
    }

    #[test]
    fn a_query_longer_than_the_word_it_starts_does_not_match() {
        assert!(
            !word_prefix_match("hello", "hellos"),
            "a word is a word, not a stem"
        );
    }

    // ---- walking the matches --------------------------------------------

    #[test]
    fn n_wraps_around_the_matches() {
        let mut state = local("x", &[2, 5, 8]);

        assert_eq!(state.next(), Some(2), "the first walk lands on the oldest");
        assert_eq!(state.next(), Some(5));
        assert_eq!(state.next(), Some(8));
        assert_eq!(state.next(), Some(2), "and wraps back to the oldest");
        assert_eq!(state.current(), Some(2));
    }

    #[test]
    fn big_n_wraps_backwards() {
        let mut state = local("x", &[2, 5, 8]);

        assert_eq!(
            state.prev(),
            Some(8),
            "the first walk back lands on the newest"
        );
        assert_eq!(state.prev(), Some(5));
        assert_eq!(state.prev(), Some(2));
        assert_eq!(state.prev(), Some(8), "and wraps forward to the newest");
    }

    #[test]
    fn walking_an_empty_list_moves_nothing() {
        let mut state = local("x", &[]);

        assert_eq!(state.next(), None);
        assert_eq!(state.prev(), None);
        assert_eq!(state.current(), None);
        assert_eq!(state.index(), None);
    }

    #[test]
    fn a_single_match_is_its_own_neighbour() {
        let mut state = local("x", &[7]);

        assert_eq!(state.next(), Some(7));
        assert_eq!(state.next(), Some(7));
        assert_eq!(state.prev(), Some(7));
    }

    // ---- adopting the server's answer -----------------------------------

    #[test]
    fn a_server_list_replaces_the_local_one_wholesale() {
        let mut state = local("x", &[1, 2, 3]);
        let replaced = state.adopt_server("x", vec![10, 20], 2, None);

        assert!(replaced);
        assert_eq!(
            state.ids(),
            &[10, 20],
            "not a union: the local list is gone"
        );
        assert_eq!(state.source(), SearchSource::Server);
        assert!(!state.in_flight());
    }

    /// The stale answer: the reader has typed another search and this one is for
    /// a question nobody is asking any more.
    #[test]
    fn a_result_for_a_replaced_query_is_refused() {
        let mut state = local("bar", &[1]);

        assert!(
            !state.adopt_server("foo", vec![9], 9, None),
            "an answer for `foo` must not overwrite `bar`'s list"
        );
        assert_eq!(state.ids(), &[1]);
        assert_eq!(state.source(), SearchSource::Local);
    }

    #[test]
    fn a_result_for_a_query_never_searched_is_refused() {
        let mut state = SearchState::default();

        assert!(!state.adopt_server("foo", vec![9], 9, None));
        assert_eq!(state.ids(), &[]);
    }

    /// The cursor is on a match the server confirmed: the walk continues from
    /// there, and the reader is not moved.
    #[test]
    fn adopting_from_a_cursor_on_a_match_keeps_the_reader_where_they_are() {
        let mut state = server("x", &[10, 20, 30], 3, Some(20));

        assert_eq!(
            state.landing(Some(20)),
            None,
            "the cursor is already a match"
        );
        assert_eq!(state.current(), Some(20));
        assert_eq!(state.next(), Some(30), "the next match is the one after it");
    }

    /// The cursor is on something the server did not confirm: it moves to the
    /// nearest match in front, and the next `n` moves forward from there.
    #[test]
    fn adopting_from_an_unconfirmed_cursor_lands_on_the_match_before_it() {
        let mut state = server("x", &[10, 20, 30], 3, Some(25));

        assert_eq!(state.landing(Some(25)), Some(20));
        assert_eq!(state.current(), Some(20));
        assert_eq!(state.next(), Some(30), "forward from where the reader is");
    }

    /// The cursor is older than every match: the nearest in front does not
    /// exist, so the first match answers.
    #[test]
    fn adopting_from_before_every_match_lands_on_the_first() {
        let mut state = server("x", &[10, 20, 30], 3, Some(5));

        assert_eq!(state.landing(Some(5)), Some(10));
        assert_eq!(state.current(), Some(10));
        assert_eq!(state.next(), Some(20));
    }

    #[test]
    fn landing_does_nothing_when_the_server_matched_nothing() {
        let state = server("x", &[], 0, Some(5));

        assert_eq!(state.landing(Some(5)), None);
        assert_eq!(state.current(), None);
    }

    // ---- what the label says --------------------------------------------

    #[test]
    fn the_local_label_is_never_a_final_count() {
        let state = local("word", &[1, 2, 3]);

        assert_eq!(
            state.label(),
            "/word — 3 loaded — searching…",
            "a count from the loaded window is provisional until the server speaks"
        );
    }

    #[test]
    fn a_server_label_says_where_the_walk_is_and_how_much_there_is() {
        let mut state = server("word", &[10, 20, 30], 1243, Some(20));

        assert_eq!(state.label(), "/word — match 2 of 1243 (first 3)");

        let _ = state.next();
        assert_eq!(state.label(), "/word — match 3 of 1243 (first 3)");
    }

    #[test]
    fn a_complete_server_list_needs_no_truncation_note() {
        let state = server("word", &[10, 20, 30], 3, Some(20));

        assert_eq!(state.label(), "/word — match 2 of 3");
    }

    #[test]
    fn a_server_list_with_no_matches_says_so() {
        let state = server("word", &[], 0, None);

        assert_eq!(state.label(), "/word — no matches");
    }

    #[test]
    fn a_settled_local_list_says_what_it_is() {
        let mut state = local("word", &[1, 2]);
        state.finish_local();

        assert_eq!(state.label(), "/word — 2 loaded");
    }

    #[test]
    fn a_failed_server_pass_says_why_and_keeps_the_local_list() {
        let mut state = local("word", &[1, 2]);
        state.fail("flood wait, retry in 42s");

        assert_eq!(
            state.label(),
            "/word — 2 loaded (search failed: flood wait, retry in 42s)"
        );
        assert_eq!(state.ids(), &[1, 2], "the local list stands");
    }

    #[test]
    fn a_wrap_is_announced_in_the_direction_that_wrapped() {
        let mut state = server("word", &[10, 20], 2, Some(10));

        state.note_wrap(true);
        assert_eq!(
            state.label(),
            "/word — match 1 of 2 — search hit BOTTOM, continuing at TOP"
        );

        state.note_wrap(false);
        assert_eq!(
            state.label(),
            "/word — match 1 of 2 — search hit TOP, continuing at BOTTOM"
        );
    }

    #[test]
    fn a_new_search_clears_a_stale_wrap_notice() {
        let mut state = local("x", &[1]);
        state.note_wrap(true);

        state.begin_local("y", vec![2]);

        assert!(
            !state.label().contains("hit"),
            "the notice belonged to the search before: {}",
            state.label()
        );
    }

    // ---- the cap, the join, and clearing --------------------------------

    /// The bound is not about memory, but the state still holds no more than it:
    /// a list that grew with the conversation would be a second window.
    #[test]
    fn a_list_longer_than_the_bound_is_capped() {
        let bound = i64::try_from(SEARCH_MATCHES).expect("the bound fits an identifier");
        let ids: Vec<i64> = (0..bound + 25).collect();
        let state = local("x", &ids);

        assert_eq!(state.len(), SEARCH_MATCHES);
        assert_eq!(
            state.total(),
            SEARCH_MATCHES,
            "the local total is its own list"
        );

        let mut served = local("x", &[]);
        served.adopt_server("x", ids, 500, None);
        assert_eq!(served.len(), SEARCH_MATCHES);
        assert_eq!(
            served.total(),
            500,
            "but the server's total is the conversation's"
        );
    }

    #[test]
    fn a_match_is_recognised_for_the_highlight() {
        let state = server("x", &[10, 20, 30], 3, None);

        assert!(state.is_match(20));
        assert!(
            !state.is_match(15),
            "a message that did not match is not marked"
        );
        assert!(!state.is_match(99));
    }

    #[test]
    fn clearing_forgets_everything() {
        let mut state = server("x", &[10, 20], 2, Some(10));
        state.note_wrap(true);

        state.clear();

        assert!(!state.is_active());
        assert_eq!(state.query(), None);
        assert_eq!(state.ids(), &[]);
        assert_eq!(state.total(), 0);
        assert!(!state.in_flight());
        assert_eq!(state.label(), "");
    }
}
