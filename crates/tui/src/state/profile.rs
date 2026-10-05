//! The profile card's buffer and its per-keystroke state.

use domain::vim::VimState;

use crate::app::{ContactProfile, ProfileId};

/// The profile card's buffer and its per-keystroke state.
pub struct ProfileCard {
    /// The highlight on the profile panel's rows.
    ///
    /// A second [`VimState`] rather than a share of the conversation's, because
    /// that one's total is the conversation window's length: driving two lists
    /// from one value means each resize moves the other's cursor. `VimState`
    /// knows nothing about what an item is, which is what makes the second one
    /// free.
    pub(crate) profile_vim: VimState,

    /// The subject the card is about: the account, or a contact by chat.
    ///
    /// A `ProfileId::User` already existed and was unreachable, and a chat is the
    /// handle a reader can actually name — a conversation on show is a person, and
    /// the chat list is where they were found.
    pub(crate) profile_subject: ProfileId,

    /// Where the inline position is within the cursor row's value.
    ///
    /// A **character** count, because every motion that produces one counts
    /// characters; it becomes a byte offset only where it is drawn, by
    /// [`rows::byte_span`]. Separate from [`VimState`] because that moves between
    /// rows and knows nothing about what a row is.
    pub(crate) profile_caret: usize,

    /// A card selection's fixed end, when there is one.
    ///
    /// A [`Mark`] and not a row index, because the card's two ends are the same
    /// two ends the conversation has: a row and a position within it, or the whole
    /// of it. Reusing the type is what makes `v` one keystroke here as it is
    /// there, rather than a second selection model beside the first.
    pub(crate) profile_visual: Option<domain::selection::Mark>,

    /// The contact whose profile the card on show is about, if it is about one.
    ///
    /// One at a time rather than one per person: a card is opened, read and
    /// closed, so a map would be a cache with no reader. The `peer_id` is what
    /// lets an answer be matched back to the card that asked for it.
    pub(crate) contact: Option<ContactProfile>,

    /// A count typed before a motion, as `12j` means twelve.
    pub(crate) profile_count: Option<u32>,

    /// `Ctrl-w` was pressed on a card and the next key is its argument.
    ///
    /// The pane's `h`/`l` became an inline motion, so pane navigation moved under
    /// a prefix. Bare `Ctrl-w` keeps its existing meaning — leaving the input line
    /// — which is the same prefix-with-a-bare-fallback shape `g`/`gg` has.
    pub(crate) profile_pending_w: bool,
}

impl ProfileCard {
    /// The account's own card, unread, with the caret at the start.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            profile_vim: VimState::new(0),
            profile_subject: ProfileId::SelfAccount,
            profile_caret: 0,
            profile_visual: None,
            contact: None,
            profile_count: None,
            profile_pending_w: false,
        }
    }
}
