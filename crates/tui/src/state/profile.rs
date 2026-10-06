//! The profile card's buffer and its per-keystroke state.

use domain::account::Account;
use domain::selection::Mark;
use domain::vim::{CharMotion, VimState, char_motion};

use crate::app::{AccountState, ContactProfile, ProfileId};

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

    /// Opens the card about `subject`, holding `contact` for a contact's card.
    ///
    /// The highlight starts at the top and the inline position at the start of
    /// the first value, every time. The caller sizes the highlight over the
    /// rows drawn afterwards, because the rows are drawn from the contact this
    /// just stored.
    pub(crate) fn open(&mut self, subject: ProfileId, contact: Option<ContactProfile>) {
        self.profile_subject = subject;
        self.contact = contact;
        self.profile_vim = VimState::new(0);
        self.profile_caret = 0;
        self.profile_visual = None;
        self.profile_count = None;
    }

    /// Sizes the highlight over `navigable` rows.
    ///
    /// Over the rows that are drawn, not over every row there is: a held slot
    /// at the end of a card is not somewhere the highlight goes. Zero when no
    /// card is on show.
    pub(crate) fn resize(&mut self, navigable: usize) {
        self.profile_vim = VimState::new(navigable);
    }

    /// Re-sizes the highlight over the rows now drawn, keeping the reader's place.
    ///
    /// Re-clamping rather than resetting keeps the reader where they were, which
    /// here is the top: the row count just changed under the highlight.
    pub(crate) fn retotal(&mut self, navigable: usize) {
        self.profile_vim.set_total(navigable);
    }

    /// Records what a contact's profile read came back with.
    ///
    /// The only writer of a contact's state, for the same reason the session is
    /// the only writer of the account's: three states that cannot be mixed up by
    /// a caller that knows only one of them.
    ///
    /// Reports whether the card is on show for `peer_id`. An answer about
    /// somebody else is dropped: the reader may open a second card while the
    /// first read is in flight, and the second card's fields are not the first
    /// one's.
    pub(crate) fn adopt_contact(&mut self, peer_id: i64, profile: Result<Account, String>) -> bool {
        let Some(open) = self.contact.as_ref().filter(|open| open.peer_id == peer_id) else {
            return false;
        };
        self.contact = Some(ContactProfile {
            peer_id: open.peer_id,
            state: match profile {
                Ok(account) => AccountState::Known(account),
                Err(reason) => AccountState::Unavailable(reason),
            },
        });
        true
    }

    /// Moves the card's highlight to `row`, for a caller that knows where it wants
    /// the cursor rather than which key gets it there.
    ///
    /// A test helper *and* the reason the two are separate: a card's cursor is
    /// `j`/`k` and a position is `l`/`h`, and every test that cares about the
    /// second has to walk to the first to get there.
    #[cfg(test)]
    pub(crate) fn handle_card_row(&mut self, row: usize) {
        self.profile_vim.set_cursor(row);
    }

    /// Starts a card selection at the inline position, or extends the one there is.
    pub(crate) fn start_visual(&mut self, row_id: i64) {
        if self.profile_visual.is_none() {
            self.profile_visual = Some(Mark::text(row_id, self.profile_caret));
        }
    }

    /// Drops the card selection, keeping the inline position.
    pub(crate) fn clear_visual(&mut self) {
        self.profile_visual = None;
    }

    /// Drops the selection and the inline position, and forgets any count.
    ///
    /// All three describe a card that is no longer on show: a position inside a
    /// value means nothing in a conversation, and leaving it behind would be a
    /// caret waiting to be drawn on the wrong surface.
    pub(crate) fn clear_transient(&mut self) {
        self.profile_visual = None;
        self.profile_caret = 0;
        self.profile_count = None;
    }

    /// Puts the inline position at `caret` within the cursor row's value.
    pub(crate) fn set_caret(&mut self, caret: usize) {
        self.profile_caret = caret;
    }

    /// A motion within the cursor row's value over `value`: `l`/`h`, a word
    /// motion, or a bound.
    ///
    /// Every one of them is clamped to the value rather than crossing into the
    /// next row, and that is not Vim's rule — in Vim `w` at the end of a buffer
    /// wraps. Crossing is what `j` is for, and a motion that silently changes
    /// *what it selects* is the worst failure mode a selection has.
    pub(crate) fn move_char(&mut self, motion: CharMotion, value: &str) {
        self.profile_caret = char_motion(value, self.profile_caret, motion);
    }

    /// A count before a motion, as `12j` means twelve.
    ///
    /// The digits are otherwise unbound on a card, so a count costs one line rather
    /// than shadowing a key that means something else.
    pub(crate) fn card_count(&mut self, c: char) {
        let digit = u32::from(c);
        self.profile_count = Some(self.profile_count.unwrap_or(0) * 10 + digit);
    }
}
