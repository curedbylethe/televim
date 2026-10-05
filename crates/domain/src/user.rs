//! Finding a person to start a conversation with.
//!
//! A search for a name or a username is answered from the network, but the
//! result the interface holds is plain data: [`UserCandidate`] is one person the
//! server offered, and [`UserSearchState`] is the results list — the query, the
//! candidates, and where the reader is among them. Neither knows about
//! `grammers`, the wire, or a terminal; STAGE-03 converts one into the other,
//! STAGE-04 draws the state, and STAGE-05 turns a choice into an open
//! conversation.
//!
//! # Why a plain selection, not [`VimState`](crate::vim::VimState)
//!
//! The selection here is a plain `usize` wrapping at both ends, the way
//! `emoji::Trigger` selects from a completion popup, rather than a Vim cursor.
//! The list is short and transient: it opens on a fresh query, holds a handful
//! of names, and closes when the reader chooses one. The established gesture for
//! a list shaped like that — the `:` completion popup — is a wrapping
//! selection, not a cursor with motions and a visual mode. [`VimState`] remains
//! the model for the persistent chat list, which is a different kind of thing
//! and is where a cursor earns its keep.
//!
//! [`VimState`]: crate::vim::VimState
//!
//! # The stale answer
//!
//! A search is asynchronous, and the reader can type another query before the
//! first one is answered. [`UserSearchState::adopt`] therefore refuses a result
//! whose query is no longer the one being searched for, exactly as
//! [`SearchState::adopt_server`](crate::search::SearchState::adopt_server) does:
//! the type owns the query, so the check lives with it rather than with every
//! caller.

/// One person the server offered for a search.
///
/// The `user_id` is the peer's identifier, which is also the identifier of the
/// private chat with them; a caller uses it to open or focus that conversation
/// without another lookup. `username` is what the reader may type to find the
/// person exactly, and is absent for someone who has not set one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserCandidate {
    /// The peer's identifier, and the identifier of the chat with them.
    pub user_id: i64,

    /// The name shown for the peer.
    pub display_name: String,

    /// The peer's `@username`, when they have one.
    pub username: Option<String>,
}

/// The results of the last user search, and where the reader is among them.
///
/// Pure state: no clock, no client, no error type. The query is an
/// `Option<String>` so that "no search" is a state a caller has to handle rather
/// than one an empty string silently becomes, and a failure is a plain
/// [`String`] — the reason a lookup gave, kept only so the label can say why the
/// list is empty.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UserSearchState {
    /// The query, or `None` when no search has been run.
    query: Option<String>,

    /// The people the search offered, best first as the server ranked them.
    candidates: Vec<UserCandidate>,

    /// Which candidate the reader is on, always inside `candidates` or zero on
    /// an empty list.
    selected: usize,

    /// Whether an answer is still on its way.
    in_flight: bool,

    /// Why the lookup failed, if it did.
    failure: Option<String>,
}

impl UserSearchState {
    /// A state with no search at all.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a search: records the query, clears the old list, and marks the
    /// answer as pending.
    ///
    /// Clearing rather than keeping the previous candidates means the list on
    /// screen always belongs to the query in the prompt; a stale result cannot
    /// linger under a new one while the network thinks.
    pub fn begin(&mut self, query: &str) {
        self.query = Some(query.to_owned());
        self.candidates.clear();
        self.selected = 0;
        self.in_flight = true;
        self.failure = None;
    }

    /// Fills the list with the answer to `query`, if it is still wanted.
    ///
    /// Returns `false` when `query` is not the one being searched for: the
    /// reader has typed another query, and the answer to the old one must not
    /// overwrite it. That check is here rather than only at the caller because
    /// this type owns the query the answer has to match.
    pub fn adopt(&mut self, query: &str, candidates: Vec<UserCandidate>) -> bool {
        if self.query.as_deref() != Some(query) {
            return false;
        }

        self.candidates = candidates;
        self.selected = 0;
        self.in_flight = false;
        self.failure = None;

        true
    }

