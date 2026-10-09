//! Top-level TUI state.

use std::ops::Range;
use std::time::{Duration, Instant};

#[cfg(test)]
use std::borrow::Cow;
use std::path::PathBuf;

use crossterm::event::KeyEvent;
#[cfg(test)]
use crossterm::event::{KeyCode, KeyModifiers};
use domain::account::Account;
use domain::chat::Chat;
#[cfg(test)]
use domain::history::{CONVERSATION_WINDOW, ConversationWindow};
use domain::message::Message;
#[cfg(test)]
use domain::message::MessageStatus;
use domain::presence::Presence;
use domain::search::SearchState;
use domain::selection::{Mark, Selection};
use domain::updates::UpdateEvent;
use domain::user::{UserCandidate, UserSearchState};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};

use crate::bidi::BidiMode;
use crate::emoji;
use crate::event::{AppAction, key_to_action};
use crate::rows::{self, Reserved, RowKind, RowSpan, Slice};
use crate::state::chat_list::ChatListState;
pub use crate::state::connection::ConnectionState;
use crate::state::conversation::ConversationState;
use crate::state::coordinate;
use crate::state::drafts::DraftStore;
use crate::state::input::InputState;
use crate::state::outbox::Outbox;
use crate::state::pending::Pending;
use crate::state::profile::ProfileCard;
use crate::state::session::SessionState;
use crate::state::ui::{GraphicsMode, IDLE_STATUS, StickerMode, UiState};
use crate::widgets;

/// How close to an end of the loaded messages the cursor has to get before the
/// page beyond it is worth asking for.
///
/// A margin rather than the edge itself, because a fetch costs a round trip:
/// asking a screenful early means the reader reaches the end of what is loaded
/// with the next page already on its way. Counted in rows, because a reader
/// scrolling upwards is counting the screen: twenty messages that came to fill
/// four rows is four rows from the top, not twenty.
const FETCH_MARGIN: usize = 20;

/// How long the highlight has to stay put before its conversation is opened.
///
/// A reader holding `j` moves the highlight far faster than a page can be
/// fetched, and opening on every position would fetch every conversation they
/// scrolled past. Long enough to outlast the gap between two autorepeats of a
/// held key, short enough that a deliberate choice is not left waiting: the tick
/// that calls [`App::take_pending_chat`] runs four times a second, so a real
/// press is open within a third of a second of it.
///
/// Public because the caller is the one that has to wait: the number is the
/// worst-case delay between a reader's keypress and the conversation opening, and
/// a caller reasoning about that latency needs to be able to read it.
pub const CHAT_SWITCH_DELAY: Duration = Duration::from_millis(150);

/// How long a transient status stays on the line before it reverts.
///
/// Only things that expire on their own are transient — a send or edit failure,
/// a refusal. State the reader must not lose is written straight to the status
/// and never carries a deadline.
pub(crate) const FLASH_FOR: Duration = Duration::from_secs(5);

/// How long the peer is shown as typing after the last sign of it.
///
/// The design's `TYPING_TICKS` re-expressed as a wall-clock window, because the
/// tick it was counted on is the network's and says nothing about time. Bounded
/// so a peer who stops without a final event vanishes rather than typing forever.
///
/// Re-armed by every repeat of the event, which is how a peer who keeps typing
/// past this stays shown.
pub(crate) const TYPING_FOR: Duration = Duration::from_secs(6);

/// How many operations the reader may have queued at once.
///
/// The queue exists because a second request must not replace one that has not
/// gone out yet: sending and then pressing `/` inside one tick would otherwise
/// drop the send, silently. It is drained on every pass, so the bound is only
/// reached by a burst — it is a ceiling on memory rather than a schedule.
pub(crate) const ACTION_QUEUE: usize = 4;

/// The prompt the status line shows when a deletion of the reader's own message
/// is waiting to be confirmed.
pub const DELETE_OUTGOING_PROMPT: &str = "Delete your message from both sides? (y/n)";

/// The prompt the status line shows when a deletion of the other side's message
/// is waiting to be confirmed.
///
/// "from" rather than "for" on purpose: the reader is removing something from a
/// record they do not solely own, and that asymmetry is the thing the wording
/// has to carry.
pub const DELETE_INCOMING_PROMPT: &str = "Delete their message from both sides? (y/n)";

/// The prompt for a deletion of more than one of the reader's own messages.
///
/// A function rather than a constant because the count is in it, and a single
/// deletion says [`DELETE_OUTGOING_PROMPT`] instead: "Delete 1 of your messages"
/// is worse English than one deletion deserves, and there is no reason to make a
/// single removal sound like a bulk one.
#[must_use]
pub fn delete_yours_prompt(count: usize) -> String {
    format!("Delete {count} of your messages from both sides? (y/n)")
}

/// The prompt for a deletion of more than one of the other side's messages.
#[must_use]
pub fn delete_theirs_prompt(count: usize) -> String {
    format!("Delete {count} of their messages from both sides? (y/n)")
}

/// The prompt for a deletion that spans both sides.
///
/// The two simpler prompts differ only in the possessive, and a selection can
/// contain both. A prompt reading "their messages" while removing two of the
/// reader's own would be a lie the reader has no way to detect.
#[must_use]
pub fn delete_mixed_prompt(yours: usize, theirs: usize) -> String {
    format!(
        "Delete {} message(s) from both sides ({yours} yours, {theirs} theirs)? (y/n)",
        yours + theirs
    )
}

/// The prompt the status line shows when a quit is waiting to be confirmed.
///
/// The same screen-wide shape as a deletion's, and the same `y`/`n`: quitting is
/// the other destructive thing a keypress can do here, and it is the one a
/// mistyped key can do without the reader meaning to.
pub const QUIT_PROMPT: &str = "Quit televim? (y/n)";

/// The prompt the status line shows when a sign-out is waiting to be confirmed.
///
/// The same screen-wide shape as a quit's, for the same reason: it is the other
/// destructive thing a keypress can do here, and it throws away the one secret
/// this program holds.
pub const LOGOUT_PROMPT: &str = "Sign out and forget this session? (y/n)";

/// What the profile panel says when `add account` is pressed.
///
/// A deliberate refusal, and named as one. The bracketed `[failed: …]` form is
/// for something that tried and did not come back; nothing was sent here, and a
/// reader who reads a failure as a bug would be chasing one that does not exist.
pub const ADD_ACCOUNT_REFUSAL: &str = "not yet: this build cannot add an account";

/// What a card says when `d` lands on a value rather than an action.
///
/// A deliberate refusal, and a *different* refusal from [`ADD_ACCOUNT_REFUSAL`]:
/// a contact's card has no row the reader can act on at all, and one word is too
/// long to tell a reader which of the two reasons applies.
pub const NOT_YOURS_REFUSAL: &str = "Not yours: a contact card has no row you can act on.";

/// Which page of a conversation a fetch is asking for.
///
/// Named rather than a `bool`, because they differ in what they do to the
/// window — one replaces it and two extend it from an end — and a boolean would
/// leave every call site saying which one it meant by convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchDirection {
    /// The newest page, which is what opening a conversation asks for.
    ///
    /// It replaces the window rather than extending it, and it is the only
    /// fetch a conversation with nothing loaded can be given.
    Latest,

    /// The page in front of the oldest message loaded.
    Older,

    /// The page behind the newest message loaded.
    Newer,
}

impl FetchDirection {
    /// What the conversation panel says while this fetch is in flight.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Latest => "Loading…",
            Self::Older => "Loading older…",
            Self::Newer => "Loading newer…",
        }
    }
}

/// What the status line says while a conversation drawn from the cache waits
/// for the server's newest page.
///
/// The `Loading…` row is not drawn for it: that row stands in for messages, and
/// a cached window has messages to show. So the wait is said on the status line
/// instead, and in the same voice — what is on screen, then what is coming —
/// because a reader looking at rows that may be stale is owed being told so.
pub const REVALIDATING_LABEL: &str = "Cached messages — loading the latest…";

/// What the panel and the status line say while a jump is on its way.
///
/// A jump replaces the window rather than extending it, so there is no edge for
/// it to be announced at: it is said where the messages are, and again on the
/// status line, because it is the one fetch the reader asked for by name.
pub const JUMP_LABEL: &str = "Jumping to first unread…";

/// What is said while a jump to the message a reply quotes is on its way.
pub const JUMP_REPLY_LABEL: &str = "Jumping to the quoted message…";

/// What is said while a jump back through the conversation's jumplist is on its
/// way.
pub const JUMP_BACK_LABEL: &str = "Jumping back…";

/// What is said while a jump forward through it is.
pub const JUMP_FORWARD_LABEL: &str = "Jumping forward…";

/// `gd` on a message that quotes nothing.
pub const NOT_A_REPLY: &str = "Not a reply: gd jumps to the message a reply quotes.";

/// A jump whose fetch came back without the message it was for.
pub const JUMP_UNAVAILABLE: &str = "That message is no longer available.";

/// Which jump the reader asked for, which is what the label says.
///
/// `Unread` is `gg` and a match `n` walked to; the other two are the jumplist's,
/// and are named here so that this match is complete rather than because anything
/// produces them yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JumpKind {
    Unread,
    Reply,
    Back,
    Forward,
}

impl JumpKind {
    /// What the panel and the status line say while this jump is on its way.
    ///
    /// One answer per kind, so the two places that say it cannot disagree: a
    /// reader who was told "first unread" and then shown "the quoted message"
    /// has been told two different things about the same keystroke.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Unread => JUMP_LABEL,
            Self::Reply => JUMP_REPLY_LABEL,
            Self::Back => JUMP_BACK_LABEL,
            Self::Forward => JUMP_FORWARD_LABEL,
        }
    }
}

/// A place in a conversation the reader asked to be taken to.
///
/// `gg` means the first unread message, and that message is only sometimes
/// loaded. When it is not, the request cannot be answered from the window and
/// has to travel: this is what it travels as — the conversation, and the message
/// the page should be centred on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Jump {
    /// The conversation to fetch from.
    pub peer_id: i64,

    /// The message to centre the page on.
    pub target_id: i64,

    /// Which jump this is, so the screen can say where it is going.
    pub kind: JumpKind,
}

/// What the conversation on show is in the middle of.
///
/// Only the conversation has a mode, because only the conversation can be in the
/// middle of something: a key means one thing in Normal mode and another in
/// Visual. The input line is in insert mode for as long as it has the focus,
/// which is [`Focus`]'s business rather than this enum's — see [`Focus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Visual,
    Confirm,
}

impl Mode {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Visual => "VISUAL",
            Mode::Confirm => "CONFIRM",
        }
    }
}

/// Which pane a keystroke goes to.
///
/// Focus rather than a mode per pane, because a single mode cannot describe two
/// things at once: a reader can be selecting messages in the conversation *and*
/// have a half-written line waiting. One `Mode` for the whole application had to
/// choose between them, and lost whichever it did not name. So the conversation
/// owns a [`Mode`], the line owns nothing — being on the line *is* its insert
/// mode — and this is the only thing that says where a key lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The list of conversations.
    ChatList,

    /// The messages of the open conversation, and the selection over them.
    Conversation,

    /// The line above the status bar.
    Input,
}

impl Focus {
    /// Whether the right-hand pane is showing something other than the
    /// conversation.
    ///
    /// Four places need this answer and none of them can work it out from
    /// [`Focus`] alone: a focused right-hand pane is the right-hand pane whether
    /// it holds a conversation or a profile, and the difference decides what the
    /// border, the mode label and the status line say.
    #[must_use]
    pub const fn is_profile(self, pane: Pane) -> bool {
        matches!(self, Focus::Conversation) && matches!(pane, Pane::Profile(_))
    }
}

