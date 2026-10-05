//! The open conversation's view, its editing and selection registers, and its search surfaces.

use domain::history::ConversationView;
use domain::search::SearchState;
use domain::selection::Selection;
use domain::user::UserSearchState;
use domain::vim::VimState;

use crate::app::{ConfirmKind, Register};
use crate::jumplist::Jumplist;

pub struct ConversationState {
    /// The conversation on show, and where the reader is in it.
    ///
    /// The panel renders a slice of this, and the cursor below is the reader's
    /// place within it. Nothing here holds a whole conversation: the window is
    /// the ceiling on what the open chat costs.
    pub conversation: ConversationView,

    pub vim: VimState,

    /// Set by `/` search: the query text, the matches, and where the walk is.
    ///
    /// One value rather than a list beside a query: the label, the highlight and
    /// `n`/`N` all read the same state, and keeping them apart would let the
    /// three disagree about which list is on screen.
    pub(crate) search: SearchState,

    /// Set by a new-conversation search: the query, the people found, and where
    /// the reader is among them.
    ///
    /// Its own state rather than reusing [`App::search`], which is scoped to the
    /// open conversation: this one is about the chat list, and the two can be
    /// live at once without either overwriting the other.
    pub(crate) user_search: UserSearchState,

    /// What the reader last yanked.
    ///
    /// A yank is about *this* conversation and does not follow the reader into
    /// another one: carrying it across would be a feature nobody asked for and
    /// would need its own answer about whether it survives the change. So
    /// [`App::select_chat_none`] forgets it along with everything else.
    pub(crate) register: Register,

    /// What the reader has selected over the messages, if anything.
    ///
    /// `None` outside a selection — and a selection with no mode of its own: the
    /// conversation's [`Mode`] says whether a key is being applied to it, and a
    /// `dd` puts one here for as long as the prompt is up without ever asking
    /// for Visual.
    ///
    /// Both of its ends name a message by identifier, which is what lets it
    /// survive a page landing: see [`App::after_window_change`].
    pub(crate) selection: Option<Selection>,

    /// The message the next composed message answers, if it is a reply.
    pub reply_to: Option<i64>,

    /// The message the buffer is editing, if it is an edit.
    pub editing: Option<i64>,

    /// The placeholder of the send in flight, if one is.
    ///
    /// An `Option` rather than a flag so that a result is matched to the send it
    /// answers: releasing the gate for a send that is no longer in flight is a
    /// no-op, and a duplicate result cannot release a later send's gate.
    pub sending: Option<i64>,

    /// The deletion waiting to be confirmed, if one is.
    pub confirm: Option<ConfirmKind>,

    /// Where the reader was before each jump they have taken.
    ///
    /// What `Ctrl-o` and `Ctrl-i` walk. It is per conversation and keyed by
    /// message identifier rather than by row, because a jump replaces the window
    /// and a row means a different message on either side of that.
    pub(crate) jumplist: Jumplist,
}

impl ConversationState {
    /// An empty conversation: nothing open, nothing selected, nothing in flight.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            conversation: ConversationView::new(0),
            vim: VimState::new(0),
            search: SearchState::default(),
            user_search: UserSearchState::default(),
            register: Register::default(),
            selection: None,
            reply_to: None,
            editing: None,
            sending: None,
            confirm: None,
            jumplist: Jumplist::default(),
        }
    }
}
