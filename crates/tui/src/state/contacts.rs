//! The contacts list: a directory of the account's people, not a search.
//!
//! Its own state rather than a second [`UserSearchState`](domain::user::UserSearchState),
//! because the two answer different questions: a search is driven by a query the
//! reader is typing and can be replaced by the next keystroke, while the directory
//! is fetched once and stays until it is closed. Sharing one type would give the
//! directory a query it does not have and the search a lifetime it does not keep.

use domain::user::UserCandidate;

/// An open contacts list, and where the reader is among the people in it.
#[derive(Debug, Default)]
pub struct ContactListState {
    /// Whether the overlay is up. An empty list is still up: "no contacts" is an
    /// answer, and a list that closed itself would look like a key that failed.
    open: bool,

    items: Vec<UserCandidate>,
    selected: usize,

    /// Whether a fetch is on its way. The label says so, because an empty list
    /// while the answer is in flight is not the same as an empty directory.
    in_flight: bool,

    /// Why the last fetch failed, until the next one starts.
    failure: Option<String>,
}

impl ContactListState {
    /// Opens the list and marks a fetch in flight.
    ///
    /// The last answer stays: a fetch that fails must not take a directory the
    /// reader already had.
    pub fn begin(&mut self) {
        self.open = true;
        self.in_flight = true;
        self.failure = None;
    }

    /// Takes the answer to the fetch that is in flight.
    pub fn adopt(&mut self, items: Vec<UserCandidate>) {
        self.in_flight = false;
        self.failure = None;
        self.items = items;
        self.selected = 0;
    }

    /// Records a fetch that failed. The list stays open so the reason is read.
    pub fn fail(&mut self, reason: String) {
        self.in_flight = false;
        self.failure = Some(reason);
    }

    /// Puts the list away, keeping the last answer for the next open.
    pub fn close(&mut self) {
        self.open = false;
        self.in_flight = false;
        self.failure = None;
    }

    /// Whether the list is on show.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.open
    }

    /// The people in the directory, in Telegram's order.
    #[must_use]
    pub fn items(&self) -> &[UserCandidate] {
        &self.items
    }

    /// Where the cursor is, or `0` when the list is empty.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The person the cursor is on.
    #[must_use]
    pub fn selected_candidate(&self) -> Option<&UserCandidate> {
        self.items.get(self.selected)
    }

    /// Moves the cursor by `delta`, stopping at either end.
    pub fn move_selection(&mut self, delta: isize) {
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.items.len().saturating_sub(1));
    }

    /// The sentence the status line shows while the list is up.
    #[must_use]
    pub fn label(&self) -> String {
        if let Some(reason) = &self.failure {
            return format!("contacts: {reason}");
        }
        if self.in_flight {
            return "contacts — searching…".to_owned();
        }
        format!("contacts — {}", self.items.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: i64) -> UserCandidate {
        UserCandidate {
            user_id: id,
            display_name: format!("person-{id}"),
            username: None,
        }
    }

    #[test]
    fn a_fetch_in_flight_is_not_an_empty_directory() {
        let mut list = ContactListState::default();
        list.begin();
        assert!(list.is_active());
        assert_eq!(list.label(), "contacts — searching…");

        list.adopt(vec![candidate(1), candidate(2)]);
        assert_eq!(list.label(), "contacts — 2");
    }

    #[test]
    fn a_failure_is_named_and_the_list_stays_up() {
        let mut list = ContactListState::default();
        list.begin();
        list.fail("no connection".to_owned());
        assert!(list.is_active());
        assert_eq!(list.label(), "contacts: no connection");
    }

    #[test]
    fn the_cursor_stops_at_either_end() {
        let mut list = ContactListState::default();
        list.begin();
        list.adopt(vec![candidate(1), candidate(2)]);
        list.move_selection(-1);
        assert_eq!(list.selected(), 0);
        list.move_selection(5);
        assert_eq!(list.selected(), 1);
    }

    #[test]
    fn a_failed_refetch_leaves_the_previous_directory() {
        let mut list = ContactListState::default();
        list.begin();
        list.adopt(vec![candidate(1)]);
        list.close();
        assert!(!list.is_active());

        list.begin();
        list.fail("flood wait, retry in 20s".to_owned());
        assert_eq!(list.items().len(), 1);
        assert_eq!(list.label(), "contacts: flood wait, retry in 20s");
    }
}