/// What the right-hand pane is showing.
///
/// A second axis beside [`Focus`], and the reason it is a second one is that
/// [`Focus`] is about the *line*: it says where a keystroke lands, and being on
/// the line is what makes it in insert mode. What the right-hand column holds is
/// a different question, and folding a profile into `Focus` would make one enum
/// mean both "which pane" and "what is in this one" — which is how a match on it
/// ends up needing an arm for a state it cannot describe.
///
/// A contact profile is then a variant here rather than a fourth pane: it takes
/// the rectangle the conversation already had, which is why the layout, the
/// focus ring and the `Tab` order are untouched by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    /// The open conversation's messages.
    Conversation,

    /// A profile, for the account or for somebody in a conversation.
    Profile(ProfileId),
}

impl Pane {
    /// Whether this is a profile rather than the conversation.
    #[must_use]
    pub const fn is_profile(self) -> bool {
        matches!(self, Pane::Profile(_))
    }
}

/// Whose profile the right-hand pane is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileId {
    /// The signed-in account.
    SelfAccount,

    /// A person, by the conversation they were found in.
    ///
    /// A chat rather than a bare identifier because a chat is the handle a reader
    /// can name: the conversation on show is a person, and the chat list is where
    /// they were found. The identifier is one lookup away and a peer is not —
    /// `grammers` reports none for a peer that is the account itself, so an id
    /// here could address nothing.
    User(i64),
}

/// What the profile panel shows when it opens.
///
/// One value rather than an `Option<Account>` beside a status string, because
/// the two have to agree: a panel reading "not read yet" while the status line
/// has said "offline" for a minute is wrong twice, and two fields that must
/// agree is one more place to be wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountState {
    /// Nothing has been read yet, and nothing has gone wrong.
    ///
    /// On screen for the frames between the two, and on any machine whose
    /// credentials never arrived — which is the more common of the two.
    Unfetched,

    /// The client could not be built, or the profile could not be read, and this
    /// is why.
    ///
    /// The panel draws the reason rather than an empty profile, because an empty
    /// profile is indistinguishable from a widget that has gone wrong.
    Unavailable(String),

    /// The account's own profile.
    Known(domain::account::Account),
}

/// Somebody the reader is talking to, and what is known about them.
///
/// The same three states as the account's own and for the same reason: a card
/// that has not asked yet, one that asked and could not be told, and one that
/// was told. What differs is only the wording the panel uses, because the
/// account's first line is about credentials and a contact's is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactProfile {
    /// Bare identifier of the person, which is what an answer is matched back on.
    pub peer_id: i64,

    /// Unfetched, unavailable, or known — the same three as the account's.
    pub state: AccountState,
}

/// Where the session is kept, as the panel says it.
///
/// Derived from the same configuration the session store itself is built from, so
/// the two cannot disagree about which one is in use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SessionStore {
    /// The platform's own credential store, and the default.
    #[default]
    Keyring,

    /// A file on disk, encrypted at rest.
    ///
    /// Named for the fact rather than for the type: the panel renders
    /// `session: encrypted file /path/to/file`, and a variant called `File` would
    /// have that fact nowhere to live. The store never writes plaintext, so the
    /// label does not offer a reader the other reading.
    EncryptedFile(PathBuf),
}

/// A row of the profile panel.
///
/// The kinds rather than the labels, because the label is a display concern and
/// the kind is the panel's data model — which rows exist depends on what the
/// A conversation the reader has highlighted and asked to be taken to.
///
/// The index and the moment it was chosen, rather than the index alone: the
/// caller that owns the network opens this once the movement has stopped, and
/// deciding that needs to know how long ago the highlight last moved.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChatChoice {
    /// Where in the list the highlight is.
    pub(crate) index: usize,

    /// When it got there.
    pub(crate) at: Instant,
}

/// The first and last row a selection reaches, in the card's own order.
///
/// A selection is a pair of marks and the pair may be in either order, so this is
/// the one place that normalises them for the card. `text_range` being `None` is
/// what says the selection is a set of rows rather than a range inside one, and
/// that is the same rule the conversation follows for the same reason: there is
/// no such thing as a selection that quotes half of each.
fn row_bounds(selection: &Selection, total: usize) -> (usize, usize) {
    let to_id = |id: i64| {
        usize::try_from(id)
            .unwrap_or(0)
            .min(total.saturating_sub(1))
    };
    let (from, to) = (
        to_id(selection.anchor.message_id),
        to_id(selection.focus.message_id),
    );
    if from <= to { (from, to) } else { (to, from) }
}

/// What the input bar represents when in Insert mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Message,
    Command,
    Search,
    /// Finding a person to start a conversation with, from the chat-list pane.
    ///
    /// Its own prompt rather than [`PromptKind::Search`], which searches the
    /// open conversation's messages: the two answer different questions, and a
    /// query that matched a message must not be mistaken for one that names a
    /// person.
    NewChat,
    Reply,
    Edit,
    Phone,
    Code,
    Password,
}

impl PromptKind {
    /// Whether this prompt is a buffer the reader edits, or a one-line answer
    /// to a question.
    ///
    /// A message, a reply and an edit are text the reader is writing, and get
    /// the full editor: caret movement, visual selection, quick edits. A
    /// command, a search, a new-conversation query and the three sign-in fields
    /// are a single line the reader types and submits, and get insert only —
    /// `Esc` returns to the conversation with the text kept, and there is no
    /// normal mode to leave.
    ///
    /// One method, one axis. [`crate::line::LineEditor`] consults it in exactly
    /// two places: whether to allow a newline, and whether to treat `Esc` as
    /// the first stage of leaving or as leaving.
    #[must_use]
    pub const fn is_buffer(self) -> bool {
        matches!(self, Self::Message | Self::Reply | Self::Edit)
    }

    /// Which sign-in field this prompt is, if it is one.
    ///
    /// The three-way answer rather than a `login: bool`, because the fields are
    /// not the same field: a phone is the reader's own number and pre-filled
    /// from configuration, a code is Telegram's, and a password is the one the
    /// bar paints as bullets. The conversion is what carries that distinction
    /// out to the caller holding the network, which knows nothing about prompts.
    #[must_use]
    pub const fn login_field(self) -> Option<LoginField> {
        match self {
            Self::Phone => Some(LoginField::Phone),
            Self::Code => Some(LoginField::Code),
            Self::Password => Some(LoginField::Password),
            _ => None,
        }
    }
}

/// Which field of the sign-in form the reader is filling in.
///
/// A copy of the three prompts rather than the prompts themselves, because this
/// is what crosses the seam: `Action::Login` carries it to the caller that holds
/// the network, and `tui` may not name `proto` or anything above it. The
/// `LoginCode` and `PasswordChallenge` those callers hold stay on their side of
/// that line, where Telegram's own error names are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginField {
    /// The account's phone number, with its country code.
    Phone,

    /// The login code Telegram sent to that number.
    Code,

    /// The account's two-step verification password.
    Password,
}

/// The sign-in surface, when there is one up.
///
/// An overlay field on [`App`] rather than a [`Pane`] variant and rather than a
/// fourth [`AccountState`] case, for one reason: the form has to outlive the
/// card that names it. `AccountState` is the *card's* value — not read yet, could
/// not be read, read — and a reader who signs in from the card has to be able to
/// carry on typing after the card is gone. So the flow is its own thing, drawn
/// over the right-hand column whatever it was showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignIn {
    /// There is nothing to sign in with.
    ///
    /// No `api_id` and no `api_hash`, so this is not a form: it is a sentence,
    /// and there is no phone row, because a field the reader cannot use is worse
    /// than no field.
    NoCredentials,

    /// The flow, at whatever step Telegram has got to.
    Flow(SignInFlow),
}

impl SignIn {
    /// The flow, if this is one.
    #[must_use]
    pub const fn flow(&self) -> Option<&SignInFlow> {
        match self {
            Self::NoCredentials => None,
            Self::Flow(flow) => Some(flow),
        }
    }

    /// The flow mutably, if this is one.
    pub const fn flow_mut(&mut self) -> Option<&mut SignInFlow> {
        match self {
            Self::NoCredentials => None,
            Self::Flow(flow) => Some(flow),
        }
    }
}

/// The sign-in flow's own state.
///
/// The step and the refusal come from [`domain::session::LoginState`] rather
/// than being kept here: Telegram's own state machine already describes where
/// the flow is and what it last said, and a second copy of it here is a second
/// thing to be wrong about the same conversation.
///
/// Everything else is what this program's half of the flow has and that state
/// machine has no word for: how many password attempts are spent, whether a
/// request is in flight, whether the reader has been told so already, and
/// whether the code survived a trip to another pane.
// Four flags, and the lint that objects to that is wrong here: each one is a
// fact the caller reports rather than a choice this program makes, none is
// derivable from another, and folding them into a bitfield would be one value
// meaning four things rather than four values meaning one thing each.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SignInFlow {
    /// Which step the flow is at, and the last thing Telegram refused.
    pub login: domain::session::LoginState,

    /// Password attempts spent, which the row counts down from three.
    ///
    /// A count rather than the remaining number, because the remaining number
    /// is derived — [`domain::session::attempts_left`] — and a caller that set
    /// it directly could set it to something the flow never earned.
    pub used: u8,

    /// A request is on its way.
    ///
    /// The guard on `⏎`: a second press while this is true does nothing at all,
    /// because a second sign-in request is a second login attempt the reader did
    /// not ask for.
    pub waiting: bool,

    /// The "still checking" sentence has been said.
    ///
    /// One sentence rather than a repeat: an unanswered `⏎` earns a word once,
    /// and every press after that is silence, which is what a key that has
    /// nothing to add should be.
    pub still_said: bool,

    /// The code did not survive the trip away from the line.
    ///
    /// A code is typed once and is not kept: it is Telegram's to send and the
    /// reader's to read, and a `Tab` away from it should not leave a secret
    /// sitting in a dimmed bar. The step is kept, so the return says what was
    /// lost and asks for a new one.
    pub lost_code: bool,

    /// The stored session is one Telegram no longer knows.
    ///
    /// The one row that offers the way back rather than describing a step,
    /// because there is no step to return to: the session is gone and the phone
    /// is the whole way in again.
    pub stale: bool,

    /// The two-step password hint, when the account has one.
    ///
    /// Optional because most accounts do not, and the row is then absent rather
    /// than empty — a `Password hint:` with nothing after it is a question this
    /// panel cannot answer and the reader cannot either.
    ///
    /// Cleared on every step change, because a hint belongs to the password
    /// Telegram is asking about now: carrying one past the step would draw a row
    /// about an account nobody is being asked for.
    pub hint: Option<String>,
}

/// A destructive action waiting for the reader's `y`.
///
/// Everything the wording needs is **captured** here, when the prompt is raised,
/// rather than looked up again when `y` arrives: an arrival can evict a message
/// while the prompt is up, and a selection makes it worse — the window can move
/// under a range, and re-deriving it at `y` time would delete whatever the reader
/// is looking at *now* rather than what they were asked about.
///
/// How many of the messages are the other side's is not stored: it is the length
/// of `ids` less `outgoing`, and a second count that can disagree with the first
/// is a second thing for the wording to be wrong about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmKind {
    /// Leave the program.
    ///
    /// The reader's `y` quits, so it is the one confirmation whose answer is
    /// acted on by the caller rather than queued as an [`Action`]: there is
    /// nothing to send anywhere, and a quit that waited on the network would be
    /// a quit that can hang.
    Quit,

    /// Sign out, and forget the session this program holds.
    ///
    /// Asked for, and the reader's `y` queues an [`Action::Logout`] rather than
    /// acting on it here: the session lives on the side holding the client, so
    /// the confirmation is the last thing this half can do before the key is
    /// gone. A confirmation that refused would have taught the reader the wrong
    /// thing about a key that destroys something, and the lesson would be wrong
    /// exactly when it starts doing that.
    Logout,

    /// Delete these messages, for both sides.
    DeleteMessages {
        /// The messages to delete.
        ids: Vec<i64>,

        /// How many of them the reader's own account sent.
        outgoing: usize,

        /// How many selected messages were left out of `ids` because they are
        /// placeholders for sends the server has not acknowledged.
        ///
        /// Carried so the prompt can say so: a reader who selected five and had
        /// three deleted should not have to infer the other two.
        skipped: usize,
    },
}