    /// Records that the lookup failed.
    ///
    /// The query stands, so the label can still name what was searched for;
    /// only the pending answer and the list's empty state change. The reason
    /// travels with the state so the label can say why there is nothing to show.
    pub fn fail(&mut self, reason: String) {
        self.in_flight = false;
        self.failure = Some(reason);
    }

    /// Forgets the search entirely, which is what closing the surface does.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Whether a search has been started and not cleared.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.query.is_some()
    }

    /// The query being searched for, if there has been one.
    #[must_use]
    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    /// The people the search offered, best first.
    #[must_use]
    pub fn candidates(&self) -> &[UserCandidate] {
        &self.candidates
    }

    /// How many candidates are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    /// Whether the search offered nobody.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    /// Where the selection stands.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The candidate the reader is on, if the list has one.
    #[must_use]
    pub fn selected_candidate(&self) -> Option<&UserCandidate> {
        self.candidates.get(self.selected)
    }

    /// Moves the selection `delta` candidates, wrapping at both ends.
    ///
    /// Wrapping rather than clamping: the list is short, and a reader who has
    /// run off one end is reaching for the other. A move on an empty list does
    /// nothing.
    pub fn move_selection(&mut self, delta: isize) {
        let len = self.candidates.len();
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

    /// What the status line says about the search.
    ///
    /// One function owns every wording, so the status line cannot drift from the
    /// state it describes. The shape follows
    /// [`SearchState::label`](crate::search::SearchState::label): `/query — …`,
    /// with `searching…` while an answer is pending and the failure reason when
    /// one arrived instead. The design run is authoritative for the final
    /// wording; this keeps the shape.
    #[must_use]
    pub fn label(&self) -> String {
        let Some(query) = self.query() else {
            return String::new();
        };

        let count = self.candidates.len();
        let noun = if count == 1 {
            "candidate"
        } else {
            "candidates"
        };

        let description = if self.in_flight {
            if count == 0 {
                "searching…".to_owned()
            } else {
                format!("{count} {noun} — searching…")
            }
        } else {
            match (count, &self.failure) {
                (0, None) => "no candidates".to_owned(),
                (0, Some(reason)) => format!("no candidates (search failed: {reason})"),
                (_, Some(reason)) => format!("{count} {noun} (search failed: {reason})"),
                (_, None) => format!("{count} {noun}"),
            }
        };

        format!("/{query} — {description}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A candidate with the given identifier.
    fn candidate(user_id: i64) -> UserCandidate {
        UserCandidate {
            user_id,
            display_name: format!("user-{user_id}"),
            username: Some(format!("user{user_id}")),
        }
    }

    #[test]
    fn a_search_begins_in_flight_with_the_query_and_no_candidates() {
        let mut state = UserSearchState::new();
        state.begin("ada");

        assert!(state.is_active());
        assert_eq!(state.query(), Some("ada"), "the query is recorded");
        assert!(state.is_empty(), "the old list is cleared at the start");
        assert_eq!(state.selected(), 0);
        assert_eq!(state.label(), "/ada — searching…", "an answer is pending");
    }

    /// The stale answer: the reader has typed another query and this one is for
    /// a question nobody is asking any more.
    #[test]
    fn a_result_for_a_replaced_query_is_refused() {
        let mut state = UserSearchState::new();
        state.begin("bar");

        assert!(
            !state.adopt("foo", vec![candidate(1)]),
            "an answer for `foo` must not overwrite `bar`'s list"
        );
        assert!(state.is_empty());
        assert_eq!(state.query(), Some("bar"));
    }

    #[test]
    fn a_result_for_a_query_never_searched_is_refused() {
        let mut state = UserSearchState::new();

        assert!(!state.adopt("foo", vec![candidate(1)]));
        assert!(state.is_empty());
    }

    #[test]
    fn adopting_the_matching_answer_fills_the_list_and_settles_the_search() {
        let mut state = UserSearchState::new();
        state.begin("ada");

        assert!(state.adopt("ada", vec![candidate(1), candidate(2)]));

        assert_eq!(state.len(), 2);
        assert_eq!(state.candidates()[0].user_id, 1);
        assert!(!state.in_flight);
        assert_eq!(state.selected(), 0);
    }

    #[test]
    fn failing_keeps_the_query_and_records_why() {
        let mut state = UserSearchState::new();
        state.begin("ada");
        state.fail("flood wait".to_owned());

        assert_eq!(state.query(), Some("ada"));
        assert!(state.is_empty(), "the list is untouched");
        assert_eq!(
            state.label(),
            "/ada — no candidates (search failed: flood wait)"
        );
    }

    #[test]
    fn clearing_forgets_everything() {
        let mut state = UserSearchState::new();
        state.begin("ada");
        state.adopt("ada", vec![candidate(1)]);

        state.clear();

        assert!(!state.is_active());
        assert_eq!(state.query(), None);
        assert!(state.is_empty());
        assert_eq!(state.label(), "");
    }

    // ---- moving within the list -----------------------------------------

    #[test]
    fn move_selection_wraps_around_the_candidates() {
        let mut state = UserSearchState::new();
        state.begin("x");
        state.adopt("x", vec![candidate(10), candidate(20), candidate(30)]);

        state.move_selection(1);
        assert_eq!(state.selected(), 1);
        state.move_selection(1);
        assert_eq!(state.selected(), 2);
        state.move_selection(1);
        assert_eq!(state.selected(), 0, "and wraps back to the first");
    }

    #[test]
    fn move_selection_wraps_backwards() {
        let mut state = UserSearchState::new();
        state.begin("x");
        state.adopt("x", vec![candidate(10), candidate(20), candidate(30)]);

        state.move_selection(-1);

        assert_eq!(state.selected(), 2, "up from the first wraps to the last");
    }

    #[test]
    fn move_selection_stays_in_range_for_a_multi_step_move() {
        let mut state = UserSearchState::new();
        state.begin("x");
        state.adopt("x", vec![candidate(10), candidate(20), candidate(30)]);

        state.move_selection(5);
        assert_eq!(state.selected(), 2, "five steps over three wraps once");

        state.move_selection(-5);
        assert_eq!(state.selected(), 0, "and five back returns to the first");
    }

    #[test]
    fn move_selection_leaves_an_empty_list_alone() {
        let mut state = UserSearchState::new();
        state.begin("x");

        state.move_selection(1);
        state.move_selection(-1);

        assert_eq!(state.selected(), 0, "there is nothing to select");
        assert_eq!(state.selected_candidate(), None);
    }

    // ---- selecting and labelling ----------------------------------------

    #[test]
    fn a_single_candidate_is_still_selectable() {
        let mut state = UserSearchState::new();
        state.begin("x");
        state.adopt("x", vec![candidate(7)]);

        assert_eq!(state.selected_candidate().map(|c| c.user_id), Some(7));

        state.move_selection(1);
        assert_eq!(state.selected(), 0, "one candidate is its own neighbour");
    }

    #[test]
    fn the_label_is_empty_when_no_search_is_active() {
        assert_eq!(UserSearchState::new().label(), "");
    }

    #[test]
    fn the_label_names_the_query_and_the_candidate_count() {
        let mut state = UserSearchState::new();
        state.begin("ada");
        state.adopt("ada", vec![candidate(1), candidate(2), candidate(3)]);

        assert_eq!(state.label(), "/ada — 3 candidates");
    }

    #[test]
    fn the_label_names_a_lonely_candidate() {
        let mut state = UserSearchState::new();
        state.begin("ada");
        state.adopt("ada", vec![candidate(1)]);

        assert_eq!(state.label(), "/ada — 1 candidate");
    }

    #[test]
    fn the_label_says_a_search_is_still_in_flight() {
        let mut state = UserSearchState::new();
        state.begin("ada");

        assert_eq!(state.label(), "/ada — searching…");
    }

    #[test]
    fn the_label_says_an_answer_found_nobody() {
        let mut state = UserSearchState::new();
        state.begin("ada");
        state.adopt("ada", Vec::new());

        assert_eq!(state.label(), "/ada — no candidates");
    }
}