/// What deleting a selection would ask the server for.
///
/// The breakdown rather than a bare list of identifiers, because every word of the
/// prompt turns on it: whose the messages are decides the possessive, and how many
/// there are decides whether it counts.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Deletion {
    /// The identifiers the server knows, oldest first.
    pub(crate) ids: Vec<i64>,

    /// How many of them the reader's own account sent.
    pub(crate) outgoing: usize,

    /// How many were placeholders and had to be left out.
    pub(crate) skipped: usize,
}

/// What the status line shows while a deletion is waiting to be confirmed.
///
/// A single message says `your message` and anything more counts, because "Delete
/// 1 of your messages" is worse English than one deletion deserves. A deletion
/// that touches both sides says which is which: the two simpler wordings differ
/// only in the possessive, and a selection can contain both, so a prompt reading
/// "their messages" while removing two of the reader's own would be a lie the
/// reader has no way to detect.
///
/// What was left out is in the prompt rather than flashed, because a confirmation
/// outranks a transient status: a `flash` written while the prompt is up is a line
/// the reader never sees.
fn delete_prompt(ids: &[i64], outgoing: usize, skipped: usize) -> String {
    let theirs = ids.len().saturating_sub(outgoing);
    let asked = match (ids.len(), outgoing, theirs) {
        (1, 1, 0) => DELETE_OUTGOING_PROMPT.to_owned(),
        (1, 0, 1) => DELETE_INCOMING_PROMPT.to_owned(),
        (_, count, 0) => delete_yours_prompt(count),
        (_, 0, count) => delete_theirs_prompt(count),
        (_, mine, theirs) => delete_mixed_prompt(mine, theirs),
    };

    match skipped {
        0 => asked,
        count => format!("{asked} · {count} never sent"),
    }
}

/// Something the reader asked the interface to do that only the network side can.
///
/// The same idempotent hand-over as [`Jump`]: `tui` may not name `proto`, so an
/// operation that needs the network is recorded here and taken once by the
/// caller that owns both halves. Taking it clears it, so a caller that takes
/// twice gets one action, not two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Send `text` to `chat_id` as a reply to `reply_to`, if any.
    ///
    /// `temp_id` is the placeholder the message is already rendered as, and is
    /// what the outcome is matched back to.
    Send {
        /// The conversation to send to.
        chat_id: i64,

        /// The placeholder the message is rendered as.
        temp_id: i64,

        /// The text to send.
        text: String,

        /// The message to reply to, if this is a reply.
        reply_to: Option<i64>,
    },

    /// Replace the text of `message_id` in `chat_id`.
    Edit {
        /// The conversation the message belongs to.
        chat_id: i64,

        /// The message to edit.
        message_id: i64,

        /// The text to replace it with.
        text: String,
    },

    /// Delete messages from `chat_id`.
    Delete {
        /// The conversation the messages belong to.
        chat_id: i64,

        /// The messages to delete.
        message_ids: Vec<i64>,
    },

    /// Forward messages from `chat_id` to `dest_chat_id`.
    Forward {
        /// The conversation the messages come from.
        chat_id: i64,

        /// The messages to forward, in the order they are to arrive.
        message_ids: Vec<i64>,

        /// The conversation to forward them to.
        dest_chat_id: i64,
    },

    /// Read the profile of the peer with this bare identifier.
    ///
    /// A question rather than a fetch, for the same reason as [`Action::Search`]:
    /// it does not touch the conversation's window or its cursor, and its answer
    /// is matched back to the card that asked for it. One per card opened, because
    /// a card is opened, read and closed, and the alternative is a cache of
    /// profiles for people the reader glanced at and moved on from.
    FetchContact {
        /// Bare identifier of the person to read.
        peer_id: i64,
    },

    /// Search `chat_id` for `query`.
    ///
    /// A question rather than a fetch: it does not touch the window or the
    /// cursor, and its answer is a list of identifiers matched back to the query
    /// it was asked for.
    Search {
        /// The conversation to search.
        chat_id: i64,

        /// What to look for.
        query: String,
    },

    /// Find a person named or handle-matching `query`, to start a conversation
    /// with.
    ///
    /// The same hand-over as [`Action::Search`] and [`Action::FetchContact`]:
    /// `tui` cannot reach the network, so the lookup is recorded here and taken
    /// once by the caller that can. The answer is matched back to the query it
    /// was asked for, so a late result cannot replace a newer one's list.
    ResolveUser {
        /// What the reader typed: a `@username` or a display name.
        query: String,
    },

    /// Pin or unpin the chat with `chat_id` on the account's own list.
    ///
    /// The same hand-over as every other action. Only queued with a client up:
    /// the key that asks for it refuses without one.
    TogglePin {
        /// The chat to pin or unpin.
        chat_id: i64,

        /// Whether it should be pinned once Telegram has agreed.
        pinned: bool,
    },

    /// Ask Telegram to move the sign-in flow on with `value`.
    ///
    /// The same hand-over as every other action, and for the same reason: a
    /// password check is the network's, and `tui` may not name the client that
    /// does it. Which field is being answered is [`LoginField`] rather than the
    /// prompt, because the caller holds no prompts and would otherwise have to
    /// match on three strings.
    Login {
        /// The field the value answers.
        field: LoginField,

        /// What the reader typed into it.
        value: String,
    },

    /// Give up on the flow and take the shell card back.
    ///
    /// Not a refusal: `Esc` at the code step is the reader changing their mind,
    /// and the cancellation has to reach the side holding the client so it can
    /// drop the code it asked Telegram to send. A key that cancelled locally and
    /// left a code pending would earn another one the reader is no longer there
    /// to answer.
    LoginCancelled,

    /// Sign out, and forget the session this program holds.
    ///
    /// Asked for rather than performed, like everything else here: the session
    /// lives on the side holding the client, and the answer destroys the key this
    /// program holds — which is why the reader confirms it first
    /// ([`ConfirmKind::Logout`]) and why `tui` never learns what came back. A
    /// sign-out with no argument is the one action whose whole content is that it
    /// happened, so it carries nothing.
    Logout,

    /// Download the media on `message_id` in `chat_id` and open it in an
    /// external viewer.
    ///
    /// The same hand-over as every other action: the download is the network's,
    /// so `tui` only records which message the reader asked for. Only queued for
    /// a message that carries media, by the key that asks for it.
    OpenMedia {
        /// The conversation the message belongs to.
        chat_id: i64,

        /// The message whose media to open.
        message_id: i64,
    },

    /// Stop the download of `message_id` in `chat_id`, if one is in flight.
    ///
    /// Only the network can stop a transfer, so `tui` records the reader's
    /// request and the loop sets the flag the download checks per chunk. A
    /// download that has already finished is not affected.
    CancelMediaDownload {
        /// The conversation the message belongs to.
        chat_id: i64,

        /// The message whose download to stop.
        message_id: i64,
    },
}

/// A `f`, `t`, `F` or `T` that has been pressed and is waiting for its character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Find {
    /// Which way to look.
    pub(crate) forward: bool,

    /// Whether to land on the character itself rather than one short of it.
    pub(crate) onto: bool,
}

/// Text the reader has yanked.
///
/// One unnamed register, oldest line first: a yank of three messages pastes back
/// as three messages, which is what "yank these" means in a conversation.
///
/// Owned strings rather than borrows of the window's, because a selection can be
/// yanked and then paged out from under it, and a borrow would dangle on the next
/// page. This is the same reason `after_window_change` anchors by identifier.
///
/// Named registers are a Vim feature with no consumer here, and a second register
/// is a second thing to keep in step with the first.
#[derive(Debug, Clone, Default)]
pub struct Register(Vec<String>);

impl Register {
    /// Replaces the contents with `lines`.
    pub(crate) fn set(lines: Vec<String>) -> Self {
        Self(lines)
    }

    /// The lines, oldest first.
    #[must_use]
    pub fn lines(&self) -> &[String] {
        &self.0
    }

    /// Whether anything has been yanked.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Everything yankable, as one string, which is what pasting into the line
    /// wants.
    ///
    /// The lines joined by newlines rather than concatenated: a yank of three
    /// messages pasted into the line is three lines, and a reader who wants them
    /// as one sentence can join them.
    #[must_use]
    pub fn text(&self) -> String {
        self.0.join("\n")
    }
}

/// Which pages are in flight.
///
/// One flag per direction rather than a single "busy": the three are asked for
/// independently, so a page arriving in one direction must not release another,
/// and opening a conversation must release all three.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Fetching {
    latest: bool,
    older: bool,
    newer: bool,
}

impl Fetching {
    /// Whether a page in `direction` is on its way.
    const fn is_in_flight(self, direction: FetchDirection) -> bool {
        match direction {
            FetchDirection::Latest => self.latest,
            FetchDirection::Older => self.older,
            FetchDirection::Newer => self.newer,
        }
    }

    /// Records that a page in `direction` has been asked for, or that it is no
    /// longer on its way.
    pub(crate) fn set(&mut self, direction: FetchDirection, in_flight: bool) {
        let slot = match direction {
            FetchDirection::Latest => &mut self.latest,
            FetchDirection::Older => &mut self.older,
            FetchDirection::Newer => &mut self.newer,
        };
        *slot = in_flight;
    }

    /// Forgets every direction, which is what opening another conversation
    /// does.
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Every piece of the screen's state, and the only thing that draws.
///
/// Nine fields, one concern each: [`UiState`], [`SessionState`],
/// [`ProfileCard`], [`ChatListState`], [`Outbox`], [`Pending`],
/// [`ConversationState`], [`InputState`] and [`DraftStore`]. Dispatch mode,
/// focus and the frame-measurement cells live in `state/ui.rs`, on [`UiState`]
/// and its `FrameMetrics`.
pub struct App {
    /// Dispatch mode, focus, chrome, and the frame's measurements.
    pub ui: UiState,

    /// The account's session and the sign-in surface.
    pub session: SessionState,

    /// The profile card's buffer and its per-keystroke state.
    pub profile: ProfileCard,

    /// The chat list and the selection cursor.
    pub list: ChatListState,

    /// Queued actions, in-flight fetches, and the clipboard the driver takes.
    pub outbox: Outbox,

    /// Half-typed keys and the requests a keystroke defers.
    pub pending: Pending,

    /// The open conversation's view, its editing and selection registers, and its
    /// search surfaces.
    pub conversation: ConversationState,

    /// The live input line and the emoji popup over it.
    pub input: InputState,

    /// Per-peer parked drafts and read receipts.
    pub drafts: DraftStore,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// An application with nothing in it.
    ///
    /// Nothing is fabricated: the chat list is empty and no conversation is
    /// open, because neither has been fetched yet. The screen is a frame waiting
    /// for a client rather than a picture of one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            ui: UiState::new(),
            session: SessionState::new(),
            profile: ProfileCard::new(),
            list: ChatListState::new(),
            outbox: Outbox::new(),
            pending: Pending::new(),
            conversation: ConversationState::new(),
            input: InputState::new(),
            drafts: DraftStore::new(),
        }
    }

    /// The same application, drawing right-to-left rows itself.
    ///
    /// Delegates to [`UiState::with_bidi`].
    #[must_use]
    pub fn with_bidi(mut self, bidi: BidiMode) -> Self {
        self.ui = self.ui.with_bidi(bidi);
        self
    }

    /// The same application, drawing `[sticker]` where a picture would go.
    ///
    /// Delegates to [`UiState::with_stickers`].
    #[must_use]
    pub fn with_stickers(mut self, stickers: StickerMode) -> Self {
        self.ui = self.ui.with_stickers(stickers);
        self
    }

    /// The same application, placing sticker pictures with the kitty graphics
    /// protocol instead of painting them as cells.
    ///
    /// Delegates to [`UiState::with_graphics`].
    #[must_use]
    pub fn with_graphics(mut self, graphics: GraphicsMode) -> Self {
        self.ui = self.ui.with_graphics(graphics);
        self
    }

    /// How a sticker's picture reaches the terminal.
    ///
    /// Asked once per frame by the conversation panel, and never by the layout
    /// — see the field's doc.
    #[must_use]
    pub fn graphics(&self) -> GraphicsMode {
        self.ui.graphics
    }

    /// Who permutes a right-to-left row: this program, or the terminal.
    ///
    /// Asked once per row by the conversation panel, and never per frame by the
    /// layout — see the field's doc.
    #[must_use]
    pub fn bidi(&self) -> BidiMode {
        self.ui.bidi
    }

    /// Whether a decoded sticker paints its picture or its token.
    ///
    /// Asked once per tick by the loop's sticker drain, and never per frame by
    /// the layout — see the field's doc.
    #[must_use]
    pub fn sticker_mode(&self) -> StickerMode {
        self.ui.stickers
    }

    /// An application holding the sample conversation the tests read from.
    ///
    /// The sample data is not part of the program. A build of the client starts
    /// empty and fills in once it can fetch, which is what keeps it from showing
    /// a conversation nobody sent.
    #[cfg(test)]
    #[must_use]
    pub fn mock() -> Self {
        let mut app = Self::new();
        app.set_chats(mock_chats());
        app.select_chat(0);
        app.apply_latest(mock_messages());
        app.set_account(Ok(mock_account()));
        // The design's own number, so the sign-in scenes are the frames the
        // design drew rather than an approximation of them: the phone row is
        // pre-filled in every one of them.
        app.session.phone = "+44 7700 900142".to_owned();
        // The sign-in scenes are a machine that *can* sign in; the one that
        // cannot is its own scene, and it sets this back rather than inheriting
        // the other answer.
        app.session.credentials_configured = true;
        // And one with a client up, which is what the sample account above is: a
        // fetched account is what a `Ready` brings, and a `Ready` brings a
        // client. Without it every `⏎` in these scenes would report the
        // client-less sentence instead of the request under test.
        app.session.client_available = true;
        app
    }

    // ---- the chat list --------------------------------------------------

    /// The conversations, as they were last fetched.
    #[must_use]
    pub fn chats(&self) -> &[Chat] {
        &self.list.list.chats
    }

    /// The chats a forward can land in: the chat list without deleted accounts.
    ///
    /// Delegates to [`ChatListState::forward_destinations`].
    #[must_use]
    pub fn forward_destinations(&self) -> Vec<&Chat> {
        self.list.forward_destinations()
    }

    /// The jump the reader is waiting on, if any.
    ///
    /// What the caller fetches: a jump the window cannot answer is recorded here
    /// rather than acted on, because nothing on this side of the boundary can
    /// reach the network.
    #[must_use]
    pub fn pending_jump(&self) -> Option<Jump> {
        self.pending.pending_jump
    }

    /// The query the open conversation is being searched for, if any.
    ///
    /// The half of a search result's identity that `chat_id` does not carry: a
    /// result is dropped when it no longer names the query the reader is asking.
    #[must_use]
    pub fn search_query(&self) -> Option<&str> {
        self.conversation.search.query()
    }

    /// The search on the open conversation, for the panel to mark matches with.
    #[must_use]
    pub fn search(&self) -> &SearchState {
        &self.conversation.search
    }

    /// The new-conversation search, for the overlay to draw and the status line
    /// to name.
    #[must_use]
    pub fn user_search(&self) -> &UserSearchState {
        &self.conversation.user_search
    }

    /// What the reader has selected, for the panel to mark and the operations to
    /// act on.
    ///
    /// One value rather than two marks on the application: an anchor and a focus
    /// kept apart are two things to keep consistent, and the arithmetic between
    /// them is the same in every reader.
    #[must_use]
    pub fn selection(&self) -> Option<&Selection> {
        self.conversation.selection.as_ref()
    }

    /// The `:query` being completed, for the popup to draw and the status line
    /// to name.
    ///
    /// The same shape as [`App::selection`]: one answer, read by both.
    #[must_use]
    pub fn completion(&self) -> Option<&emoji::Trigger> {
        self.input.emoji.as_ref()
    }

    /// The window positions the selection covers, oldest first.
    ///
    /// Delegates to [`ConversationState::covered`].
    #[must_use]
    pub fn covered(&self, selection: Option<&Selection>) -> Range<usize> {
        self.conversation.covered(selection)
    }

    /// How much is selected, in whatever the selection is of.
    ///
    /// Characters for a text selection and messages for a set of them, because the
    /// unit is what the reader is counting: "3 selected" beside a set of three
    /// messages is three messages, and beside three characters it is three
    /// characters. A single number cannot carry both, and picking the wrong unit is
    /// a number the reader cannot act on.
    ///
    /// `None` when there is no selection.
    #[must_use]
    pub fn selection_len(&self) -> Option<usize> {
        let selection = self.conversation.selection.as_ref()?;

        Some(selection.text_range().map_or_else(
            || self.covered(Some(selection)).len(),
            |(_, range)| range.len(),
        ))
    }

    /// Starts a selection at `message_id`, and reports whether it could be.
    ///
    /// Delegates to [`ConversationState::select`].
    #[must_use]
    pub fn select(&mut self, message_id: i64, char: Option<usize>) -> bool {
        self.conversation.select(message_id, char)
    }

    /// Replaces the selection outright.
    ///
    /// Delegates to [`ConversationState::set_selection`].
    pub fn set_selection(&mut self, selection: Selection) {
        self.conversation.set_selection(selection);
    }

    /// What the reader last yanked, for the caller that hands it to the system
    /// clipboard.
    #[must_use]
    pub fn register(&self) -> &Register {
        &self.conversation.register
    }

    /// Takes the text the reader asked to copy to the system clipboard.
    ///
    /// Delegates to [`Outbox::take_clipboard`].
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.outbox.take_clipboard()
    }

    /// Records a pin Telegram accepted: the chat moves to its place and the
    /// highlight stays on the chat it was on.
    ///
    /// Delegates to [`ChatListState::set_pinned`].
    pub fn set_pinned(&mut self, chat_id: i64, pinned: bool) {
        self.list.set_pinned(chat_id, pinned);
    }

    /// Clears a conversation's unread count once Telegram has accepted the read
    /// marker, and says whether there was a count to clear.
    ///
    /// Delegates to [`domain::updates::ChatList::mark_read`].
    pub fn mark_chat_read(&mut self, chat_id: i64) -> bool {
        self.list.list.mark_read(chat_id)
    }

    /// Installs a freshly fetched chat list.
    ///
    /// Delegates to [`coordinate::set_chats`].
    pub fn set_chats(&mut self, chats: Vec<Chat>) {
        coordinate::set_chats(
            &mut self.ui,
            &mut self.list,
            &mut self.outbox,
            &mut self.pending,
            &mut self.conversation,
            &mut self.input,
            &mut self.drafts,
            chats,
        );
    }

    /// Installs a freshly fetched chat list while keeping the open conversation.
    ///
    /// Delegates to [`coordinate::refresh_chats`].
    pub fn refresh_chats(&mut self, chats: Vec<Chat>) -> bool {
        coordinate::refresh_chats(&mut self.list, &mut self.conversation, chats)
    }

    // ---- the profile panel ----------------------------------------------

    /// Records what the account's own profile turned out to be.
    ///
    /// Delegates to [`SessionState::set_account`].
    pub fn set_account(&mut self, account: Result<Account, String>) {
        self.session.set_account(account);
    }

    /// The contact the card on show is about, if it is about one.
    ///
    /// `None` for the account's own card, which is what tells the card whether
    /// there is a second subject's data at all.
    #[must_use]
    pub fn contact(&self) -> Option<&ContactProfile> {
        self.profile.contact.as_ref()
    }

    /// Records what a contact's profile read came back with.
    ///
    /// The only writer of a contact's state, for the same reason `set_account` is
    /// the only writer of the account's: three states that cannot be mixed up by a
    /// caller that knows only one of them.
    ///
    /// An answer about somebody the card is not on show for is dropped. The
    /// reader may open a second card while the first read is in flight, and the
    /// second card's fields are not the first one's.
    pub fn set_contact(&mut self, peer_id: i64, profile: Result<Account, String>) {
        if !self.profile.adopt_contact(peer_id, profile) {
            return;
        }
        // The highlight is re-sized because the row count just changed under it.
        // The account's own card is read once at start-up, so its rows are settled
        // before the card opens; a contact's card is drawn with *no* rows until
        // this answer arrives, and a highlight bounded over zero rows can never be
        // moved afterwards — `j` clamps to a buffer of nothing. Re-clamping rather
        // than resetting keeps the reader where they were, which here is the top.
        self.profile
            .retotal(crate::card::navigable(&crate::card::rows(self)));
    }

    /// Records where the session is kept.
    ///
    /// Delegates to [`SessionState::set_session_store`].
    pub fn set_session_store(&mut self, store: SessionStore) {
        self.session.set_session_store(store);
    }

    /// Where the profile's highlight is, for the panel to draw.
    #[must_use]
    pub fn profile_cursor(&self) -> usize {
        self.profile.profile_vim.cursor()
    }

    /// Whom the card is about.
    ///
    /// A contact is looked up in the chat list by the id the `ProfileId` holds, and
    /// a subject that is not there is the account's own: a card whose subject has
    /// gone should not read as somebody else's, and there is nobody else.
    #[must_use]
    pub fn card_subject(&self) -> crate::card::CardSubject<'_> {
        match self.profile.profile_subject {
            ProfileId::SelfAccount => crate::card::CardSubject::SelfAccount,
            ProfileId::User(id) => self
                .list
                .list
                .chats
                .iter()
                .find(|chat| chat.id == id)
                .map_or(
                    crate::card::CardSubject::SelfAccount,
                    crate::card::CardSubject::Contact,
                ),
        }
    }

    /// The row the inline position is in, as a character count within its value.
    #[must_use]
    pub fn card_caret(&self) -> usize {
        self.profile.profile_caret
    }

    /// The inline position as a byte offset into `value`.
    ///
    /// A caret is a byte offset like every other field of
    /// [`crate::text_row::TextRow`], and a motion is a character count, so this is
    /// where the two meet — by [`crate::rows::byte_span`], the one converter in the
    /// program. A position past the end clamps to the end, because a caret belongs
    /// at the end of a value and not one past it.
    #[must_use]
    pub fn card_caret_byte(&self, value: &str) -> usize {
        let chars = value.chars().count();
        let at = self.profile.profile_caret.min(chars);
        if at >= chars {
            return value.len();
        }
        crate::rows::byte_span(value, at..at + 1).start
    }

    /// The card's selection, when there is one.
    ///
    /// A row is named by its index in [`crate::card::rows`], which is what the
    /// panel and the keys already agree on, and a position within it is a character
    /// count — so the same `text_range` rule applies unchanged: a selection whose
    /// two ends are in **one** row is a text range, and a selection reaching two
    /// rows is a set of rows, because there is no such thing as a selection that
    /// quotes half of each.
    #[must_use]
    pub fn card_selection(&self) -> Option<Selection> {
        let anchor = self.profile.profile_visual?;
        let focus = self.card_mark();

        // `char: None` whenever the selection covers whole rows, which is what `V`
        // leaves behind and what moving off the row it started on becomes: the
        // reader selected rows, and the values inside them are yankable whole.
        let both_chars = anchor.char.is_some() && focus.char.is_some();
        let (anchor, focus) = if both_chars {
            (anchor, focus)
        } else {
            (
                Mark::whole(anchor.message_id),
                Mark::whole(focus.message_id),
            )
        };

        Some(Selection { anchor, focus })
    }

    /// Moves the card's highlight to `row`, for a caller that knows where it wants
    /// the cursor rather than which key gets it there.
    ///
    /// Delegates to [`ProfileCard::handle_card_row`].
    #[cfg(test)]
    pub(crate) fn handle_card_row(&mut self, row: usize) {
        self.profile.handle_card_row(row);
    }

    /// One charwise motion within the cursor row's value.
    ///
    /// Delegates to [`coordinate::card_motion_char`], over the rows on show.
    #[cfg(test)]
    pub(crate) fn handle_card_motion(&mut self, key: char) {
        let rows = crate::card::rows(self);
        coordinate::card_motion_char(&mut self.profile, &rows, key);
    }

    /// One key at the card, for a test that is about the key and not the path.
    #[cfg(test)]
    pub(crate) fn handle_card_key(&mut self, key: char) {
        self.handle_profile(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
    }

    /// A chat from the list, for a test that needs a contact to open a card about.
    #[cfg(test)]
    pub(crate) fn any_chat(&self) -> Option<domain::chat::Chat> {
        self.list.list.chats.first().cloned()
    }

    /// Whether the card's highlight is on the row with this label.
    ///
    /// Walked to by label rather than by index, because a row that is *absent* is
    /// the whole rule — a test that asked for row 4 would be asserting that the
    /// row after `bio` is `birthday`, which is false for every account whose
    /// privacy hides one of them.
    #[cfg(test)]
    pub(crate) fn on_card_row(&self, label: &str) -> bool {
        crate::card::rows(self)
            .get(self.profile_cursor())
            .is_some_and(|row| row.label == label)
    }

    /// The moving end of a card selection: the cursor row and the inline position.
    ///
    /// The inline position is only part of the mark while the selection is inside
    /// one row, because a selection that has reached a second row is a selection
    /// of rows and the position within the first is not what it covers.
    fn card_mark(&self) -> Mark {
        let inside = self
            .profile
            .profile_visual
            .is_some_and(|anchor| anchor.message_id == self.card_row_id());
        if inside {
            Mark::text(self.card_row_id(), self.profile.profile_caret)
        } else {
            Mark::whole(self.card_row_id())
        }
    }

    /// The cursor row as the identifier a [`Mark`] names.
    ///
    /// Delegates to [`ProfileCard::card_row_id`].
    fn card_row_id(&self) -> i64 {
        self.profile.card_row_id()
    }

    /// Handles a key while the profile has the focus.
    ///
    /// Delegates to [`coordinate::handle_profile`], sizing the highlight
    /// afterwards when a card opened.
    #[cfg(test)]
    fn handle_profile(&mut self, key: KeyEvent) {
        let rows = crate::card::rows(self);
        let lines = self.card_yanked();
        let is_contact = matches!(self.card_subject(), crate::card::CardSubject::Contact(_));
        let cursor = self.profile_cursor();
        let layout = self.row_layout();
        let opened = coordinate::handle_profile(
            &mut self.profile,
            &mut self.ui,
            &mut self.conversation,
            &mut self.outbox,
            &mut self.list,
            &mut self.pending,
            &mut self.input,
            &mut self.drafts,
            &rows,
            lines,
            is_contact,
            cursor,
            &layout,
            key,
        );
        if opened {
            self.profile.resize(crate::card::navigable(&rows));
        }
    }

    /// The lines a card selection yanks, or the cursor row's value when there is
    /// no selection — which is what `yy` is.
    fn card_yanked(&self) -> Vec<String> {
        let rows = crate::card::rows(self);
        let Some(selection) = self.card_selection() else {
            // No selection: the whole value of the row the cursor is on. A card
            // has no buffer, so there is no linewise equivalent to reach for.
            return rows
                .get(self.profile.profile_vim.cursor())
                .map(|row| vec![row.value.clone()])
                .unwrap_or_default();
        };

        // Two cases, and which one it is falls out of `text_range` being `Some`:
        // inside one row it is a range of characters, and anything else is a set of
        // rows, because there is no such thing as a selection that quotes half of
        // each.
        if let Some((id, range)) = selection.text_range() {
            let Some(row) = usize::try_from(id).ok().and_then(|id| rows.get(id)) else {
                return Vec::new();
            };
            let end = range.end.min(row.value.chars().count());
            let start = range.start.min(end);
            return vec![row.value[crate::rows::byte_span(&row.value, start..end)].to_owned()];
        }

        // Across rows: one line per row, oldest first, which is the register's own
        // order and the conversation's. A held slot inside the range is left out
        // rather than yanking as an empty line, because it is not a row the
        // reader selected — it is a position, and the register holds text.
        let (from, to) = row_bounds(&selection, rows.len());
        rows.iter()
            .enumerate()
            .filter(|(index, row)| (from..=to).contains(index) && !row.is_reserved())
            .map(|(_, row)| row.value.clone())
            .collect()
    }

    // ---- what is on show ------------------------------------------------

    /// The conversation the reader has stopped on, once they have stopped.
    ///
    /// Delegates to [`Pending::take_pending_chat`].
    pub fn take_pending_chat(&mut self, now: Instant) -> Option<usize> {
        self.pending.take_pending_chat(now)
    }

    /// Shows the profile card for the chat the highlight has settled on.
    ///
    /// The conversation is neither opened nor replaced, so browsing never
    /// fetches or marks a chat. The focus and mode stay where the reader left
    /// them: a settle can land while they are still on the list.
    ///
    /// Returns whether a card opened.
    pub fn open_settled_card(&mut self, now: Instant) -> bool {
        let Some(index) = self.take_pending_chat(now) else {
            return false;
        };
        self.list.select(index);
        let (focus, mode) = (self.ui.focus, self.ui.mode);
        coordinate::open_contact(
            &self.list,
            &mut self.profile,
            &mut self.ui,
            &mut self.conversation,
            &mut self.outbox,
        );
        self.ui.set_focus(focus);
        self.ui.set_mode(mode);
        let rows = crate::card::rows(self);
        self.profile.resize(crate::card::navigable(&rows));
        true
    }

    /// Opens the conversation at `index` in the chat list.
    ///
    /// Delegates to [`coordinate::select_chat`].
    pub fn select_chat(&mut self, index: usize) {
        coordinate::select_chat(
            &mut self.ui,
            &mut self.list,
            &mut self.outbox,
            &mut self.pending,
            &mut self.conversation,
            &mut self.input,
            &mut self.drafts,
            index,
        );
    }

    /// Opens the conversation whose chat id is `id`.
    ///
    /// The same lookup `:chat` does: the position of the id in the list, and
    /// nothing is opened when it is not there. Reports whether the id was
    /// found, so a caller that must land somewhere can say where it landed
    /// instead of staying silent the way `:chat` does.
    ///
    /// Delegates to [`coordinate::select_chat_by_id`].
    pub fn select_chat_by_id(&mut self, id: i64) -> bool {
        coordinate::select_chat_by_id(
            &mut self.ui,
            &mut self.list,
            &mut self.outbox,
            &mut self.pending,
            &mut self.conversation,
            &mut self.input,
            &mut self.drafts,
            id,
        )
    }

    /// Records the `--chat` id to open when the first chat list arrives.
    ///
    /// Launch state, set once before the first frame and taken when the list
    /// lands, so a later refresh keeps the reader where they are.
    ///
    /// Delegates to [`Pending::set_initial_chat`].
    pub fn set_initial_chat(&mut self, id: i64) {
        self.pending.set_initial_chat(id);
    }

    /// Takes the pending `--chat` id, if one was set and not yet consumed.
    ///
    /// Delegates to [`Pending::take_initial_chat`].
    pub fn take_initial_chat(&mut self) -> Option<i64> {
        self.pending.take_initial_chat()
    }

    /// Whether a conversation is open to put messages in.
    ///
    /// Delegates to [`ConversationState::has_conversation`].
    #[cfg(test)]
    #[must_use]
    fn has_conversation(&self) -> bool {
        self.conversation.has_conversation()
    }
    /// The conversation on show, as the chat list holds it.
    ///
    /// Delegates to [`coordinate::open_chat`].
    fn open_chat(&self) -> Option<&Chat> {
        coordinate::open_chat(&self.list, &self.conversation)
    }

    /// The name of the conversation on show, for the input bar's title.
    ///
    /// The one thing a reader cannot work out from the bar alone is where the
    /// words in it will be sent: each conversation keeps its own draft, so the
    /// draft itself no longer names one. `None` when nothing is open, which is
    /// the one case in which composing does nothing at all.
    #[must_use]
    pub fn open_chat_name(&self) -> Option<&str> {
        self.open_chat().map(|chat| chat.title.as_str())
    }

    /// The message the cursor is on, if the window holds anything.
    ///
    /// Delegates to [`ConversationState::cursor_message`].
    fn cursor_message(&self) -> Option<&Message> {
        self.conversation.cursor_message()
    }

    // ---- pages coming back ----------------------------------------------

    /// Replaces the window with the newest page of the open conversation.
    ///
    /// Delegates to [`ConversationState::apply_latest`].
    pub fn apply_latest(&mut self, page: Vec<Message>) -> bool {
        self.conversation.apply_latest(page)
    }

    /// Fills the conversation `chat_id` names with what the cache holds for it,
    /// if it is the one on show and nothing has filled it yet.
    ///
    /// Meant to be called once, as a conversation is opened — after
    /// [`Self::select_chat`] and before the newest page lands — with the cached
    /// messages oldest first. The newest page is still asked for and still
    /// replaces what this put on screen: this is the first frame, not the answer.
    ///
    /// Delegates to [`ConversationState::seed`].
    pub fn seed_from_cache(&mut self, chat_id: i64, messages: Vec<Message>) -> bool {
        self.conversation.seed(chat_id, messages)
    }

    /// Whether the window on show came from the cache and the server's newest
    /// page for it is on its way.
    ///
    /// Both halves, because neither alone is a revalidation: a cached window with
    /// nothing in flight — offline, or holding back after a failure — has nothing
    /// coming to say, and a newest page in flight over an empty window is the
    /// `Loading…` row's to announce.
    #[must_use]
    pub fn is_revalidating(&self) -> bool {
        self.conversation.cached && self.is_fetching(FetchDirection::Latest)
    }

    /// Puts a page in front of what the window holds.
    ///
    /// Delegates to [`ConversationState::apply_older`].
    pub fn apply_older(&mut self, page: Vec<Message>) -> bool {
        self.conversation.apply_older(page)
    }

    /// Puts messages behind what the window holds: a fetched page, or one the
    /// reader has just typed.
    ///
    /// Delegates to [`ConversationState::apply_newer`].
    pub fn apply_newer(&mut self, page: Vec<Message>) -> bool {
        self.conversation.apply_newer(page)
    }

    // ---- jumping to the unread messages ---------------------------------

    /// Takes the reader to where the conversation's unread messages start.
    ///
    /// Delegates to [`coordinate::jump_to_unread`].
    #[must_use]
    pub fn jump_to_unread(&mut self) -> Option<Jump> {
        coordinate::jump_to_unread(&mut self.list, &mut self.conversation, &mut self.ui)
    }

    /// Takes the reader to the message the one under the cursor quotes.
    ///
    /// Delegates to [`coordinate::jump_to_reply`].
    #[must_use]
    pub fn jump_to_reply(&mut self) -> Option<Jump> {
        coordinate::jump_to_reply(&mut self.list, &mut self.conversation, &mut self.ui)
    }

    /// Takes the reader back to where they were before the last jump.
    ///
    /// Delegates to [`coordinate::jump_back`].
    pub fn jump_back(&mut self) {
        coordinate::jump_back(&mut self.pending, &mut self.conversation);
    }

    /// Ends the reader's wait for a jump, reporting whether it was still the one
    /// being waited on.
    ///
    /// Delegates to [`Pending::clear_jump`].
    pub fn clear_jump(&mut self, target_id: i64) -> bool {
        self.pending.clear_jump(target_id)
    }

    /// Replaces the window with a page fetched around a message the reader asked
    ///
    /// Delegates to [`coordinate::apply_jump`].
    pub fn apply_jump(&mut self, page: &[Message], target_id: i64) -> bool {
        coordinate::apply_jump(
            &mut self.pending,
            &mut self.conversation,
            &mut self.ui,
            page,
            target_id,
        )
    }

    // ---- events from the feed -------------------------------------------

    /// Applies an event from the feed to everything it touches.
    ///
    /// Delegates to [`coordinate::apply_update`].
    #[must_use]
    pub fn apply_update(&mut self, event: &UpdateEvent) -> bool {
        coordinate::apply_update(
            &mut self.ui,
            &mut self.list,
            &mut self.conversation,
            &mut self.drafts,
            event,
        )
    }

    // ---- fetching --------------------------------------------------------

    /// Whether the page in front of what is loaded is worth asking for.
    ///
    /// A window shorter than the margin is near both of its ends at once, and
    /// asking is still right: the conversation simply is not loaded yet.
    ///
    /// Counted in rows rather than in messages, which is the only way "near the
    /// top" means what a reader scrolling upwards thinks it means.
    ///
    /// Never while the newest page is on its way: that page replaces the window,
    /// so a page from either end of the one on show is work it would throw away —
    /// and the cursor that would anchor it describes nothing yet. An empty window
    /// used to be the whole of that guard; a cached one is not empty.
    #[must_use]
    pub fn wants_older(&self) -> bool {
        let window = &self.conversation.conversation.window;

        !self.outbox.fetching.is_in_flight(FetchDirection::Older)
            && !self.outbox.fetching.is_in_flight(FetchDirection::Latest)
            && !window.is_empty()
            && !window.exhausted_older
            && self.cursor_extent().0 < FETCH_MARGIN
    }

    /// Whether the page behind what is loaded is worth asking for.
    ///
    /// Only while the reader is away from the bottom. A view pinned to the
    /// newest message is already there, and an arrival reaches it through the
    /// feed rather than through a fetch. Never while the newest page is on its
    /// way, for the reason [`Self::wants_older`] gives.
    #[must_use]
    pub fn wants_newer(&self) -> bool {
        let window = &self.conversation.conversation.window;

        !self.outbox.fetching.is_in_flight(FetchDirection::Newer)
            && !self.outbox.fetching.is_in_flight(FetchDirection::Latest)
            && !window.is_empty()
            && !window.exhausted_newer
            && !self.conversation.conversation.auto_follow()
            && self.near_the_end()
    }

    /// Whether the rows behind the cursor's message are within the fetch
    /// margin of the end of the window.
    fn near_the_end(&self) -> bool {
        let (first, total) = self.cursor_extent();
        total - first <= FETCH_MARGIN
    }

    /// The first row the cursor's message occupies, and how many rows the whole
    /// window occupies, at the panel's width.
    ///
    /// What the two paging triggers are measured against. Both are rows,
    /// because both are about where the reader is on the screen.
    fn cursor_extent(&self) -> (usize, usize) {
        let layout = self.row_layout();
        let first =
            rows::first_row_of_message(&layout, self.conversation.vim.cursor()).unwrap_or(0);

        (first, rows::total_rows(&layout))
    }

    /// Records that a fetch for `direction` has been asked for.
    ///
    /// Delegates to [`Outbox::begin_fetch`].
    pub fn begin_fetch(&mut self, direction: FetchDirection) {
        self.outbox.begin_fetch(direction);
    }

    /// Records that the fetch for `direction` is over, however it ended.
    ///
    /// Delegates to [`Outbox::end_fetch`].
    pub fn end_fetch(&mut self, direction: FetchDirection) {
        self.outbox.end_fetch(direction);
    }

    /// Whether a fetch for `direction` is in flight.
    #[must_use]
    pub const fn is_fetching(&self, direction: FetchDirection) -> bool {
        self.outbox.fetching.is_in_flight(direction)
    }

    /// Records that a direction has run out.
    ///
    /// A page's own length cannot say this: a short page and the last full one
    /// look the same once they are in the window. Only the cursor that asked
    /// knows, and the trigger reads the window — so its answer has to arrive
    /// here.
    pub fn exhaust(&mut self, direction: FetchDirection) {
        match direction {
            // Nothing is loaded, so there is no end to have run out of.
            FetchDirection::Latest => {}
            FetchDirection::Older => self.conversation.conversation.window.exhausted_older = true,
            FetchDirection::Newer => self.conversation.conversation.window.exhausted_newer = true,
        }
    }

    // ---- what the reader asked for --------------------------------------

    /// Takes the operation the reader asked for, if there is one.
    ///
    /// Delegates to [`Outbox::take_action`].
    pub fn take_action(&mut self) -> Option<Action> {
        self.outbox.take_action()
    }

    /// Records that a send for `temp_id` is in flight.
    ///
    /// Delegates to [`ConversationState::begin_send`].
    pub fn begin_send(&mut self, temp_id: i64) {
        self.conversation.begin_send(temp_id);
    }

    /// Releases the in-flight gate, if it is still held for `temp_id`.
    ///
    /// Delegates to [`ConversationState::end_send`].
    pub fn end_send(&mut self, temp_id: i64) {
        self.conversation.end_send(temp_id);
    }

    /// Replaces a send's placeholder with the message the server accepted.
    ///
    /// Delegates to [`ConversationState::confirm_sent`].
    pub fn confirm_sent(&mut self, temp_id: i64, real: Message) -> bool {
        self.conversation.confirm_sent(temp_id, real)
    }

    /// Marks a send as failed, keeping the message and recording why.
    ///
    /// Delegates to [`ConversationState::fail_send`].
    pub fn fail_send(&mut self, temp_id: i64, reason: String) -> bool {
        self.conversation.fail_send(temp_id, reason)
    }

    /// Removes a failed message and the reason recorded for it.
    ///
    /// Delegates to [`ConversationState::dismiss_failed`].
    pub fn dismiss_failed(&mut self, temp_id: i64) -> bool {
        self.conversation.dismiss_failed(temp_id)
    }

    // ---- transient status -----------------------------------------------

    /// Shows `text` on the status line for a while, then reverts.
    ///
    /// Delegates to [`UiState::flash`].
    pub fn flash(&mut self, text: impl Into<String>) {
        self.ui.flash(text);
    }

    /// Reverts a transient status once its time is up.
    ///
    /// Delegates to [`UiState::expire_status`].
    pub fn expire_status(&mut self, now: Instant) -> bool {
        self.ui.expire_status(now)
    }

    /// Writes a sentence the reader must not lose.
    ///
    /// Straight to [`UiState::status`] rather than through [`App::flash`], and
    /// clearing any flash deadline on the way: a persistent sentence ends in an
    /// event that brings its own sentence, so it must not expire back to idle
    /// while what it reports is still true. The contract [`App::flash`]
    /// documents from the other side.
    ///
    /// Delegates to [`UiState::show_persistent`].
    pub fn set_status(&mut self, status: impl Into<String>) {
        let status: String = status.into();
        self.ui.show_persistent(&status);
    }

    // ---- the peer's typing -----------------------------------------------

    /// The presence the peer was last reported with, if one has arrived.
    ///
    /// Sticky until the next update for that peer: nothing here expires it, and
    /// `None` means no update has named this peer yet.
    #[must_use]
    pub fn peer_presence(&self, peer_id: i64) -> Option<Presence> {
        self.ui.peer_presence.get(&peer_id).copied()
    }

    /// Whether the peer in the conversation on show is being shown as typing.
    ///
    /// Pure: liveness is the deadline being set, and it is
    /// [`App::expire_typing`] that clears it once the deadline has passed. A
    /// frame cannot read a clock, and it does not need to — the loop calls the
    /// expiry on its tick, so by the time a frame is drawn the deadline is
    /// either still in the future or already gone.
    #[must_use]
    pub fn peer_is_typing(&self) -> bool {
        self.ui
            .typing_until
            .is_some_and(|(chat, _)| chat == self.conversation.conversation.window.chat_id)
    }

    /// Stops showing the peer as typing once its deadline has passed.
    ///
    /// Delegates to [`UiState::expire_typing`].
    pub fn expire_typing(&mut self, now: Instant) -> bool {
        self.ui.expire_typing(now)
    }

    // ---- key handling --------------------------------------------------

    /// The top-level key dispatch.
    ///
    /// Delegates to [`coordinate::handle_key`]. Sizing the highlight afterwards
    /// when a card opened: the rows render from `&App`, so the measurement
    /// stays here while the dispatch moves behind the seam.
    pub fn handle_key(&mut self, key: KeyEvent) {
        // The context-free globals answer here, before the precompute.
        // `coordinate::preempt_key` answers `Ctrl-C` the same way below, so the
        // two arms agree; the lower one can go in a later cleanup.
        if key_to_action(key, self.ui.mode) == AppAction::Quit {
            self.ui.quit();
            return;
        }
        let rows = crate::card::rows(self);
        let lines = self.card_yanked();
        let is_contact = matches!(self.card_subject(), crate::card::CardSubject::Contact(_));
        let cursor = self.profile_cursor();
        let layout = self.row_layout();
        let opened = coordinate::handle_key(
            &mut self.ui,
            &mut self.session,
            &mut self.profile,
            &mut self.pending,
            &mut self.list,
            &mut self.outbox,
            &mut self.conversation,
            &mut self.input,
            &mut self.drafts,
            &rows,
            lines,
            is_contact,
            cursor,
            &layout,
            key,
        );
        if opened {
            self.profile.resize(crate::card::navigable(&rows));
        }
    }

    /// Opens the buffer to answer the message under the cursor.
    ///
    /// Delegates to [`coordinate::start_reply`].
    #[cfg(test)]
    fn start_reply(&mut self) {
        coordinate::start_reply(&mut self.ui, &mut self.conversation, &mut self.input);
    }

    /// Opens the conversation with a person the search found, or focuses the one
    ///
    /// Delegates to [`coordinate::open_user`].
    pub fn open_user(&mut self, user: &UserCandidate) {
        coordinate::open_user(
            &mut self.ui,
            &mut self.list,
            &mut self.outbox,
            &mut self.pending,
            &mut self.conversation,
            &mut self.input,
            &mut self.drafts,
            &mut self.profile,
            user,
        );
    }

    /// Answers `/`: scans the window now, and asks the server if it can do
    ///
    /// Delegates to [`coordinate::run_search`].
    #[cfg(test)]
    fn run_search(&mut self, query: &str) {
        coordinate::run_search(
            &mut self.ui,
            &mut self.conversation,
            &mut self.outbox,
            query,
        );
    }

    /// Replaces the local matches with the server's answer, if it is still
    /// wanted.
    ///
    /// Delegates to [`ConversationState::apply_searched`].
    pub fn apply_searched(
        &mut self,
        chat_id: i64,
        query: &str,
        ids: Vec<i64>,
        total: usize,
    ) -> bool {
        self.conversation.apply_searched(chat_id, query, ids, total)
    }

    /// Records that the server pass for `query` failed, keeping the local list.
    ///
    /// Delegates to [`ConversationState::search_failed`].
    pub fn search_failed(&mut self, query: &str, reason: String) {
        self.conversation.search_failed(query, reason);
    }

    /// Fills the new-conversation list with the answer to `query`, if it is still
    /// wanted.
    ///
    /// Delegates to [`ConversationState::apply_users`].
    pub fn apply_users(&mut self, query: &str, candidates: Vec<UserCandidate>) -> bool {
        self.conversation.apply_users(query, candidates)
    }

    /// Records that the lookup for `query` failed.
    ///
    /// Delegates to [`ConversationState::fail_users`].
    pub fn fail_users(&mut self, query: &str, reason: String) -> bool {
        self.conversation.fail_users(query, reason)
    }

    // ---- the sign-in flow -----------------------------------------------

    /// The sign-in surface, if it is up.
    #[must_use]
    pub fn signin(&self) -> Option<&SignIn> {
        self.session.signin.as_ref()
    }

    /// The field the reader is filling in, which is whatever step the flow is at.
    ///
    /// Delegates to [`SessionState::signin_field`].
    #[must_use]
    pub fn signin_field(&self) -> Option<LoginField> {
        self.session.signin_field()
    }

    /// Records that the client is to be brought up again.
    ///
    /// Delegates to [`coordinate::request_retry`].
    pub fn request_retry(&mut self) {
        coordinate::request_retry(&mut self.ui, &mut self.session);
    }

    /// The retry the reader asked for, once.
    ///
    /// Delegates to [`SessionState::take_retry_request`].
    pub fn take_retry_request(&mut self) -> bool {
        self.session.take_retry_request()
    }

    /// Puts the sign-in flow up, with the phone field open and the configured
    /// number in it.
    ///
    /// Delegates to [`SessionState::set_client_available`].
    pub fn set_client_available(&mut self, available: bool) {
        self.session.set_client_available(available);
    }

    /// What the network has last told the screen about the connection.
    ///
    /// Reads [`UiState::connection`]: the structured half of what the status
    /// sentences say in words, for the indicator that will draw it.
    #[must_use]
    pub fn connection(&self) -> ConnectionState {
        self.ui.connection
    }

    /// Records what the network last told the screen about the connection.
    ///
    /// Delegates to [`UiState::set_connection`]. Called from `app`'s
    /// `net::apply` beside the sentence for the same event, and nowhere else:
    /// the state and the sentence are two answers about one event, and two
    /// writers would let them disagree.
    pub fn set_connection(&mut self, connection: ConnectionState) {
        self.ui.set_connection(connection);
    }

    /// The one way in, whatever the reader came from: the signed-out card's
    ///
    /// Delegates to [`coordinate::begin_signin`].
    pub fn begin_signin(&mut self) {
        coordinate::begin_signin(
            &mut self.ui,
            &mut self.session,
            &mut self.conversation,
            &mut self.input,
            &mut self.profile,
        );
    }

    /// Puts up the sentence a machine with no credentials gets.
    ///
    /// Delegates to [`coordinate::begin_no_credentials`].
    pub fn begin_no_credentials(&mut self) {
        coordinate::begin_no_credentials(
            &mut self.ui,
            &mut self.session,
            &mut self.conversation,
            &mut self.input,
            &mut self.profile,
        );
    }

    /// Records that the stored session is one Telegram no longer knows.
    ///
    /// The flow starts as it always does, with one row differing: the phone
    /// carries an offer instead of a state, because there is no step to go back
    /// to — the session is gone and the number is the whole way in again.
    pub fn begin_stale_signin(&mut self) {
        self.begin_signin();
        self.flash("the stored session is no longer valid — sign in again");
        if let Some(flow) = self.session.signin.as_mut().and_then(SignIn::flow_mut) {
            flow.stale = true;
        }
    }

    /// Telegram answered: the flow is at `state` now.
    ///
    /// Delegates to [`coordinate::login_advanced`].
    pub fn login_advanced(&mut self, state: domain::session::SessionState, hint: Option<String>) {
        coordinate::login_advanced(
            &mut self.session,
            &mut self.input,
            &mut self.ui,
            &mut self.conversation,
            &mut self.profile,
            state,
            hint,
        );
    }

    /// Telegram refused, in the reader's own words.
    ///
    /// Delegates to [`coordinate::login_refused`].
    pub fn login_refused(&mut self, sentence: String, used: u8) {
        coordinate::login_refused(
            &mut self.session,
            &mut self.input,
            &mut self.ui,
            &mut self.conversation,
            &mut self.profile,
            sentence,
            used,
        );
    }

    /// The account is signed in: the surface has nothing left to say.
    ///
    /// Delegates to [`coordinate::login_complete`].
    pub fn login_complete(&mut self) {
        coordinate::login_complete(
            &mut self.session,
            &mut self.input,
            &mut self.ui,
            &mut self.conversation,
            &mut self.profile,
        );
    }

    // ---- rendering -----------------------------------------------------

    pub fn render(&self, frame: &mut Frame<'_>) {
        self.render_layout(&self.row_layout(), frame);
    }

    /// The same frame, with the conversation panel drawing `layout` rather than a
    /// layout of its own.
    ///
    /// The layout is the caller's so that it is laid out once per frame and
    /// drawn, sliced and measured from that one answer — and so that a test can
    /// draw a layout holding a row that names no message, which nothing in the
    /// program emits yet.
    pub(crate) fn render_layout(&self, layout: &[RowSpan], frame: &mut Frame<'_>) {
        // What the conversation panel places is what it drew this frame; a frame
        // that draws no panel places nothing.
        self.ui.placements.borrow_mut().clear();
        let area = frame.area();

        // The bar is as tall as the draft the reader is typing in, up to its
        // ceiling, and the conversation takes what is left. Nothing is cached
        // between frames: the height is one wrap of a bounded string, and a
        // cache would be a second thing to keep in step with the line.
        let width = area.width.saturating_sub(2).max(1);
        // `Length` counts `u16` rows and the ceiling is `INPUT_MAX_ROWS`, so this
        // cannot overflow in practice; saturating rather than converting keeps
        // the release profile's `panic = "abort"` from having a say about it.
        let input = 2 + widgets::input_bar::content_rows(self, width);
        let input = u16::try_from(input).unwrap_or(u16::MAX);

        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(input),
                Constraint::Length(1),
            ])
            .split(area);

        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(vertical[0]);

        widgets::chat_list::render(self, horizontal[0], frame);
        // The right-hand column, whichever of the two is in it — or the sign-in
        // surface over it, which is an overlay and not a pane: the flow outlives
        // the card that named it. The chat list is not in the match: it is the
        // other column and it is always there.
        match self.session.signin.as_ref() {
            Some(signin) => widgets::signin::render(self, signin, horizontal[1], frame),
            None => match self.ui.pane {
                Pane::Conversation => {
                    widgets::conversation::render(self, horizontal[1], frame, layout);
                }
                Pane::Profile(_) => widgets::profile::render(self, horizontal[1], frame),
            },
        }
        widgets::input_bar::render(self, vertical[1], frame);
        widgets::emoji_popup::render(self, vertical[0], vertical[1], frame);
        // The new-conversation choices are drawn over the chat list they were
        // asked from, and above it — the `Clear` inside the widget is what makes
        // the list a panel rather than a smear of two lists.
        widgets::user_list::render(self, horizontal[0], frame);
        // The forward picker is drawn over the conversation column it forwards
        // from, above the messages and the bar, and below the status line.
        widgets::forward_picker::render(self, horizontal[1], frame);
        widgets::status_bar::render(self, vertical[2], frame);
    }

    /// Records how many message rows the conversation panel has room for.
    ///
    /// Delegates to [`UiState::record_rows`].
    pub fn record_rows(&self, rows: usize) {
        self.ui.record_rows(rows);
    }

    /// The columns the conversation panel's messages have room for, as of the
    /// last frame.
    ///
    /// What the rows are laid out at. Recorded by the panel after the scrollbar
    /// has taken its column, because a message must never be laid out — or
    /// drawn — under the bar.
    #[must_use]
    pub fn body_width(&self) -> u16 {
        self.ui.metrics.body_width.get()
    }

    /// Records how many columns the conversation panel's messages have room for.
    ///
    /// Delegates to [`UiState::record_body`].
    pub fn record_body(&self, width: u16) {
        self.ui.record_body(width);
    }

    /// The unix second the reader's clock last read.
    ///
    /// Zero until the host records one, which is what makes a day label say a
    /// date rather than `Today`: this crate owns no clock, so a relative label
    /// would otherwise be a claim it cannot support.
    #[must_use]
    pub fn now(&self) -> i64 {
        self.ui.metrics.now.get()
    }

    /// Records what the reader's clock says, in unix seconds.
    ///
    /// Delegates to [`UiState::record_now`].
    pub fn record_now(&self, now: i64) {
        self.ui.record_now(now);
    }

    /// The rows every message in the window occupies, laid out at the panel's
    /// width.
    ///
    /// The single source of truth for the panel's geometry. Everything that
    /// needs to know how tall something is — the viewport, the scrollbar, the
    /// paging keys, the fetch triggers — asks here rather than working it out
    /// again, because two measurements of one thing is a bug waiting for the
    /// case where they disagree.
    ///
    /// A pure function of the window's messages, the open draft, [`App::body_width`]
    /// and [`App::now`], and of nothing else: not the cursor, not the mode, not when
    /// it was asked. A layout worked out before a page lands is thrown away
    /// rather than kept, which is why a [`RowSpan`] is named by message id.
    ///
    /// The entries are the window's messages and the day separators in front of
    /// them: a separator takes a row of the screen between two days and is
    /// counted by the scrollbar beside it, so it belongs in here rather than
    /// counted on the side. [`RowKind`] is what tells the two apart, and the
    /// cursor — which is a message index — never rests on one. The open draft,
    /// when it has words, is the last entry; it is drawn but never counted.
    #[must_use]
    pub fn row_layout(&self) -> Vec<RowSpan> {
        let width = self.body_width();
        let now = self.now();
        let mut laid_out: Vec<RowSpan> =
            Vec::with_capacity(self.conversation.conversation.window.len());
        let mut first = 0;
        // The day of the last message that had one. A send still on its way has
        // no day of its own, so it neither opens a day nor closes the search for
        // the next message that does (T3).
        let mut day: Option<i64> = None;

        for (index, message) in self.conversation.conversation.window.iter().enumerate() {
            if rows::opens_day(message.timestamp, day) {
                laid_out.push(RowSpan {
                    kind: RowKind::Other {
                        label: rows::separator_label(message.timestamp, now).into_owned(),
                    },
                    message_id: None,
                    first,
                    len: 1,
                    text: 0..0,
                });
                first += 1;
            }
            day = rows::day_of(message.timestamp).or(day);

            // The row's own slice of the message, and its height. Neither reads
            // the bidi mode: a row is broken logically and permuted at paint
            // time, so the same window is the same height in either mode.
            let text = 0..message.display_body().len();
            let rows = rows::message_rows(self, message, rows::group_of(self, index), width);
            // A decoded sticker paints its picture below its (empty) text: the
            // block rows come after the text rows, so the two heights add.
            let block = rows::sticker_block_rows(message, &self.conversation.stickers);
            let len = rows.len() + block;

            laid_out.push(RowSpan {
                kind: RowKind::Message { index },
                message_id: Some(message.id),
                first,
                len,
                text,
            });
            first += len;
        }

        // The open chat's draft, when it is a message being written and has
        // words in it: the same test `park_draft` makes, so a `:` command or a
        // search is never drawn as one. A chat that is not open has no draft.
        let draft = self.input.line.text();
        if self.input.line.purpose().is_buffer()
            && !draft.is_empty()
            && self.conversation.conversation.window.chat_id != 0
        {
            laid_out.push(RowSpan {
                kind: RowKind::Draft,
                message_id: None,
                first,
                len: rows::draft_rows(draft, width).len(),
                text: 0..draft.len(),
            });
        }

        laid_out
    }

    /// The rows the panel spends on what is not a message, in a panel `height`
    /// rows tall: the fetches it is announcing, and the open draft's rows.
    ///
    /// One answer, read by the panel for both what it draws and what the
    /// messages have left, because the two cannot be allowed to disagree about
    /// how tall an announcement or a draft is. `layout` is the one the panel
    /// draws, whose trailing [`RowKind::Draft`] span, if any, gives the draft's
    /// rows.
    ///
    /// Cap: the draft may take every row of the panel but one, so the messages
    /// always keep a row. A draft taller than that is drawn in the rows it is
    /// given and the rest is cut off; the input bar still shows all of it. The
    /// cap only changes how many rows the messages may fill, never a count.
    ///
    /// Only while following the newest message: the draft is drawn after the
    /// slice, so scrolled up it would sit under an older message. Then it is not
    /// drawn and reserves nothing.
    #[must_use]
    pub fn reserved(&self, layout: &[RowSpan], height: usize) -> Reserved {
        let mut reserved = Reserved {
            older: self.outbox.fetching.is_in_flight(FetchDirection::Older),
            jumping: self.pending.pending_jump.is_some(),
            newer: self.outbox.fetching.is_in_flight(FetchDirection::Newer),
            draft: 0,
        };
        let draft = layout
            .last()
            .filter(|span| span.kind == RowKind::Draft)
            .filter(|_| self.conversation.conversation.auto_follow())
            .map_or(0, |span| span.len);
        let room = height.saturating_sub(reserved.above() + reserved.below());

        reserved.draft = draft.min(room.saturating_sub(1));
        reserved
    }

    /// What a jump in flight is called, on the panel and on the status line.
    ///
    /// `JUMP_LABEL` names a destination — the first unread message — and a jump
    /// to a reply is not going there, so each kind has its own sentence and both
    /// places that say one read this.
    #[must_use]
    pub fn jump_label(&self) -> &'static str {
        self.pending
            .pending_jump
            .map_or(JUMP_LABEL, |jump| jump.kind.label())
    }

    /// The rows the conversation panel shows, given a `budget` of room for
    /// messages and the `layout` it is drawing from.
    ///
    /// While the view is pinned to the newest message the slice ends at it;
    /// otherwise it is centred on the cursor, which is the reader's place, and
    /// then pulled back inside the window so that the slice is always exactly as
    /// tall as the panel and never starts past the end.
    ///
    /// The layout is the caller's, because the caller has one to draw: laying it
    /// out a second time to ask what the first one says is the work this
    /// arrangement exists to avoid.
    #[must_use]
    pub fn viewport(&self, layout: &[RowSpan], budget: usize) -> Slice {
        rows::slice(
            layout,
            self.conversation.vim.cursor(),
            budget,
            self.conversation.conversation.auto_follow(),
        )
    }

    // ---- helpers -------------------------------------------------------

    #[must_use]
    pub fn current_chat_id(&self) -> i64 {
        self.list
            .list
            .chats
            .get(self.list.selected_chat)
            .map_or(0, |c| c.id)
    }

    /// The prefix a prompt's text is drawn behind: `:` and `/`, and nothing for
    /// a message, a reply or an edit.
    ///
    /// Read from the line rather than held beside it, because the line's purpose
    /// *is* what this names — two fields answering the same question is two
    /// things to disagree. A sign-in field has no prefix either: a phone number
    /// is not a command and a code is not a query.
    #[must_use]
    pub fn prompt_prefix(&self) -> &'static str {
        match self.input.line.purpose() {
            // Everything that is text rather than a question: a message, a
            // reply, an edit, and the three sign-in fields, which are the
            // reader's own words for the same reason a message is.
            PromptKind::Message
            | PromptKind::Reply
            | PromptKind::Edit
            | PromptKind::Phone
            | PromptKind::Code
            | PromptKind::Password => "",
            PromptKind::Command => ":",
            // A message search and a person search are both queries, so they
            // share the prefix. They are told apart by what answers them — the
            // status label and the pane the list is drawn over — not by a
            // second glyph.
            PromptKind::Search | PromptKind::NewChat => "/",
        }
    }

    /// What the status line shows.
    ///
    /// A confirmation outranks everything: it is a question waiting for an
    /// answer, and it is over as soon as one is given. A selection comes next —
    /// also state the reader must not lose, and the one thing on screen whose
    /// extent is not otherwise visible. The new-conversation search is below it,
    /// and the conversation's own search below that; both outrank a transient
    /// status, because each describes state the reader must not lose: neither is
    /// a `flash`, so `expire_status` must not be able to take one away. Below
    /// them, a jump in flight — what the reader has just asked for — and then the
    /// full reason a failed message failed while the cursor is on it, then
    /// whatever was written to the status, and last a cached window's wait for
    /// its newest page, which outranks only the resting hint.
    ///
    /// A key inside the line is above all of them, because a keystroke cannot be
    /// deferred and none of the rest is a question waiting for a reply: a reader
    /// halfway through `dw` needs the rest of that line before anything else on
    /// the screen.
    #[must_use]
    pub fn status_text(&self) -> String {
        // The sign-in flow's refusal, above the line's own hint.
        //
        // The hint is about the keys and this is about the value the reader just
        // typed, and a reader who cannot see that the code was wrong types the
        // same one again — so the refusal ranks with the selection and the search
        // above the transient status, and above the line too. It is the one place
        // the line's hint does not win, and it is that because the line is not the
        // reader's words here: it is Telegram's code, and the state of it is
        // theirs rather than the draft's.
        if let Some(refusal) = self
            .session
            .signin
            .as_ref()
            .and_then(SignIn::flow)
            .and_then(|flow| flow.login.refusal.as_ref())
        {
            return refusal.clone();
        }
        if self.ui.focus == Focus::Input {
            return widgets::input_bar::hint(self).to_owned();
        }
        match &self.conversation.confirm {
            Some(ConfirmKind::Quit) => return QUIT_PROMPT.to_owned(),
            Some(ConfirmKind::Logout) => return LOGOUT_PROMPT.to_owned(),
            Some(ConfirmKind::DeleteMessages {
                ids,
                outgoing,
                skipped,
            }) => return delete_prompt(ids, *outgoing, *skipped),
            None => {}
        }
        // The picker outranks the selection's own note: it is the question the
        // reader is answering, and the note would describe the selection only.
        if self.conversation.picking().is_some() {
            return widgets::input_bar::FORWARD_PICKER_HINT.to_owned();
        }
        if let Some(selection) = &self.conversation.selection {
            return selection_note(selection, self.selection_len().unwrap_or(0));
        }
        // The new-conversation search outranks the conversation's own, and both
        // describe state the reader must not lose, so both sit above the flash.
        // A reader who asked for a person is asking the wider question, and its
        // label names where the answer stands.
        if self.conversation.user_search.is_active() {
            return self.conversation.user_search.label();
        }
        if self.conversation.search.is_active() {
            return self.conversation.search.label();
        }
        if self.pending.pending_jump.is_some() {
            return self.jump_label().to_owned();
        }
        if let Some(message) = self.cursor_message()
            && let Some(reason) = self.conversation.conversation.failure(message.id)
        {
            return reason.to_owned();
        }
        // A status worth reading — a refusal, a failure — outranks the hint. The
        // resting state is the hint rather than the program's name, because the
        // name says nothing and a bar showing it looks like a bar with nothing
        // in it, which is exactly what a half-written message used to look like.
        if self.ui.status != IDLE_STATUS {
            return self.ui.status.clone();
        }
        // Below everything that was written to the status, so a cache can never
        // talk over a failure: a revalidation that fails says `history:` like any
        // other page, and `offline:` keeps the line it always had.
        if self.is_revalidating() {
            return REVALIDATING_LABEL.to_owned();
        }

        widgets::input_bar::hint(self).to_owned()
    }
}

// ---- helpers -----------------------------------------------------------

/// What the status line says about a selection, of `selected` characters or
/// messages as the selection is of.
///
/// The wording lives here rather than in the panel because the title is one row
/// wide and can only carry the count. The unit is stated because one number
/// cannot carry both: three characters and three messages are both "3", and a
/// reader who has just pressed `v` has to be able to tell which of the two they
/// are holding.
fn selection_note(selection: &Selection, selected: usize) -> String {
    let what = if selection.text_range().is_some() {
        "character(s)"
    } else {
        "message(s)"
    };

    format!("{selected} {what} selected — Esc clears")
}
// ---- sample data -------------------------------------------------------

/// The conversation the sample messages belong to.
#[cfg(test)]
const MOCK_CHAT: i64 = 1;

/// Converts a `usize` (e.g. a length or index) into the `i64` id space.
///
/// Saturates rather than panicking. The release profile sets `panic = "abort"`,
/// so an identifier derived from a length must not be able to take the process
/// down, and no real machine holds anywhere near `i64::MAX` elements — the
/// saturated value is unreachable rather than merely unlikely.
#[cfg(test)]
fn to_id(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// The account the sample profile panel is drawn from.
///
/// Every optional field set, so the panel's tests see the panel a reader with a
/// complete profile gets rather than the narrower one a bare account produces.
#[cfg(test)]
pub(crate) fn mock_account() -> Account {
    Account {
        user_id: 1_234_567,
        first_name: "Ada".into(),
        last_name: "Lovelace".into(),
        username: Some("ada".into()),
        phone: Some("+15551234567".into()),
        birthday: Some(domain::account::Birthday {
            day: 10,
            month: 12,
            year: Some(1815),
        }),
        bio: Some("Notes on the analytical engine.".into()),
        presence: None,
    }
}

#[cfg(test)]
fn mock_chats() -> Vec<Chat> {
    use domain::chat::ChatKind;

    vec![
        Chat {
            id: MOCK_CHAT,
            title: "Ada Lovelace".into(),
            kind: ChatKind::Private,
            last_message: Some("See you at the demo.".into()),
            // The conversation the sample data opens, so nothing in it is
            // waiting to be read: `gg` means the top of it, and the tests that
            // are about where unread messages start say how many there are.
            unread_count: 0,
            // Matches the last of `mock_messages`, which is where the preview
            // text came from.
            last_message_id: Some(10),
            last_timestamp: Some(1_730_000_000),
            pinned: false,
            presence: None,
            deleted: false,
        },
        Chat {
            id: 2,
            title: "Grace Hopper".into(),
            kind: ChatKind::Private,
            last_message: Some("The compiler is ready.".into()),
            unread_count: 0,
            last_message_id: None,
            last_timestamp: Some(1_729_999_000),
            pinned: false,
            presence: None,
            deleted: false,
        },
        Chat {
            id: 3,
            title: "Alan Turing".into(),
            kind: ChatKind::Private,
            last_message: Some("Halting problem again…".into()),
            unread_count: 1,
            last_message_id: None,
            last_timestamp: Some(1_729_998_000),
            pinned: false,
            presence: None,
            deleted: false,
        },
    ]
}

#[cfg(test)]
fn mock_messages() -> Vec<Message> {
    let texts = [
        "Hey, is the build green?",
        "Yes — clippy is happy.",
        "Nice. Did you pin the toolchain?",
        "1.85.0, edition 2024.",
        "Perfect. Let's meet tomorrow.",
        "I'll bring the slides.",
        "And the benchmarks.",
        "50 MB RSS or bust.",
        "No pressure then :)",
        "See you at the demo.",
    ];
    texts
        .iter()
        .enumerate()
        .map(|(i, t)| Message {
            id: to_id(i) + 1,
            chat_id: MOCK_CHAT,
            text: Cow::Borrowed(*t),
            timestamp: 1_730_000_000 + to_id(i) * 60,
            status: MessageStatus::Received,
            is_outgoing: i % 2 == 0,
            reply_to: None,
            media: None,
        })
        .collect()
}
// ---- tests -------------------------------------------------------------

#[cfg(test)]
mod tests;
