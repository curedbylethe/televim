//! Top-level TUI state.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::time::{Duration, Instant};

#[cfg(test)]
use std::borrow::Cow;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use domain::account::Account;
use domain::chat::Chat;
use domain::history::{CONVERSATION_WINDOW, ConversationView, ConversationWindow, unread_target};
use domain::message::{Message, MessageStatus};
use domain::search::{SearchState, word_prefix_match};
use domain::selection::{Mark, Selection};
use domain::updates::{ChatList, UpdateEvent};
use domain::vim::{CharMotion, Motion, VimState, char_motion};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};

use crate::bidi::BidiMode;
use crate::emoji;
use crate::jumplist::Jumplist;
use crate::line::{LineEditor, LineVerdict};
use crate::rows::{self, Reserved, RowKind, RowSpan, Slice};
use crate::theme::Theme;
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

/// How many message rows the conversation panel is assumed to have before it
/// has been drawn once.
///
/// Only the panel knows the real number, and only during a frame. This is what
/// the key handling falls back on in between, and it is deliberately a normal
/// size rather than a small one: a page that overshoots is clamped.
const ASSUMED_ROWS: usize = 20;

/// How many columns the conversation panel's messages are assumed to have
/// before it has been drawn once.
///
/// The same fallback as [`ASSUMED_ROWS`] and for the same reason: the layout
/// has to be answerable before the first frame.
const ASSUMED_BODY_WIDTH: u16 = 80;

/// What the status line shows before anything has happened.
const IDLE_STATUS: &str = "televim";

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
const FLASH_FOR: Duration = Duration::from_secs(5);

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
const ACTION_QUEUE: usize = 4;

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

    /// A file on disk, in plaintext.
    ///
    /// Named for the fact rather than for the type: the panel renders
    /// `session: /path/to/file (plaintext)`, and a variant called `File` would
    /// have that fact nowhere to live.
    PlaintextFile(PathBuf),
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
struct ChatChoice {
    /// Where in the list the highlight is.
    index: usize,

    /// When it got there.
    at: Instant,
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
    /// command, a search and the three sign-in fields are a single line the
    /// reader types and submits, and get insert only — `Esc` returns to the
    /// conversation with the text kept, and there is no normal mode to leave.
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
struct Deletion {
    /// The identifiers the server knows, oldest first.
    ids: Vec<i64>,

    /// How many of them the reader's own account sent.
    outgoing: usize,

    /// How many were placeholders and had to be left out.
    skipped: usize,
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
}

/// A `f`, `t`, `F` or `T` that has been pressed and is waiting for its character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Find {
    /// Which way to look.
    forward: bool,

    /// Whether to land on the character itself rather than one short of it.
    onto: bool,
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
    fn set(lines: Vec<String>) -> Self {
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
struct Fetching {
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
    fn set(&mut self, direction: FetchDirection, in_flight: bool) {
        let slot = match direction {
            FetchDirection::Latest => &mut self.latest,
            FetchDirection::Older => &mut self.older,
            FetchDirection::Newer => &mut self.newer,
        };
        *slot = in_flight;
    }

    /// Forgets every direction, which is what opening another conversation
    /// does.
    fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Every piece of the screen's state, and the only thing that draws.
///
/// `struct_excessive_bools` is off deliberately: the booleans here are the
/// screen's own facts rather than a state machine wearing a disguise — a
/// [`Focus`] and a [`Mode`] already carry the two axes that could be enums, and
/// the sign-in's `credentials_configured` is a fact about the machine rather than
/// a step the reader is in.
#[allow(clippy::struct_excessive_bools)]
pub struct App {
    pub mode: Mode,
    pub focus: Focus,
    pub theme: Theme,

    /// What the right-hand pane is showing.
    ///
    /// A field rather than a variant of [`Focus`], because the two answer
    /// different questions: this says what the right-hand column holds, and
    /// `Focus` says where a keystroke lands. See [`Pane`].
    pub pane: Pane,

    /// What the profile panel shows, and why it might show nothing.
    pub account: AccountState,

    /// Where the session is kept, as the panel says it.
    pub session_store: SessionStore,

    /// The sign-in surface, when it is up.
    ///
    /// `None` for every reader who is already signed in, which is most of the
    /// life of the program. `Some` is an overlay and not a pane: see [`SignIn`].
    pub signin: Option<SignIn>,

    /// The phone number configuration carried, if there is one.
    ///
    /// Held rather than read from a prompt because `:signin` needs it twice —
    /// to fill the field in, and to draw the row that says where the code went —
    /// and because it is the one thing about a sign-in a reader does not have to
    /// type. It is rewritten by the network with the number the request actually
    /// went out with, so it is "where the code went" rather than "what the
    /// configuration suggested".
    pub phone: String,

    /// What the configuration carried for the login code, if any.
    ///
    /// A **pre-fill**, and the only place a code may come from that is not
    /// Telegram: Telegram sends the code, and the reader types it. This saves
    /// that on a machine where the code is already written down, and it is read
    /// once — when the step opens — because a wrong code has to be retyped, not
    /// restored.
    pub code_prefill: String,

    /// What the configuration carried for the two-factor password, if any.
    ///
    /// A pre-fill on the same terms as [`App::code_prefill`]: read when the step
    /// opens, never restored after a refusal.
    pub password_prefill: String,

    /// Whether this machine carries application credentials at all.
    ///
    /// **The gate on the flow.** Without an `api_id` and `api_hash` there is no
    /// client to sign in *to*, so a phone field would be asking the reader for
    /// something the program still could not do with — and the sentence that
    /// says so is a better screen than a form that cannot be finished.
    pub credentials_configured: bool,

    /// Whether a client is there to carry a request.
    ///
    /// **The gate on the sign-in's `waiting` flag.** A request no client will
    /// take is not a request on its way, so the flow must not be told one is:
    /// the panel would say "Checking…" for an answer that is never coming, and
    /// nothing else on the screen can put it right. `false` until the caller
    /// says otherwise — this side of the boundary cannot see a client.
    client_available: bool,

    /// The highlight on the profile panel's rows.
    ///
    /// A second [`VimState`] rather than a share of the conversation's, because
    /// that one's total is the conversation window's length: driving two lists
    /// from one value means each resize moves the other's cursor. `VimState`
    /// knows nothing about what an item is, which is what makes the second one
    /// free.
    profile_vim: VimState,

    /// The subject the card is about: the account, or a contact by chat.
    ///
    /// A `ProfileId::User` already existed and was unreachable, and a chat is the
    /// handle a reader can actually name — a conversation on show is a person, and
    /// the chat list is where they were found.
    profile_subject: ProfileId,

    /// Where the inline position is within the cursor row's value.
    ///
    /// A **character** count, because every motion that produces one counts
    /// characters; it becomes a byte offset only where it is drawn, by
    /// [`rows::byte_span`]. Separate from [`VimState`] because that moves between
    /// rows and knows nothing about what a row is.
    profile_caret: usize,

    /// A card selection's fixed end, when there is one.
    ///
    /// A [`Mark`] and not a row index, because the card's two ends are the same
    /// two ends the conversation has: a row and a position within it, or the whole
    /// of it. Reusing the type is what makes `v` one keystroke here as it is
    /// there, rather than a second selection model beside the first.
    profile_visual: Option<domain::selection::Mark>,

    /// The contact whose profile the card on show is about, if it is about one.
    ///
    /// One at a time rather than one per person: a card is opened, read and
    /// closed, so a map would be a cache with no reader. The `peer_id` is what
    /// lets an answer be matched back to the card that asked for it.
    contact: Option<ContactProfile>,

    /// A count typed before a motion, as `12j` means twelve.
    profile_count: Option<u32>,

    /// `Ctrl-w` was pressed on a card and the next key is its argument.
    ///
    /// The pane's `h`/`l` became an inline motion, so pane navigation moved under
    /// a prefix. Bare `Ctrl-w` keeps its existing meaning — leaving the input line
    /// — which is the same prefix-with-a-bare-fallback shape `g`/`gg` has.
    profile_pending_w: bool,

    /// The conversations, and the messages the client has seen in them.
    ///
    /// One value rather than a list beside a window: an event from the feed
    /// moves both, and keeping them apart would leave the preview and the
    /// unread count somewhere the event never reached. [`App::apply_update`] is
    /// the one place either is folded in.
    list: ChatList,

    pub selected_chat: usize,

    /// The conversation on show, and where the reader is in it.
    ///
    /// The panel renders a slice of this, and the cursor below is the reader's
    /// place within it. Nothing here holds a whole conversation: the window is
    /// the ceiling on what the open chat costs.
    pub conversation: ConversationView,

    pub vim: VimState,

    /// What the reader is composing, and the editor working on it.
    ///
    /// Was a `String`, and was enough of a design not to notice it was wrong:
    /// append-only, no caret, and cleared by the key every reader presses
    /// reflexively. A line is a buffer with a caret in it, and it is the
    /// wrapper's whole job.
    pub line: LineEditor,

    /// The `:query` being completed, if there is one.
    ///
    /// `None` is the whole of "the popup is closed", and it is reached from five
    /// places: a query that stopped being one, a query nobody matches, the focus
    /// leaving the line, a submit, and an acceptance. There is no flag to fall
    /// out of step with the state it describes.
    emoji: Option<emoji::Trigger>,

    pub status: String,
    pub should_quit: bool,

    /// Set by `/` search: the query text, the matches, and where the walk is.
    ///
    /// One value rather than a list beside a query: the label, the highlight and
    /// `n`/`N` all read the same state, and keeping them apart would let the
    /// three disagree about which list is on screen.
    search: SearchState,

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

    /// What the reader has selected over the messages, if anything.
    ///
    /// `None` outside a selection — and a selection with no mode of its own: the
    /// conversation's [`Mode`] says whether a key is being applied to it, and a
    /// `dd` puts one here for as long as the prompt is up without ever asking
    /// for Visual.
    ///
    /// Both of its ends name a message by identifier, which is what lets it
    /// survive a page landing: see [`App::after_window_change`].
    selection: Option<Selection>,

    /// What the reader last yanked.
    ///
    /// A yank is about *this* conversation and does not follow the reader into
    /// another one: carrying it across would be a feature nobody asked for and
    /// would need its own answer about whether it survives the change. So
    /// [`App::select_chat_none`] forgets it along with everything else.
    register: Register,

    /// The text a yank asked to be copied to the system clipboard, if one is
    /// waiting to be written.
    ///
    /// Recorded rather than written, because `tui` does not hold stdout and a
    /// widget that writes to the terminal behind the renderer's back is a race.
    /// The caller that owns the terminal takes it with
    /// [`App::take_clipboard`], which it does on the same pass of its loop that
    /// the yank was read on — so this cannot outlive a conversation change by more
    /// than a frame, and clearing it here would lose a yank rather than a stale
    /// one.
    clipboard: Option<String>,

    /// The operations the reader asked for, waiting to be taken by the caller.
    ///
    /// The outbound half of the [`Jump`] pattern: recorded here because `tui`
    /// cannot reach the network, and taken once by the caller that can. A queue
    /// rather than a single slot, because two requests made inside one tick are
    /// two requests — a send followed by `/` has to perform both, not lose the
    /// send to the key that came after it.
    actions: VecDeque<Action>,

    /// When a transient status stops applying, if it is transient.
    status_until: Option<Instant>,

    /// When the peer stops being shown as typing, and in which conversation.
    ///
    /// One conversation rather than one per chat, because the note is drawn on
    /// the open conversation's title and nowhere else: an event for a chat the
    /// reader is not in is dropped, and a note for a chat left behind belongs to
    /// the chat that was left.
    ///
    /// The same deadline shape as [`App::status_until`] — an instant compared
    /// only where the loop supplies one — because a frame is drawn from a shared
    /// reference and cannot expire anything itself.
    typing_until: Option<(i64, Instant)>,

    /// The pages on their way from the network.
    fetching: Fetching,

    /// The jump the reader has asked for and no page has answered yet.
    ///
    /// Set when `gg` cannot be answered from what is loaded, and cleared when the
    /// page arrives — or fails, or comes back empty, because a jump that went
    /// wrong must not wedge the key. It is what makes the key idempotent: a
    /// second `gg` produces the same intent, which the caller recognises as one
    /// already on its way.
    pending_jump: Option<Jump>,

    /// Where the reader was before each jump they have taken.
    ///
    /// What `Ctrl-o` and `Ctrl-i` walk. It is per conversation and keyed by
    /// message identifier rather than by row, because a jump replaces the window
    /// and a row means a different message on either side of that.
    jumplist: Jumplist,

    /// The conversation the highlight has moved onto but has not been taken to.
    ///
    /// The same hand-over as [`App::pending_jump`] — recorded here because
    /// `tui` cannot reach the network — and for the same reason it carries a
    /// time: a reader holding `j` would otherwise fetch every conversation they
    /// scrolled past, and one page per chat as fast as a key repeats is how a
    /// scroll through the list becomes a flood wait.
    pending_chat: Option<ChatChoice>,

    /// Whether the reader has asked for the client to be brought up again.
    ///
    /// Recorded here because `tui` cannot reach the network, and taken by the
    /// one caller that can. **Not an [`Action`]**, and the reason is the state a
    /// retry is needed in: actions are drained only while there is a client, so a
    /// queued one would be invisible at exactly the moment it matters — a launch
    /// that came up `offline:`. One slot rather than a queue, because a second
    /// retry while one is on its way is the same retry; the caller says so with
    /// a transient status rather than asking twice.
    retry_requested: bool,

    /// Whether a `g` was just pressed in the chat list and a second one would
    /// take the reader to the top of it.
    ///
    /// The same latch as `dd` and for the same reason: `gg` is two presses in
    /// Vim, and a key held down is not two of them.
    pending_g: bool,

    /// A `f`, `t`, `F` or `T` waiting for the character to look for.
    ///
    /// Two keys rather than one, as in Vim, and a latch for the same reason `dd`
    /// has one: the key after `f` is the character, not a motion. The character
    /// itself is not recorded, because it has not been typed yet.
    pending_find: Option<Find>,

    /// How many message rows the conversation panel had room for as of the last
    /// frame.
    ///
    /// A cell rather than a field because a frame is drawn from a shared
    /// reference, and the panel is the only place that knows how tall the
    /// terminal made it. It is a measurement rather than state anything decides,
    /// so recording it late is the same as recording it at all.
    rows: Cell<usize>,

    /// How many columns the conversation panel's messages had room for as of
    /// the last frame, which is the width the rows are laid out at.
    ///
    /// Recorded beside [`App::rows`] and for the same reason: only the panel
    /// knows, and the layout cannot be worked out without it. What is given up
    /// for the scrollbar is given up before this, so no message is ever laid
    /// out — or drawn — under the bar.
    body_width: Cell<u16>,

    /// The unix second the reader's clock last read, as of the last frame.
    ///
    /// A measurement rather than state anything decides, for the same reason as
    /// [`App::rows`] and [`App::body_width`]: only the host owns a clock, and this
    /// crate reads none ([`crate::date`] is pure). Zero means no clock has been
    /// recorded, which the day labels read as "say the date rather than `Today`"
    /// rather than as 1970.
    now: Cell<i64>,

    /// How far each conversation this client has been told about has been read.
    ///
    /// One number per conversation, kept here rather than on
    /// [`ConversationView`] because the view is replaced on every chat switch:
    /// a reader who looks away and comes back must find the reading of that
    /// conversation as it was, not as a fresh view believes it. The view holds
    /// the one on show; this holds the rest.
    ///
    /// Monotone per conversation, because the wire says so: `max_id` is a
    /// watermark, and a read that has already been shown cannot be taken back by
    /// a later, lower one. Grows with the conversations the client is told about,
    /// which is no more than the chat list already holds.
    ///
    /// Not written to disk: a launch starts with nothing recorded, so a receipt
    /// the reader has not been shown is never drawn from a previous session's
    /// memory of it. That is the "never claim more than was received" rule at the
    /// storage layer.
    read_receipts: RefCell<HashMap<i64, i64>>,

    /// Who permutes a right-to-left row: this program, or the terminal.
    ///
    /// **Fixed at construction**, and private so that it stays that way: the
    /// layout is a pure function of the window, the panel's width and the clock
    /// ([`crate::rows`], invariant 4), and a mode read out of mutable state while
    /// a frame is being drawn would make the same conversation two different
    /// heights depending on when it was asked. [`App::with_bidi`] is the one way
    /// in, so a caller that has read the configuration says so once and every
    /// later frame draws the same rows.
    ///
    /// [`BidiMode::Terminal`] — the default — emits rows as they are stored and
    /// lets the terminal rearrange them, which is what a shaping terminal needs.
    bidi: BidiMode,
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
            mode: Mode::Normal,
            focus: Focus::Conversation,
            theme: Theme::default(),
            pane: Pane::Conversation,
            account: AccountState::Unfetched,
            session_store: SessionStore::default(),
            signin: None,
            phone: String::new(),
            code_prefill: String::new(),
            password_prefill: String::new(),
            // A program that is handed nothing assumes nothing: the caller that
            // has read the configuration says so, and a launch without one gets
            // the sentence rather than a form.
            credentials_configured: false,
            // A program that is handed nothing has no client either, and a
            // sign-in it accepts now would be a request nobody carries.
            client_available: false,
            profile_vim: VimState::new(0),
            profile_subject: ProfileId::SelfAccount,
            profile_caret: 0,
            profile_visual: None,
            profile_count: None,
            contact: None,
            profile_pending_w: false,
            list: ChatList::default(),
            selected_chat: 0,
            conversation: ConversationView::new(0),
            vim: VimState::new(0),
            line: LineEditor::new(),
            emoji: None,
            status: IDLE_STATUS.to_string(),
            should_quit: false,
            search: SearchState::default(),
            reply_to: None,
            editing: None,
            sending: None,
            confirm: None,
            selection: None,
            register: Register::default(),
            clipboard: None,
            actions: VecDeque::new(),
            status_until: None,
            typing_until: None,
            fetching: Fetching::default(),
            pending_jump: None,
            jumplist: Jumplist::default(),
            pending_chat: None,
            retry_requested: false,
            pending_g: false,
            pending_find: None,
            rows: Cell::new(ASSUMED_ROWS),
            body_width: Cell::new(ASSUMED_BODY_WIDTH),
            now: Cell::new(0),
            read_receipts: RefCell::new(HashMap::new()),
            bidi: BidiMode::Terminal,
        }
    }

    /// The same application, drawing right-to-left rows itself.
    ///
    /// By value and at construction rather than a setter, because the mode is an
    /// input to the layout rather than a thing that changes while the window is
    /// open: every row is then the same height whichever mode was asked for, and
    /// [`App::row_layout`] stays a pure function of the window and the width. A
    /// caller that has read the configuration calls this once, where it builds
    /// the application; nothing else needs to say anything.
    #[must_use]
    pub fn with_bidi(mut self, bidi: BidiMode) -> Self {
        self.bidi = bidi;
        self
    }

    /// Who permutes a right-to-left row: this program, or the terminal.
    ///
    /// Asked once per row by the conversation panel, and never per frame by the
    /// layout — see the field's doc.
    #[must_use]
    pub fn bidi(&self) -> BidiMode {
        self.bidi
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
        app.phone = "+44 7700 900142".to_owned();
        // The sign-in scenes are a machine that *can* sign in; the one that
        // cannot is its own scene, and it sets this back rather than inheriting
        // the other answer.
        app.credentials_configured = true;
        // And one with a client up, which is what the sample account above is: a
        // fetched account is what a `Ready` brings, and a `Ready` brings a
        // client. Without it every `⏎` in these scenes would report the
        // client-less sentence instead of the request under test.
        app.client_available = true;
        app
    }

    // ---- the chat list --------------------------------------------------

    /// The conversations, as they were last fetched.
    #[must_use]
    pub fn chats(&self) -> &[Chat] {
        &self.list.chats
    }

    /// The jump the reader is waiting on, if any.
    ///
    /// What the caller fetches: a jump the window cannot answer is recorded here
    /// rather than acted on, because nothing on this side of the boundary can
    /// reach the network.
    #[must_use]
    pub fn pending_jump(&self) -> Option<Jump> {
        self.pending_jump
    }

    /// The query the open conversation is being searched for, if any.
    ///
    /// The half of a search result's identity that `chat_id` does not carry: a
    /// result is dropped when it no longer names the query the reader is asking.
    #[must_use]
    pub fn search_query(&self) -> Option<&str> {
        self.search.query()
    }

    /// The search on the open conversation, for the panel to mark matches with.
    #[must_use]
    pub fn search(&self) -> &SearchState {
        &self.search
    }

    /// What the reader has selected, for the panel to mark and the operations to
    /// act on.
    ///
    /// One value rather than two marks on the application: an anchor and a focus
    /// kept apart are two things to keep consistent, and the arithmetic between
    /// them is the same in every reader.
    #[must_use]
    pub fn selection(&self) -> Option<&Selection> {
        self.selection.as_ref()
    }

    /// The `:query` being completed, for the popup to draw and the status line
    /// to name.
    ///
    /// The same shape as [`App::selection`]: one answer, read by both.
    #[must_use]
    pub fn completion(&self) -> Option<&emoji::Trigger> {
        self.emoji.as_ref()
    }

    /// The window positions the selection covers, oldest first.
    ///
    /// **Positions**, and not the span between the two identifiers, because the
    /// numbers do not say what covers what: a placeholder for a send in flight is
    /// numbered below zero and sits at the *end* of the window, where the
    /// conversation has reached. A selection reaching one spans a different set of
    /// messages by identifier than by position, and acting on the wrong one is a
    /// deletion of messages the reader did not select.
    ///
    /// Empty when there is no selection, and when one of its ends is not in the
    /// window — which [`App::retain_selection`] makes unreachable and which is
    /// answered as "nothing" rather than as a panic.
    ///
    /// The one answer, for the panel to mark with, the operations to act on, and
    /// the count to come from. Two answers would be two things to disagree.
    #[must_use]
    pub fn covered(&self, selection: Option<&Selection>) -> Range<usize> {
        let Some(selection) = selection else {
            return 0..0;
        };
        let window = &self.conversation.window;

        match (
            window.position_of(selection.anchor.message_id),
            window.position_of(selection.focus.message_id),
        ) {
            (Some(anchor), Some(focus)) => anchor.min(focus)..anchor.max(focus) + 1,
            _ => 0..0,
        }
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
        let selection = self.selection.as_ref()?;

        Some(selection.text_range().map_or_else(
            || self.covered(Some(selection)).len(),
            |(_, range)| range.len(),
        ))
    }

    /// Starts a selection at `message_id`, and reports whether it could be.
    ///
    /// A mark can only be placed on a message the window holds, so a selection
    /// over one that is not loaded is refused rather than recorded: `d` on it
    /// would name an identifier nothing can act on, and a mark the panel cannot
    /// draw is a mark the reader cannot see.
    ///
    /// The mode is not touched. A selection is not a mode — `dd` leaves one here
    /// while the reader is in Normal, answering a question, and the conversation's
    /// [`Mode`] is about what a key means rather than about what is selected.
    #[must_use]
    pub fn select(&mut self, message_id: i64, char: Option<usize>) -> bool {
        if self.conversation.window.position_of(message_id).is_none() {
            return false;
        }

        self.set_selection(Selection::at(message_id, char));
        true
    }

    /// Replaces the selection outright.
    ///
    /// The writer half of [`App::selection`], and the whole of what a motion
    /// needs: a motion moves one end of a selection and changes nothing else
    /// about it. It does not check that the marks are loaded, because a motion
    /// works from the window and the text in front of it and cannot name
    /// anything else.
    pub fn set_selection(&mut self, selection: Selection) {
        self.selection = Some(selection);
    }

    /// What the reader last yanked, for the caller that hands it to the system
    /// clipboard.
    #[must_use]
    pub fn register(&self) -> &Register {
        &self.register
    }

    /// Takes the text the reader asked to copy to the system clipboard.
    ///
    /// Drained by the caller that owns the terminal, on the same pass it read the
    /// yank on. Idempotent in the way every other hand-over here is: once taken it
    /// is forgotten, so a caller that asks twice gets one copy and not two.
    ///
    /// A yank is also offered to the clipboard, which is a convenience rather than
    /// the point: whether the terminal honours OSC 52 at all is not this crate's to
    /// know, so the register — which always works — is what a yank can be relied
    /// on for.
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.clipboard.take()
    }

    /// Installs a freshly fetched chat list.
    ///
    /// The window the messages were seen in goes with it: the list is replaced
    /// wholesale, and a message kept from the one before would be matched
    /// against conversations that are no longer on screen.
    ///
    /// The selection is clamped rather than reset, because the reader's place
    /// is a position in a list that may have become shorter. An empty list
    /// leaves nothing selected, which is what closes the conversation: there is
    /// no chat for the window to belong to.
    pub fn set_chats(&mut self, chats: Vec<Chat>) {
        self.list = ChatList::with_chats(chats);

        let last = self.list.chats.len().saturating_sub(1);
        self.selected_chat = self.selected_chat.min(last);

        if self.list.chats.is_empty() {
            self.select_chat_none();
        }
    }

    /// Installs a freshly fetched chat list while keeping the open conversation.
    ///
    /// The other half of [`App::set_chats`], and the one a client that has been
    /// brought back up needs: the list is replaced wholesale, but the reader's
    /// place in it is not. The conversation, its window, the cursor in it, the
    /// jumplist, the selection, the register and the draft are left exactly as
    /// they were, because a re-fetch is not the reader changing conversations —
    /// so none of [`App::select_chat_none`]'s resets run here.
    ///
    /// The highlight is restored by the open conversation's own id, never by
    /// index: the list comes back ordered by recency, and an index means a
    /// different conversation on either side of the fetch. An id the new list
    /// does not hold falls back to the top and reports `false`, so the caller
    /// can say where the reader landed.
    pub fn refresh_chats(&mut self, chats: Vec<Chat>) -> bool {
        let open = self.conversation.window.chat_id;
        self.list = ChatList::with_chats(chats);

        if let Some(index) = self.list.chats.iter().position(|chat| chat.id == open) {
            self.selected_chat = index;
            true
        } else {
            self.selected_chat = 0;
            false
        }
    }

    /// Closes the conversation on show.
    ///
    /// Every other act of this function is a reset, and the draft is the one
    /// thing it does not touch: a reader who switches chats mid-sentence does
    /// not lose the sentence. What *is* forgotten is the draft's subject — the
    /// reply it answers and the message it edits — because those name something
    /// in the conversation that has just closed, and a reply sent into a
    /// different chat to a message that is not in it is not a reply at all. The
    /// words survive; what they were for does not, and the draft becomes a
    /// message.
    fn select_chat_none(&mut self) {
        self.conversation = ConversationView::new(0);
        // The note belongs to the chat being left: returning must not revive it,
        // so the deadline goes with the view rather than with the reader's memory.
        self.typing_until = None;
        self.vim = VimState::new(0);
        self.fetching.clear();
        self.pending_jump = None;
        // The marks are per conversation, and this is the path every switch goes
        // through, so this is where they go too: a reader who has closed the
        // conversation has nowhere to walk back to.
        self.jumplist = Jumplist::default();
        self.search.clear();
        self.reply_to = None;
        self.editing = None;
        self.confirm = None;
        self.selection = None;
        self.register = Register::default();
        self.line.forget_purpose();
    }

    // ---- the profile panel ----------------------------------------------

    /// Records what the account's own profile turned out to be.
    ///
    /// The only writer of [`App::account`], so the three states cannot be mixed
    /// up by a caller that knows only one of them.
    pub fn set_account(&mut self, account: Result<Account, String>) {
        self.account = match account {
            Ok(account) => AccountState::Known(account),
            Err(reason) => AccountState::Unavailable(reason),
        };
    }

    /// The contact the card on show is about, if it is about one.
    ///
    /// `None` for the account's own card, which is what tells the card whether
    /// there is a second subject's data at all.
    #[must_use]
    pub fn contact(&self) -> Option<&ContactProfile> {
        self.contact.as_ref()
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
        let Some(open) = self.contact.as_ref().filter(|open| open.peer_id == peer_id) else {
            return;
        };
        self.contact = Some(ContactProfile {
            peer_id: open.peer_id,
            state: match profile {
                Ok(account) => AccountState::Known(account),
                Err(reason) => AccountState::Unavailable(reason),
            },
        });
        // The highlight is re-sized because the row count just changed under it.
        // The account's own card is read once at start-up, so its rows are settled
        // before the card opens; a contact's card is drawn with *no* rows until
        // this answer arrives, and a highlight bounded over zero rows can never be
        // moved afterwards — `j` clamps to a buffer of nothing. Re-clamping rather
        // than resetting keeps the reader where they were, which here is the top.
        self.profile_vim
            .set_total(crate::card::navigable(&crate::card::rows(self)));
    }

    /// Records where the session is kept.
    ///
    /// Its own setter rather than a field of `set_account`, because it is known
    /// from the configuration before anything is read from the network — and
    /// "the session has not been placed yet" is one of the states the panel has
    /// to draw.
    pub fn set_session_store(&mut self, store: SessionStore) {
        self.session_store = store;
    }

    /// Where the profile's highlight is, for the panel to draw.
    #[must_use]
    pub fn profile_cursor(&self) -> usize {
        self.profile_vim.cursor()
    }

    /// Whom the card is about.
    ///
    /// A contact is looked up in the chat list by the id the `ProfileId` holds, and
    /// a subject that is not there is the account's own: a card whose subject has
    /// gone should not read as somebody else's, and there is nobody else.
    #[must_use]
    pub fn card_subject(&self) -> crate::card::CardSubject<'_> {
        match self.profile_subject {
            ProfileId::SelfAccount => crate::card::CardSubject::SelfAccount,
            ProfileId::User(id) => self.list.chats.iter().find(|chat| chat.id == id).map_or(
                crate::card::CardSubject::SelfAccount,
                crate::card::CardSubject::Contact,
            ),
        }
    }

    /// The row the inline position is in, as a character count within its value.
    #[must_use]
    pub fn card_caret(&self) -> usize {
        self.profile_caret
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
        let at = self.profile_caret.min(chars);
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
        let anchor = self.profile_visual?;
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
    /// A test helper *and* the reason the two are separate: a card's cursor is
    /// `j`/`k` and a position is `l`/`h`, and every test that cares about the
    /// second has to walk to the first to get there.
    #[cfg(test)]
    pub(crate) fn handle_card_row(&mut self, row: usize) {
        self.profile_vim.set_cursor(row);
    }

    /// One charwise motion within the cursor row's value.
    #[cfg(test)]
    pub(crate) fn handle_card_motion(&mut self, key: char) {
        self.card_motion_char(key);
    }

    /// One key at the card, for a test that is about the key and not the path.
    #[cfg(test)]
    pub(crate) fn handle_card_key(&mut self, key: char) {
        self.handle_profile(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
    }

    /// A chat from the list, for a test that needs a contact to open a card about.
    #[cfg(test)]
    pub(crate) fn any_chat(&self) -> Option<domain::chat::Chat> {
        self.list.chats.first().cloned()
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
            .profile_visual
            .is_some_and(|anchor| anchor.message_id == self.card_row_id());
        if inside {
            Mark::text(self.card_row_id(), self.profile_caret)
        } else {
            Mark::whole(self.card_row_id())
        }
    }

    /// The cursor row as the identifier a [`Mark`] names.
    fn card_row_id(&self) -> i64 {
        i64::try_from(self.profile_vim.cursor()).unwrap_or(i64::MAX)
    }

    /// Starts a card selection at the inline position, or extends the one there is.
    fn start_card_visual(&mut self) {
        if self.profile_visual.is_none() {
            self.profile_visual = Some(Mark::text(self.card_row_id(), self.profile_caret));
        }
    }

    /// Puts the profile in the right-hand pane.
    ///
    /// Sizes the highlight from whatever rows exist, so an empty panel has
    /// nothing to move a highlight over rather than a highlight on nothing.
    fn open_profile(&mut self) {
        self.open_card(ProfileId::SelfAccount);
    }

    /// Puts the card for whoever the chat list is highlighting, in the pane.
    ///
    /// The highlight rather than the open conversation, so a reader can look
    /// somebody up without opening them — which is the whole difference between a
    /// chat list and a list of links, and the reason the chat list is one.
    ///
    /// An empty list opens nothing: there is nobody to open, and a card about
    /// nobody would fall back to the account's own, which is a card the reader did
    /// not ask for.
    fn open_contact(&mut self) {
        // From the chat list it is the highlight, and from the conversation it is
        // the open one: the two are the same value, because opening a conversation
        // is what moves the highlight to it.
        let Some(chat) = self.list.chats.get(self.selected_chat) else {
            return;
        };
        self.open_card(ProfileId::User(chat.id));
    }

    /// Puts a card about `subject` in the right-hand pane.
    ///
    /// The highlight starts at the top and the inline position at the start of the
    /// first value, every time. The conversation keeps its message cursor across a
    /// trip to the card and back because a conversation is a document the reader
    /// has a place in; a card is a fixed-shape list of values with no order to lose
    /// a place in, and restoring the row they looked at last time would answer a
    /// question they did not ask.
    pub(crate) fn open_card(&mut self, subject: ProfileId) {
        self.profile_subject = subject;
        // One read per card opened, asked for here rather than by the panel: the
        // panel draws what it has, and the reader asked a question by pressing `A`.
        self.contact = match subject {
            ProfileId::User(peer_id) => {
                self.queue_action(Action::FetchContact { peer_id });
                Some(ContactProfile {
                    peer_id,
                    state: AccountState::Unfetched,
                })
            }
            ProfileId::SelfAccount => None,
        };
        // Over the rows that are drawn, not over every row there is: a held slot
        // at the end of a card is not somewhere the highlight goes.
        self.profile_vim = VimState::new(crate::card::navigable(&crate::card::rows(self)));
        self.profile_caret = 0;
        self.profile_visual = None;
        self.profile_count = None;
        self.pane = Pane::Profile(subject);
        self.focus = Focus::Conversation;
        self.mode = Mode::Normal;
        self.selection = None;
    }

    /// Puts the conversation back in the right-hand pane.
    fn close_profile(&mut self) {
        self.pane = Pane::Conversation;
        self.profile_vim = VimState::new(0);
    }

    /// Leaves a card, for the way back rather than for `Esc`.
    ///
    /// Drops the selection and the inline position as well as the pane, because
    /// all three describe a card that is no longer on show: a position inside a
    /// value means nothing in a conversation, and leaving it behind would be a
    /// caret waiting to be drawn on the wrong surface.
    fn close_card(&mut self) {
        self.profile_visual = None;
        self.profile_caret = 0;
        self.profile_count = None;
        self.close_profile();
    }

    /// Handles a key while the profile has the focus.
    ///
    /// `h` goes to the chat list and `l` and `Esc` come back to the
    /// conversation, and `j`/`k`/`gg`/`G` move the highlight — all of them
    /// through the same [`VimState`] the conversation uses, because that type
    /// moves a cursor between items and knows nothing about what an item is.
    ///
    /// A key on a card.
    ///
    /// Three families, and the order they are matched in is the design:
    ///
    /// - **Leaving.** `Esc` walks down — a selection, then the card, then the
    ///   conversation — and `h` leaves from the row's first cell, where the inline
    ///   position has nowhere left to go. That is `h` doing two jobs, and it is
    ///   sound because a card is one column of values: there is no column to the
    ///   left of the first one, so the motion is at its edge rather than the key
    ///   being repurposed mid-word.
    /// - **Within a row.** `l` and `h` move the inline position, and `w`/`b`/`e`
    ///   and `0`/`$` move it by word, clamped to the value rather than crossing
    ///   into the next row — crossing is what `j` is for, and a motion that
    ///   silently changes *what it selects* is the worst failure a selection has.
    /// - **Between rows.** `j`/`k` and `gg`/`G` move the row and reset the inline
    ///   position to its start, because a position inside a row means nothing in
    ///   the next one.
    ///
    /// A key that is none of those is the conversation's, and taking it is what
    /// stops a reader who pressed `i` by reflex from having to press it twice.
    fn handle_profile(&mut self, key: KeyEvent) {
        // `Ctrl-w` is a prefix here, and bare it still leaves the input line: the
        // same prefix-with-a-bare-fallback shape `g`/`gg` already has.
        if self.profile_pending_w {
            self.profile_pending_w = false;
            match key.code {
                KeyCode::Char('h') => return self.set_focus(Focus::ChatList),
                // Nothing is drawn to the right of a card, so `Ctrl-w l` has no
                // destination. It is named nowhere on the card's hint for that
                // reason, and saying so here is what keeps the key from looking
                // broken to a reader who tries it.
                KeyCode::Char('l') => return self.flash("nothing to the right of a card"),
                _ => return,
            }
        }

        match key.code {
            KeyCode::Esc => self.escape_card(),
            // `h` at the first cell is the way back, because that is where the
            // inline position has nothing to its left. A card is one column of
            // values, so there is no second column for the motion to reach — which
            // is what lets one key be the motion everywhere else and the way out
            // at the one edge where the motion is finished.
            KeyCode::Char('h') if self.card_caret_at_start() => self.close_card(),
            KeyCode::Char(c @ ('l' | 'h' | 'w' | 'b' | 'e' | '0' | '$')) => {
                self.card_motion_char(c);
            }
            KeyCode::Char(c @ ('j' | 'k' | 'g' | 'G')) => self.card_motion_row(c),
            // `v`, and `j` to reach further. There is no `V`: one gesture for one
            // thing is one thing to learn, and the design's own entry table has
            // only `v` on a card — `V` there is the *mode* label for Visual. A
            // selection that grows to a second row becomes a set of whole rows by
            // itself, which is what rowwise meant.
            KeyCode::Char('v') => self.start_card_visual(),
            KeyCode::Char('y') => self.yank_card(),
            KeyCode::Char('d') => self.activate_profile_row(),
            KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => self.card_count(c),
            _ => {
                self.close_profile();
                self.handle_normal(key);
            }
        }
    }

    /// `y` on a card, which yanks one of two things.
    ///
    /// A selection inside one row is the selected characters, and a selection
    /// across rows is one line per row, oldest first — the same two cases the
    /// conversation's `y` answers with, and for the same reason. A selection that
    /// has not been moved is a position rather than a span, and there is nothing in
    /// it to take; that is said rather than silently replacing the register with
    /// nothing.
    ///
    /// The register is also offered to the system clipboard, best-effort: whether a
    /// terminal honours OSC 52 at all is not something this program can find out,
    /// so a refused or capped write is not a failure of the yank and is not
    /// reported as one.
    fn yank_card(&mut self) {
        let lines = self.card_yanked();
        self.profile_visual = None;

        if lines.iter().all(String::is_empty) {
            self.flash("nothing to yank — move the selection first");
            return;
        }

        self.register = Register::set(lines);
        self.clipboard = Some(self.register.text());
    }

    /// The lines a card selection yanks, or the cursor row's value when there is
    /// no selection — which is what `yy` is.
    fn card_yanked(&self) -> Vec<String> {
        let rows = crate::card::rows(self);
        let Some(selection) = self.card_selection() else {
            // No selection: the whole value of the row the cursor is on. A card
            // has no buffer, so there is no linewise equivalent to reach for.
            return rows
                .get(self.profile_vim.cursor())
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

    /// `Esc` on a card, as a ladder: a selection, then the card, then out.
    ///
    /// Four presses from a selection on a second row, and no special case
    /// anywhere in it — a key that means one thing in one state and another in the
    /// next is a key a reader has to learn twice, and `Esc` is the one key every
    /// reader already reaches for.
    fn escape_card(&mut self) {
        if self.profile_visual.take().is_some() {
            return;
        }
        self.close_card();
    }

    /// Whether the inline position is at the start of its value, which is where
    /// `h` leaves the card rather than moving.
    #[must_use]
    fn card_caret_at_start(&self) -> bool {
        self.profile_caret == 0
    }

    /// A motion within the cursor row's value: `l`/`h`, a word motion, or a bound.
    ///
    /// Every one of them is clamped to the value rather than crossing into the next
    /// row, and that is not Vim's rule — in Vim `w` at the end of a buffer wraps.
    /// Crossing is what `j` is for, and a motion that silently changes *what it
    /// selects* is the worst failure mode a selection has.
    fn card_motion_char(&mut self, c: char) {
        let Some(motion) = CharMotion::from_key(c) else {
            return;
        };
        let Some(value) = self.card_value() else {
            return;
        };
        self.profile_caret = char_motion(&value, self.profile_caret, motion);
    }

    /// A motion between rows, which resets the inline position.
    ///
    /// The reset is the point: a character position inside one value means nothing
    /// in the next, and carrying it over would put the caret at a column the next
    /// value may not have.
    fn card_motion_row(&mut self, c: char) {
        // `handle_char` applies the motion *and* returns it, so calling
        // `apply_motion` on the result would move the row twice — which is a bug
        // that looks like a card with one more row than it has.
        self.profile_vim.handle_char(c);
        self.off_reserved(matches!(c, 'j' | 'G'));
        self.profile_caret = 0;
    }

    /// Steps the row cursor off a held slot.
    ///
    /// A held slot draws nothing, so a highlight landing on one would be a
    /// highlight the reader cannot see and a value they cannot read. Stepping is
    /// needed rather than bounding alone because the colour slot is *interior* on
    /// a contact's card: the name is above it and the identity and everything
    /// after it are below.
    ///
    /// Bounded by the row count, and it scans the other way as well as the way
    /// the reader was going — a card that is nothing but held slots should leave
    /// the highlight on a real row rather than on nothing, and there is no reason
    /// to prefer one end to the other.
    fn off_reserved(&mut self, forward: bool) {
        let rows = crate::card::rows(self);
        let last = rows.len().saturating_sub(1);
        let start = self.profile_vim.cursor().min(last);
        if !rows
            .get(start)
            .is_some_and(crate::card::CardRow::is_reserved)
        {
            return;
        }

        for ahead in [forward, !forward] {
            let mut cursor = start;
            for _ in 0..rows.len() {
                if !rows
                    .get(cursor)
                    .is_some_and(crate::card::CardRow::is_reserved)
                {
                    self.profile_vim.set_cursor(cursor);
                    return;
                }
                cursor = if ahead {
                    cursor + 1
                } else {
                    cursor.saturating_sub(1)
                };
            }
        }
    }

    /// A count before a motion, as `12j` means twelve.
    ///
    /// The digits are otherwise unbound on a card, so a count costs one line rather
    /// than shadowing a key that means something else.
    fn card_count(&mut self, c: char) {
        let digit = u32::from(c);
        self.profile_count = Some(self.profile_count.unwrap_or(0) * 10 + digit);
    }

    /// The cursor row's value, if the card has one on show.
    fn card_value(&self) -> Option<String> {
        crate::card::rows(self)
            .get(self.profile_vim.cursor())
            .map(|row| row.value.clone())
    }

    /// Acts on the row under the profile's highlight.
    ///
    /// `d` rather than `Enter`, because `d` is what the same key does on a
    /// selected message: one interaction model on a screen that has one, not a
    /// button. The rows that are neither of the two actions do nothing, and the
    /// panel draws them dim so that a key with nothing to do is not a surprise.
    fn activate_profile_row(&mut self) {
        // A contact's card has no row that acts, and that is decided by *who it is
        // about* rather than by which row the cursor is on — so the refusal comes
        // before the row is looked for. A card whose profile has not been read has
        // no rows at all, and a key that means "you cannot" has to say so there
        // too: swallowing it is the one thing such a key must not do.
        if let crate::card::CardSubject::Contact(_) = self.card_subject() {
            self.flash(NOT_YOURS_REFUSAL);
            return;
        }

        // The row is whatever `card::rows` says the panel is drawing, rather than
        // a second enumeration of it. A key that acted on its own list could act
        // on a row the panel is not showing, and that failure is silent: no
        // message, no wrong frame, just a key that did the wrong thing.
        let Some(label) = crate::card::rows(self)
            .get(self.profile_cursor())
            .filter(|row| row.is_action())
            .map(|row| row.label)
        else {
            return;
        };
        match label {
            // A deliberate refusal rather than a failure. The bracketed
            // `[failed: …]` form is for something that tried and did not come
            // back, and nothing was sent.
            crate::card::ADD_ACCOUNT => self.flash(ADD_ACCOUNT_REFUSAL),
            // Confirm first and refuse inside the confirmation. A panel that
            // flashes instead of confirming has taught the reader the wrong
            // thing about a key that will eventually discard the one secret this
            // program holds.
            crate::card::LOGOUT => {
                self.mode = Mode::Confirm;
                self.confirm = Some(ConfirmKind::Logout);
            }
            _ => {}
        }
    }

    // ---- what is on show ------------------------------------------------

    /// Moves the highlight to `index` in the chat list, and asks for the
    /// conversation it names to be taken to.
    ///
    /// The highlight moves at once and the open is recorded rather than made,
    /// because a reader who holds `j` would otherwise have every conversation
    /// they passed fetched. What the reader sees follows their key; what the
    /// network is asked for waits for them to stop.
    ///
    /// An index outside the list moves nothing.
    fn choose_chat(&mut self, index: usize) {
        if self.list.chats.get(index).is_none() {
            return;
        }

        self.selected_chat = index;
        self.pending_chat = Some(ChatChoice {
            index,
            at: Instant::now(),
        });
    }

    /// The conversation the reader has stopped on, once they have stopped.
    ///
    /// Nothing while they are still moving, so a held key opens the chat they
    /// land on rather than every one between here and there. Idempotent in the
    /// way [`App::pending_jump`] is: once handed over it is forgotten, so a
    /// caller that asks twice gets one conversation.
    pub fn take_pending_chat(&mut self, now: Instant) -> Option<usize> {
        let choice = self.pending_chat?;
        if now.saturating_duration_since(choice.at) < CHAT_SWITCH_DELAY {
            return None;
        }

        self.pending_chat = None;
        Some(choice.index)
    }

    /// Opens the conversation at `index` in the chat list.
    ///
    /// The window is replaced rather than extended: it holds one conversation,
    /// and the one before it is gone. Whatever was in flight for the old one is
    /// forgotten too — a page that arrives late belongs to a conversation that
    /// is no longer open, and the window refuses it.
    ///
    /// An index outside the list leaves the screen as it was.
    pub fn select_chat(&mut self, index: usize) {
        let Some(chat) = self.list.chats.get(index) else {
            return;
        };
        let chat_id = chat.id;

        self.selected_chat = index;
        self.pending_chat = None;
        self.select_chat_none();
        // The new view starts its placeholder ids at the bottom again, so an
        // identifier the old view handed out can be handed out once more. That is
        // safe only because a result for the old view cannot reach this one —
        // whatever else changes here, that has to stay true. The counter is not
        // carried across on purpose; `net`'s drop test is the executable form of
        // this sentence.
        self.conversation = ConversationView::new(chat_id);
        // The window is gone and with it the watermark the new view starts
        // without. How far this conversation has been read is not a fact about
        // the page on show, so it is put back from what the feed has said.
        self.restore_read_watermark(chat_id);
    }

    /// Puts the conversation's recorded read watermark on the view just opened.
    ///
    /// Reports whether there was one to put back, which is what a reader switching
    /// to a conversation nobody has read yet gets.
    fn restore_read_watermark(&mut self, chat_id: i64) -> bool {
        let recorded = self.read_receipts.borrow().get(&chat_id).copied();
        match recorded {
            Some(max_id) => self.conversation.set_read_watermark(max_id),
            None => false,
        }
    }

    /// Records how far a conversation has been read, keeping the highest figure
    /// seen for it.
    ///
    /// The feed's acknowledgement can repeat or arrive late, so a lower one is
    /// dropped rather than applied: a read already shown cannot be taken back.
    /// Reports whether this figure moved the conversation's watermark.
    fn note_read(&self, chat_id: i64, max_id: i64) -> bool {
        if max_id <= 0 {
            return false;
        }

        let mut recorded = self.read_receipts.borrow_mut();
        let moved = recorded.get(&chat_id).is_none_or(|read| max_id > *read);
        if moved {
            recorded.insert(chat_id, max_id);
        }

        moved
    }

    /// Whether a conversation is open to put messages in.
    ///
    /// Telegram numbers peers from one, so a zero here is the absence of a
    /// conversation rather than a conversation with an odd identifier.
    #[must_use]
    fn has_conversation(&self) -> bool {
        self.conversation.window.chat_id != 0
    }

    /// The conversation on show, as the chat list holds it.
    ///
    /// Looked up by the window's own identifier rather than by the selected
    /// index: the two agree, and the window is what every question here is about.
    fn open_chat(&self) -> Option<&Chat> {
        let chat_id = self.conversation.window.chat_id;
        self.list.chats.iter().find(|chat| chat.id == chat_id)
    }

    /// The name of the conversation on show, for the input bar's title.
    ///
    /// A draft belongs to no conversation, so this is the one thing a reader
    /// cannot work out for themselves: where the words in the bar will be
    /// sent. `None` when nothing is open, which is the one case in which
    /// composing does nothing at all.
    #[must_use]
    pub fn open_chat_name(&self) -> Option<&str> {
        self.open_chat().map(|chat| chat.title.as_str())
    }

    /// Where the open conversation's unread messages start, as far as its
    /// numbering can say.
    ///
    /// Counted from the message the conversation last showed, which the chat list
    /// has held since it was fetched — no round trip. A conversation the list has
    /// no preview for falls back on the newest message loaded, which is the same
    /// message whenever anything has arrived while the conversation was open.
    fn first_unread(&self) -> Option<i64> {
        let chat = self.open_chat()?;
        let last = chat
            .last_message_id
            .or_else(|| self.conversation.window.newest_id());

        unread_target(last, chat.unread_count)
    }

    /// Whether the window ends where the conversation does.
    ///
    /// Two ways to know, and the second is the one that covers a conversation
    /// that was just opened — its newest page *is* the end, whatever a fetch
    /// behind it has or has not said. An arrival updates the preview, so this
    /// stays true as the conversation grows.
    fn holds_newest_edge(&self) -> bool {
        let window = &self.conversation.window;
        if window.is_empty() {
            return false;
        }
        if window.exhausted_newer {
            return true;
        }

        self.open_chat()
            .and_then(|chat| chat.last_message_id)
            .is_some_and(|last| window.newest_id() == Some(last))
    }

    /// The message the cursor is on, if the window holds anything.
    fn cursor_message(&self) -> Option<&Message> {
        self.conversation.window.get(self.vim.cursor())
    }

    /// Identifier of the message the cursor is on, if the window holds anything.
    fn cursor_message_id(&self) -> Option<i64> {
        self.cursor_message().map(|message| message.id)
    }

    /// Whether `page` holds anything for the conversation on show.
    ///
    /// A page is fetched for one conversation and the reader can open another
    /// while one is in flight, so a page that arrives late is recognised here
    /// rather than allowed to replace what is on screen.
    fn page_belongs_to_open_chat(&self, page: &[Message]) -> bool {
        let chat_id = self.conversation.window.chat_id;
        page.iter().any(|message| message.chat_id == chat_id)
    }

    // ---- pages coming back ----------------------------------------------

    /// Replaces the window with the newest page of the open conversation.
    ///
    /// The reader is put at the newest message: opening a conversation and
    /// loading the page that ends one both mean "show me the end".
    ///
    /// A selection does not survive, because the messages it named are not
    /// necessarily the ones on show now. There is nothing to restore it *to*:
    /// unlike a page that extends the window, a page that replaces it has shifted
    /// every message in it.
    ///
    /// Reports whether anything was shown.
    pub fn apply_latest(&mut self, page: Vec<Message>) -> bool {
        if !self.page_belongs_to_open_chat(&page) {
            return false;
        }

        self.conversation.window.replace(page);
        self.selection = None;
        self.vim.set_total(self.conversation.window.len());
        self.conversation.follow();
        self.vim.apply_motion(Motion::Last);

        true
    }

    /// Puts a page in front of what the window holds.
    ///
    /// Reports whether anything was added, and leaves the reader on the message
    /// they were reading: the window moved under them, not the other way round.
    pub fn apply_older(&mut self, page: Vec<Message>) -> bool {
        if !self.page_belongs_to_open_chat(&page) {
            return false;
        }

        let anchor = self.cursor_message_id();
        if !self.conversation.window.push_front(page) {
            return false;
        }

        self.after_window_change(anchor);

        true
    }

    /// Puts messages behind what the window holds: a fetched page, or one the
    /// reader has just typed.
    ///
    /// Reports whether anything was added.
    pub fn apply_newer(&mut self, page: Vec<Message>) -> bool {
        if !self.page_belongs_to_open_chat(&page) {
            return false;
        }

        let anchor = self.cursor_message_id();
        if !self.conversation.window.push_back(page) {
            return false;
        }

        self.after_window_change(anchor);

        true
    }

    // ---- jumping to the unread messages ---------------------------------

    /// Takes the reader to where the conversation's unread messages start.
    ///
    /// `gg` is Vim's top-of-buffer, and that is what it stays when there is
    /// nothing unread to be taken to. When there is, the reader means the first
    /// unread message, and this answers it from what is loaded wherever it can:
    /// the cursor moves and nothing is returned.
    ///
    /// What comes back is a jump the window cannot answer — the target is
    /// somewhere the client has not fetched, and only the caller can go and get
    /// it. Returning the intent rather than recording it keeps the two in step
    /// at the one call site: the reader asked for *this*, and anything they had
    /// asked for before is replaced by it, whether or not there is one.
    ///
    /// The estimate is arithmetic on identifiers, and identifiers have gaps
    /// wherever messages were deleted, so it can land in front of the true first
    /// unread. A window that ends where the conversation does is the exception,
    /// and it is the common case: the unread messages are then the newest ones
    /// there are, so they are counted back from the end and land exactly.
    #[must_use]
    pub fn jump_to_unread(&mut self) -> Option<Jump> {
        if !self.has_conversation() {
            return None;
        }

        let target = self.first_unread()?;

        // Counted from the end when the window reaches the end of the
        // conversation: the unread messages are the newest ones there are, so
        // their number says exactly where they start however the messages are
        // numbered.
        if self.holds_newest_edge()
            && let Some(index) = landing_position(self.conversation.window.len(), self.unread())
        {
            self.vim.set_cursor(index);
            return None;
        }

        // Or found by identifier, when the window holds the target but not the
        // end of the conversation.
        if let Some(index) = self.conversation.window.position_of(target) {
            self.vim.set_cursor(index);
            return None;
        }

        Some(Jump {
            peer_id: self.conversation.window.chat_id,
            target_id: target,
            kind: JumpKind::Unread,
        })
    }

    /// Takes the reader to the message the one under the cursor quotes.
    ///
    /// `gd`, and the second producer of a jump: a reply's quote is a message in
    /// the same conversation, which is loaded or is not. A target the window
    /// holds is a cursor move and nothing else — no round trip for a message
    /// already on screen. A target it does not hold is a jump, on the same terms
    /// as `gg`'s.
    ///
    /// A message that quotes nothing refuses, because there is nowhere to go:
    /// the sentence names the key and what it does, which is more use to a
    /// reader than silence.
    ///
    /// Where the reader was is recorded before the cursor moves and before the
    /// jump is armed, and in the in-window case too: that is the same place to
    /// come back to as any other, and `Ctrl-o` after a jump that needed no fetch
    /// is the case where the reader most expects to be able to undo it.
    #[must_use]
    pub fn jump_to_reply(&mut self) -> Option<Jump> {
        if !self.has_conversation() {
            return None;
        }

        let Some(target) = self.cursor_message().and_then(|message| message.reply_to) else {
            self.flash(NOT_A_REPLY);
            return None;
        };

        if let Some(origin) = self.cursor_message_id() {
            self.jumplist
                .record(self.conversation.window.chat_id, origin);
        }

        if let Some(index) = self.conversation.window.position_of(target) {
            self.vim.set_cursor(index);
            return None;
        }

        Some(Jump {
            peer_id: self.conversation.window.chat_id,
            target_id: target,
            kind: JumpKind::Reply,
        })
    }

    /// Takes the reader back to where they were before the last jump.
    ///
    /// `Ctrl-o`. A mark the window still holds is a cursor move; one it does not
    /// is a jump of its own, on the same terms as any other, because the jump
    /// that took the reader away replaced the window the mark was in.
    pub fn jump_back(&mut self) {
        // Asked before the stack is walked: a walk moves a mark between the two
        // stacks, and a reader who presses `Ctrl-o` while a page is on its way
        // must not have moved one for a return that did not happen.
        if self.pending_jump.is_some() || !self.has_conversation() {
            return;
        }

        let Some(from) = self.cursor_message_id() else {
            return;
        };
        let Some(target) = self.jumplist.back(self.conversation.window.chat_id, from) else {
            return;
        };

        self.go_to(target, JumpKind::Back);
    }

    /// Takes the reader forward to the place a `Ctrl-o` walked them away from.
    ///
    /// `Ctrl-i`, and nothing else: a terminal that cannot report it apart from
    /// `Tab` sends `Tab` instead, and `Tab` is the pane switch here — so on such
    /// a terminal only `Ctrl-o` works, which is what the design accepted.
    pub fn jump_forward(&mut self) {
        if self.pending_jump.is_some() || !self.has_conversation() {
            return;
        }

        let Some(from) = self.cursor_message_id() else {
            return;
        };
        let Some(target) = self
            .jumplist
            .forward(self.conversation.window.chat_id, from)
        else {
            return;
        };

        self.go_to(target, JumpKind::Forward);
    }

    /// Goes to `id`, moving the cursor when the window holds it and asking for a
    /// page centred on it when it does not.
    ///
    /// One answer for a return in either direction, because the two differ only
    /// in which stack was walked — and in what the status line says while the
    /// page is on its way, which is `kind`'s whole job.
    fn go_to(&mut self, id: i64, kind: JumpKind) {
        if self.pending_jump.is_some() {
            return;
        }

        if let Some(index) = self.conversation.window.position_of(id) {
            self.vim.set_cursor(index);
            self.settle_follow();
            return;
        }

        self.pending_jump = Some(Jump {
            peer_id: self.conversation.window.chat_id,
            target_id: id,
            kind,
        });
        self.settle_follow();
    }

    /// Ends the reader's wait for a jump, reporting whether it was still the one
    /// being waited on.
    ///
    /// A page that failed, or came back empty, has to end it exactly as a page
    /// that landed does. The reader stays where they were either way; what this
    /// is for is that the key is free again — a jump nothing releases is a key
    /// that never works again.
    pub fn clear_jump(&mut self, target_id: i64) -> bool {
        if self.pending_jump.map(|jump| jump.target_id) != Some(target_id) {
            return false;
        }

        self.pending_jump = None;
        true
    }

    /// Replaces the window with a page fetched around a message the reader asked
    /// to be taken to, and puts them on it.
    ///
    /// Reports whether the window took the page. A page nobody is waiting for any
    /// more is refused — the reader has opened another conversation, or told the
    /// client to take them to the end instead — and the jump is over either way,
    /// so that a fetch which failed or came back empty cannot leave the key
    /// wedged.
    ///
    /// The cursor lands on the target, or on the first message after it when the
    /// page does not hold it: the page is centred on the target, so that is the
    /// nearest the fetch came to where the reader was going.
    ///
    /// A selection does not survive, for the same reason it does not survive
    /// [`App::apply_latest`]: the page replaced the window.
    pub fn apply_jump(&mut self, page: &[Message], target_id: i64) -> bool {
        // Which jump this page answers, read before `clear_jump` ends the wait.
        // A jump the reader asked for by name says so when its message cannot be
        // found; `gg`'s first-unread jump keeps its old silence, because its key
        // means "take me to the unread" and not "take me to message 19", and its
        // behaviour does not change here.
        let kind = self.pending_jump.map(|jump| jump.kind);
        if !self.clear_jump(target_id) {
            return false;
        }

        if !self.page_belongs_to_open_chat(page) {
            // A page that came back with nothing in it is the fetch's answer:
            // the client does not hold the message the reader was taken to, so
            // there is nowhere for the window to go. Said as a refusal because
            // the jump is over and the key is free, and the status line is
            // where a refusal belongs.
            if page.is_empty() && kind.is_some_and(|kind| kind != JumpKind::Unread) {
                self.flash(JUMP_UNAVAILABLE);
            }
            return false;
        }

        // Copied into the window rather than moved: a page that replaces a
        // window is the caller's to report to the cursor it keeps, and that
        // cursor is counted from the same messages.
        self.conversation.window.replace(page.iter().cloned());
        self.selection = None;

        // A window that jumped is surrounded by the unknown on both sides,
        // whatever the one before it had run out of.
        self.conversation.window.exhausted_older = false;
        self.conversation.window.exhausted_newer = false;

        self.vim.set_total(self.conversation.window.len());

        let landing = self.landing_index(target_id);
        self.vim.set_cursor(landing);
        self.settle_follow();

        true
    }

    /// Where the reader is put in a window that was replaced around `target`.
    ///
    /// The target itself when the page holds it; otherwise the first message
    /// after it, which is the nearest the page came; and the newest message in
    /// the window when the target is past every one of them — an estimate that
    /// outran the conversation, which the nearest survivor answers honestly.
    fn landing_index(&self, target: i64) -> usize {
        let window = &self.conversation.window;

        window
            .position_of(target)
            .or_else(|| window.iter().position(|message| message.id >= target))
            .unwrap_or_else(|| window.len().saturating_sub(1))
    }

    /// How many messages the open conversation has unread.
    fn unread(&self) -> u32 {
        self.open_chat().map_or(0, |chat| chat.unread_count)
    }

    // ---- events from the feed -------------------------------------------

    /// Applies an event from the feed to everything it touches.
    ///
    /// One event, two places: the list keeps the preview and the unread count,
    /// the open conversation keeps the messages. The same contract as the flat
    /// window's — `false` means nothing observable moved, so the caller owes no
    /// redraw. Deduplicating by identifier is what makes the overlap between
    /// the two windows harmless.
    ///
    /// A read acknowledgement is the one event that is not a window change, and
    /// it is handled apart from the rest rather than by a flag on the path: see
    /// the comment where it is matched.
    ///
    /// The event is copied rather than shared because the list moves an arrival
    /// into its own window, so it needs one of its own. One copy per event is
    /// the price of a single event reaching both.
    #[must_use]
    pub fn apply_update(&mut self, event: &UpdateEvent) -> bool {
        // The peer's typing is the title's business and nothing else's: no
        // message arrived, changed or left, so there is no window to re-anchor
        // and the reader's place in it is untouched. It answers for the open
        // conversation only, because the note is drawn on that conversation's
        // title — an event about a chat the reader is not in would be zeroed on
        // their arrival anyway.
        if let UpdateEvent::PeerTyping { chat_id, typing } = event {
            return self.apply_typing(*chat_id, *typing);
        }

        let listed = self.list.apply_update(event.clone());

        // The message is what the typing was for, so it ends it. Cleared here
        // rather than on the cancel action alone because the cancel is not sent
        // reliably: a peer who sends instead of cancelling would otherwise be
        // shown as typing with their own message on screen.
        if let UpdateEvent::NewMessage(message) = event
            && message.chat_id == self.conversation.window.chat_id
        {
            self.typing_until = None;
        }

        // A read acknowledgement is not a window change. It moves the watermark
        // and nothing else — no message arrived, changed or left — so there is
        // nothing to re-anchor and a reader scrolled back up stays exactly where
        // they are (US-X4). The redraw is still owed: the receipt is on the screen
        // now. It is recorded for the conversation either way, so a chat the
        // reader is not in yet carries its reading when they open it.
        if let UpdateEvent::ReadReceipt { chat_id, max_id } = event {
            let noted = self.note_read(*chat_id, *max_id);
            let open = self.conversation.apply_event(event);

            return listed || noted || open;
        }

        let anchor = self.cursor_message_id();
        let windowed = self.conversation.apply_event(event);
        if windowed {
            self.after_window_change(anchor);
        }

        listed || windowed
    }

    /// Puts the reader back where they were, now that the window has moved.
    ///
    /// There are now **two** anchors to restore rather than one, and both are
    /// restored the same way: by message identifier, because an index means a
    /// different message on either side of a page. Restoring only the cursor is
    /// the failure this arrangement exists to prevent — an older page landing
    /// under a live selection shifts every index, so a selection left in index
    /// terms would silently come to cover different messages and the next `d`
    /// would delete something the reader did not select.
    fn after_window_change(&mut self, anchor: Option<i64>) {
        self.vim.set_total(self.conversation.window.len());

        let cursor = self.vim.cursor();
        let restored = anchor
            .and_then(|id| self.conversation.window.position_of(id))
            .unwrap_or(cursor);
        self.vim.set_cursor(restored);

        self.retain_selection();

        if self.conversation.auto_follow() {
            // A view pinned to the end stays pinned: what arrived is what the
            // reader asked to see.
            self.vim.apply_motion(Motion::Last);
        } else {
            self.settle_follow();
        }
    }

    /// Drops the selection unless both of its ends still name a message the
    /// window holds.
    ///
    /// Whole or not at all. A mark whose message has been paged out or pushed past
    /// the window's cap cannot be put back anywhere, and narrowing the selection
    /// to the end that survived would be worse than losing it: `d` on half a
    /// selection is a one-message deletion the reader never asked for, and it
    /// would be asked for by the same key they used last time.
    fn retain_selection(&mut self) {
        let Some(selection) = self.selection.take() else {
            return;
        };

        let window = &self.conversation.window;
        let held = window.position_of(selection.anchor.message_id).is_some()
            && window.position_of(selection.focus.message_id).is_some();

        if held {
            self.selection = Some(selection);
        }
    }

    /// Keeps the follow state in step with where the cursor ended up.
    ///
    /// The two are one fact seen twice: a view pinned to the newest message is
    /// one whose cursor is on it. Anywhere else means the reader has moved away,
    /// and an arrival no longer has the right to move them.
    fn settle_follow(&mut self) {
        let last = self.conversation.window.len().saturating_sub(1);

        if self.conversation.window.is_empty() || self.vim.cursor() >= last {
            self.conversation.follow();
        } else {
            self.conversation.unfollow();
        }
    }

    // ---- fetching --------------------------------------------------------

    /// Whether the page in front of what is loaded is worth asking for.
    ///
    /// A window shorter than the margin is near both of its ends at once, and
    /// asking is still right: the conversation simply is not loaded yet.
    ///
    /// Counted in rows rather than in messages, which is the only way "near the
    /// top" means what a reader scrolling upwards thinks it means.
    #[must_use]
    pub fn wants_older(&self) -> bool {
        let window = &self.conversation.window;

        !self.fetching.is_in_flight(FetchDirection::Older)
            && !window.is_empty()
            && !window.exhausted_older
            && self.cursor_extent().0 < FETCH_MARGIN
    }

    /// Whether the page behind what is loaded is worth asking for.
    ///
    /// Only while the reader is away from the bottom. A view pinned to the
    /// newest message is already there, and an arrival reaches it through the
    /// feed rather than through a fetch.
    #[must_use]
    pub fn wants_newer(&self) -> bool {
        let window = &self.conversation.window;

        !self.fetching.is_in_flight(FetchDirection::Newer)
            && !window.is_empty()
            && !window.exhausted_newer
            && !self.conversation.auto_follow()
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
        let first = rows::first_row_of_message(&layout, self.vim.cursor()).unwrap_or(0);

        (first, rows::total_rows(&layout))
    }

    /// Records that a fetch for `direction` has been asked for.
    ///
    /// One fetch per direction at a time: this is what a trigger checks before
    /// it fires, so holding a key down cannot turn into a stream of requests.
    pub fn begin_fetch(&mut self, direction: FetchDirection) {
        self.fetching.set(direction, true);
    }

    /// Records that the fetch for `direction` is over, however it ended.
    ///
    /// A failed fetch releases the direction as surely as a successful one: the
    /// alternative is a conversation that can never be paged again because one
    /// request went wrong.
    pub fn end_fetch(&mut self, direction: FetchDirection) {
        self.fetching.set(direction, false);
    }

    /// Whether a fetch for `direction` is in flight.
    #[must_use]
    pub const fn is_fetching(&self, direction: FetchDirection) -> bool {
        self.fetching.is_in_flight(direction)
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
            FetchDirection::Older => self.conversation.window.exhausted_older = true,
            FetchDirection::Newer => self.conversation.window.exhausted_newer = true,
        }
    }

    // ---- what the reader asked for --------------------------------------

    /// Takes the operation the reader asked for, if there is one.
    ///
    /// Idempotent in the same way [`App::pending_jump`] is: once taken it is
    /// cleared, and a caller that asks twice gets one action. The request is
    /// handed over rather than made here because the network is the caller's.
    pub fn take_action(&mut self) -> Option<Action> {
        self.actions.pop_front()
    }

    /// Adds an operation to the queue the caller drains.
    ///
    /// The queue is bounded so that a burst cannot grow without limit. It is
    /// drained every pass, so reaching the bound means [`ACTION_QUEUE`]
    /// operations were queued between two ticks; the oldest is refused to make
    /// room, and the refusal is said out loud rather than that operation
    /// vanishing.
    fn queue_action(&mut self, action: Action) {
        if self.actions.len() >= ACTION_QUEUE {
            self.actions.pop_front();
            self.flash("too many requests at once — the oldest was dropped");
        }
        self.actions.push_back(action);
    }

    /// Records that a send for `temp_id` is in flight.
    ///
    /// The identifier rather than a flag, so releasing the gate can be matched
    /// to the send it answers.
    pub fn begin_send(&mut self, temp_id: i64) {
        self.sending = Some(temp_id);
    }

    /// Releases the in-flight gate, if it is still held for `temp_id`.
    ///
    /// A no-op for a send that has already been released, so a duplicate result
    /// cannot clear the gate of a later one.
    pub fn end_send(&mut self, temp_id: i64) {
        if self.sending == Some(temp_id) {
            self.sending = None;
        }
    }

    /// Replaces a send's placeholder with the message the server accepted.
    ///
    /// Reports whether anything changed, so the caller knows whether a redraw is
    /// owed.
    pub fn confirm_sent(&mut self, temp_id: i64, real: Message) -> bool {
        let anchor = self.cursor_message_id();
        let changed = self.conversation.confirm_sent(temp_id, real);
        if changed {
            self.after_window_change(anchor);
        }
        changed
    }

    /// Marks a send as failed, keeping the message and recording why.
    ///
    /// Reports whether the placeholder was there to mark.
    pub fn fail_send(&mut self, temp_id: i64, reason: String) -> bool {
        self.conversation.fail_send(temp_id, reason)
    }

    /// Removes a failed message and the reason recorded for it.
    ///
    /// Reports whether either was there.
    pub fn dismiss_failed(&mut self, temp_id: i64) -> bool {
        let anchor = self.cursor_message_id();
        let changed = self.conversation.dismiss_failed(temp_id);
        if changed {
            self.after_window_change(anchor);
        }
        changed
    }

    // ---- transient status -----------------------------------------------

    /// Shows `text` on the status line for a while, then reverts.
    ///
    /// For things that pass on their own: a send that failed, a refusal. State
    /// the reader must not lose is written straight to [`App::status`], which
    /// never carries a deadline.
    pub fn flash(&mut self, text: impl Into<String>) {
        self.status = text.into();
        self.status_until = Some(Instant::now() + FLASH_FOR);
    }

    /// Reverts a transient status once its time is up.
    ///
    /// Reports whether a redraw is owed. Called from the loop, which already
    /// runs on a timer: a status cannot expire during a frame, because a frame
    /// is drawn from a shared reference.
    pub fn expire_status(&mut self, now: Instant) -> bool {
        if self.status_until.is_none_or(|at| now < at) {
            return false;
        }

        self.status_until = None;
        IDLE_STATUS.clone_into(&mut self.status);
        true
    }

    // ---- the peer's typing -----------------------------------------------

    /// Records or drops the peer's typing for the conversation on show.
    ///
    /// Reports whether a redraw is owed. A conversation other than the open one
    /// is dropped rather than remembered: the note is drawn on one title, and a
    /// deadline kept for a chat the reader has left would be one nobody is shown
    /// and the reader has not been told about.
    fn apply_typing(&mut self, chat_id: i64, typing: bool) -> bool {
        if chat_id != self.conversation.window.chat_id {
            return false;
        }

        self.typing_until = if typing {
            // Re-armed rather than set once, because a peer who keeps typing past
            // the deadline is still typing.
            Some((chat_id, Instant::now() + TYPING_FOR))
        } else {
            None
        };
        true
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
        self.typing_until
            .is_some_and(|(chat, _)| chat == self.conversation.window.chat_id)
    }

    /// Stops showing the peer as typing once its deadline has passed.
    ///
    /// Reports whether a redraw is owed. Called from the loop beside
    /// [`App::expire_status`], which already runs on a timer: nothing repaints on
    /// a schedule for this, so a peer who stops without a final event is gone by
    /// the tick after the deadline rather than by a frame of its own.
    pub fn expire_typing(&mut self, now: Instant) -> bool {
        let Some((_, at)) = self.typing_until else {
            return false;
        };
        if now < at {
            return false;
        }

        self.typing_until = None;
        true
    }

    // ---- key handling --------------------------------------------------

    /// Puts the focus somewhere, leaving whatever the pane it came from was in.
    ///
    /// Visual mode belongs to the conversation and names messages in it, so
    /// leaving the conversation drops the selection and returns the mode to
    /// Normal. A selection for a conversation nobody is looking at would leave
    /// `d` holding something the reader cannot see.
    fn set_focus(&mut self, focus: Focus) {
        if focus != Focus::Conversation {
            self.mode = Mode::Normal;
            self.selection = None;
        }
        // The single clear point for every way out of a pane, beside the one
        // below it for the line. A profile is not a stack: leaving it means the
        // conversation is on show again, and there is no previous pane to go
        // back to.
        self.close_profile();
        // The single clear point for every way out of the line: `Tab`,
        // `BackTab`, `Ctrl+w` and `Esc`-to-leave all pass through here, so the
        // completion does not need a case in each of them.
        if focus != Focus::Input {
            self.emoji = None;
        }
        self.focus = focus;
    }

    /// Moves the focus one pane on, in the direction given, wrapping.
    ///
    /// The order is the order the panes are drawn in, so `Tab` walks the screen
    /// rather than an arbitrary list of them.
    fn cycle_focus(&mut self, forward: bool) {
        const PANES: [Focus; 3] = [Focus::ChatList, Focus::Conversation, Focus::Input];
        let step = if forward { 1 } else { PANES.len() - 1 };

        let at = PANES
            .iter()
            .position(|pane| *pane == self.focus)
            .unwrap_or(0);

        self.set_focus(PANES[(at + step) % PANES.len()]);
    }

    /// Leaves the input line for the conversation, keeping what was typed.
    ///
    /// `Ctrl+w` is Vim's other idiom for this, and the one bound to the pane
    /// walk, because `Esc` is no longer a single key: a reader stepping between
    /// panes should not have to know how many `Esc` presses the line's current
    /// mode takes, and this one leaves from any of them. It only ever looks
    /// away — nothing typed is lost to it.
    fn leave_line(&mut self) {
        if self.focus == Focus::Input {
            self.set_focus(Focus::Conversation);
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        // Ctrl-C always quits.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        // A confirmation is a question about the whole screen rather than about
        // a pane, so it outranks the focus: it has to be answered before another
        // key is addressed anywhere.
        if self.mode == Mode::Confirm {
            self.handle_confirm(key);
            return;
        }

        // The sign-in surface answers every key itself, and it has to be asked
        // **before** the pane walk below: while it is up `Tab` pauses the flow
        // rather than walking the panes, and `q` is unbound, so a reader typing a
        // phone number cannot quit the program out from under themselves.
        if self.signin.is_some() {
            self.handle_signin(key);
            return;
        }

        // A jump in flight is a page on its way to replace the window under the
        // reader, so nothing else acts until it lands: every other key is
        // swallowed rather than answered, because a key that moves the cursor
        // would move it out from under the page. `Esc` is the one answer — it
        // drops the jump and leaves the reader where they were. The page may
        // still land, and is dropped when it does: nobody is waiting for it.
        if self.pending_jump.is_some() {
            if key.code == KeyCode::Esc {
                self.pending_jump = None;
            }
            return;
        }

        // A completion owns a few keys for as long as it is up. `Ctrl-C` above
        // stays first: a reader reaching for it to abandon a half-typed
        // shortcode gets out of the program, which is what they asked for.
        if self.handle_completion(key) {
            return;
        }

        // Pane movement is the one thing every pane answers the same way, so it
        // is read here rather than bound in each of them.
        match key.code {
            KeyCode::Tab => {
                self.cycle_focus(true);
                return;
            }
            KeyCode::BackTab => {
                self.cycle_focus(false);
                return;
            }
            // `Ctrl-w` is the input line's way out, and a card's prefix for pane
            // movement — because the card's own `h`/`l` are an inline motion and a
            // key that is a motion in one place and a pane in the next is a key a
            // reader has to learn twice. The prefix is armed only on a card, so on
            // every other pane `Ctrl-w` is still exactly what it was.
            _ if key.modifiers.contains(KeyModifiers::CONTROL)
                && key.code == KeyCode::Char('w') =>
            {
                if self.pane.is_profile() {
                    self.profile_pending_w = true;
                } else {
                    self.leave_line();
                }
                return;
            }
            _ => {}
        }

        match self.focus {
            Focus::ChatList => self.handle_chat_list(key),
            // Matched on both axes rather than on `mode` alone: the profile is a
            // content of this pane, not a pane, and the wildcard that would save
            // the tuple here is the kind of arm that is right until the day it
            // is not.
            Focus::Conversation => match (self.mode, self.pane) {
                (Mode::Normal, Pane::Profile(_)) => self.handle_profile(key),
                (Mode::Normal, Pane::Conversation) => self.handle_normal(key),
                (Mode::Visual, _) => self.handle_visual(key),
                (Mode::Confirm, _) => self.handle_confirm(key),
            },
            Focus::Input => self.handle_line(key),
        }
    }

    /// Handles a key while the chat list has the focus.
    ///
    /// `j`, `k`, `gg` and `G` move the highlight and record the conversation it
    /// now names; `Enter` opens it at once, because a reader who presses it is
    /// not going to press anything else. `h` and `l` are the pane movement, and
    /// both mean the same thing from here: the conversation is the only pane
    /// beside this one, so there is nothing for the two of them to choose
    /// between.
    fn handle_chat_list(&mut self, key: KeyEvent) {
        let here = self.selected_chat;
        let last = self.list.chats.len().saturating_sub(1);

        match key.code {
            // The list is the only pane beside this one, so both keys are the
            // way into it.
            KeyCode::Char('h' | 'l') => self.set_focus(Focus::Conversation),

            // The account's own card, from the list as well as from the
            // conversation: a reader looking for settings has usually not opened a
            // conversation to look in.
            KeyCode::Char('S') => {
                self.pending_g = false;
                self.open_profile();
            }

            // The contact the highlight is on. `A` and not `l`, because `l` is
            // already the way into the conversation on this pane, and a key that
            // means two things in two panes is a key a reader has to learn twice.
            KeyCode::Char('A') => {
                self.pending_g = false;
                self.open_contact();
            }

            KeyCode::Char('j') => {
                self.pending_g = false;
                self.choose_chat(here.saturating_add(1).min(last));
            }
            KeyCode::Char('k') => {
                self.pending_g = false;
                self.choose_chat(here.saturating_sub(1));
            }
            KeyCode::Char('g') => {
                if std::mem::take(&mut self.pending_g) {
                    self.choose_chat(0);
                } else {
                    self.pending_g = true;
                }
            }
            KeyCode::Char('G') => {
                self.pending_g = false;
                self.choose_chat(last);
            }

            KeyCode::Enter => {
                self.pending_g = false;
                self.select_chat(here);
                self.set_focus(Focus::Conversation);
            }

            // Any other key ends the sequence, so a lone `g` does not become a
            // jump to the top the next time one is pressed.
            _ => self.pending_g = false,
        }
    }

    fn handle_normal(&mut self, key: KeyEvent) {
        // A screenful at a time, which is what a terminal scrolls by. Bound here
        // rather than in the motion table because how much a page is depends on
        // how tall the panel turned out to be.
        //
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('d') => self.page(true),
                KeyCode::Char('u') => self.page(false),
                // `Ctrl-o` and `Ctrl-i`: back and forward through the places the
                // reader has jumped from. Not while a jump is on its way — one
                // fetch is in flight and the reader may have escaped it, and a
                // second jump would replace the one they are still waiting for.
                KeyCode::Char('o') if self.pending_jump.is_none() => self.jump_back(),
                KeyCode::Char('i') if self.pending_jump.is_none() => self.jump_forward(),
                _ => {}
            }
            return;
        }

        // Every plain character goes to the motion table before anything else:
        // `gd` is a two-key sequence built on the same `g` as `gg`, and the
        // table owns that prefix. A character that never reached it left the `g`
        // armed for whatever key came next, so `g` then `d` then `G` deleted a
        // message instead of going to the end.
        let KeyCode::Char(c) = key.code else {
            return;
        };

        if let Some(motion) = self.vim.handle_char(c) {
            match motion {
                // `gg` is where the unread messages start when there are
                // any, and the top of what is loaded when there are not. A
                // jump the window can answer is taken here; one it cannot is
                // left for the caller to fetch. Either way the reader has
                // asked for something, so whatever they asked for before is
                // replaced by it.
                Motion::First => self.pending_jump = self.jump_to_unread(),

                // `G` is the reader overriding a jump with "take me to the
                // end". The page on its way is for a place they no longer
                // want to be, and it is dropped when it lands.
                Motion::Last => self.pending_jump = None,

                // `n` and `N` walk the search's matches. `VimState` reports
                // the motion but cannot answer it, because a match is a
                // place in a conversation and the list of them lives here.
                Motion::NextMatch => self.walk_search(true),
                Motion::PrevMatch => self.walk_search(false),

                // `gd`, likewise: which message this one quotes is a fact
                // about the conversation, and the window is what holds it.
                Motion::GotoReply => self.pending_jump = self.jump_to_reply(),

                // `j` and `k` are the motions themselves, and the table has
                // already applied them.
                Motion::Down | Motion::Up => {}
            }

            self.settle_follow();
            return;
        }

        match c {
            'i' | 'a' => self.start_compose(),
            'r' => self.start_reply(),
            'e' => self.start_edit(),
            // A plain `d`: the table reports nothing for it, so a `d` that does
            // not follow a `g` deletes here.
            'd' => self.request_delete(),
            'p' => self.paste(),
            'D' => self.dismiss_failed_at_cursor(),
            'v' => self.begin_selection(Some(0)),
            'V' => self.begin_selection(None),
            // The list is beside the conversation, so `h` is how the reader gets
            // to it. `l` has nothing to move to from here and is left unbound
            // rather than made to wrap.
            'h' => self.set_focus(Focus::ChatList),
            '/' => {
                self.focus = Focus::Input;
                self.line.open(PromptKind::Search);
            }
            ':' => {
                self.focus = Focus::Input;
                self.line.open(PromptKind::Command);
            }
            'q' => self.request_quit(),
            'S' => self.open_profile(),
            'A' => self.open_contact(),
            _ => {}
        }
    }

    /// Asks whether the reader meant to quit.
    ///
    /// `q` sits where a reader's hand already is and next to keys that type
    /// nothing else, so an accidental one is a real event rather than a
    /// hypothetical: a confirmation is the whole difference between losing the
    /// window and having pressed a key.
    ///
    /// `Ctrl-C` deliberately does not come through here. It is the way out when
    /// the program is wedged, and a question in front of it would be a question
    /// the reader cannot see, because a terminal that is not answering cannot
    /// draw one either.
    fn request_quit(&mut self) {
        self.mode = Mode::Confirm;
        self.confirm = Some(ConfirmKind::Quit);
    }

    /// Opens the buffer for a new message, with no reply and no edit.
    ///
    /// The draft is kept: a half-written message is not garbage, and the one
    /// thing a reader who pressed `i` by reflex should never lose is the thing
    /// they were writing.
    fn start_compose(&mut self) {
        self.focus = Focus::Input;
        self.line.open(PromptKind::Message);
        self.reply_to = None;
        self.editing = None;
    }

    /// Opens the buffer to answer the message under the cursor.
    ///
    /// Replying needs a message to answer; with the window empty there is none,
    /// and the key does nothing rather than opening a reply to nowhere.
    fn start_reply(&mut self) {
        let Some(id) = self.cursor_message_id() else {
            return;
        };

        self.focus = Focus::Input;
        self.line.open(PromptKind::Reply);
        self.reply_to = Some(id);
        self.editing = None;
    }

    /// Opens the buffer with the cursor's own message in it, for editing.
    ///
    /// The one opener that replaces the text, and the only one: the buffer's
    /// meaning changes from "a message" to "an edit of message 42", so there is
    /// only one right thing for it to contain.
    ///
    /// Both refusals are the same fact — there is nothing on the server to edit
    /// yet — so they share one line. A key that does nothing and says nothing
    /// reads as a hang, and this one is hit constantly now that `dd` accepts an
    /// incoming message.
    fn start_edit(&mut self) {
        let Some(message) = self.cursor_message() else {
            return;
        };

        if message.id <= 0 {
            self.flash("it hasn't been sent yet");
            return;
        }
        if !message.is_outgoing {
            self.flash("you can only edit your own messages");
            return;
        }

        let text = message.text.to_string();
        let id = message.id;

        self.focus = Focus::Input;
        self.line.open_with(PromptKind::Edit, text);
        self.editing = Some(id);
        self.reply_to = None;
    }

    /// Asks to delete what the selection covers, or the message under the cursor
    /// when there is none.
    ///
    /// A `d` in Normal is `dd` in Vim: a selection of exactly the message under
    /// the cursor, with no second press to distinguish. Two ways to say one thing
    /// is exactly what the old latch existed to arbitrate, and a reader who
    /// presses `dd` gets the same answer either way — which is why there is no
    /// latch any more, and why the test that a motion between the two `d`s cleared
    /// it is now a test that `j` and then `dd` deletes the message now under the
    /// cursor.
    ///
    /// Deletion is allowed on any real message, incoming included: Telegram
    /// permits it, and a private chat does remove the other side's words.
    fn request_delete(&mut self) {
        if self.selection.is_none() {
            // The mark goes on directly rather than through `App::select`: the
            // message came out of the window a line ago, so there is nothing to
            // check.
            let Some(id) = self.cursor_message_id() else {
                return;
            };
            self.set_selection(Selection::at(id, None));
        }

        self.confirm_delete();
    }

    /// Raises the confirmation for deleting every message the selection covers.
    ///
    /// A selection inside one message deletes the whole of it: a partial message
    /// is not something the protocol can do, and half a deletion is not something
    /// the reader would recognise afterwards.
    fn confirm_delete(&mut self) {
        let Some(selection) = self.selection else {
            return;
        };

        let Some(deletion) = self.deletion(&selection) else {
            self.selection = None;
            self.mode = Mode::Normal;
            self.flash(self.refuse_placeholders(&selection));
            return;
        };

        self.mode = Mode::Confirm;
        self.confirm = Some(ConfirmKind::DeleteMessages {
            ids: deletion.ids,
            outgoing: deletion.outgoing,
            skipped: deletion.skipped,
        });
    }

    /// What deleting `selection` would ask the server for, or `None` when every
    /// message in it is a placeholder.
    ///
    /// A placeholder is a local stand-in for a send the server has not
    /// acknowledged, so it has no identifier the server knows: naming one would
    /// have the whole request refused and take the real messages down with it.
    /// They are left out of `ids` and counted, and a selection of nothing but
    /// placeholders has nothing left to ask for.
    fn deletion(&self, selection: &Selection) -> Option<Deletion> {
        let mut deletion = Deletion::default();
        let covered = self.covered(Some(selection));

        for message in self
            .conversation
            .window
            .iter()
            .skip(covered.start)
            .take(covered.len())
        {
            if message.id <= 0 {
                deletion.skipped += 1;
                continue;
            }

            deletion.ids.push(message.id);
            deletion.outgoing += usize::from(message.is_outgoing);
        }

        (!deletion.ids.is_empty()).then_some(deletion)
    }

    /// The refusal for a selection of nothing but placeholders.
    ///
    /// The two sentences that already existed, kept: a failed message has a `D` to
    /// offer and one still on its way does not, and pointing at `D` for a message
    /// that has not left would be wrong. A selection of several gets the same
    /// distinction in the only words that are true of all of them — `D` dismisses
    /// one message at a time, and there is no bulk dismiss.
    fn refuse_placeholders(&self, selection: &Selection) -> &'static str {
        let covered = self.covered(Some(selection));
        let mut count = 0;
        let mut in_flight = false;

        for message in self
            .conversation
            .window
            .iter()
            .skip(covered.start)
            .take(covered.len())
        {
            count += 1;
            in_flight |= !matches!(message.status, MessageStatus::Failed);
        }

        let one = count == 1;

        match (one, in_flight) {
            (true, true) => "that message is still on its way",
            (true, false) => "that message never left — D dismisses it",
            (false, true) => "those messages are still on their way",
            (false, false) => "those messages never left — D dismisses one at a time",
        }
    }

    /// Dismisses the failed message under the cursor, if that is what it is.
    fn dismiss_failed_at_cursor(&mut self) {
        let Some((id, status)) = self
            .cursor_message()
            .map(|message| (message.id, message.status))
        else {
            return;
        };

        if !matches!(status, MessageStatus::Failed) {
            return;
        }

        let anchor = self.cursor_message_id();
        if self.conversation.dismiss_failed(id) {
            self.after_window_change(anchor);
        }
    }

    /// Handles a key while a confirmation is up.
    ///
    /// However it ends, the selection goes with it: it was made for this
    /// question, and a second `d` afterwards must ask about whatever is under the
    /// cursor then rather than reusing a range the reader has already answered.
    fn handle_confirm(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') => {
                match &self.confirm {
                    Some(ConfirmKind::Quit) => self.should_quit = true,
                    Some(ConfirmKind::Logout) => self.queue_action(Action::Logout),
                    Some(ConfirmKind::DeleteMessages { ids, .. }) => {
                        let chat_id = self.conversation.window.chat_id;
                        self.queue_action(Action::Delete {
                            chat_id,
                            message_ids: ids.clone(),
                        });
                    }
                    None => {}
                }
                self.confirm = None;
                self.selection = None;
                self.mode = Mode::Normal;
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                self.confirm = None;
                self.selection = None;
                self.mode = Mode::Normal;
            }
            _ => {}
        }
    }

    /// Moves the cursor a screenful, which is what `Ctrl+d` and `Ctrl+u` mean.
    ///
    /// A screenful is rows, and a page lands on a message: moving down can
    /// arrive in the middle of one, and the message that owns the row the
    /// reader asked for is what the cursor stands on — its first row, as a
    /// terminal page puts the reader at the top of what it moved to.
    ///
    /// Landing on the newest message re-engages following and moving away from
    /// it disengages, on the same rule as `j` and `k`, so a page and a line
    /// cannot disagree about whether the view is pinned.
    fn page(&mut self, down: bool) {
        let step = self.rows.get().max(1);
        let layout = self.row_layout();
        let total = rows::total_rows(&layout);
        let here = rows::first_row_of_message(&layout, self.vim.cursor()).unwrap_or(0);

        let target = if down {
            here.saturating_add(step).min(total.saturating_sub(1))
        } else {
            here.saturating_sub(step)
        };

        // A row that names no message — a day separator — is not somewhere the
        // cursor stops, so the page carries on past it in the direction it was
        // going rather than landing on it.
        if let Some(cursor) = rows::message_at_row_moving(&layout, target, down) {
            self.vim.set_cursor(cursor);
        }
        self.settle_follow();
    }

    /// Handles a key while the line has the focus.
    ///
    /// The line decides what the key means and says what the host has to do;
    /// this only carries out the two answers that are the host's. Everything
    /// else — motions, quick edits, a selection, the two stages of `Esc` — is
    /// answered inside [`LineEditor`], and is the reason the line is a wrapper
    /// rather than a `String`.
    fn handle_line(&mut self, key: KeyEvent) {
        match self.line.feed(key) {
            LineVerdict::Submit => self.submit(),
            LineVerdict::LeftEditing => self.leave_line(),
            LineVerdict::TooLong => self.flash("message is too long"),
            LineVerdict::Refused => self.flash("that motion on non-ASCII text is not built yet"),
            LineVerdict::Edited | LineVerdict::Ignored => {}
        }

        // A yank in the line is a yank: the same slot, the same drain, and the
        // same OSC 52 write the conversation's goes through. One seam, two
        // producers.
        if let Some(yanked) = self.line.take_yanked() {
            self.clipboard = Some(yanked);
        }

        // Last, because it is a function of what the line now holds: deriving
        // rather than maintaining is what keeps the popup from describing a
        // fragment the reader has already typed past.
        self.refresh_completion();
    }

    /// Handles the keys a completion takes while it is up.
    ///
    /// Answers whether the key was consumed. Only four keys are: `Up`/`Down`
    /// move the candidate, `Tab` and `Enter` accept, and `Esc` puts the
    /// completion away. Everything else — `j`, `k`, a space, `Backspace`,
    /// `Ctrl+J`, a `p` — is passed through to the line, which is what keeps
    /// `:joy` and `:jack_o_lantern` typable and lets a shortened query grow its
    /// list.
    ///
    /// `Enter` accepted here does not send: `Some` becomes `None`, and the next
    /// `Enter` arrives with nothing up and submits. No debounce, because the
    /// state already gives the right answer.
    fn handle_completion(&mut self, key: KeyEvent) -> bool {
        if self.emoji.is_none() || key.modifiers != KeyModifiers::NONE {
            return false;
        }

        match key.code {
            KeyCode::Up => self.move_completion(false),
            KeyCode::Down => self.move_completion(true),
            KeyCode::Tab | KeyCode::Enter => self.accept_completion(),
            KeyCode::Esc => self.emoji = None,
            _ => return false,
        }

        true
    }

    /// Moves the selected candidate one place, wrapping.
    fn move_completion(&mut self, forward: bool) {
        if let Some(trigger) = &mut self.emoji {
            trigger.move_selection(forward);
        }
    }

    /// Commits the chosen emoji over the `:query` that named it.
    ///
    /// The inserted text is exactly what was accepted: no trailing space,
    /// because a character the reader did not ask for is one they would have to
    /// delete. The range comes from the trigger, so what is replaced is what
    /// the popup was describing.
    fn accept_completion(&mut self) {
        let Some(trigger) = self.emoji.as_ref() else {
            return;
        };
        let Some(chosen) = trigger.chosen() else {
            return;
        };
        let range = trigger.range.clone();
        let text = chosen.as_str();

        match self.line.replace(range, text) {
            LineVerdict::TooLong => self.flash("message is too long"),
            _ => self.emoji = None,
        }
    }

    /// Re-derives the completion from the draft and the caret.
    ///
    /// Called after every key the line answered, and never anywhere else. A
    /// re-detection is a few microseconds, so there is nothing to be careful
    /// about. The gate is [`PromptKind::is_buffer`] rather than
    /// [`PromptKind::Message`]: a `:` command line and a `/` search line never
    /// complete, but a reply and an edit do. It is also insert mode only,
    /// because a shortcode is typed: the key that leaves insert for the line's
    /// own normal mode is not editing the text, so it must not re-open what the
    /// reader just put away.
    ///
    /// The row the reader was on is carried across, clamped into the new list,
    /// so typing one more character does not move them off a candidate that is
    /// still there.
    fn refresh_completion(&mut self) {
        if self.focus != Focus::Input
            || !self.line.purpose().is_buffer()
            || self.line.status() != "INSERT"
        {
            self.emoji = None;
            return;
        }

        let selected = self.emoji.as_ref().map_or(0, |trigger| trigger.selected);
        self.emoji = emoji::detect(self.line.text(), self.line.caret());
        if let Some(trigger) = &mut self.emoji {
            trigger.reselect(selected);
        }
    }

    /// Handles a key while a selection is being made.
    ///
    /// `Esc` is the way out of it: the selection goes and the mode returns to
    /// Normal, rather than the selection being left behind for a `d` to find.
    /// `o` and `O` exchange the two ends, so a selection dragged "backwards" can
    /// be re-anchored without being put back where it was.
    ///
    /// The character motions move the *focus* — the end that moves — rather than
    /// the anchor, which is what makes a selection grow from one end. They clamp
    /// to the message's own text and never cross into the next one: `j` and `k`
    /// are for that, and a motion that silently changed what it selected would be
    /// the worst thing a selection could do.
    fn handle_visual(&mut self, key: KeyEvent) {
        // A `f` takes the very next keypress as the character to look for, whatever
        // it is: that is what `fw` means, and reading the `w` as a motion would be
        // a different key entirely. Anything else ends the sequence.
        if let Some(Find { forward, onto }) = self.pending_find.take()
            && let KeyCode::Char(target) = key.code
        {
            self.move_focus(CharMotion::Find {
                target,
                forward,
                onto,
            });
            return;
        }
        self.pending_find = None;

        match key.code {
            KeyCode::Esc => {
                self.selection = None;
                self.mode = Mode::Normal;
                self.status = IDLE_STATUS.into();
            }

            // Re-anchoring on the cursor's message is `v` again, which is what it
            // is for: the reader is saying "start here instead".
            KeyCode::Char('v') => self.begin_selection(Some(0)),
            KeyCode::Char('V') => self.begin_selection(None),
            KeyCode::Char('o' | 'O') => {
                if let Some(selection) = &mut self.selection {
                    selection.swap();
                }
            }

            KeyCode::Char('y') => self.yank(),
            KeyCode::Char('d') => self.request_delete(),
            KeyCode::Char('r') => self.reply_to_selection(),

            KeyCode::Char('j') => self.move_focus_to_message(true),
            KeyCode::Char('k') => self.move_focus_to_message(false),

            KeyCode::Char('h') => self.move_focus(CharMotion::Step { forward: false }),
            KeyCode::Char('l') => self.move_focus(CharMotion::Step { forward: true }),
            KeyCode::Char('w') => self.move_focus(CharMotion::WordStart { forward: true }),
            KeyCode::Char('b') => self.move_focus(CharMotion::WordStart { forward: false }),
            KeyCode::Char('e') => self.move_focus(CharMotion::WordEnd),
            KeyCode::Char('0') => self.move_focus(CharMotion::Bound { end: false }),
            KeyCode::Char('$') => self.move_focus(CharMotion::Bound { end: true }),

            KeyCode::Char('f') => {
                self.pending_find = Some(Find {
                    forward: true,
                    onto: true,
                });
            }
            KeyCode::Char('t') => {
                self.pending_find = Some(Find {
                    forward: true,
                    onto: false,
                });
            }
            KeyCode::Char('F') => {
                self.pending_find = Some(Find {
                    forward: false,
                    onto: true,
                });
            }
            KeyCode::Char('T') => {
                self.pending_find = Some(Find {
                    forward: false,
                    onto: false,
                });
            }

            _ => {}
        }
    }

    /// Handles `y` in Visual: what the selection covers goes into the register.
    ///
    /// A text selection yanks exactly the characters selected and nothing else.
    /// Anything else yanks one line per message, oldest first, so a yank of three
    /// messages pastes back as three messages — which is what "yank these" means
    /// in a conversation, where a message is the unit a reader thinks in.
    ///
    /// Visual is left either way, worked or not. A `y` that found nothing has
    /// still answered the key, and staying in Visual would hide the refusal: the
    /// selection's own note outranks a transient status, so a `flash` written
    /// while a selection is up is a message the reader never sees.
    ///
    /// The register is the load-bearing half. The system clipboard is a
    /// convenience that depends on the terminal, the terminal emulator and often
    /// the user's settings — three things none of which can be tested here — and a
    /// yank that only works in the second is a yank that appears broken.
    fn yank(&mut self) {
        let Some(selection) = self.selection else {
            return;
        };

        let lines = self.yanked(&selection);
        self.selection = None;
        self.mode = Mode::Normal;

        if lines.iter().all(String::is_empty) {
            // A charwise selection that has not been moved is a position rather
            // than a span, and there is nothing in it to take. Said rather than
            // silently replacing whatever was in the register with nothing.
            self.flash("nothing to yank — move the selection first");
            return;
        }

        self.register = Register::set(lines);
        self.clipboard = Some(self.register.text());
    }

    /// The lines a selection yanks: one for a text selection, one per message for
    /// anything else.
    ///
    /// All of them possibly empty — a collapsed charwise selection yields one
    /// empty string, and a set of messages that happen to be blank yields several
    /// — which [`App::yank`] is what notices.
    ///
    /// A selection naming a message the window no longer holds yields nothing,
    /// which [`App::retain_selection`] makes unreachable and which is answered
    /// with an empty yank rather than a panic.
    fn yanked(&self, selection: &Selection) -> Vec<String> {
        if let Some((id, range)) = selection.text_range() {
            let Some(message) = self.conversation.window.iter().find(|m| m.id == id) else {
                return Vec::new();
            };

            let body = message.display_body();
            return vec![body[rows::byte_span(body, range)].to_owned()];
        }

        let covered = self.covered(Some(selection));
        self.conversation
            .window
            .iter()
            .skip(covered.start)
            .take(covered.len())
            .map(|message| message.display_body().to_owned())
            .collect()
    }

    /// Handles `r` in Visual, which is a refusal.
    ///
    /// This is the whole implementation, and it is a refusal for one of two
    /// reasons:
    ///
    /// - A selection that is not inside one message has no quote to send. Telegram
    ///   quotes a fragment of *one* message, and there is no wire representation
    ///   for quoting five — so `V` and a range have nothing to answer.
    /// - A quote of one message cannot be sent at all on the pinned `grammers`:
    ///   its `InputMessage` has no field for one, and it hard-codes
    ///   `quote_text`/`quote_offset` to `None` in the reply it builds. The
    ///   upstream gap is written up in
    ///   `~/.opencode/plan/pr-grammers-quote-support.md`, and it is a refusal
    ///   rather than a workaround because composing the quote as ordinary message
    ///   text produces something that *looks* like a quote and is not — the
    ///   difference is visible to the person receiving it.
    ///
    /// Which is also why this is not "reply to the cursor's message instead": a key
    /// that answered a different question than the one asked, while the screen said
    /// `-- VISUAL --`, would be worse than a refusal.
    ///
    /// Visual is left either way, for the reason [`App::yank`] gives: the selection's
    /// own note outranks a transient status, so a refusal written while a selection
    /// is up is a line the reader never sees.
    fn reply_to_selection(&mut self) {
        let Some(selection) = self.selection else {
            return;
        };

        let refused = if selection.text_range().is_some() {
            "quoting a reply is not built yet"
        } else {
            "a reply can only quote words inside one message"
        };

        self.selection = None;
        self.mode = Mode::Normal;
        self.flash(refused);
    }

    /// Handles `p` in Normal: opens the line with what was last yanked.
    ///
    /// A yank with no paste is a one-way trip to the system clipboard, and the
    /// system clipboard is not somewhere a message can be sent from. This is the
    /// paste that puts the reader's own words back in front of them, at the
    /// caret, to be edited and sent like anything else they typed.
    ///
    /// A draft already in the bar is kept rather than replaced, and the register
    /// goes in at the caret: `p` is a paste, so it behaves like one everywhere
    /// else, and a reader who wants to throw their draft away has a key that
    /// does that.
    fn paste(&mut self) {
        if self.register.is_empty() {
            self.flash("nothing has been yanked");
            return;
        }

        self.start_compose();
        self.line.insert(&self.register.text());
    }

    /// Starts a selection at the cursor's message, character-wise or whole.    ///
    /// `Some(0)` is a charwise selection from the message's first character;
    /// `None` is the whole message, which is what `V` selects. Both set the mode,
    /// because this is the only way *into* Visual.
    ///
    /// The mark goes on directly rather than through [`App::select`]: the message
    /// came out of the window a line ago, so there is nothing to check.
    fn begin_selection(&mut self, char: Option<usize>) {
        let Some(id) = self.cursor_message_id() else {
            return;
        };

        self.set_selection(Selection::at(id, char));
        self.mode = Mode::Visual;
    }

    /// Applies a character motion to the focus's position within its message.
    ///
    /// Nothing happens without a character position to move: a linewise selection
    /// is of a whole message and there is no place inside it to move to, which is
    /// also what Vim does. The cursor does not follow — it stands on the anchor's
    /// message until the focus moves to another one, so that a charwise selection
    /// does not drag the viewport along with every character.
    fn move_focus(&mut self, motion: CharMotion) {
        let Some(selection) = &mut self.selection else {
            return;
        };
        let Some(at) = selection.focus.char else {
            return;
        };
        let id = selection.focus.message_id;

        let Some(message) = self.conversation.window.iter().find(|m| m.id == id) else {
            return;
        };

        selection.focus.char = Some(char_motion(message.display_body(), at, motion));
    }

    /// Moves the focus to the next or the previous message, and the cursor with it.
    ///
    /// The cursor follows because this is the only motion that leaves the message:
    /// a reader stepping through messages with `j` is reading, not selecting
    /// characters, and a cursor left behind would be off the selection entirely.
    ///
    /// A character position carries over where it still fits, so a selection that
    /// has already been moved within a message keeps its relative place in the
    /// next one. It stops mattering as soon as the two ends are in different
    /// messages — which is exactly what they now are.
    fn move_focus_to_message(&mut self, forward: bool) {
        let Some(focus) = self.selection.map(|selection| selection.focus) else {
            return;
        };
        let Some(index) = self.conversation.window.position_of(focus.message_id) else {
            return;
        };
        let next = if forward {
            index + 1
        } else {
            index.saturating_sub(1)
        };
        let Some(message) = self.conversation.window.get(next) else {
            return;
        };

        let id = message.id;
        let last = message.display_body().chars().count().saturating_sub(1);
        let char = focus.char.map(|at| at.min(last));

        if let Some(selection) = &mut self.selection {
            selection.focus = Mark {
                message_id: id,
                char,
            };
        }
        self.vim.set_cursor(next);
        self.settle_follow();
    }

    /// Handles `Enter` in the input line.
    ///
    /// Which prompt it is decides what is handed over; the buffer and the reply
    /// context are cleared either way, because the work leaves here rather than
    /// happening here. The focus goes back to the conversation for the same
    /// reason: the line has given up what it was for.
    ///
    /// The text is taken *after* the check that a send is allowed, so a send
    /// refused here leaves the words in the bar rather than taking them out of
    /// it — the reader would otherwise lose a message they had already written
    /// to a line that was already refusing.
    ///
    /// The one command that leaves the line as something else is `:signin`,
    /// which opens a field rather than finishing on the conversation: that one
    /// returns before the reset, because the reset is what would empty it.
    fn submit(&mut self) {
        match self.line.purpose() {
            PromptKind::Message | PromptKind::Reply => self.submit_message(),
            PromptKind::Edit => self.submit_edit(),
            PromptKind::Command => {
                let cmd = self.line.take();
                self.run_command(cmd.trim());
            }
            PromptKind::Search => {
                let query = self.line.take();
                self.run_search(query.trim());
            }
            // Unreachable: a sign-in field answers `Enter` itself, so that its
            // `⏎` can be refused while a request is on its way — which a submit
            // with no way to refuse is.
            PromptKind::Phone | PromptKind::Code | PromptKind::Password => {}
        }

        // A sign-in field is what the line is now: `:signin` opened it
        // pre-filled, and the reset below belongs to the commands that finish
        // on the conversation. Clearing it would empty the field the reader
        // came for, and handing the focus to the conversation would route
        // their keys to an arm with nothing to say about a flow in progress.
        if self.signin_field().is_some() {
            self.reply_to = None;
            self.editing = None;
            return;
        }

        self.focus = Focus::Conversation;
        self.line.clear();
        self.reply_to = None;
        self.editing = None;
    }

    /// Queues the composed message as a send, and shows it immediately.
    ///
    /// The placeholder is what the reader sees until the server answers, and its
    /// identifier is what the answer is matched against. One send is in flight at
    /// a time: Telegram throttles per conversation, and a second send would only
    /// earn a `FLOOD_WAIT` — but a silent no-op reads as a hang, so the refusal
    /// says so.
    fn submit_message(&mut self) {
        if self.sending.is_some() {
            self.flash("a message is already on its way");
            return;
        }
        if !self.has_conversation() || self.line.text().trim().is_empty() {
            return;
        }

        let anchor = self.cursor_message_id();
        let chat_id = self.conversation.window.chat_id;
        let text = self.line.take();
        let temp_id = self.conversation.queue_send(&text, self.reply_to);
        self.begin_send(temp_id);
        self.queue_action(Action::Send {
            chat_id,
            temp_id,
            text,
            reply_to: self.reply_to,
        });
        self.after_window_change(anchor);
    }

    /// Queues the edit of the message the buffer was opened with.
    ///
    /// Nothing is shown optimistically: an edit is reflected when the server's
    /// `MessageEdited` arrives, which is the only path by which its new text
    /// reaches the window.
    fn submit_edit(&mut self) {
        let Some(message_id) = self.editing else {
            return;
        };
        if !self.has_conversation() || self.line.text().trim().is_empty() {
            return;
        }

        let chat_id = self.conversation.window.chat_id;
        let text = self.line.take();
        self.queue_action(Action::Edit {
            chat_id,
            message_id,
            text,
        });
    }

    fn run_command(&mut self, cmd: &str) {
        match cmd {
            "q" | "quit" => self.request_quit(),
            "settings" => self.open_profile(),
            // The same path from the signed-out card and from a launch with no
            // session: they are the same question, and two entry points would be
            // two flows that could come to differ.
            "signin" => self.begin_signin(),
            "retry" => self.request_retry(),
            _ if cmd.starts_with("chat ") => {
                if let Ok(id) = cmd[5..].trim().parse::<i64>()
                    && let Some(pos) = self.list.chats.iter().position(|c| c.id == id)
                {
                    self.select_chat(pos);
                }
            }
            _ => self.status = format!("unknown command: :{cmd}"),
        }
    }

    /// Answers `/`: scans the window now, and asks the server if it can do
    /// better.
    ///
    /// The local pass is free and synchronous, so `n` works in the same frame.
    /// It is provisional — it can only see what is loaded, and it approximates
    /// what the server does — so the server's answer replaces it when it comes.
    ///
    /// An empty query repeats the last search, as in Vim. With nothing to
    /// repeat, the refusal is visible rather than the key doing nothing.
    fn run_search(&mut self, query: &str) {
        let Some(query) = self.search_to_run(query) else {
            self.flash("no previous search");
            return;
        };

        let chat_id = self.conversation.window.chat_id;
        let ids: Vec<i64> = self
            .conversation
            .window
            .iter()
            .filter(|message| word_prefix_match(message.display_body(), &query))
            .map(|message| message.id)
            .collect();

        self.search.begin_local(&query, ids);
        self.land_on_match();

        // A conversation the window holds in full cannot be searched better, so
        // the round trip would be pure latency. Otherwise the request is handed
        // to the caller, which can reach the network, and the answer arrives at
        // [`App::apply_searched`].
        if holds_everything(&self.conversation.window) {
            self.search.finish_local();
        } else {
            self.queue_action(Action::Search { chat_id, query });
        }
    }

    /// The query a search should run, resolving an empty one to the last search.
    ///
    /// `None` when there is nothing to repeat, which is the one case `/` cannot
    /// answer.
    fn search_to_run(&self, query: &str) -> Option<String> {
        let query = query.trim();
        if !query.is_empty() {
            return Some(query.to_owned());
        }

        self.search.query().map(str::to_owned)
    }

    /// Lands the reader on the match the walk has just moved to.
    ///
    /// The local pass's matches are all in the window, so this is synchronous.
    fn land_on_match(&mut self) {
        if let Some(id) = self.search.next()
            && let Some(position) = self.conversation.window.position_of(id)
        {
            self.vim.set_cursor(position);
        }

        self.settle_follow();
    }

    /// Walks the search's matches in the direction given.
    ///
    /// A match that is loaded is a cursor move; one that is not is a [`Jump`],
    /// which is the same path `gg` takes. Wrapping announces itself, because a
    /// walk that looped silently reads as a stuck key.
    fn walk_search(&mut self, forward: bool) {
        if !self.search.is_active() {
            self.flash("no previous search");
            return;
        }
        if self.search.is_empty() {
            self.flash("nothing matched");
            return;
        }

        self.search.clear_notice();
        let before = self.search.index();
        let Some(id) = (if forward {
            self.search.next()
        } else {
            self.search.prev()
        }) else {
            return;
        };

        if wrapped(before, self.search.index(), self.search.len()) {
            self.search.note_wrap(forward);
        }

        if let Some(position) = self.conversation.window.position_of(id) {
            self.vim.set_cursor(position);
        } else {
            self.pending_jump = Some(Jump {
                peer_id: self.conversation.window.chat_id,
                target_id: id,
                kind: JumpKind::Unread,
            });
        }

        self.settle_follow();
    }

    /// Replaces the local matches with the server's answer, if it is still
    /// wanted.
    ///
    /// Returns whether the answer landed. It is refused for a conversation that
    /// is no longer open and for a query the reader has replaced — the same
    /// discipline a send's result gets, applied to the other half of the
    /// answer's identity.
    pub fn apply_searched(
        &mut self,
        chat_id: i64,
        query: &str,
        ids: Vec<i64>,
        total: usize,
    ) -> bool {
        if self.conversation.window.chat_id != chat_id {
            return false;
        }

        let cursor_id = self.cursor_message_id();
        if !self.search.adopt_server(query, ids, total, cursor_id) {
            return false;
        }

        // The cursor was on a local match the server may not have confirmed.
        // Landing it on the nearest surviving match keeps its sense of place;
        // the next `n` then moves forward from there rather than restarting.
        if let Some(target) = self.search.landing(cursor_id)
            && let Some(position) = self.conversation.window.position_of(target)
        {
            self.vim.set_cursor(position);
        }

        self.settle_follow();
        true
    }

    /// Records that the server pass for `query` failed, keeping the local list.
    pub fn search_failed(&mut self, query: &str, reason: String) {
        if self.search.query() != Some(query) {
            return;
        }

        self.search.fail(reason);
    }

    // ---- the sign-in flow -----------------------------------------------

    /// The sign-in surface, if it is up.
    #[must_use]
    pub fn signin(&self) -> Option<&SignIn> {
        self.signin.as_ref()
    }

    /// The field the reader is filling in, which is whatever step the flow is at.
    ///
    /// **Derived, never stored.** A second field naming the current one is a
    /// second thing that can be wrong: `login.step` is Telegram's own answer to
    /// the same question, and the moment the two disagree the bar would be
    /// asking for a code at the phone step. One answer, read from the state.
    #[must_use]
    pub fn signin_field(&self) -> Option<LoginField> {
        match self.signin.as_ref().and_then(SignIn::flow) {
            None => None,
            Some(flow) => match &flow.login.step {
                domain::session::SessionState::LoggedOut => Some(LoginField::Phone),
                domain::session::SessionState::AwaitingCode { .. } => Some(LoginField::Code),
                domain::session::SessionState::AwaitingPassword { .. } => {
                    Some(LoginField::Password)
                }
                // Signed in is not a step anybody fills a field in at: the flow
                // is over by the time the account's own identifier exists.
                domain::session::SessionState::LoggedIn { .. } => None,
            },
        }
    }

    /// Records that the client is to be brought up again.
    ///
    /// The `offline:` sentence is answered the moment it is read, so the status
    /// line says the retry is under way rather than leaving a sentence about a
    /// failed launch up beside one in flight — and what the network side writes
    /// next (the retry sentence, or the next `offline:`) replaces it.
    pub fn request_retry(&mut self) {
        self.retry_requested = true;
        "reconnecting".clone_into(&mut self.status);
        // Written straight to `status` rather than through `flash`, because a
        // bring-up is not a thing that passes on its own: it ends in an event, and
        // that event brings its own sentence.
        self.status_until = None;
    }

    /// The retry the reader asked for, once.
    ///
    /// Forgotten on the way out, the way [`App::take_pending_chat`] is: a request
    /// taken is a request being carried out, and a caller that asks again on the
    /// next pass gets `None` rather than a second bring-up.
    pub fn take_retry_request(&mut self) -> bool {
        std::mem::take(&mut self.retry_requested)
    }

    /// Puts the sign-in flow up, with the phone field open and the configured
    /// number in it.
    ///
    /// Records whether a client is there to carry a request, and ends the wait
    /// when one is not.
    ///
    /// The wait is cleared here rather than left to time: an in-flight flag
    /// with no client behind it is a sentence on the panel that nothing will
    /// ever answer, so losing the client takes the flow out of "Checking…" and
    /// lets the reader press `⏎` again. The draft is untouched — the reader
    /// typed it, and a client that comes back is not a reason to type it twice.
    pub fn set_client_available(&mut self, available: bool) {
        self.client_available = available;

        if !available && let Some(flow) = self.signin.as_mut().and_then(SignIn::flow_mut) {
            flow.waiting = false;
        }
    }

    /// The one way in, whatever the reader came from: the signed-out card's
    /// `:signin`, and a launch with no session are the same flow, because they
    /// are the same question. The card is closed rather than covered — a
    /// conversation and a card have nothing to say while somebody is typing a
    /// password, and a card the reader asked to leave should be left.
    ///
    /// **A machine with no application credentials gets the sentence instead.**
    /// `:signin` is answered wherever it is typed, including from the no-credentials
    /// screen and from a card that is not about the account — and a form whose
    /// answer could not be used is worse than the sentence that explains why.
    pub fn begin_signin(&mut self) {
        if !self.credentials_configured {
            self.begin_no_credentials();
            return;
        }

        self.pane = Pane::Conversation;
        self.mode = Mode::Normal;
        self.signin = Some(SignIn::Flow(SignInFlow::default()));
        self.open_signin_field(LoginField::Phone);
        self.set_focus(Focus::Input);
    }

    /// Puts up the sentence a machine with no credentials gets.
    ///
    /// A sentence and not a form, because there is nothing to type: the missing
    /// `api_id` and `api_hash` are in a file, and a phone field here would ask
    /// the reader for something the program still could not do with.
    pub fn begin_no_credentials(&mut self) {
        self.pane = Pane::Conversation;
        self.mode = Mode::Normal;
        self.signin = Some(SignIn::NoCredentials);
        self.set_focus(Focus::Conversation);
    }

    /// Records that the stored session is one Telegram no longer knows.
    ///
    /// The flow starts as it always does, with one row differing: the phone
    /// carries an offer instead of a state, because there is no step to go back
    /// to — the session is gone and the number is the whole way in again.
    pub fn begin_stale_signin(&mut self) {
        self.begin_signin();
        self.flash("the stored session is no longer valid — sign in again");
        if let Some(flow) = self.signin.as_mut().and_then(SignIn::flow_mut) {
            flow.stale = true;
        }
    }

    /// Opens `field` in the line, pre-filled from the configuration.
    ///
    /// The phone is the configuration's number because a phone number is not a
    /// secret and is the one thing about a sign-in a reader does not have to
    /// type. The code and the password are pre-filled only where the
    /// configuration carries them, and only when the step *opens*: what arrives
    /// here from a refusal is the same call, which is why a wrong code is not
    /// restored — a line holding a wrong answer is a line holding what Telegram
    /// already refused.
    fn open_signin_field(&mut self, field: LoginField) {
        self.open_signin_field_with(field, true);
    }

    /// Opens `field` with nothing in it, whatever the configuration carries.
    ///
    /// The refusal path, and the reason it is a separate call: a step the reader
    /// has just been told was wrong opens on a blank line, because a pre-filled
    /// one would put back the answer that was refused — and a code Telegram
    /// expires is worse than one the reader has to look up again.
    fn open_signin_field_blank(&mut self, field: LoginField) {
        self.open_signin_field_with(field, false);
    }

    /// Opens `field`, taking the configuration's value only when `prefill` says
    /// this is a step opening rather than a step being refused.
    fn open_signin_field_with(&mut self, field: LoginField, prefill: bool) {
        let prompt = match field {
            LoginField::Phone => PromptKind::Phone,
            LoginField::Code => PromptKind::Code,
            LoginField::Password => PromptKind::Password,
        };
        let text = match field {
            LoginField::Phone if prefill => self.phone.trim().to_owned(),
            LoginField::Code if prefill => self.code_prefill.clone(),
            LoginField::Password if prefill => self.password_prefill.clone(),
            _ => String::new(),
        };
        self.line.open_with(prompt, text);
    }

    /// Steps away from the flow, keeping the step.
    ///
    /// `Tab` and `Ctrl+w` are the pane walk everywhere else and here they mean
    /// this, which is why the sign-in is answered before the walk rather than
    /// through it. The code does not survive the trip — see
    /// [`SignInFlow::lost_code`] — and everything else does, because a phone
    /// number is not a secret and a password stays in a line that paints itself
    /// as bullets whether it has the focus or not.
    fn signin_away(&mut self) {
        if self.signin_field() == Some(LoginField::Code) {
            self.line.clear();
            if let Some(flow) = self.signin.as_mut().and_then(SignIn::flow_mut) {
                flow.lost_code = true;
            }
        }
        self.set_focus(Focus::ChatList);
        self.flash("sign-in paused; Tab brings it back");
    }

    /// Comes back to the field the step is at.
    ///
    /// The step survives the trip away and the code does not, so the return says
    /// what was lost and what to do about it rather than opening an empty line
    /// the reader has to work out the state of.
    fn signin_back(&mut self) {
        let Some(field) = self.signin_field() else {
            return;
        };
        self.open_signin_field(field);
        self.set_focus(Focus::Input);
        let lost = self
            .signin
            .as_mut()
            .and_then(SignIn::flow_mut)
            .is_some_and(|flow| std::mem::take(&mut flow.lost_code));
        if lost {
            self.flash("the code did not survive; ⏎ asks for a new one");
        } else {
            self.clear_status();
        }
    }

    /// `Esc` at the code or the password: back to the phone.
    ///
    /// The code Telegram sent is discarded rather than kept, so the sentence
    /// says what that costs and offers the way to get another one. The flow
    /// stays up: the reader asked to sign in, not to stop.
    fn signin_cancel(&mut self) {
        self.queue_action(Action::LoginCancelled);
        if let Some(flow) = self.signin.as_mut().and_then(SignIn::flow_mut) {
            flow.login = domain::session::LoginState::default();
        }
        self.open_signin_field(LoginField::Phone);
        self.set_focus(Focus::Input);
        self.flash("cancelling discards the code Telegram sent; ⏎ asks for a new one");
    }

    /// `⏎` on a field: asks the caller to move the flow on.
    ///
    /// The value leaves as an [`Action::Login`] rather than as a call, because a
    /// sign-in request is the network's and `tui` may not name it.
    ///
    /// **The in-flight guard is the whole reason this is a method rather than
    /// three lines in the key handler.** A second `⏎` while `waiting` fires no
    /// second request: Telegram counts a login attempt per request, and a reader
    /// pressing `⏎` twice is asking whether the first went out, not for two
    /// codes. The first one is answered with the sentence, and after that the
    /// key is silence — see [`SignInFlow::still_said`].
    fn submit_signin_field(&mut self) {
        let Some(field) = self.signin_field() else {
            return;
        };

        if let Some(flow) = self.signin.as_ref().and_then(SignIn::flow)
            && flow.waiting
        {
            let said = self
                .signin
                .as_mut()
                .and_then(SignIn::flow_mut)
                .is_some_and(|flow| !std::mem::take(&mut flow.still_said));
            if said {
                self.flash("still checking — the answer is on its way");
            }
            return;
        }

        let value = self.line.text().trim().to_owned();

        // An empty field is refused here rather than sent. A blank phone number
        // is a request Telegram throttles, a blank code is a login attempt the
        // reader did not mean to spend, and a blank password says nothing at
        // all — none of which is worth a round trip to be told.
        if value.is_empty() {
            self.flash(match field {
                LoginField::Phone => "there is no phone number to ask for a code with".to_owned(),
                LoginField::Code => "there is no login code to send".to_owned(),
                LoginField::Password => "there is no password to check".to_owned(),
            });
            return;
        }

        // **The client-less branch.** A request nobody will carry is not a
        // request on its way, so the flow is not told one is: that would put
        // "Checking…" on the panel for an answer that is never coming, and the
        // key after it would be swallowed by the guard as a second press. The
        // sentence is the whole answer, the draft stays in the line because the
        // client coming up is not the reader typing it again, and nothing is
        // queued — a queued login would fire on its own if a client appeared
        // later, which is a sign-in attempt nobody asked for.
        if !self.client_available {
            self.flash("not connected yet — the client is not up");
            return;
        }

        self.queue_action(Action::Login { field, value });
        if let Some(flow) = self.signin.as_mut().and_then(SignIn::flow_mut) {
            flow.waiting = true;
            flow.still_said = false;
        }
        // The line stays, dimmed, because the reader has to see what they sent
        // while it is in flight — and because emptying it would make the answer
        // arrive against nothing at all.
        self.set_focus(Focus::ChatList);
    }

    /// Answers a key while the sign-in surface is up.
    ///
    /// Ahead of the pane walk and ahead of every focus arm, which is the shape
    /// of the whole surface: while it is up, `Tab` means *pause* rather than
    /// *walk*, and `q` is unbound, because a reader who types their phone number
    /// into a chat list should not be able to quit the program from it.
    fn handle_signin(&mut self, key: KeyEvent) {
        match self.focus {
            Focus::Input => self.handle_signin_field(key),
            // Paused, or waiting for an answer. Nothing here answers anything
            // except the way back and the stale session's offer.
            Focus::ChatList => self.handle_signin_away(key),
            // The no-credentials sentence: no field, so no flow to pause, and
            // the two keys it answers are the two the shell cards name.
            Focus::Conversation => {
                if self.signin.as_ref() == Some(&SignIn::NoCredentials) {
                    match key.code {
                        KeyCode::Char('q') => self.request_quit(),
                        // A command line is the way to `:q` from a shell that has
                        // no session and no chat, so the key has to answer here
                        // rather than in a focus that does not exist yet.
                        KeyCode::Char(':') => {
                            self.signin = None;
                            self.line.open(PromptKind::Command);
                            self.set_focus(Focus::Input);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// A key with a sign-in field open.
    ///
    /// `Enter` submits, `Esc` gives up the step — or the flow, at the phone step,
    /// where the step *is* the flow — and everything else is the
    /// line's: an insert-only prompt, so there is no line's Normal mode to
    /// leave and the editor answers insert keys the same way it does for a
    /// command line.
    fn handle_signin_field(&mut self, key: KeyEvent) {
        let field = self.signin_field();
        let control = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Enter => self.submit_signin_field(),
            // `Esc` at the two steps that hold a secret asks Telegram for a new
            // code rather than leaving a typed password in a dimmed bar. At the
            // phone step there is nothing to discard but the flow itself, and
            // the hint names that key `cancel` — so it goes, rather than
            // pausing into an overlay every other key is swallowed by.
            KeyCode::Esc if matches!(field, Some(LoginField::Code | LoginField::Password)) => {
                self.signin_cancel();
            }
            KeyCode::Esc => self.signin_dismiss(),
            KeyCode::Tab | KeyCode::BackTab => self.signin_away(),
            _ if control && key.code == KeyCode::Char('w') => self.signin_away(),
            _ => self.handle_line(key),
        }
    }

    /// `Esc` at the phone step: the flow is done with, and so is the surface.
    ///
    /// **A pause would be a trap here.** `Tab` and `Ctrl-w` step away and are
    /// documented as the way back, so a reader who used them is not stuck; `Esc`
    /// is the one key the hint calls `cancel`, and it used to route to that same
    /// pause, which left the flow swallowing every key that is not `Tab`,
    /// `BackTab` or `Ctrl-w` — the conversation included, so `q` was dead and
    /// `Ctrl-C` was the only way out. A key that says `cancel` has to cancel.
    ///
    /// The shape is [`App::login_complete`]'s and it is that call rather than a
    /// second copy of it: dropping the flow, emptying the bar and giving the
    /// conversation its focus are the same three things in both cases, and two
    /// copies of that is two places for them to drift. What differs is only why,
    /// which is what this function is for.
    fn signin_dismiss(&mut self) {
        self.login_complete();
    }

    /// A key while the flow is paused or waiting.
    ///
    /// `Tab`, `BackTab` and `Ctrl-w` all come back, because the reader stepped
    /// away with one of them and one route back is not one per way out. `Enter`
    /// answers the stale session's offer, and nothing else answers anything: a
    /// request is in flight, or the reader is somewhere else, and a key that
    /// moved the flow on from here would be a key moving it on without a field.
    fn handle_signin_away(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => self.signin_back(),
            _ if key.modifiers.contains(KeyModifiers::CONTROL)
                && key.code == KeyCode::Char('w') =>
            {
                self.signin_back();
            }
            KeyCode::Enter
                if self
                    .signin
                    .as_ref()
                    .and_then(SignIn::flow)
                    .is_some_and(|flow| flow.stale) =>
            {
                self.signin_back();
                self.submit_signin_field();
            }
            _ => {}
        }
    }

    /// Telegram answered: the flow is at `state` now.
    ///
    /// The answer opens the next field, because a step the reader cannot type
    /// into is a step they are waiting on, and it opens it with whatever the
    /// configuration carries for it — this is the step change, which is the one
    /// moment a pre-fill is right. `SESSION_PASSWORD_NEEDED` lands here rather
    /// than in [`App::login_refused`] — it is not a refusal, it is the answer
    /// that puts the password row up.
    pub fn login_advanced(&mut self, state: domain::session::SessionState, hint: Option<String>) {
        let Some(flow) = self.signin.as_mut().and_then(SignIn::flow_mut) else {
            return;
        };
        flow.login.step = state;
        flow.login.refusal = None;
        flow.waiting = false;
        flow.still_said = false;
        // Kept only where it means something. An account with no two-step
        // password answers `None`, and so does every other step, so a hint read
        // on the password step cannot be drawn on the code step that follows it.
        flow.hint = match &flow.login.step {
            domain::session::SessionState::AwaitingPassword { .. } => hint,
            _ => None,
        };

        match self.signin_field() {
            Some(field) => {
                self.open_signin_field(field);
                self.set_focus(Focus::Input);
            }
            None => self.login_complete(),
        }
    }

    /// Telegram refused, in the reader's own words.
    ///
    /// The sentence is carried rather than a code, because the words are what
    /// the reader reads and the code is Telegram's — and `tui` may not name the
    /// enum that holds it. `used` is the password count the same sentence is
    /// counted from, so the row and the refusal cannot disagree about how many
    /// attempts are left.
    ///
    /// The field is re-opened **blank** rather than left as it was: a refusal
    /// arrives after the reader has stopped looking at the line, and a line
    /// holding what they typed is a line holding a wrong answer.
    pub fn login_refused(&mut self, sentence: String, used: u8) {
        let Some(flow) = self.signin.as_mut().and_then(SignIn::flow_mut) else {
            return;
        };
        flow.login.refusal = Some(sentence);
        flow.used = used;
        flow.waiting = false;
        flow.still_said = false;

        if let Some(field) = self.signin_field() {
            self.open_signin_field_blank(field);
            self.set_focus(Focus::Input);
        }
    }

    /// The account is signed in: the surface has nothing left to say.
    ///
    /// The flow is dropped rather than left at its last step, so the conversation
    /// is the whole program again — which is what signing in is for.
    pub fn login_complete(&mut self) {
        self.signin = None;
        self.line.clear();
        self.set_focus(Focus::Conversation);
        self.clear_status();
    }

    /// Puts the status line back to its resting sentence.
    ///
    /// The other half of [`App::flash`], for the answers that are not transient:
    /// a flow that has said its sentence and moved on must not keep showing it
    /// over the next thing the reader does.
    fn clear_status(&mut self) {
        IDLE_STATUS.clone_into(&mut self.status);
        self.status_until = None;
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
        match self.signin.as_ref() {
            Some(signin) => widgets::signin::render(self, signin, horizontal[1], frame),
            None => match self.pane {
                Pane::Conversation => {
                    widgets::conversation::render(self, horizontal[1], frame, layout);
                }
                Pane::Profile(_) => widgets::profile::render(self, horizontal[1], frame),
            },
        }
        widgets::input_bar::render(self, vertical[1], frame);
        widgets::emoji_popup::render(self, vertical[0], vertical[1], frame);
        widgets::status_bar::render(self, vertical[2], frame);
    }

    /// Records how many message rows the conversation panel has room for.
    ///
    /// Called from the panel, which is the only place the terminal's height has
    /// been turned into a rectangle. Zero is not a measurement anything can act
    /// on, so it is stored as one row: a page that moves nowhere is worse than a
    /// page that moves too little.
    ///
    /// This is the panel's height, and it is rows rather than messages: a
    /// message is as tall as its text is, and how tall that is depends on the
    /// width the panel gave it.
    pub fn record_rows(&self, rows: usize) {
        self.rows.set(rows.max(1));
    }

    /// The columns the conversation panel's messages have room for, as of the
    /// last frame.
    ///
    /// What the rows are laid out at. Recorded by the panel after the scrollbar
    /// has taken its column, because a message must never be laid out — or
    /// drawn — under the bar.
    #[must_use]
    pub fn body_width(&self) -> u16 {
        self.body_width.get()
    }

    /// Records how many columns the conversation panel's messages have room for.
    pub fn record_body(&self, width: u16) {
        self.body_width.set(width);
    }

    /// The unix second the reader's clock last read.
    ///
    /// Zero until the host records one, which is what makes a day label say a
    /// date rather than `Today`: this crate owns no clock, so a relative label
    /// would otherwise be a claim it cannot support.
    #[must_use]
    pub fn now(&self) -> i64 {
        self.now.get()
    }

    /// Records what the reader's clock says, in unix seconds.
    ///
    /// Called by the host once a frame, alongside the other measurements it
    /// records: what a day is called depends on when it is being read, and
    /// nothing here can know that.
    pub fn record_now(&self, now: i64) {
        self.now.set(now);
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
    /// A pure function of the window's messages, [`App::body_width`] and
    /// [`App::now`], and of nothing else: not the cursor, not the mode, not when
    /// it was asked. A layout worked out before a page lands is thrown away
    /// rather than kept, which is why a [`RowSpan`] is named by message id.
    ///
    /// The entries are the window's messages and the day separators in front of
    /// them: a separator takes a row of the screen between two days and is
    /// counted by the scrollbar beside it, so it belongs in here rather than
    /// counted on the side. [`RowKind`] is what tells the two apart, and the
    /// cursor — which is a message index — never rests on one.
    #[must_use]
    pub fn row_layout(&self) -> Vec<RowSpan> {
        let width = self.body_width();
        let now = self.now();
        let mut laid_out: Vec<RowSpan> = Vec::with_capacity(self.conversation.window.len());
        let mut first = 0;
        // The day of the last message that had one. A send still on its way has
        // no day of its own, so it neither opens a day nor closes the search for
        // the next message that does (T3).
        let mut day: Option<i64> = None;

        for (index, message) in self.conversation.window.iter().enumerate() {
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
            let len = rows::message_rows(self, message, rows::group_of(self, index), width).len();

            laid_out.push(RowSpan {
                kind: RowKind::Message { index },
                message_id: Some(message.id),
                first,
                len,
                text,
            });
            first += len;
        }

        laid_out
    }

    /// The rows the panel spends on the fetches it is announcing.
    ///
    /// One answer, read by the panel for both what it draws and what the
    /// messages have left, because the two cannot be allowed to disagree about
    /// how tall an announcement is.
    #[must_use]
    pub fn reserved(&self) -> Reserved {
        Reserved {
            older: self.fetching.is_in_flight(FetchDirection::Older),
            jumping: self.pending_jump.is_some(),
            newer: self.fetching.is_in_flight(FetchDirection::Newer),
        }
    }

    /// What a jump in flight is called, on the panel and on the status line.
    ///
    /// `JUMP_LABEL` names a destination — the first unread message — and a jump
    /// to a reply is not going there, so each kind has its own sentence and both
    /// places that say one read this.
    #[must_use]
    pub fn jump_label(&self) -> &'static str {
        self.pending_jump
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
            self.vim.cursor(),
            budget,
            self.conversation.auto_follow(),
        )
    }

    // ---- helpers -------------------------------------------------------

    #[must_use]
    pub fn current_chat_id(&self) -> i64 {
        self.list.chats.get(self.selected_chat).map_or(0, |c| c.id)
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
        match self.line.purpose() {
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
            PromptKind::Search => "/",
        }
    }

    /// What the status line shows.
    ///
    /// A confirmation outranks everything: it is a question waiting for an
    /// answer, and it is over as soon as one is given. A selection comes next —
    /// also state the reader must not lose, and the one thing on screen whose
    /// extent is not otherwise visible. A search's label is below it, and outranks
    /// a transient status, because it describes state the reader must not lose: it
    /// is not a `flash`, so `expire_status` must not be able to take it away.
    /// Below both, a jump in flight — what the reader has just asked for — and
    /// then the full reason a failed message failed while the cursor is on it, and
    /// finally whatever was written to the status.
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
            .signin
            .as_ref()
            .and_then(SignIn::flow)
            .and_then(|flow| flow.login.refusal.as_ref())
        {
            return refusal.clone();
        }
        if self.focus == Focus::Input {
            return widgets::input_bar::hint(self).to_owned();
        }
        match &self.confirm {
            Some(ConfirmKind::Quit) => return QUIT_PROMPT.to_owned(),
            Some(ConfirmKind::Logout) => return LOGOUT_PROMPT.to_owned(),
            Some(ConfirmKind::DeleteMessages {
                ids,
                outgoing,
                skipped,
            }) => return delete_prompt(ids, *outgoing, *skipped),
            None => {}
        }
        if let Some(selection) = &self.selection {
            return selection_note(selection, self.selection_len().unwrap_or(0));
        }
        if self.search.is_active() {
            return self.search.label();
        }
        if self.pending_jump.is_some() {
            return self.jump_label().to_owned();
        }
        if let Some(message) = self.cursor_message()
            && let Some(reason) = self.conversation.failure(message.id)
        {
            return reason.to_owned();
        }
        // A status worth reading — a refusal, a failure — outranks the hint. The
        // resting state is the hint rather than the program's name, because the
        // name says nothing and a bar showing it looks like a bar with nothing
        // in it, which is exactly what a half-written message used to look like.
        if self.status != IDLE_STATUS {
            return self.status.clone();
        }

        widgets::input_bar::hint(self).to_owned()
    }
}

// ---- helpers -----------------------------------------------------------

/// Where the unread messages start in a window that ends where the conversation
/// does.
///
/// Counted back from the end rather than looked up by identifier, which is what
/// makes the answer exact where the numbering has gaps: the unread messages are
/// the newest ones there are, so they are the last `unread` positions of the
/// window.
///
/// `None` when there is nothing unread, and when the unread messages reach past
/// the window — they start somewhere the client has not loaded, and counting
/// them from the end would land on a message that is not one of them.
fn landing_position(len: usize, unread: u32) -> Option<usize> {
    if unread == 0 {
        return None;
    }

    // A count that does not fit an index is far larger than any window, which
    // the comparison below settles without the conversion mattering.
    let unread = usize::try_from(unread).unwrap_or(usize::MAX);

    (unread <= len).then(|| len - unread)
}

/// Whether the window holds the whole conversation.
///
/// The broad reading — "both ends exhausted means the window holds the whole
/// conversation" — is false, because the window keeps its **newest**
/// [`CONVERSATION_WINDOW`] messages and drops the rest from the front: a
/// conversation whose ends have both been reached still holds no more than the
/// cap. Only both-ended **and** shorter than the cap means nothing was ever
/// dropped, which is precisely the conversation a full scan is cheapest in and a
/// round trip would only delay.
///
/// A conversation exactly at the cap is excluded conservatively: that is a
/// missed optimisation, not a wrong answer.
fn holds_everything(window: &ConversationWindow) -> bool {
    window.exhausted_older && window.exhausted_newer && window.len() < CONVERSATION_WINDOW
}

/// Whether a walk wrapped from one end of the match list to the other.
///
/// A single match is its own neighbour, so its "wrap" carries no information and
/// is not announced.
fn wrapped(before: Option<usize>, after: Option<usize>, len: usize) -> bool {
    if len <= 1 {
        return false;
    }

    (before == Some(len - 1) && after == Some(0)) || (before == Some(0) && after == Some(len - 1))
}

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
        },
        Chat {
            id: 2,
            title: "Grace Hopper".into(),
            kind: ChatKind::Private,
            last_message: Some("The compiler is ready.".into()),
            unread_count: 0,
            last_message_id: None,
            last_timestamp: Some(1_729_999_000),
        },
        Chat {
            id: 3,
            title: "Alan Turing".into(),
            kind: ChatKind::Private,
            last_message: Some("Halting problem again…".into()),
            unread_count: 1,
            last_message_id: None,
            last_timestamp: Some(1_729_998_000),
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
mod tests {
    use super::*;

    /// A message in the sample conversation.
    fn message(id: i64, text: &'static str) -> Message {
        Message {
            id,
            chat_id: MOCK_CHAT,
            text: Cow::Borrowed(text),
            timestamp: 1_730_000_000 + id,
            status: MessageStatus::Received,
            is_outgoing: false,
            reply_to: None,
            media: None,
        }
    }

    /// Messages of the sample conversation, with these identifiers.
    fn page(ids: &[i64]) -> Vec<Message> {
        ids.iter().map(|id| message(*id, "text")).collect()
    }

    /// The same, however the identifiers are spelled.
    fn numbered(ids: impl IntoIterator<Item = i64>) -> Vec<Message> {
        ids.into_iter().map(|id| message(id, "text")).collect()
    }

    /// `to` messages of a screenful each, which is a window of rows rather
    /// than of lines.
    fn tall_page(to: i64) -> Vec<Message> {
        (0..to)
            .map(|id| Message {
                text: Cow::Owned("x".repeat(400)),
                ..message(id, "text")
            })
            .collect()
    }

    /// A message in a conversation the sample data does not hold.
    fn stranger(id: i64) -> Message {
        Message {
            chat_id: MOCK_CHAT + 1,
            ..message(id, "stranger")
        }
    }

    /// A message in a conversation the client holds nowhere at all.
    fn unknown(id: i64) -> Message {
        Message {
            chat_id: 999,
            ..message(id, "unknown")
        }
    }

    /// How many unread messages the list holds for a conversation.
    fn unread(app: &App, chat_id: i64) -> u32 {
        app.list
            .chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .expect("the chat is in the list")
            .unread_count
    }

    /// The identifier of the message the cursor is on.
    fn reading(app: &App) -> Option<i64> {
        app.conversation
            .window
            .get(app.vim.cursor())
            .map(|message| message.id)
    }

    /// The text the open conversation holds for a message.
    fn text_of(app: &App, id: i64) -> Option<&str> {
        app.conversation
            .window
            .iter()
            .find(|message| message.id == id)
            .map(|message| message.text.as_ref())
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn press_ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn type_text(app: &mut App, text: &str) {
        for ch in text.chars() {
            app.handle_key(press(KeyCode::Char(ch)));
        }
    }

    /// The cursor to the top of what is loaded, which is where `gg` leaves it.
    fn go_to_top(app: &mut App) {
        app.handle_key(press(KeyCode::Char('g')));
        app.handle_key(press(KeyCode::Char('g')));
    }

    /// The sample conversation, with `unread` messages waiting in it and its
    /// newest message numbered `last`.
    ///
    /// The sample conversation has nothing unread — it is the one the reader is
    /// in — so the tests that are about where the unread messages start say how
    /// many there are, and how the conversation is numbered.
    fn with_unread(unread: u32, last: i64) -> App {
        let mut app = App::mock();
        let chat = app
            .list
            .chats
            .iter_mut()
            .find(|chat| chat.id == MOCK_CHAT)
            .expect("the sample conversation is in the list");
        chat.unread_count = unread;
        chat.last_message_id = Some(last);

        app
    }

    /// The sample conversation with its unread messages in front of what is
    /// loaded: the conversation runs to 20, and the window stops at 8.
    fn with_unread_out_of_reach(unread: u32) -> App {
        let mut app = with_unread(unread, 20);
        app.apply_latest(page(&[1, 2, 3, 4, 5, 6, 7, 8]));
        app
    }

    // ---- the frame -----------------------------------------------------

    #[test]
    fn a_new_application_holds_nothing_it_has_not_fetched() {
        let app = App::new();

        assert!(app.chats().is_empty());
        assert!(app.conversation.window.is_empty());
        assert_eq!(app.current_chat_id(), 0);
        assert!(!app.has_conversation());
        assert!(!app.wants_older(), "there is nothing to page through yet");
        assert!(!app.wants_newer());
    }

    #[test]
    fn opening_a_chat_replaces_the_conversation_on_show() {
        let mut app = App::mock();
        app.select_chat(1);

        assert_eq!(app.selected_chat, 1);
        assert_eq!(app.current_chat_id(), 2);
        assert_eq!(
            app.conversation.window.chat_id, 2,
            "the window belongs to the chat that was opened"
        );
        assert!(
            app.conversation.window.is_empty(),
            "nothing has been fetched for it yet"
        );
        assert!(app.conversation.auto_follow());
        assert_eq!(app.vim.total(), 0);
    }

    /// The fetched list replaces whatever was there, and the reader's place
    /// comes back inside it rather than pointing past the end of a shorter one.
    #[test]
    fn a_fetched_list_replaces_the_one_before_it() {
        let mut app = App::mock();
        app.select_chat(2);

        app.set_chats(mock_chats().into_iter().take(2).collect());

        assert_eq!(app.chats().len(), 2);
        assert_eq!(app.selected_chat, 1, "clamped into the shorter list");
        assert_eq!(app.current_chat_id(), 2);
    }

    /// A fetch that returns nobody leaves no conversation to be in: the window
    /// belongs to a chat the list no longer holds.
    #[test]
    fn an_empty_fetch_closes_the_conversation() {
        let mut app = App::mock();

        app.set_chats(Vec::new());

        assert!(app.chats().is_empty());
        assert_eq!(app.current_chat_id(), 0);
        assert!(app.conversation.window.is_empty());
    }

    /// A refresh installs a new list around the reader's place, rather than
    /// through the reset a chat switch runs: the conversation, the selection and
    /// the draft stay, and the highlight follows the open conversation's id into
    /// a list whose order has changed.
    #[test]
    fn a_refresh_keeps_the_conversation_selection_and_draft_purpose() {
        let mut app = App::mock();
        app.start_reply();
        let reply_to = app.reply_to.expect("the sample cursor is on a message");
        spanning(&mut app, 3, 5);

        // The same conversations, reversed: an index would point at a different
        // one, so only restoring by id can keep the highlight where it was.
        let reversed: Vec<_> = app.chats().iter().rev().cloned().collect();
        let restored = app.refresh_chats(reversed);

        assert!(
            restored,
            "the open conversation is still in the fetched list"
        );
        assert_eq!(
            app.current_chat_id(),
            MOCK_CHAT,
            "the conversation on show is untouched"
        );
        assert!(!app.conversation.window.is_empty(), "and so is its window");
        assert_eq!(
            app.selected_chat, 2,
            "the highlight followed the id to the end of the reversed list"
        );
        assert_eq!(
            app.line.purpose(),
            PromptKind::Reply,
            "the draft is still a reply, not reset to a plain message"
        );
        assert_eq!(app.reply_to, Some(reply_to));
        assert_eq!(
            app.selection()
                .map(|selection| (selection.anchor.message_id, selection.focus.message_id)),
            Some((3, 5)),
            "the selection survives the list being replaced"
        );
    }

    /// Regression: every keystroke must be applied exactly once. Previously
    /// the reader thread in `runtime.rs` dropped every other event, so typing
    /// `s` then `q` produced only `q`.
    #[test]
    fn entering_insert_mode_then_typing_records_every_key() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('i')));
        assert_eq!(app.focus, Focus::Input);

        type_text(&mut app, "hello");
        assert_eq!(app.line.text(), "hello");
    }

    #[test]
    fn escape_returns_to_the_line_and_then_to_the_conversation() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('i')));
        type_text(&mut app, "hi");

        app.handle_key(press(KeyCode::Esc));

        assert_eq!(
            app.focus,
            Focus::Input,
            "one escape stops typing, and the reader is still in the line"
        );
        assert_eq!(app.line.text(), "hi", "and the text is not thrown away");
    }

    #[test]
    fn backspace_removes_exactly_one_char_per_press() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('i')));
        type_text(&mut app, "abc");
        app.handle_key(press(KeyCode::Backspace));

        assert_eq!(app.line.text(), "ab");
    }

    /// The headline behaviour, in the reader's words. Every Vim user presses
    /// `Esc` to stop typing and look at the conversation, and losing four lines
    /// to it with no warning was the most complaint-worthy thing this program
    /// did.
    #[test]
    fn a_typed_message_survives_two_escapes() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('i')));
        type_text(&mut app, "half a thought\nand the rest of it");

        app.handle_key(press(KeyCode::Esc));
        assert_eq!(
            app.focus,
            Focus::Input,
            "the first escape stops typing: the reader is still in the line"
        );

        app.handle_key(press(KeyCode::Esc));
        assert_eq!(app.focus, Focus::Conversation, "and the second looks away");
        assert_eq!(
            app.line.text(),
            "half a thought\nand the rest of it",
            "with every word of it"
        );
    }

    #[test]
    fn a_draft_survives_a_conversation_switch() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('i')));
        type_text(&mut app, "half a th");
        app.handle_key(press(KeyCode::Esc));
        app.handle_key(press(KeyCode::Esc));

        app.select_chat(1);

        assert_eq!(
            app.line.text(),
            "half a th",
            "a reader who switches chats mid-sentence does not lose the sentence"
        );
    }

    /// The draft's *subject* does not survive, though, because it names something
    /// in the conversation that has been closed. The words stay; what they were
    /// written against does not.
    #[test]
    fn a_reply_that_outlives_its_conversation_becomes_a_message() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('r')));
        let replied_to = app
            .reply_to
            .expect("a reply answers the message on the cursor");
        assert_eq!(app.line.purpose(), PromptKind::Reply);
        type_text(&mut app, "sure");

        app.select_chat(1);

        assert_eq!(app.line.text(), "sure", "the words");
        assert_eq!(
            app.line.purpose(),
            PromptKind::Message,
            "but not the subject"
        );
        assert_eq!(
            app.reply_to, None,
            "and nothing to reply to any more: {replied_to} was in the other chat"
        );
    }

    /// The rule for what a draft is: the bar is always a draft, so switching away
    /// from it and coming back finds the text rather than a blank field.
    #[test]
    fn a_draft_is_still_there_after_a_submit_that_sent_it() {
        let mut app = App::mock();
        submit(&mut app, "ping");

        assert!(
            app.line.is_empty(),
            "a sent message leaves nothing behind, or `Enter` would send it twice"
        );

        app.handle_key(press(KeyCode::Char('i')));
        type_text(&mut app, "next");

        assert_eq!(app.line.text(), "next", "and the next one starts clean");
    }

    /// Types `text` and submits it, leaving a placeholder in flight.
    fn submit(app: &mut App, text: &str) {
        app.handle_key(press(KeyCode::Char('i')));
        type_text(app, text);
        app.handle_key(press(KeyCode::Enter));
    }

    #[test]
    fn enter_shows_the_typed_message_while_it_is_on_its_way() {
        let mut app = App::mock();
        let before = app.conversation.window.len();

        submit(&mut app, "ping");

        assert_eq!(app.conversation.window.len(), before + 1);
        let id = app.sending.expect("the send is in flight");
        assert_eq!(id, -1, "the first placeholder is minus one");
        assert_eq!(text_of(&app, id), Some("ping"));
        assert_eq!(
            reading(&app),
            Some(id),
            "a message just typed is the one on screen"
        );
        assert_eq!(app.mode, Mode::Normal);

        assert_eq!(
            app.take_action(),
            Some(Action::Send {
                chat_id: MOCK_CHAT,
                temp_id: id,
                text: "ping".to_owned(),
                reply_to: None,
            })
        );
        assert_eq!(app.take_action(), None, "an action is taken once");
    }

    #[test]
    fn typing_with_no_conversation_open_composes_nothing() {
        let mut app = App::new();
        app.handle_key(press(KeyCode::Char('i')));
        type_text(&mut app, "ping");
        app.handle_key(press(KeyCode::Enter));

        assert!(app.conversation.window.is_empty());
    }

    #[test]
    fn a_second_send_is_refused_while_one_is_on_its_way() {
        let mut app = App::mock();
        submit(&mut app, "first");
        let before = app.conversation.window.len();

        submit(&mut app, "second");

        assert_eq!(
            app.conversation.window.len(),
            before,
            "the second message is not shown, because it was not queued"
        );
        assert!(
            app.status.contains("already on its way"),
            "a refusal has to say so: {:?}",
            app.status
        );
    }

    /// Two requests made between two ticks are two requests. A single slot
    /// would let the second replace the first, and the first would never be
    /// sent — the bug this queue exists to prevent.
    #[test]
    fn two_actions_queued_together_are_taken_in_order() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('k')));
        assert_eq!(reading(&app), Some(9), "an outgoing message");

        // First edit.
        app.handle_key(press(KeyCode::Char('e')));
        type_text(&mut app, " one");
        app.handle_key(press(KeyCode::Enter));
        // Second edit, before the caller has taken the first.
        app.handle_key(press(KeyCode::Char('e')));
        type_text(&mut app, " two");
        app.handle_key(press(KeyCode::Enter));

        let first = app.take_action().expect("the first edit is queued");
        let second = app.take_action().expect("the second edit is queued");

        assert!(
            first != second,
            "the two edits must be distinct operations, not one twice"
        );
        assert_eq!(app.take_action(), None, "and the queue is drained");
    }

    // ---- focus and the panes --------------------------------------------

    /// The sample data, with the focus on the chat list.
    fn on_the_chat_list() -> App {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('h')));
        app
    }

    #[test]
    fn a_new_application_has_the_conversation_focused() {
        assert_eq!(App::new().focus, Focus::Conversation);
    }

    #[test]
    fn h_leaves_the_conversation_for_the_chat_list_and_l_comes_back() {
        let mut app = App::mock();
        assert_eq!(app.focus, Focus::Conversation);

        app.handle_key(press(KeyCode::Char('h')));
        assert_eq!(app.focus, Focus::ChatList);

        // The conversation's own motions are not the list's: `k` up there moved
        // the cursor, and here it moves the highlight.
        app.handle_key(press(KeyCode::Char('k')));
        assert_eq!(app.selected_chat, 0);
        assert_eq!(reading(&app), Some(10), "the cursor did not move");

        app.handle_key(press(KeyCode::Char('l')));
        assert_eq!(app.focus, Focus::Conversation);
    }

    #[test]
    fn tab_walks_the_panes_in_the_order_they_are_drawn_and_wraps() {
        let mut app = App::mock();

        let mut seen = vec![app.focus];
        for _ in 0..3 {
            app.handle_key(press(KeyCode::Tab));
            seen.push(app.focus);
        }

        assert_eq!(
            seen,
            vec![
                Focus::Conversation,
                Focus::Input,
                Focus::ChatList,
                Focus::Conversation
            ],
            "one full turn of Tab is back where it started"
        );
    }

    #[test]
    fn backtab_walks_the_other_way() {
        let mut app = App::mock();

        app.handle_key(press(KeyCode::BackTab));
        assert_eq!(app.focus, Focus::ChatList);

        app.handle_key(press(KeyCode::BackTab));
        assert_eq!(app.focus, Focus::Input);
    }

    /// `Esc` abandons the line and `Ctrl+w` only looks away from it, because a
    /// reader stepping between panes should not lose a half-written sentence.
    #[test]
    fn ctrl_w_leaves_the_line_keeping_what_was_typed() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('i')));
        type_text(&mut app, "half a th");

        app.handle_key(press_ctrl('w'));

        assert_eq!(app.focus, Focus::Conversation);
        assert_eq!(app.line.text(), "half a th", "the line is not thrown away");

        // And it is still there to come back to, rather than lost.
        app.handle_key(press(KeyCode::Tab));
        assert_eq!(app.line.text(), "half a th");
    }

    #[test]
    fn ctrl_w_outside_the_line_does_nothing() {
        let mut app = App::mock();

        app.handle_key(press_ctrl('w'));

        assert_eq!(app.focus, Focus::Conversation);
    }

    /// A selection belongs to the conversation, so a pane that is not the
    /// conversation cannot be entered over the top of one.
    #[test]
    fn leaving_the_conversation_drops_a_visual_selection() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('v')));
        assert_eq!(app.mode, Mode::Visual);

        app.handle_key(press(KeyCode::Tab));

        assert_eq!(app.focus, Focus::Input);
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn the_chat_list_highlight_moves_and_stops_at_both_ends() {
        let mut app = on_the_chat_list();

        app.handle_key(press(KeyCode::Char('j')));
        assert_eq!(app.selected_chat, 1);
        app.handle_key(press(KeyCode::Char('j')));
        assert_eq!(app.selected_chat, 2);
        app.handle_key(press(KeyCode::Char('j')));
        assert_eq!(app.selected_chat, 2, "and clamps at the end");

        app.handle_key(press(KeyCode::Char('k')));
        app.handle_key(press(KeyCode::Char('k')));
        app.handle_key(press(KeyCode::Char('k')));
        assert_eq!(app.selected_chat, 0, "and at the start");
    }

    /// A moment long enough after any keystroke this test could have pressed for
    /// the highlight to have settled.
    fn settled() -> Instant {
        Instant::now() + CHAT_SWITCH_DELAY
    }

    /// Moving the highlight asks for the conversation it names, but not before
    /// the reader has stopped: a held `j` would otherwise fetch every chat it
    /// scrolled past.
    #[test]
    fn a_moving_highlight_waits_for_the_reader_to_stop() {
        let mut app = on_the_chat_list();

        app.handle_key(press(KeyCode::Char('j')));
        assert!(
            app.take_pending_chat(Instant::now()).is_none(),
            "nothing is asked for while the key is still moving"
        );

        app.handle_key(press(KeyCode::Char('j')));
        assert_eq!(
            app.selected_chat, 2,
            "the highlight follows the key at once"
        );

        assert_eq!(
            app.take_pending_chat(settled()),
            Some(2),
            "and the conversation it landed on is asked for once it settles"
        );
        assert_eq!(
            app.take_pending_chat(settled()),
            None,
            "taken once, like every other hand-over"
        );
    }

    #[test]
    fn a_debounce_elapsed_but_the_highlight_moving_again_defers_the_open() {
        let mut app = on_the_chat_list();

        app.handle_key(press(KeyCode::Char('j')));
        let settled = settled();
        assert_eq!(app.take_pending_chat(settled), Some(1));

        app.handle_key(press(KeyCode::Char('j')));
        assert_eq!(
            app.take_pending_chat(settled),
            None,
            "the second press restarts the wait rather than slipping through it"
        );
    }

    #[test]
    fn gg_and_g_reach_both_ends_of_the_chat_list() {
        let mut app = on_the_chat_list();

        app.handle_key(press(KeyCode::Char('G')));
        assert_eq!(app.selected_chat, 2);
        assert_eq!(app.take_pending_chat(settled()), Some(2));

        app.handle_key(press(KeyCode::Char('g')));
        app.handle_key(press(KeyCode::Char('g')));
        assert_eq!(app.selected_chat, 0);
        assert_eq!(app.take_pending_chat(settled()), Some(0));
    }

    /// A lone `g` is a key with no meaning of its own, so it must not still be
    /// waiting to be the first half of a `gg` several keys later.
    #[test]
    fn a_lone_g_does_not_wait_to_be_the_first_half_of_gg() {
        let mut app = on_the_chat_list();

        app.handle_key(press(KeyCode::Char('G')));
        app.handle_key(press(KeyCode::Char('g')));
        app.handle_key(press(KeyCode::Char('x')));
        app.handle_key(press(KeyCode::Char('g')));
        assert_eq!(
            app.selected_chat, 2,
            "the `g` after the `x` starts a sequence rather than finishing one"
        );

        app.handle_key(press(KeyCode::Char('g')));
        assert_eq!(app.selected_chat, 0);
    }

    /// `Enter` is a reader saying "this one", not a movement, so it opens at once
    /// and takes the focus to the messages.
    #[test]
    fn enter_in_the_chat_list_opens_it_and_moves_to_the_conversation() {
        let mut app = on_the_chat_list();
        app.handle_key(press(KeyCode::Char('j')));

        app.handle_key(press(KeyCode::Enter));

        assert_eq!(app.focus, Focus::Conversation);
        assert_eq!(app.conversation.window.chat_id, 2);
        assert_eq!(app.selected_chat, 1);
        assert_eq!(
            app.take_pending_chat(settled()),
            None,
            "and there is nothing left to open afterwards"
        );
    }

    /// A movement in one pane must not answer for the other: the sample
    /// conversation's `k` walks messages, and the list's walks conversations.
    #[test]
    fn a_pane_only_answers_for_itself() {
        let mut app = on_the_chat_list();

        app.handle_key(press(KeyCode::Char('g')));
        assert_eq!(
            app.selected_chat, 0,
            "`gg` in the list, not the top of a window"
        );
        assert_eq!(app.conversation.window.chat_id, MOCK_CHAT);
    }

    // ---- a selection the window can move under -------------------------

    /// A selection spanning two whole messages, made directly.
    ///
    /// The keys that make one are bound in a later step, and the question this
    /// section is about is what survives the window moving rather than how a
    /// selection was made — so it is built here rather than pressed.
    fn spanning(app: &mut App, anchor: i64, focus: i64) {
        app.selection = Some(Selection {
            anchor: Mark::whole(anchor),
            focus: Mark::whole(focus),
        });
    }

    /// The invariant a selection is most likely to break: a page landing under a
    /// live one shifts every index in the window, so restoring the cursor alone
    /// would leave the selection covering different messages — and the next `d`
    /// would delete something the reader did not select.
    #[test]
    fn a_page_landing_under_a_selection_keeps_both_of_its_ends() {
        let mut app = App::mock();
        app.apply_latest(numbered(10..=20));
        spanning(&mut app, 11, 15);

        assert!(app.apply_older(numbered(1..=9)));

        let selection = app.selection().expect("both messages are still loaded");
        assert_eq!(
            (selection.anchor.message_id, selection.focus.message_id),
            (11, 15),
            "the ends are identifiers, so the nine that went in front of them cannot move them"
        );
        assert_eq!(
            app.covered(app.selection()),
            10..15,
            "and it still covers exactly what it did"
        );
    }

    /// The same, from the other end: an arrival pushes the oldest messages out of
    /// a window that is at its cap.
    #[test]
    fn an_arrival_that_pushes_an_end_out_of_the_window_drops_the_selection() {
        let cap = i64::try_from(CONVERSATION_WINDOW).expect("the cap fits an identifier");
        let mut app = App::mock();
        app.apply_latest(numbered(1..=cap));
        spanning(&mut app, 1, 3);

        assert!(app.apply_newer(numbered(cap + 1..=cap + 2)));
        assert!(
            app.conversation.window.position_of(1).is_none(),
            "the oldest message has been pushed out of a window at its cap"
        );

        assert_eq!(
            app.selection(),
            None,
            "half a selection is worse than none: `d` on it would be a one-message \
             deletion the reader did not ask for"
        );
    }

    #[test]
    fn a_window_that_is_replaced_takes_the_selection_with_it() {
        let mut app = App::mock();
        spanning(&mut app, 2, 4);

        app.apply_latest(numbered(1..=10));

        assert_eq!(
            app.selection(),
            None,
            "a page that replaces the window has moved every message in it"
        );
    }

    #[test]
    fn a_jump_replaces_the_window_and_the_selection_with_it() {
        let mut app = with_unread(2, 20);
        go_to_top(&mut app);
        let jump = app.pending_jump().expect("the target is not loaded");
        spanning(&mut app, 1, 2);

        assert!(app.apply_jump(&numbered(1..=8), jump.target_id));

        assert_eq!(app.selection(), None);
    }

    #[test]
    fn opening_another_conversation_takes_the_selection_with_it() {
        let mut app = App::mock();
        spanning(&mut app, 2, 4);

        app.select_chat(1);

        assert_eq!(app.selection(), None);
    }

    /// A mark on a message the window does not hold is a mark nothing can draw
    /// and `d` cannot act on, so it is refused rather than recorded.
    #[test]
    fn a_selection_can_only_be_started_on_a_message_that_is_loaded() {
        let mut app = App::mock();

        assert!(app.select(4, Some(0)));
        assert_eq!(
            app.selection().and_then(Selection::text_range),
            Some((4, 0..0)),
            "and it starts collapsed, which is what `v` leaves behind"
        );

        assert!(!app.select(999, Some(0)));
        assert_eq!(
            app.selection().and_then(Selection::text_range),
            Some((4, 0..0)),
            "and a refused mark leaves the selection that was there"
        );
    }

    // ---- the motions ----------------------------------------------------

    /// The characters of the message under the cursor, for a motion's answer.
    fn selected_chars(app: &App) -> Option<(i64, Range<usize>)> {
        app.selection().and_then(Selection::text_range)
    }

    /// The messages the selection covers, oldest first.
    fn selected_messages(app: &App) -> Vec<i64> {
        let covered = app.covered(app.selection());

        app.conversation
            .window
            .iter()
            .skip(covered.start)
            .take(covered.len())
            .map(|message| message.id)
            .collect()
    }

    /// The focus, as a message and a character position.
    fn focused(app: &App) -> Option<(i64, Option<usize>)> {
        app.selection()
            .map(|selection| (selection.focus.message_id, selection.focus.char))
    }

    fn key(app: &mut App, c: char) {
        app.handle_key(press(KeyCode::Char(c)));
    }

    #[test]
    fn v_starts_a_charwise_selection_at_the_first_character() {
        let mut app = App::mock();
        go_to_top(&mut app);

        key(&mut app, 'v');

        assert_eq!(app.mode, Mode::Visual);
        assert_eq!(
            selected_chars(&app),
            Some((1, 0..0)),
            "a position, which is a span of no characters yet"
        );
    }

    #[test]
    fn v_then_l_selects_one_character() {
        let mut app = App::mock();
        go_to_top(&mut app);

        key(&mut app, 'v');
        key(&mut app, 'l');

        assert_eq!(selected_chars(&app), Some((1, 0..1)));
    }

    #[test]
    fn v_then_j_selects_two_messages() {
        let mut app = App::mock();
        go_to_top(&mut app);

        key(&mut app, 'v');
        key(&mut app, 'j');

        assert_eq!(
            selected_messages(&app),
            vec![1, 2],
            "two ends in two messages are a set of messages"
        );
        assert_eq!(selected_chars(&app), None);
        assert_eq!(reading(&app), Some(2), "and the cursor followed the focus");
    }

    /// The first sample message, whose words are `Hey,` `is` `the` `build`
    /// `green?`.
    const SAMPLE: &str = "Hey, is the build green?";

    /// A selection dropped and Normal restored, the way a reader leaves one.
    fn escape(app: &mut App) {
        app.handle_key(press(KeyCode::Esc));
    }

    #[test]
    fn v_then_selecting_upward_covers_the_same_messages() {
        let mut app = App::mock();
        go_to_top(&mut app);
        key(&mut app, 'j');
        key(&mut app, 'j');

        key(&mut app, 'V');
        assert_eq!(selected_messages(&app), vec![3]);
        key(&mut app, 'k');
        key(&mut app, 'k');

        assert_eq!(
            selected_messages(&app),
            vec![1, 2, 3],
            "a selection dragged upwards covers the same messages as one dragged down"
        );
    }

    #[test]
    fn capital_v_selects_a_whole_message() {
        let mut app = App::mock();
        go_to_top(&mut app);

        key(&mut app, 'V');

        assert_eq!(selected_messages(&app), vec![1]);
        assert_eq!(
            focused(&app),
            Some((1, None)),
            "and there is no place inside it"
        );
    }

    /// A linewise selection has nowhere inside it to move, so the character
    /// motions do nothing — which is what Vim does, and what the status line's
    /// count already tells the reader.
    #[test]
    fn a_character_motion_over_a_whole_message_does_nothing() {
        let mut app = App::mock();
        go_to_top(&mut app);
        key(&mut app, 'V');

        for motion in ['h', 'l', 'w', 'b', 'e', '0', '$'] {
            key(&mut app, motion);
            assert_eq!(focused(&app), Some((1, None)), "after {motion}");
        }

        assert_eq!(
            selected_messages(&app),
            vec![1],
            "and the selection is intact"
        );
    }

    #[test]
    fn o_swaps_the_ends_without_changing_the_selection() {
        let mut app = App::mock();
        go_to_top(&mut app);
        key(&mut app, 'v');
        for _ in 0..3 {
            key(&mut app, 'l');
        }
        let before = selected_chars(&app);
        let anchor = app.selection().expect("held").anchor;
        let focus = focused(&app).expect("held");

        key(&mut app, 'o');

        assert_eq!(selected_chars(&app), before, "only the direction changed");
        assert_eq!(focused(&app), Some((anchor.message_id, anchor.char)));
        assert_eq!(app.selection().expect("held").focus.message_id, focus.0);

        key(&mut app, 'o');
        assert_eq!(focused(&app), Some(focus), "and twice is the original");
    }

    #[test]
    fn escape_drops_the_selection_and_returns_to_normal() {
        let mut app = App::mock();
        go_to_top(&mut app);
        key(&mut app, 'v');
        key(&mut app, 'j');

        escape(&mut app);

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.selection(), None);
    }

    #[test]
    fn a_selection_does_not_outlive_the_status_that_announced_it() {
        let mut app = App::mock();
        go_to_top(&mut app);
        key(&mut app, 'v');
        let selected = app.status_text();
        assert!(
            selected.contains("selected"),
            "a selection says so: {selected:?}"
        );

        escape(&mut app);

        assert!(
            !app.status_text().contains("selected"),
            "so the status line does not keep describing a selection that is gone: {:?}",
            app.status_text()
        );
    }

    #[test]
    fn the_word_motions_walk_the_words_of_a_message() {
        let mut app = App::mock();
        go_to_top(&mut app);
        key(&mut app, 'v');

        key(&mut app, 'w');
        assert_eq!(focused(&app), Some((1, Some(5))), "over `is`");
        key(&mut app, 'w');
        assert_eq!(focused(&app), Some((1, Some(8))), "and over `the`");
        key(&mut app, 'e');
        assert_eq!(
            focused(&app),
            Some((1, Some(10))),
            "and `e` to the end of it"
        );
        key(&mut app, 'b');
        assert_eq!(
            focused(&app),
            Some((1, Some(8))),
            "and `b` back to its start"
        );
    }

    #[test]
    fn zero_and_the_end_are_the_ends_of_the_message() {
        let mut app = App::mock();
        go_to_top(&mut app);
        key(&mut app, 'v');

        key(&mut app, '$');
        let last = SAMPLE.chars().count() - 1;
        assert_eq!(focused(&app), Some((1, Some(last))));

        key(&mut app, '0');
        assert_eq!(focused(&app), Some((1, Some(0))));
    }

    #[test]
    fn f_and_t_find_a_character_in_the_message() {
        let mut app = App::mock();
        go_to_top(&mut app);
        key(&mut app, 'v');

        key(&mut app, 'f');
        key(&mut app, 'i');
        assert_eq!(
            focused(&app),
            Some((1, Some(5))),
            "`fi` lands on the `i` of `is`"
        );

        key(&mut app, 'f');
        key(&mut app, 'i');
        assert_eq!(focused(&app), Some((1, Some(14))), "and the next one");

        key(&mut app, 'F');
        key(&mut app, 'i');
        assert_eq!(focused(&app), Some((1, Some(5))), "`Fi` goes back");
    }

    /// The key after `f` is the character to look for, whatever it is — that is
    /// what `fw` means. A `w` there is a letter to find, not a motion, and this
    /// message has no `w` in it.
    #[test]
    fn the_key_after_f_is_the_character_and_not_another_motion() {
        let mut app = App::mock();
        go_to_top(&mut app);
        key(&mut app, 'v');

        key(&mut app, 'f');
        key(&mut app, 'w');

        assert_eq!(
            focused(&app),
            Some((1, Some(0))),
            "nothing was found, so nothing moved — and `w` was not a motion"
        );
    }

    /// A `f` whose character has not been typed must not still be waiting when an
    /// unrelated key arrives, or that key would be read as the character.
    #[test]
    fn a_find_waiting_for_its_character_is_forgotten_by_another_key() {
        let mut app = App::mock();
        go_to_top(&mut app);
        key(&mut app, 'v');

        key(&mut app, 'f');
        app.handle_key(press(KeyCode::Enter));
        key(&mut app, 'l');

        assert_eq!(
            focused(&app),
            Some((1, Some(1))),
            "the `l` was a motion, so the `f` was not half of one"
        );
    }

    /// A message's own text is the boundary, whatever the motion: crossing into
    /// the next message is `j`'s job, and a motion that did it silently would
    /// change what the reader thinks they selected.
    #[test]
    fn a_character_motion_never_leaves_the_message() {
        let mut app = App::mock();
        go_to_top(&mut app);
        key(&mut app, 'v');
        let last = SAMPLE.chars().count() - 1;

        for motion in ['w', 'e', '$', 'b'] {
            for _ in 0..40 {
                key(&mut app, motion);
            }

            let (id, at) = focused(&app).expect("still a selection");
            assert_eq!(id, 1, "{motion} stayed on the message it started on");
            assert!(at.is_some_and(|at| at <= last), "{motion} landed on {at:?}");
        }

        // Where each of them does end up, so the test above is not satisfied by a
        // motion that simply does nothing.
        for (motion, bound) in [('b', 0), ('e', last), ('$', last), ('w', 18)] {
            let mut app = App::mock();
            go_to_top(&mut app);
            key(&mut app, 'v');
            for _ in 0..40 {
                key(&mut app, motion);
            }
            assert_eq!(focused(&app), Some((1, Some(bound))), "{motion}");
        }
    }

    #[test]
    fn a_character_motion_moves_over_a_multibyte_character_rather_than_inside_it() {
        let mut app = App::mock();
        app.apply_latest(vec![Message {
            text: Cow::Borrowed("é😀x"),
            ..message(1, "unused")
        }]);
        go_to_top(&mut app);
        key(&mut app, 'v');

        key(&mut app, 'l');
        assert_eq!(
            focused(&app),
            Some((1, Some(1))),
            "over the two-byte character"
        );
        key(&mut app, 'l');
        assert_eq!(focused(&app), Some((1, Some(2))), "and the four-byte one");
        key(&mut app, 'l');
        assert_eq!(focused(&app), Some((1, Some(2))), "and stops at the last");
        key(&mut app, 'h');
        assert_eq!(focused(&app), Some((1, Some(1))));
    }

    /// The character position rides along to the next message, clamped, so that a
    /// selection which has been moved within a message keeps its relative place.
    #[test]
    fn a_character_position_carries_to_the_next_message_and_clamps() {
        let mut app = App::mock();
        app.apply_latest(vec![message(1, "a long first message"), message(2, "hi")]);
        go_to_top(&mut app);
        key(&mut app, 'v');
        for _ in 0..10 {
            key(&mut app, 'l');
        }
        assert_eq!(focused(&app), Some((1, Some(10))));

        key(&mut app, 'j');

        assert_eq!(
            focused(&app),
            Some((2, Some(1))),
            "clamped to the end of a two-character message"
        );
    }

    /// Leaving the pane drops a selection, because a selection for a conversation
    /// nobody is looking at would leave `d` holding something invisible.
    #[test]
    fn leaving_the_conversation_drops_the_selection() {
        let mut app = App::mock();
        go_to_top(&mut app);
        key(&mut app, 'v');
        key(&mut app, 'j');

        app.handle_key(press(KeyCode::Tab));

        assert_eq!(app.selection(), None);
        assert_eq!(app.mode, Mode::Normal);
    }

    // ---- yanking and pasting --------------------------------------------

    /// The sample conversation, at the top, with the first message selected.
    fn selecting_message_one(charwise: bool) -> App {
        let mut app = App::mock();
        go_to_top(&mut app);
        key(&mut app, if charwise { 'v' } else { 'V' });
        app
    }

    /// What the register holds.
    fn yanked(app: &App) -> Vec<String> {
        app.register().lines().to_vec()
    }

    #[test]
    fn y_pulls_a_text_selection_into_the_register_and_ends_the_selection() {
        let mut app = selecting_message_one(true);
        for _ in 0..8 {
            key(&mut app, 'l');
        }

        key(&mut app, 'y');

        assert_eq!(yanked(&app), vec!["Hey, is ".to_owned()]);
        assert_eq!(
            app.mode,
            Mode::Normal,
            "a yank ends the selection, as in Vim"
        );
        assert_eq!(app.selection(), None);
    }

    #[test]
    fn y_pulls_one_line_per_message_oldest_first() {
        let mut app = selecting_message_one(false);
        key(&mut app, 'j');
        key(&mut app, 'j');

        key(&mut app, 'y');

        assert_eq!(
            yanked(&app),
            vec![
                "Hey, is the build green?".to_owned(),
                "Yes — clippy is happy.".to_owned(),
                "Nice. Did you pin the toolchain?".to_owned(),
            ],
            "three messages, oldest first, whatever order they were selected in"
        );
    }

    /// A yank with no motion behind it is a position, not a span, and there is
    /// nothing in it to take. Said rather than silently emptying the register.
    #[test]
    fn a_selection_that_covers_nothing_says_so_rather_than_yanking_nothing() {
        let mut app = selecting_message_one(true);

        key(&mut app, 'y');

        assert!(
            app.status.contains("nothing to yank"),
            "got {:?}",
            app.status
        );
        assert_eq!(
            app.status_text(),
            app.status,
            "and the refusal is on the screen: a selection's own note outranks a \
             transient status, so leaving Visual is what makes it visible at all"
        );
        assert!(yanked(&app).is_empty());
        assert_eq!(app.mode, Mode::Normal);
    }

    /// A yank with no paste is a one-way trip to the system clipboard, which is
    /// not somewhere a message can be sent from.
    #[test]
    fn p_opens_the_line_with_what_was_yanked() {
        let mut app = selecting_message_one(false);
        key(&mut app, 'j');
        key(&mut app, 'y');

        key(&mut app, 'p');

        assert_eq!(app.focus, Focus::Input);
        assert_eq!(app.line.purpose(), PromptKind::Message);
        assert_eq!(
            app.line.text(),
            "Hey, is the build green?\nYes — clippy is happy.",
            "two messages, pasted as two lines"
        );
    }

    /// The register holds owned text, so a page landing between the yank and the
    /// paste cannot pull the text out from under it.
    #[test]
    fn what_was_yanked_survives_a_page_landing() {
        let mut app = App::mock();
        app.apply_latest(numbered(10..=20));
        go_to_top(&mut app);
        key(&mut app, 'V');
        key(&mut app, 'j');
        key(&mut app, 'y');
        let before = yanked(&app);

        assert!(app.apply_older(numbered(1..=9)));
        key(&mut app, 'p');

        assert_eq!(yanked(&app), before, "and the paste is the same text");
        assert_eq!(app.line.text(), before.join("\n"));
    }

    #[test]
    fn p_with_nothing_yanked_says_so() {
        let mut app = App::mock();

        key(&mut app, 'p');

        assert!(
            app.status.contains("nothing has been yanked"),
            "got {:?}",
            app.status
        );
        assert_eq!(app.focus, Focus::Conversation, "and no line was opened");
    }

    /// A yank is about this conversation, so opening another one forgets it —
    /// the same discipline as the search and the selection.
    #[test]
    fn opening_another_conversation_forgets_what_was_yanked() {
        let mut app = selecting_message_one(false);
        key(&mut app, 'y');

        app.select_chat(1);

        assert!(yanked(&app).is_empty());
    }

    /// A yank is offered to the system clipboard as well as kept in the register,
    /// and taking it is a hand-over like every other one here: once, not twice.
    #[test]
    fn a_yank_is_offered_to_the_clipboard_and_taken_once() {
        let mut app = App::mock();
        assert_eq!(app.take_clipboard(), None, "nothing has been yanked yet");

        go_to_top(&mut app);
        key(&mut app, 'V');
        key(&mut app, 'j');
        key(&mut app, 'y');

        assert_eq!(
            app.take_clipboard().as_deref(),
            Some("Hey, is the build green?\nYes — clippy is happy."),
            "the whole of the register, which is what a yank is for"
        );
        assert_eq!(app.take_clipboard(), None, "and it is gone once taken");
    }

    /// A yank in the line is a yank: the line's `y` fills the same slot the
    /// conversation's does, and the caller that owns the terminal drains it the
    /// same way. One seam, two producers.
    #[test]
    fn a_yank_in_the_line_is_offered_to_the_clipboard_and_taken_once() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('i')));
        type_text(&mut app, "héllo");
        app.handle_key(press(KeyCode::Esc));
        app.handle_key(press(KeyCode::Char('0')));
        app.handle_key(press(KeyCode::Char('v')));
        app.handle_key(press(KeyCode::Char('l')));
        app.handle_key(press(KeyCode::Char('l')));
        app.handle_key(press(KeyCode::Char('y')));

        assert_eq!(
            app.take_clipboard().as_deref(),
            Some("hél"),
            "the line's yank, multi-byte characters and all"
        );
        assert_eq!(app.take_clipboard(), None, "and it is drained once");
    }

    /// A word motion on multi-byte text runs, and the caret it leaves is on a
    /// character boundary — the snap after every key is what makes that so.
    #[test]
    fn a_word_motion_on_non_ascii_text_runs_and_says_nothing() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('i')));
        type_text(&mut app, "héllo wörld");
        app.handle_key(press(KeyCode::Esc));

        app.handle_key(press(KeyCode::Char('w')));

        assert_eq!(app.line.text(), "héllo wörld", "and it only moved");
        assert!(
            app.line.text().is_char_boundary(app.line.caret()),
            "onto a character: {:?}",
            app.line.caret()
        );
        assert!(
            !app.status.contains("not built yet"),
            "and nothing is owed the reader: {:?}",
            app.status
        );
    }

    /// Behind an operator the motion and the slice happen inside one key, which
    /// is the one place snapping cannot help. It is refused — and it says so,
    /// because a key that does nothing and says nothing reads as a hang.
    #[test]
    fn a_word_motion_behind_an_operator_on_non_ascii_text_is_refused_and_says_so() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('i')));
        type_text(&mut app, "héllo wörld");
        app.handle_key(press(KeyCode::Esc));
        app.handle_key(press(KeyCode::Char('d')));

        app.handle_key(press(KeyCode::Char('w')));

        assert_eq!(app.line.text(), "héllo wörld", "nothing ran");
        assert!(
            app.status.contains("not built yet"),
            "and the refusal says so: {:?}",
            app.status
        );
    }

    /// The register is the half that always works; the clipboard is a courtesy
    /// whose terminal may or may not honour it, so nothing about a yank depends on
    /// the offer having been taken.
    #[test]
    fn a_yank_survives_its_clipboard_offer_being_never_taken() {
        let mut app = selecting_message_one(false);
        key(&mut app, 'y');

        let _offer = app.take_clipboard();

        assert_eq!(
            yanked(&app),
            vec!["Hey, is the build green?".to_owned()],
            "the register is what is left whatever happened to the offer"
        );
    }

    /// Two refusals, and a reader who cannot tell them apart cannot tell what to
    /// select instead.
    #[test]
    fn a_visual_r_is_refused_and_says_which_way() {
        let mut app = App::mock();
        go_to_top(&mut app);
        key(&mut app, 'v');
        for _ in 0..5 {
            key(&mut app, 'l');
        }

        key(&mut app, 'r');

        assert_eq!(app.mode, Mode::Normal, "a refusal still answers the key");
        assert_eq!(app.selection(), None);
        assert_eq!(
            app.status_text(),
            "quoting a reply is not built yet",
            "a selection inside one message is the case a quote would serve"
        );
        assert_eq!(app.focus, Focus::Conversation, "and no line was opened");
    }

    #[test]
    fn a_visual_r_over_several_messages_is_refused_for_the_other_reason() {
        let mut app = App::mock();
        go_to_top(&mut app);
        key(&mut app, 'V');
        key(&mut app, 'j');

        key(&mut app, 'r');

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(
            app.status_text(),
            "a reply can only quote words inside one message",
            "there is no wire representation for quoting five messages"
        );
    }

    /// `r` in Normal is a plain reply with no quote, and is not affected by any of
    /// the above.
    #[test]
    fn a_normal_r_still_opens_a_plain_reply() {
        let mut app = App::mock();
        go_to_top(&mut app);

        key(&mut app, 'r');

        assert_eq!(app.focus, Focus::Input);
        assert_eq!(app.line.purpose(), PromptKind::Reply);
        assert_eq!(app.reply_to, Some(1));
    }

    #[test]
    fn p_is_not_bound_in_visual_mode() {
        let mut app = selecting_message_one(false);

        key(&mut app, 'p');

        assert_eq!(
            app.focus,
            Focus::Conversation,
            "replacing a selection with the reader's own text is a destructive reading \
             of a key that looks additive, so it does nothing here"
        );
        assert!(yanked(&app).is_empty());
        assert_eq!(app.mode, Mode::Visual, "and the selection is untouched");
    }

    // ---- deleting, and the confirm --------------------------------------

    // ---- deleting, and the confirm --------------------------------------

    /// A conversation holding exactly these turns: an identifier and whose it is.
    ///
    /// The sample data alternates which side sent each message, so a run of two
    /// from the same side — which is the ordinary shape of a conversation, and the
    /// only way to reach the "all yours" and "all theirs" wordings — is not
    /// something it can say.
    fn conversation(turns: &[(i64, bool)]) -> App {
        let mut app = App::new();
        app.set_chats(mock_chats());
        app.select_chat(0);
        app.apply_latest(
            turns
                .iter()
                .map(|(id, outgoing)| Message {
                    id: *id,
                    chat_id: MOCK_CHAT,
                    text: Cow::Borrowed("text"),
                    timestamp: 0,
                    status: MessageStatus::Received,
                    is_outgoing: *outgoing,
                    reply_to: None,
                    media: None,
                })
                .collect(),
        );
        app
    }

    /// Three of the reader's own, then two of theirs.
    fn one_way_then_the_other() -> App {
        conversation(&[(1, true), (2, true), (3, true), (4, false), (5, false)])
    }

    /// Moves the cursor down onto the message with this identifier.
    ///
    /// Forward only, which is all these tests need: they all start at the top.
    /// Bounded, so a target that is behind the cursor fails the test rather than
    /// hanging the suite.
    fn cursor_onto(app: &mut App, id: i64) {
        for _ in 0..=app.conversation.window.len() {
            if reading(app) == Some(id) {
                return;
            }
            key(app, 'j');
        }

        panic!(
            "no message {id} below the cursor: it holds {:?}",
            reading(app)
        );
    }

    /// A linewise selection from the first message to the last.
    fn spanning_all(app: &mut App) {
        go_to_top(app);
        key(app, 'V');
        while app.vim.cursor() + 1 < app.conversation.window.len() {
            key(app, 'j');
        }
    }

    /// The identifiers a pending deletion would ask the server for.
    fn asked_to_delete(app: &App) -> Vec<i64> {
        match &app.confirm {
            Some(ConfirmKind::DeleteMessages { ids, .. }) => ids.clone(),
            other => panic!("expected a deletion to be waiting, got {other:?}"),
        }
    }

    #[test]
    fn q_asks_before_quitting() {
        let mut app = App::mock();

        key(&mut app, 'q');

        assert_eq!(app.mode, Mode::Confirm);
        assert_eq!(app.confirm, Some(ConfirmKind::Quit));
        assert!(!app.should_quit);
        assert_eq!(app.status_text(), QUIT_PROMPT);
    }

    /// The command asks the same question as the key, because they are the same
    /// request written down.
    #[test]
    fn the_command_quit_asks_too() {
        let mut app = App::mock();

        run_command_line(&mut app, "q");

        assert_eq!(app.mode, Mode::Confirm);
        assert_eq!(app.confirm, Some(ConfirmKind::Quit));
        assert!(!app.should_quit);
    }

    #[test]
    fn y_on_the_quit_prompt_quits_and_asks_the_network_for_nothing() {
        let mut app = App::mock();

        key(&mut app, 'q');
        key(&mut app, 'y');

        assert!(app.should_quit);
        assert_eq!(app.confirm, None);
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.actions.is_empty());
    }

    /// Either way of saying no, and neither of them is a third key: `Esc` is
    /// how a reader abandons a question they have read past the end of.
    #[test]
    fn n_and_esc_keep_the_program_running() {
        for answer in [
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        ] {
            let mut app = App::mock();

            key(&mut app, 'q');
            app.handle_key(answer);

            assert!(!app.should_quit, "{answer:?} quit");
            assert_eq!(app.confirm, None, "{answer:?} left the prompt up");
            assert_eq!(app.mode, Mode::Normal, "{answer:?} left the mode alone");
        }
    }

    /// The one key that does not ask, and the reason it is not asked about.
    #[test]
    fn ctrl_c_still_quits_at_once() {
        let mut app = App::mock();

        app.handle_key(press_ctrl('c'));

        assert!(app.should_quit);
        assert_eq!(app.confirm, None);
    }

    #[test]
    fn d_asks_to_delete_the_message_under_the_cursor() {
        let mut app = App::mock();
        // The newest sample message is one of theirs.
        assert_eq!(reading(&app), Some(10));

        key(&mut app, 'd');

        assert_eq!(app.mode, Mode::Confirm);
        assert_eq!(asked_to_delete(&app), vec![10]);
        assert_eq!(app.status_text(), DELETE_INCOMING_PROMPT);
    }

    /// `dd` is `d` with no second press to distinguish, so a reader who types it
    /// gets the same answer.
    #[test]
    fn dd_asks_about_the_same_message_d_does() {
        let mut app = App::mock();

        key(&mut app, 'd');
        assert_eq!(app.mode, Mode::Confirm);

        app.handle_key(press(KeyCode::Char('n')));
        key(&mut app, 'd');
        assert_eq!(app.mode, Mode::Confirm);
        key(&mut app, 'd');

        assert_eq!(asked_to_delete(&app), vec![10], "and so does `dd`");
    }

    #[test]
    fn a_motion_then_dd_deletes_the_message_the_cursor_is_on() {
        let mut app = App::mock();
        go_to_top(&mut app);

        key(&mut app, 'j');
        key(&mut app, 'd');
        assert_eq!(asked_to_delete(&app), vec![2]);
        key(&mut app, 'y');

        assert_eq!(
            app.take_action(),
            Some(Action::Delete {
                chat_id: MOCK_CHAT,
                message_ids: vec![2],
            }),
            "the message the cursor was on when the `d` landed, and not the one \
             it was on before the `j`"
        );
    }

    /// The asymmetry this whole feature is built on: a deletion accepts the other
    /// side's message, and the prompt says which side it is about.
    #[test]
    fn a_deletion_accepts_an_outgoing_message_and_the_prompt_names_that_side() {
        let mut app = App::mock();
        key(&mut app, 'k');
        assert_eq!(reading(&app), Some(9), "an outgoing message");

        key(&mut app, 'd');

        assert_eq!(app.status_text(), DELETE_OUTGOING_PROMPT);
    }

    /// With no latch there is nothing for a stray key to disturb, and a second `d`
    /// is a second question about whatever is under the cursor then — not the end
    /// of a two-key sequence over the first one's message.
    #[test]
    fn two_ds_are_two_questions_and_nothing_else_is_half_of_one() {
        let mut app = App::mock();

        key(&mut app, 'x');
        assert_eq!(
            app.mode,
            Mode::Normal,
            "an unbound key is not half of a `dd`"
        );

        key(&mut app, 'd');
        assert_eq!(asked_to_delete(&app), vec![10]);
        key(&mut app, 'n');

        key(&mut app, 'k');
        key(&mut app, 'd');
        assert_eq!(asked_to_delete(&app), vec![9]);
    }

    #[test]
    fn a_page_then_d_deletes_where_the_page_landed() {
        let mut app = App::mock();
        go_to_top(&mut app);

        app.handle_key(press_ctrl('d'));
        let after = reading(&app).expect("a message is on screen");

        key(&mut app, 'd');

        assert_eq!(asked_to_delete(&app), vec![after]);
    }

    #[test]
    fn a_visual_d_asks_for_every_message_the_selection_covers() {
        let mut app = App::mock();
        go_to_top(&mut app);
        key(&mut app, 'V');
        cursor_onto(&mut app, 3);

        key(&mut app, 'd');

        assert_eq!(asked_to_delete(&app), vec![1, 2, 3], "oldest first");
        assert_eq!(
            app.status_text(),
            delete_mixed_prompt(2, 1),
            "a range that mixes both sides says which is which"
        );
    }

    /// A selection inside one message deletes the whole of it: a partial message
    /// is not something the protocol can do, and half a deletion is not something
    /// the reader would recognise afterwards.
    #[test]
    fn a_text_selection_deletes_the_whole_message_it_is_in() {
        let mut app = App::mock();
        go_to_top(&mut app);
        key(&mut app, 'v');
        for _ in 0..5 {
            key(&mut app, 'l');
        }

        key(&mut app, 'd');

        assert_eq!(
            asked_to_delete(&app),
            vec![1],
            "all of it, not the five characters"
        );
    }

    #[test]
    fn a_selection_of_only_the_reader_s_own_messages_counts_them() {
        let mut app = conversation(&[(1, true), (2, true), (3, true)]);
        spanning_all(&mut app);

        key(&mut app, 'd');

        assert_eq!(app.status_text(), delete_yours_prompt(3));
    }

    #[test]
    fn a_selection_of_only_their_messages_counts_them() {
        let mut app = conversation(&[(1, false), (2, false), (3, false), (4, false)]);
        spanning_all(&mut app);

        key(&mut app, 'd');

        assert_eq!(app.status_text(), delete_theirs_prompt(4));
    }

    /// A placeholder is a local stand-in for a send the server has not
    /// acknowledged, so naming one would have the whole request refused and take
    /// the real messages down with it. It is left out — and said.
    #[test]
    fn a_selection_with_some_placeholders_skips_them_and_says_how_many() {
        let mut app = conversation(&[(1, false), (2, false), (3, false)]);
        submit(&mut app, "hi");
        let placeholder = app.sending.expect("the send is in flight");
        go_to_top(&mut app);
        key(&mut app, 'V');
        cursor_onto(&mut app, placeholder);

        key(&mut app, 'd');

        assert_eq!(asked_to_delete(&app), vec![1, 2, 3]);
        assert!(
            app.status_text().ends_with("· 1 never sent"),
            "and the prompt says what it left out: {}",
            app.status_text()
        );
    }

    /// A selection of nothing but placeholders has nothing to ask for, and the
    /// refusal keeps the distinction between a send still on its way and one that
    /// failed: only the second has a `D`.
    #[test]
    fn a_selection_of_only_placeholders_is_refused() {
        let mut app = conversation(&[(1, false)]);
        submit(&mut app, "hi");
        let placeholder = app.sending.expect("the send is in flight");
        go_to_top(&mut app);
        cursor_onto(&mut app, placeholder);
        key(&mut app, 'V');

        key(&mut app, 'd');

        assert_eq!(app.mode, Mode::Normal, "no confirm is raised");
        assert!(
            app.status.contains("still on its way"),
            "got {:?}",
            app.status
        );

        app.fail_send(placeholder, "boom".to_owned());
        key(&mut app, 'd');

        assert_eq!(app.mode, Mode::Normal, "and still none");
        assert!(
            app.status.contains("D dismisses"),
            "a failed message points at the key that clears it: {:?}",
            app.status
        );
    }

    /// A placeholder for a send in flight is numbered below zero and sits at the
    /// *end* of the window, so the numbers between the two ends of a selection say
    /// something different from what the selection covers. Reading coverage off the
    /// identifiers deleted the wrong messages.
    #[test]
    fn a_selection_reaching_a_placeholder_covers_the_window_positions() {
        let mut app = App::mock();
        submit(&mut app, "hi");
        let placeholder = app.sending.expect("the send is in flight");
        go_to_top(&mut app);
        key(&mut app, 'V');
        cursor_onto(&mut app, placeholder);

        assert_eq!(
            app.covered(app.selection()),
            0..11,
            "from the first message to the placeholder, which is the last position"
        );

        key(&mut app, 'd');

        assert_eq!(
            asked_to_delete(&app),
            (1..=10).collect::<Vec<i64>>(),
            "the ten between them, and not the one the identifier span names"
        );
    }

    /// Everything the wording needs is captured when the prompt is raised, so
    /// nothing that happens to the selection while the prompt is up can change
    /// what `y` deletes. A page landing under a range, a focus change, a
    /// conversation change: all of it moves what the reader is looking at, and
    /// re-deriving the selection at `y` time would delete that instead.
    #[test]
    fn a_confirmation_hands_over_what_it_captured_and_not_the_selection_now() {
        let mut app = one_way_then_the_other();
        spanning_all(&mut app);
        key(&mut app, 'd');
        assert_eq!(asked_to_delete(&app), vec![1, 2, 3, 4, 5]);

        app.set_selection(Selection::at(5, None));

        key(&mut app, 'y');

        assert_eq!(
            app.take_action(),
            Some(Action::Delete {
                chat_id: MOCK_CHAT,
                message_ids: vec![1, 2, 3, 4, 5],
            })
        );
    }

    #[test]
    fn editing_a_message_that_has_not_been_sent_or_is_not_yours_is_refused() {
        let mut app = App::mock();
        submit(&mut app, "hi");
        let id = app.sending.expect("the send is in flight");

        app.handle_key(press(KeyCode::Char('e')));
        assert_eq!(app.mode, Mode::Normal);
        assert!(
            app.status.contains("hasn't been sent yet"),
            "got {:?}",
            app.status
        );

        app.fail_send(id, "boom".to_owned());
        app.handle_key(press(KeyCode::Char('e')));
        assert!(
            app.status.contains("hasn't been sent yet"),
            "a failed send is refused on the same fact: {:?}",
            app.status
        );

        // Step back off the placeholder to a message that came from them.
        app.handle_key(press(KeyCode::Char('k')));
        assert_eq!(
            reading(&app),
            Some(10),
            "the newest sample message is incoming"
        );
        app.handle_key(press(KeyCode::Char('e')));
        assert_eq!(app.mode, Mode::Normal);
        assert!(
            app.status.contains("only edit your own"),
            "got {:?}",
            app.status
        );
    }

    // ---- composing a reply and an edit ---------------------------------

    #[test]
    fn r_opens_a_reply_to_the_message_under_the_cursor() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('k')));
        assert_eq!(reading(&app), Some(9));

        app.handle_key(press(KeyCode::Char('r')));
        assert_eq!(app.focus, Focus::Input);
        assert_eq!(app.line.purpose(), PromptKind::Reply);
        assert_eq!(app.reply_to, Some(9));

        type_text(&mut app, "sure");
        app.handle_key(press(KeyCode::Enter));

        assert_eq!(
            app.take_action(),
            Some(Action::Send {
                chat_id: MOCK_CHAT,
                temp_id: -1,
                text: "sure".to_owned(),
                reply_to: Some(9),
            })
        );
    }

    #[test]
    fn e_opens_the_cursor_s_own_message_for_editing() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('k')));
        assert_eq!(reading(&app), Some(9), "an outgoing message");

        app.handle_key(press(KeyCode::Char('e')));
        assert_eq!(app.focus, Focus::Input);
        assert_eq!(app.line.purpose(), PromptKind::Edit);
        assert_eq!(app.editing, Some(9));
        assert!(
            app.line.text().starts_with("No pressure then :)"),
            "the buffer opens with the message's text: {:?}",
            app.line.text()
        );

        type_text(&mut app, "!");
        app.handle_key(press(KeyCode::Enter));

        let Some(Action::Edit {
            chat_id,
            message_id,
            text,
        }) = app.take_action()
        else {
            panic!("an edit is handed to the caller");
        };
        assert_eq!(chat_id, MOCK_CHAT);
        assert_eq!(message_id, 9);
        assert!(text.starts_with("No pressure then :)"), "got {text:?}");
    }

    // ---- confirming, dismissing, and the status line -------------------

    #[test]
    fn confirming_a_delete_hands_the_captured_message_over() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('d')));
        app.handle_key(press(KeyCode::Char('d')));

        app.handle_key(press(KeyCode::Char('y')));

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.confirm, None);
        assert_eq!(
            app.take_action(),
            Some(Action::Delete {
                chat_id: MOCK_CHAT,
                message_ids: vec![10],
            })
        );
    }

    #[test]
    fn cancelling_a_delete_leaves_no_side_effect() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('d')));
        app.handle_key(press(KeyCode::Char('d')));

        app.handle_key(press(KeyCode::Esc));

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.confirm, None);
        assert_eq!(app.take_action(), None);
    }

    #[test]
    fn the_dismiss_key_clears_a_failed_message() {
        let mut app = App::mock();
        submit(&mut app, "hi");
        let id = app.sending.expect("the send is in flight");
        app.fail_send(id, "boom".to_owned());

        app.handle_key(press(KeyCode::Char('D')));

        assert_eq!(app.conversation.window.len(), 10);
        assert!(app.conversation.message(id).is_none());
    }

    /// The row only has room for a short reason; the whole of it is on the
    /// status line while the cursor is on the message.
    #[test]
    fn the_full_reason_a_send_failed_is_on_the_status_line() {
        let mut app = App::mock();
        submit(&mut app, "hi");
        let id = app.sending.expect("the send is in flight");

        app.fail_send(id, "flood wait, retry in 42s".to_owned());

        assert_eq!(app.status_text(), "flood wait, retry in 42s");
    }

    #[test]
    fn a_flash_reverts_once_its_time_is_up() {
        let mut app = App::mock();

        app.flash("something went wrong");
        assert_eq!(app.status, "something went wrong");
        assert!(
            !app.expire_status(Instant::now()),
            "the deadline has not passed"
        );
        assert_eq!(app.status, "something went wrong");

        assert!(app.expire_status(Instant::now() + FLASH_FOR));
        assert_eq!(app.status, IDLE_STATUS);
        assert!(!app.expire_status(Instant::now() + FLASH_FOR), "only once");
    }

    fn run_command_line(app: &mut App, command: &str) {
        app.handle_key(press(KeyCode::Char(':')));
        type_text(app, command);
        app.handle_key(press(KeyCode::Enter));
    }

    // ---- the profile panel ------------------------------------------------

    /// The profile, opened from a mock application.
    fn profile() -> App {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('S')));
        app
    }

    #[test]
    fn settings_command_opens_the_profile() {
        let mut app = App::mock();
        run_command_line(&mut app, "settings");

        assert_eq!(app.pane, Pane::Profile(ProfileId::SelfAccount));
    }

    /// A `:` line is a command line, not a message, so it is not a buffer and
    /// never opens a shortcode completion — which is why the catalog cannot
    /// intercept `Enter` here. The test is here because that is the reason, and
    /// a reason nobody has checked is a reason that stops being true.
    #[test]
    fn a_command_line_never_completes_a_shortcode() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char(':')));
        type_text(&mut app, "sett");

        assert!(
            app.completion().is_none(),
            "a `:shortcode` is a message's, and this line is a command's"
        );
    }

    #[test]
    fn the_profile_opens_from_the_chat_list_too() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('h')));
        assert_eq!(app.focus, Focus::ChatList);

        app.handle_key(press(KeyCode::Char('S')));
        assert_eq!(app.pane, Pane::Profile(ProfileId::SelfAccount));
        assert_eq!(app.focus, Focus::Conversation);
    }

    /// The hint row is the one thing that says which keys the panel answers, and
    /// naming the conversation's would tell the reader they are wrong.
    #[test]
    fn the_profile_has_its_own_hint() {
        let mut app = App::mock();
        assert!(
            app.status_text().contains("dd:del"),
            "the conversation's row"
        );

        app.handle_key(press(KeyCode::Char('S')));
        let hint = app.status_text();
        assert!(hint.contains("j/k: row"), "{hint}");
        assert!(!hint.contains("dd:del"), "the conversation's keys: {hint}");
    }

    /// `d` on a row with nothing to do must change nothing at all: no mode, no
    /// prompt, and nothing handed to the caller to put on the wire. A key that
    /// quietly armed the conversation's delete would make the *next* `d`,
    /// wherever the reader had been by then, a deletion.
    #[test]
    fn a_profile_row_with_nothing_to_do_changes_nothing() {
        let mut app = profile();
        assert!(app.on_card_row("name"));

        app.handle_key(press(KeyCode::Char('d')));

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.confirm, None);
        assert!(app.take_action().is_none(), "nothing was queued");
    }

    /// The same for the row that does have something to do: it confirms, and what
    /// it queues is not a deletion. A card is a place to read, and the only thing
    /// `d` can destroy from one is the reader's own session — which is what the
    /// confirmation is asking about.
    #[test]
    fn a_profile_row_never_queues_a_deletion() {
        let mut app = profile();
        while !app.on_card_row(crate::card::LOGOUT) {
            app.handle_key(press(KeyCode::Char('j')));
        }

        app.handle_key(press(KeyCode::Char('d')));
        assert_eq!(app.confirm, Some(ConfirmKind::Logout));

        app.handle_key(press(KeyCode::Char('y')));
        assert!(
            !matches!(app.take_action(), Some(Action::Delete { .. })),
            "a card destroys no messages"
        );
    }

    /// The keys walk the rows the panel is drawing.
    ///
    /// There used to be a second enumeration of the card's rows — a bare enum the
    /// keys asked instead of the panel — and the two agreed only because both were
    /// built from the same field. The failure was never a crash: it was a key that
    /// acted on a row the panel was not showing, or a row nothing could reach.
    /// Walking the card and comparing the two lists is the assertion that catches
    /// it, and the reason this test walks by `j` rather than by index: an index
    /// would match even if the two lists were ordered differently.
    #[test]
    fn the_keys_walk_the_rows_the_panel_is_drawing() {
        let mut app = profile();
        let drawn: Vec<&'static str> = crate::card::rows(&app)
            .iter()
            .map(|row| row.label)
            .collect();
        assert!(drawn.len() > 2, "the mock account has rows: {drawn:?}");

        let mut walked = Vec::new();
        for _ in 0..drawn.len() {
            walked.push(crate::card::rows(&app)[app.profile_cursor()].label);
            app.handle_key(press(KeyCode::Char('j')));
        }

        assert_eq!(
            walked, drawn,
            "`j` walked a different list than the panel drew"
        );
    }

    /// Two cursors, one type. With one value shared, the conversation's total —
    /// the window's length — would size the profile's highlight too, and the
    /// profile's row count would stand in for it. Neither test fails on its own:
    /// each one only sees its own half move.
    #[test]
    fn the_profile_and_the_conversation_cursors_are_independent() {
        let mut app = profile();
        let in_the_conversation = app.vim.cursor();
        for _ in 0..3 {
            app.handle_key(press(KeyCode::Char('j')));
        }
        assert!(app.profile_cursor() > 0, "the card's highlight moved");
        assert_eq!(
            app.vim.cursor(),
            in_the_conversation,
            "and the conversation's did not"
        );

        // `Esc` out and `k` on the conversation: `k` rather than `j`, because the
        // mock conversation opens pinned to its newest message and `j` there is
        // the clamp rather than the motion.
        app.handle_key(press(KeyCode::Esc));
        app.handle_key(press(KeyCode::Char('k')));
        assert!(
            app.vim.cursor() < in_the_conversation,
            "the conversation's highlight moves on its own"
        );
    }

    /// Leaving a pane is one rule, and `Tab` is one of the keys that does it: the
    /// card is not a stack, so the conversation is what comes back.
    ///
    /// `Esc` and `l` no longer both leave, and that is the design: `h`/`l` are an
    /// inline motion now, so a card is a column of values with no column to the
    /// right of the last one, and `l` at the end of a value has nowhere to go. The
    /// way out is `Esc`, and `h` at the *start* of a value — the two edges, each
    /// key doing the one thing its own edge allows.
    #[test]
    fn every_way_out_of_the_card_lands_on_the_conversation() {
        // `Esc` with nothing selected: one press leaves.
        let mut app = profile();
        app.handle_key(press(KeyCode::Esc));
        assert_eq!(app.pane, Pane::Conversation, "Esc closes the card");

        // `l` at the end of a value is not a way out, because it is a motion with
        // nowhere to go and saying otherwise would teach a key to lie.
        let mut app = profile();
        while app.card_caret() < 3 {
            app.handle_key(press(KeyCode::Char('l')));
        }
        app.handle_key(press(KeyCode::Char('l')));
        assert!(
            app.pane.is_profile(),
            "l past the end of a value is a motion, not a way out"
        );

        // `h` at the start of a value is the way back, and the way back is the
        // conversation. The chat list is `Ctrl-w h`, because `h` is a motion here
        // and a key that is a motion in one place and a pane in the next is a key
        // a reader has to learn twice.
        let mut app = profile();
        app.handle_key(press(KeyCode::Char('h')));
        assert_eq!(app.pane, Pane::Conversation, "h at the start goes back");

        let mut app = profile();
        app.handle_key(press_ctrl('w'));
        app.handle_key(press(KeyCode::Char('h')));
        assert_eq!(app.focus, Focus::ChatList, "Ctrl-w h is the chat list");

        // Nothing is drawn to the right of a card, so `Ctrl-w l` has nowhere to
        // go and says so rather than doing nothing.
        let mut app = profile();
        app.handle_key(press_ctrl('w'));
        app.handle_key(press(KeyCode::Char('l')));
        assert!(
            app.pane.is_profile(),
            "the card stays, and the reason is on show"
        );

        let mut app = profile();
        app.handle_key(press(KeyCode::Tab));
        assert_eq!(app.pane, Pane::Conversation, "Tab walks the panes");
        assert_eq!(app.focus, Focus::Input);
    }

    /// `Esc` is a ladder and not a switch: a selection, then the card, then out.
    /// Four presses from a selection on a second row, with no special case
    /// anywhere in it.
    #[test]
    fn escape_on_a_card_is_a_ladder_and_not_a_switch() {
        let mut app = profile();
        app.handle_key(press(KeyCode::Char('j')));
        app.handle_key(press(KeyCode::Char('v')));
        app.handle_key(press(KeyCode::Char('j')));
        assert!(
            app.card_selection().is_some(),
            "a selection across two rows"
        );

        // One press drops the selection and keeps the card.
        app.handle_key(press(KeyCode::Esc));
        assert!(app.card_selection().is_none(), "the selection went");
        assert!(app.pane.is_profile(), "and the card stayed");

        // The next press leaves.
        app.handle_key(press(KeyCode::Esc));
        assert_eq!(app.pane, Pane::Conversation, "and the next one leaves");
    }

    /// A key the panel does not answer is the conversation's, and taking it is
    /// what stops a reader who pressed `i` by reflex from pressing it twice.
    #[test]
    fn a_conversation_key_from_the_profile_does_its_own_thing() {
        let mut app = profile();
        app.handle_key(press(KeyCode::Char('i')));

        assert_eq!(app.pane, Pane::Conversation);
        assert_eq!(app.focus, Focus::Input, "and the line is open");
    }

    #[test]
    fn add_account_refuses_and_says_what_it_cannot_do() {
        let mut app = profile();
        while !app.on_card_row(crate::card::ADD_ACCOUNT) {
            app.handle_key(press(KeyCode::Char('j')));
        }
        app.handle_key(press(KeyCode::Char('d')));

        assert_eq!(app.status, ADD_ACCOUNT_REFUSAL);
        assert_eq!(
            app.mode,
            Mode::Normal,
            "and nothing was asked to be confirmed"
        );
    }

    /// A `logout` row that flashed a refusal instead of confirming would have
    /// taught the reader the wrong thing about a key that throws away the only
    /// secret this program holds.
    #[test]
    fn logout_confirms_before_anything_is_asked_for() {
        let mut app = profile();
        while !app.on_card_row(crate::card::LOGOUT) {
            app.handle_key(press(KeyCode::Char('j')));
        }
        app.handle_key(press(KeyCode::Char('d')));

        assert_eq!(app.mode, Mode::Confirm);
        assert_eq!(app.confirm, Some(ConfirmKind::Logout));
        assert_eq!(app.status_text(), LOGOUT_PROMPT);
        assert_eq!(app.take_action(), None, "nothing is asked for yet");

        app.handle_key(press(KeyCode::Char('y')));
        assert_eq!(
            app.take_action(),
            Some(Action::Logout),
            "`y` asks for the sign-out"
        );
        assert_eq!(app.confirm, None);
        assert_eq!(app.mode, Mode::Normal);
    }

    /// `n` and `Esc` are the other half of a confirmation, and here they mean
    /// *do not*: the session this program holds is the reader's, and declining is
    /// not a step towards doing it.
    #[test]
    fn declining_a_sign_out_asks_for_nothing() {
        for answer in [KeyCode::Char('n'), KeyCode::Esc] {
            let mut app = profile();
            while !app.on_card_row(crate::card::LOGOUT) {
                app.handle_key(press(KeyCode::Char('j')));
            }
            app.handle_key(press(KeyCode::Char('d')));
            assert_eq!(app.confirm, Some(ConfirmKind::Logout));

            app.handle_key(press(answer));

            assert_eq!(app.confirm, None, "{answer:?} drops the question");
            assert_eq!(app.mode, Mode::Normal);
            assert_eq!(app.take_action(), None, "{answer:?} asks for nothing");
        }
    }

    /// The rows that exist are the ones the account has something to say about.
    #[test]
    fn a_row_exists_only_when_the_account_says_something() {
        let mut app = App::mock();
        app.set_account(Ok(Account {
            username: None,
            phone: None,
            birthday: None,
            bio: None,
            ..mock_account()
        }));
        app.handle_key(press(KeyCode::Char('S')));

        let rows = crate::card::rows(&app);
        assert!(!rows.iter().any(|row| row.label == "bio"));
        assert!(!rows.iter().any(|row| row.label == "birthday"));
        assert!(rows.iter().any(|row| row.label == "name"));
        assert_eq!(
            rows.last().map(|row| row.label),
            Some(crate::card::LOGOUT),
            "the actions are last"
        );
        // The highlight counts the rows that exist, not the ones that would: six
        // rows here, and the two that are missing leave no gap for `j` to land in.
        assert_eq!(rows.len(), 6);
        assert!(app.on_card_row("name"));
    }

    #[test]
    fn chat_command_selects_the_matching_chat() {
        let mut app = App::mock();
        run_command_line(&mut app, "chat 2");

        let expected = app
            .chats()
            .iter()
            .position(|c| c.id == 2)
            .expect("chat 2 is part of the mock data");
        assert_eq!(app.selected_chat, expected);
        assert_eq!(
            app.conversation.window.chat_id, 2,
            "the panel follows the chat list"
        );
    }

    /// Both halves of the `chat <id>` guard must hold: a malformed id and a
    /// well-formed-but-unknown id must both leave the selection untouched.
    #[test]
    fn chat_command_ignores_unparseable_or_unknown_ids() {
        let mut app = App::mock();
        let before = app.selected_chat;

        run_command_line(&mut app, "chat not-a-number");
        assert_eq!(app.selected_chat, before);

        run_command_line(&mut app, "chat 999");
        assert_eq!(app.selected_chat, before);
    }

    #[test]
    fn unknown_command_sets_the_status_line() {
        let mut app = App::mock();
        run_command_line(&mut app, "frobnicate");

        assert!(app.status.contains("unknown command"));
        assert_eq!(app.mode, Mode::Normal);
    }

    /// `:retry` asks for the client again and takes the `offline:` line down with
    /// it: the sentence left up would be a failure that has just been acted on.
    #[test]
    fn retry_command_asks_for_the_client_again() {
        let mut app = App::mock();
        app.status = "offline: connection reset".to_owned();

        run_command_line(&mut app, "retry");

        assert!(
            app.take_retry_request(),
            "the request is what the network side acts on"
        );
        assert_ne!(app.status, IDLE_STATUS, "got {:?}", app.status);
        assert!(
            !app.status.contains("offline:"),
            "the failure it answers must not still be up: {:?}",
            app.status
        );
    }

    /// One slot rather than a queue: a request taken is a request being carried out,
    /// so the pass after the one that took it finds nothing. What stops a *second*
    /// bring-up is not this slot — pressing `:retry` twice before either is taken
    /// is still one request — but the network side's own in-flight guard.
    #[test]
    fn a_retry_request_is_spent_once_taken() {
        let mut app = App::mock();

        run_command_line(&mut app, "retry");
        assert!(app.take_retry_request(), "the command records the request");
        assert!(!app.take_retry_request(), "and taking it spends it");
    }

    /// Not a flash: a bring-up does not pass on its own, it ends in an event that
    /// brings its own sentence — so this one must not expire back to idle while
    /// the client is still being built.
    #[test]
    fn a_retry_outlives_a_transient_status() {
        let mut app = App::mock();
        run_command_line(&mut app, "retry");

        assert!(
            !app.expire_status(Instant::now() + FLASH_FOR),
            "got {:?}",
            app.status
        );
        assert_ne!(app.status, IDLE_STATUS);
    }

    fn run_search_line(app: &mut App, query: &str) {
        app.handle_key(press(KeyCode::Char('/')));
        type_text(app, query);
        app.handle_key(press(KeyCode::Enter));
    }

    #[test]
    fn search_finds_matches_in_the_open_conversation() {
        let mut app = App::mock();
        run_search_line(&mut app, "benchmarks");

        assert_eq!(app.search_query(), Some("benchmarks"));
        assert_eq!(
            app.vim.cursor(),
            6,
            "the cursor lands on the only match, at its index in the window"
        );
        assert!(
            !app.conversation.auto_follow(),
            "the reader moved off the end"
        );
    }

    /// The local pass is provisional: it can only see what is loaded, so the
    /// label says so and a request is queued for the authoritative answer.
    #[test]
    fn a_search_is_provisional_until_the_server_answers() {
        let mut app = App::mock();
        run_search_line(&mut app, "benchmarks");

        assert_eq!(
            app.search().label(),
            "/benchmarks — 1 loaded — searching…",
            "a count from the loaded window is not an answer"
        );
        assert_eq!(
            app.take_action(),
            Some(Action::Search {
                chat_id: MOCK_CHAT,
                query: "benchmarks".to_owned(),
            }),
            "so the server is asked"
        );
    }

    /// A conversation the window holds in full cannot be searched better, so no
    /// round trip is spent asking.
    #[test]
    fn a_search_over_a_complete_window_asks_nothing() {
        let mut app = App::mock();
        app.exhaust(FetchDirection::Older);
        app.exhaust(FetchDirection::Newer);

        run_search_line(&mut app, "benchmarks");

        assert_eq!(app.take_action(), None, "there is nobody to ask");
        assert_eq!(
            app.search().label(),
            "/benchmarks — 1 loaded",
            "and the local list stands as the answer"
        );
    }

    /// The broad reading of "the window holds everything" is wrong: the cap
    /// drops the oldest messages, so both ends being reached says nothing.
    #[test]
    fn a_window_at_the_cap_is_not_the_whole_conversation() {
        let mut window = ConversationWindow::new(MOCK_CHAT);
        let over = i64::try_from(CONVERSATION_WINDOW).expect("the cap fits an identifier") + 5;
        window.replace((1..=over).map(|id| message(id, "text")).collect::<Vec<_>>());
        window.exhausted_older = true;
        window.exhausted_newer = true;

        assert_eq!(window.len(), CONVERSATION_WINDOW);
        assert!(
            !holds_everything(&window),
            "the cap is the only thing that drops messages, and here it did"
        );

        let mut small = ConversationWindow::new(MOCK_CHAT);
        small.replace(page(&[1, 2, 3]));
        small.exhausted_older = true;
        small.exhausted_newer = true;
        assert!(holds_everything(&small), "nothing was ever dropped");
    }

    #[test]
    fn an_empty_query_repeats_the_last_search() {
        let mut app = App::mock();
        run_search_line(&mut app, "benchmarks");
        let _ = app.take_action();

        run_search_line(&mut app, "");

        assert_eq!(
            app.search_query(),
            Some("benchmarks"),
            "as in Vim, an empty `/` runs the last search again"
        );
        assert_eq!(
            app.take_action(),
            Some(Action::Search {
                chat_id: MOCK_CHAT,
                query: "benchmarks".to_owned(),
            }),
            "and asks again, because the answer may have changed"
        );
    }

    /// The regression the bounded queue exists for: a search made inside the
    /// tick a send was made in must not replace the send.
    #[test]
    fn sending_and_searching_in_one_tick_both_happen() {
        let mut app = App::mock();

        submit(&mut app, "ping");
        run_search_line(&mut app, "benchmarks");

        let first = app.take_action().expect("the send is still queued");
        let second = app.take_action().expect("and so is the search");
        assert!(
            matches!(first, Action::Send { .. }),
            "the send goes out first"
        );
        assert!(matches!(second, Action::Search { .. }));
    }

    #[test]
    fn an_empty_query_with_nothing_to_repeat_says_so() {
        let mut app = App::mock();

        run_search_line(&mut app, "");

        assert!(!app.search().is_active());
        assert!(
            app.status.contains("no previous search"),
            "a key that does nothing reads as a hang: {:?}",
            app.status
        );
    }

    #[test]
    fn n_with_no_search_says_so() {
        let mut app = App::mock();
        let before = reading(&app);

        app.handle_key(press(KeyCode::Char('n')));

        assert_eq!(reading(&app), before, "nothing moves");
        assert!(
            app.status.contains("no previous search"),
            "and the key explains itself: {:?}",
            app.status
        );
    }

    #[test]
    fn opening_another_conversation_clears_the_search() {
        let mut app = App::mock();
        run_search_line(&mut app, "benchmarks");
        assert!(app.search().is_active());

        app.select_chat(1);

        assert!(
            !app.search().is_active(),
            "a match is a place in the conversation that was open"
        );
        assert_eq!(app.search_query(), None);
    }

    /// The label is state, not a flash: a transient status must not outrank it,
    /// and its expiry must not take it away.
    #[test]
    fn the_search_label_outlives_a_transient_status() {
        let mut app = App::mock();
        run_search_line(&mut app, "benchmarks");

        app.flash("something that passes");

        assert!(
            app.status_text().contains("/benchmarks"),
            "the search line is not replaced by a passing message: {:?}",
            app.status_text()
        );

        app.expire_status(Instant::now() + FLASH_FOR);

        assert_eq!(app.status, IDLE_STATUS, "the flash did expire");
        assert!(
            app.status_text().contains("/benchmarks"),
            "and the search line is still there: {:?}",
            app.status_text()
        );
    }

    #[test]
    fn wrapping_the_walk_is_announced_and_loops_within_the_page() {
        let mut app = App::mock();
        // `the` starts a word — or a word beginning with it, like `then` — in
        // six of the sample messages.
        run_search_line(&mut app, "the");
        for _ in 0..5 {
            app.handle_key(press(KeyCode::Char('n')));
        }
        assert_eq!(reading(&app), Some(10), "the newest match");
        assert!(
            !app.search().label().contains("hit"),
            "a step that did not wrap says nothing"
        );

        app.handle_key(press(KeyCode::Char('n')));

        assert_eq!(reading(&app), Some(1), "the walk loops within the page");
        assert!(
            app.search()
                .label()
                .contains("search hit BOTTOM, continuing at TOP"),
            "and says so: {}",
            app.search().label()
        );
    }

    #[test]
    fn a_result_for_a_replaced_query_changes_nothing() {
        let mut app = App::mock();
        run_search_line(&mut app, "benchmarks");
        run_search_line(&mut app, "slides");
        let before = app.search().ids().to_vec();
        assert_eq!(before, vec![6], "the second search stands");

        assert!(
            !app.apply_searched(MOCK_CHAT, "benchmarks", vec![7], 1),
            "an answer for the query the reader has left must not land"
        );

        assert_eq!(app.search().ids().to_vec(), before);
        assert_eq!(app.search_query(), Some("slides"));
    }

    #[test]
    fn a_result_for_another_conversation_changes_nothing() {
        let mut app = App::mock();
        run_search_line(&mut app, "benchmarks");

        assert!(!app.apply_searched(MOCK_CHAT + 1, "benchmarks", vec![7], 1));

        assert_eq!(
            app.search().source(),
            domain::search::SearchSource::Local,
            "the local list is what is still on screen"
        );
    }

    /// The server's answer replaces the local one rather than being merged with
    /// it, and the cursor moves with it.
    #[test]
    fn a_server_result_replaces_the_local_matches() {
        let mut app = App::mock();
        run_search_line(&mut app, "benchmarks");
        assert_eq!(reading(&app), Some(7));

        assert!(app.apply_searched(MOCK_CHAT, "benchmarks", vec![3, 7], 1_000));

        assert_eq!(app.search().source(), domain::search::SearchSource::Server);
        assert_eq!(app.search().total(), 1_000);
        assert_eq!(app.search().ids().to_vec(), vec![3, 7]);
        assert_eq!(
            reading(&app),
            Some(7),
            "the cursor was on a match the server confirmed, so it stays"
        );
    }

    // ---- where the reader is -------------------------------------------

    #[test]
    fn opening_a_conversation_starts_pinned_to_the_newest_message() {
        let app = App::mock();

        assert!(app.conversation.auto_follow());
        assert_eq!(reading(&app), Some(10));
    }

    #[test]
    fn stepping_up_disengages_following_and_the_end_re_engages_it() {
        let mut app = App::mock();

        app.handle_key(press(KeyCode::Char('k')));
        assert_eq!(reading(&app), Some(9));
        assert!(!app.conversation.auto_follow(), "the reader has moved away");

        app.handle_key(press(KeyCode::Char('G')));
        assert_eq!(reading(&app), Some(10));
        assert!(
            app.conversation.auto_follow(),
            "`G` means the newest message"
        );
    }

    #[test]
    fn a_page_moves_a_screenful_and_the_bottom_re_engages_following() {
        let mut app = App::mock();
        assert_eq!(app.vim.cursor(), 9);

        app.handle_key(press_ctrl('u'));
        assert_eq!(
            app.vim.cursor(),
            0,
            "a screenful up from the newest message"
        );
        assert!(!app.conversation.auto_follow());

        app.handle_key(press_ctrl('d'));
        assert_eq!(app.vim.cursor(), 9);
        assert!(app.conversation.auto_follow(), "back at the newest message");
    }

    /// The page step is the panel's height, so a taller terminal pages further.
    /// Until a frame has been drawn the panel has no measurement, which is why
    /// the fallback has to be a sensible size rather than zero.
    #[test]
    fn a_page_is_as_tall_as_the_panel_was() {
        let mut app = App::mock();
        app.record_rows(4);

        app.handle_key(press_ctrl('u'));
        assert_eq!(app.vim.cursor(), 5, "one panel's worth up from the end");

        app.handle_key(press_ctrl('d'));
        assert_eq!(app.vim.cursor(), 9);
    }

    /// A page is a screenful of rows, not a screenful of messages: a message
    /// wider than the panel is more than one row, and four rows of one are four
    /// rows the reader has moved.
    #[test]
    fn a_page_moves_by_rows_and_lands_on_a_message() {
        let mut app = App::mock();
        app.record_body(53);
        app.record_rows(4);
        app.apply_latest(vec![
            message(0, "a"),
            Message {
                id: 1,
                text: Cow::Owned("y".repeat(400)),
                ..message(1, "text")
            },
            message(2, "b"),
            message(3, "c"),
        ]);
        go_to_top(&mut app);
        assert_eq!(app.vim.cursor(), 0);

        app.handle_key(press_ctrl('d'));

        assert_eq!(
            app.vim.cursor(),
            1,
            "row 4 is inside the message at row 1, which is what the cursor stands on"
        );
    }

    /// A page steps over rows and lands on a message, so wherever it lands there
    /// is a message there: the cursor is what the fetch triggers measure, and a
    /// cursor pointing at a row rather than a message is not a thing it can
    /// answer.
    #[test]
    fn a_page_never_leaves_the_cursor_off_a_message() {
        let mut app = App::mock();
        app.record_body(53);
        app.record_rows(4);
        let total = app.conversation.window.len();
        go_to_top(&mut app);

        for _ in 0..4 {
            app.handle_key(press_ctrl('d'));

            assert!(
                app.row_layout().iter().any(|span| span.kind
                    == RowKind::Message {
                        index: app.vim.cursor()
                    }),
                "the layout has a message for the cursor at {}",
                app.vim.cursor()
            );
        }

        assert_eq!(
            app.vim.cursor(),
            total - 1,
            "and four pages down of one screenful each is the newest message"
        );
    }

    /// The first message the panel shows, given a panel `budget` rows tall.
    fn shown_from(app: &App, budget: usize) -> usize {
        app.viewport(&app.row_layout(), budget).start
    }

    #[test]
    fn the_viewport_is_a_windowful_ending_at_a_pinned_view() {
        let app = App::mock();

        assert_eq!(
            shown_from(&app, 4),
            6,
            "pinned to the bottom, the slice is the last screenful"
        );
        assert_eq!(shown_from(&app, 99), 0, "a panel taller than the window");
        assert_eq!(
            shown_from(&app, 0),
            9,
            "a panel with no room still shows the newest row"
        );
    }

    #[test]
    fn the_viewport_centres_on_a_cursor_that_is_not_pinned() {
        let mut app = App::mock();
        go_to_top(&mut app);

        assert_eq!(
            shown_from(&app, 4),
            0,
            "the top of the window is the top of the slice"
        );

        app.vim.set_cursor(5);
        assert_eq!(
            shown_from(&app, 4),
            3,
            "half a panel either side of the cursor"
        );

        app.vim.set_cursor(9);
        assert_eq!(
            shown_from(&app, 4),
            6,
            "a slice is never taller than the panel, nor starts past the end"
        );
    }

    // ---- pages ---------------------------------------------------------

    #[test]
    fn an_older_page_leaves_the_reader_on_the_message_they_were_reading() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('k')));
        assert_eq!(reading(&app), Some(9));

        assert!(app.apply_older(page(&[-1, 0])));

        assert_eq!(app.conversation.window.len(), 12);
        assert_eq!(
            reading(&app),
            Some(9),
            "the window moved under the reader, not the reader with it"
        );
        assert!(!app.conversation.auto_follow());
    }

    #[test]
    fn an_older_page_keeps_a_pinned_view_pinned() {
        let mut app = App::mock();

        assert!(app.apply_older(page(&[-1, 0])));

        assert_eq!(reading(&app), Some(10), "the end did not move");
        assert!(app.conversation.auto_follow());
    }

    #[test]
    fn a_newer_page_stays_behind_what_the_window_holds() {
        let mut app = App::mock();
        go_to_top(&mut app);

        assert!(app.apply_newer(page(&[11, 12])));

        assert_eq!(app.conversation.window.newest_id(), Some(12));
        assert_eq!(reading(&app), Some(1), "the reader is still at the top");
    }

    #[test]
    fn a_page_for_another_conversation_is_refused() {
        let mut app = App::mock();
        let before = app.conversation.window.len();

        assert!(!app.apply_latest(vec![stranger(1)]));
        assert!(!app.apply_older(vec![stranger(1)]));
        assert!(!app.apply_newer(vec![stranger(1)]));

        assert_eq!(
            app.conversation.window.len(),
            before,
            "a page that arrives late must not empty the window it does not belong to"
        );
        assert_eq!(app.conversation.window.chat_id, MOCK_CHAT);
    }

    // ---- events from the feed ------------------------------------------

    #[test]
    fn an_arrival_lands_at_the_bottom_of_a_pinned_view() {
        let mut app = App::mock();
        assert!(app.conversation.auto_follow());

        assert!(app.apply_update(&UpdateEvent::NewMessage(message(11, "ping"))));

        assert_eq!(
            reading(&app),
            Some(11),
            "a pinned view follows the conversation"
        );
        assert!(app.conversation.auto_follow());
    }

    #[test]
    fn an_arrival_does_not_move_a_reader_who_scrolled_away() {
        let mut app = App::mock();
        go_to_top(&mut app);
        let reading_before = reading(&app);
        let len_before = app.conversation.window.len();

        assert!(app.apply_update(&UpdateEvent::NewMessage(message(11, "ping"))));

        assert_eq!(app.conversation.window.len(), len_before + 1);
        assert_eq!(
            reading(&app),
            reading_before,
            "the reader stays where they were"
        );
        assert!(!app.conversation.auto_follow());
    }

    /// An arrival the conversation already holds leaves it alone: the window
    /// deduplicates by identifier, so a message cannot sit in it twice.
    ///
    /// The flat window underneath is a different thing — a record of what the
    /// client has been sent, which does not deduplicate — so the event still
    /// reports a change. That difference is why the two are fed separately
    /// rather than one being derived from the other, and it is why the
    /// assertion here is about the conversation rather than about the report.
    #[test]
    fn an_arrival_the_conversation_already_holds_leaves_it_alone() {
        let mut app = App::mock();
        let before = app.conversation.window.len();

        let moved = app.apply_update(&UpdateEvent::NewMessage(message(10, "again")));

        assert_eq!(
            app.conversation.window.len(),
            before,
            "the open window holds one copy of the message"
        );
        assert_eq!(
            text_of(&app, 10),
            Some("See you at the demo."),
            "and the message on show keeps the text it arrived with"
        );
        assert!(moved, "while the flat window recorded what it was sent");
    }

    /// One event, two windows: the message lands in the conversation on show,
    /// and the conversation's unread count moves in the list behind it.
    #[test]
    fn one_arrival_reaches_both_the_window_and_the_list() {
        let mut app = App::mock();
        let before = unread(&app, MOCK_CHAT);

        assert!(app.apply_update(&UpdateEvent::NewMessage(message(11, "ping"))));

        assert_eq!(reading(&app), Some(11), "the window took the message");
        assert_eq!(
            unread(&app, MOCK_CHAT),
            before + 1,
            "and the list counted it, which is what the panel shows"
        );
    }

    /// The newest message the peer has read, for `chat_id`.
    fn read(chat_id: i64, max_id: i64) -> UpdateEvent {
        UpdateEvent::ReadReceipt { chat_id, max_id }
    }

    /// A read acknowledgement reaching the feed moves the conversation's watermark
    /// with nothing else happening: no message arrives, changes or leaves, so the
    /// window, the cursor and the reader's place in it are all as they were.
    #[test]
    fn a_read_acknowledgement_advances_the_open_conversation_and_moves_nothing_else() {
        let mut app = App::mock();
        // Two of the reader's own messages, as the server leaves them: the
        // sample conversation's are every one `Received`, which no sent message
        // ever is.
        app.apply_latest(vec![
            Message {
                id: 20,
                chat_id: MOCK_CHAT,
                text: Cow::Borrowed("mine"),
                timestamp: 1_730_000_600,
                status: MessageStatus::Sent,
                is_outgoing: true,
                reply_to: None,
                media: None,
            },
            Message {
                id: 21,
                chat_id: MOCK_CHAT,
                text: Cow::Borrowed("also mine"),
                timestamp: 1_730_000_660,
                status: MessageStatus::Sent,
                is_outgoing: true,
                reply_to: None,
                media: None,
            },
        ]);
        let before = (
            reading(&app),
            app.vim.cursor(),
            app.conversation.window.len(),
        );
        let newest = before.0.expect("the window holds messages");

        assert_eq!(
            rows::group_of(&app, 1).receipt,
            rows::Receipt::None,
            "nothing has been read yet, so the group claims nothing"
        );

        assert!(app.apply_update(&read(MOCK_CHAT, newest)));

        assert_eq!(
            app.conversation.read_watermark(),
            Some(newest),
            "so the group's state is derived from it on the next frame"
        );
        assert_eq!(
            rows::group_of(&app, 1).receipt,
            rows::Receipt::Read,
            "and the state the panel draws follows the feed, without anything else changing"
        );
        assert_eq!(
            (
                reading(&app),
                app.vim.cursor(),
                app.conversation.window.len()
            ),
            before,
            "and nothing about the reader's place moved"
        );
    }

    /// A receipt about a conversation the reader is not in does not touch the one
    /// that is — but it is still remembered, so opening that chat shows how far it
    /// has been read (AC-15's read half).
    #[test]
    fn a_read_acknowledgement_for_another_conversation_is_remembered_and_not_applied() {
        let mut app = App::mock();
        let other = MOCK_CHAT + 1;

        assert!(app.apply_update(&read(other, 7)));

        assert_eq!(
            app.conversation.read_watermark(),
            None,
            "the conversation on show is a different one"
        );

        app.select_chat(1);
        assert_eq!(
            app.conversation.read_watermark(),
            Some(7),
            "and the chat that owns it is opened carrying what it was told"
        );
    }

    /// The watermark is a fact about a conversation and not about the page on
    /// show, so looking away and coming back finds it as it was.
    #[test]
    fn a_read_watermark_survives_a_switch_away_and_back() {
        let mut app = App::mock();
        assert!(app.apply_update(&read(MOCK_CHAT, 6)));

        app.select_chat(1);
        app.select_chat(0);

        assert_eq!(
            app.conversation.read_watermark(),
            Some(6),
            "the view is new and the conversation's reading is not"
        );
    }

    /// The wire's watermark can repeat or arrive late, so a lower one is dropped
    /// rather than applied, and an acknowledgement that named nothing real is not
    /// recorded at all. Both report no change, which is what tells the loop there
    /// is nothing to redraw for.
    #[test]
    fn a_read_acknowledgement_that_says_nothing_new_changes_nothing() {
        let mut app = App::mock();
        assert!(app.apply_update(&read(MOCK_CHAT, 9)));

        for (chat_id, max_id) in [(MOCK_CHAT, 9), (MOCK_CHAT, 4), (MOCK_CHAT, 0)] {
            assert!(
                !app.apply_update(&read(chat_id, max_id)),
                "{chat_id} read up to {max_id} says nothing the reader has not been shown"
            );
        }
        assert_eq!(app.conversation.read_watermark(), Some(9));
    }

    /// A receipt while the reader is scrolled back up moves neither the cursor nor
    /// the reader's place: a read is not a message arriving, and it is not entitled
    /// to move anyone (US-X4).
    #[test]
    fn a_read_acknowledgement_leaves_a_scrolled_up_reader_where_they_are() {
        let mut app = App::mock();
        app.handle_key(press_ctrl('u'));
        let before = (app.vim.cursor(), app.conversation.auto_follow());
        assert!(
            !app.conversation.auto_follow(),
            "the fixture is scrolled back up"
        );

        assert!(app.apply_update(&read(MOCK_CHAT, 10)));

        assert_eq!(
            (app.vim.cursor(), app.conversation.auto_follow()),
            before,
            "and still there after it"
        );
    }

    // ---- what a window with separators does to search and to motions -----

    /// A window spanning three days, with a separator between each and a word
    /// worth searching for in more than one of them.
    ///
    /// Every message is the reader's own, a minute apart within its day, so a day
    /// is three messages in one group — the shape PR 8 puts on the screen at once:
    /// a group, a separator, and (with a watermark) a receipt.
    fn across_three_days() -> App {
        let mut app = App::mock();
        app.record_body(53);
        app.conversation.window.replace((1..=9).map(message_of_day));
        app.vim.set_total(9);
        app.vim.set_cursor(8);

        app
    }

    /// A search finds messages and lands on messages, whatever else is in the
    /// window: a separator names no message, carries no identifier, and cannot be
    /// walked onto however the walk arrived (AC-20).
    #[test]
    fn a_search_in_a_window_with_separators_matches_and_lands_on_messages_only() {
        let mut app = across_three_days();
        let layout = app.row_layout();
        let separators: Vec<usize> = layout
            .iter()
            .filter(|span| !span.kind.is_message())
            .map(|span| span.first)
            .collect();
        assert_eq!(separators.len(), 3, "one for each of the three days");

        app.run_search("benchmarks");
        let matched = app.search().ids().to_vec();
        assert_eq!(
            matched,
            vec![102, 104, 106, 108],
            "only message identifiers, which is all a match can be"
        );

        // Every landing the walk can produce, in both directions and across the
        // wrap, is a message row rather than one of the separator rows.
        for key in ['n', 'n', 'n', 'N', 'N'] {
            app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
            let on = app.vim.cursor();
            assert!(
                !separators.contains(&rows::first_row_of_message(&layout, on).expect("a message")),
                "{key} landed on a separator row at message {on}"
            );
            assert!(
                layout.iter().any(|span| {
                    span.kind.index() == Some(on) && !span.first.eq(&separators[0])
                }),
                "{key} left the cursor on a message"
            );
        }
    }

    /// The landing positions are the messages themselves, which is the whole claim:
    /// walking a search crosses separators because it never had to stop on one.
    #[test]
    fn every_search_landing_is_the_position_of_a_message_it_matched() {
        let mut app = across_three_days();

        app.run_search("benchmarks");
        for _ in 0..5 {
            let on = app.vim.cursor();
            let id = app
                .conversation
                .window
                .get(on)
                .expect("the cursor names a message the window holds")
                .id;
            assert!(
                app.search().is_match(id),
                "the cursor is on message {id}, which matched"
            );
            app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        }
    }

    /// Every motion in the vocabulary lands on a message. The cursor counts
    /// messages, so this is structural — but a separator is a row on the screen
    /// and only a test proves the two never come apart (AC-21, US-X6).
    #[test]
    fn no_motion_lands_on_a_separator_row() {
        let mut app = across_three_days();
        let separator_rows: Vec<usize> = app
            .row_layout()
            .iter()
            .filter(|span| !span.kind.is_message())
            .map(|span| span.first)
            .collect();

        let keys = [
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            press_ctrl('d'),
            press_ctrl('u'),
            press_ctrl('d'),
        ];
        for key in keys {
            app.handle_key(key);
            let on = app.vim.cursor();
            assert!(
                app.conversation.window.get(on).is_some(),
                "the cursor at {on} names a message the window holds"
            );
            assert!(
                !separator_rows.contains(
                    &rows::first_row_of_message(&app.row_layout(), on)
                        .expect("the message is laid out")
                ),
                "and that message is not a separator row"
            );
        }
    }

    /// `gg` and `G` are the two ends of the window, and each is a message: the
    /// first message of the window is below the first day's separator, and the
    /// last is the last message rather than a row after it.
    #[test]
    fn the_ends_of_the_window_are_messages_and_not_separators() {
        let mut app = across_three_days();
        app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        assert_eq!(app.vim.cursor(), 0, "gg lands on the first message");

        app.handle_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE));
        assert_eq!(
            app.vim.cursor(),
            8,
            "and G on the newest, neither of which is a separator row"
        );
    }

    /// Grouping, separators and read state in one window, at the layer that
    /// draws them: three groups of three, a separator before each, and the newest
    /// group read.
    #[test]
    fn grouping_separators_and_read_state_agree_in_one_window() {
        let mut app = across_three_days();
        assert!(app.apply_update(&read(MOCK_CHAT, 109)));

        let layout = app.row_layout();
        assert_eq!(
            layout.len(),
            12,
            "nine messages and three separators: {:?}",
            layout.iter().map(|span| span.first).collect::<Vec<_>>()
        );
        // Each separator is the entry immediately before a message, so a day
        // opens with a message one row below its own separator.
        for separator in [0_usize, 4, 8] {
            assert!(
                !layout[separator].kind.is_message(),
                "row {separator} is the separator"
            );
            let under = &layout[separator + 1];
            assert_eq!(
                under.kind.index(),
                Some((separator / 4) * 3),
                "and the message below it opens that day"
            );
        }
        assert!(
            rows::group_of(&app, 0).first,
            "a day boundary is a group break as well as a separator"
        );
        assert_eq!(
            rows::group_of(&app, 8).receipt,
            rows::Receipt::Read,
            "and the newest group is read"
        );
        assert_eq!(
            rows::group_of(&app, 4).receipt,
            rows::Receipt::None,
            "while the first is not: the watermark covers only the last"
        );
    }

    /// Looking away and coming back leaves everything PR 8 derives from the window
    /// as it was, and restores the one thing the window does not carry (US-X1).
    ///
    /// The window itself is replaced by the switch, so grouping and the separators
    /// are worked out again from the same messages and the read state is put back
    /// from what the feed said — which is what makes this a test of the
    /// arrangement rather than of a value that never moved.
    #[test]
    fn grouping_separators_and_read_state_survive_a_switch_away_and_back() {
        let mut app = across_three_days();
        // Through the feed rather than the view's setter: what the feed says is
        // what survives the switch, and a figure nobody was told is not recorded.
        assert!(app.apply_update(&read(MOCK_CHAT, 109)));
        let before = app.row_layout();

        app.select_chat(1);
        app.select_chat(0);
        // The same messages, in the same order, as the conversation being reopened
        // is filled.
        app.conversation.window.replace((1..=9).map(message_of_day));
        app.vim.set_total(9);

        assert_eq!(
            app.row_layout(),
            before,
            "the same rows in the same places, worked out again rather than kept"
        );
        assert_eq!(
            app.conversation.read_watermark(),
            Some(109),
            "and the reading of the conversation, which the switch replaced"
        );
    }

    /// The messages [`across_three_days`] is built from, so a test can put the
    /// same conversation back after a switch without spelling them out twice.
    fn message_of_day(id: i64) -> Message {
        let day = (id - 1) / 3;
        Message {
            id: 100 + id,
            chat_id: MOCK_CHAT,
            text: if id % 2 == 0 {
                Cow::Borrowed("benchmarks and more")
            } else {
                Cow::Borrowed("text")
            },
            timestamp: 1_730_000_000 + day * 86_400 + (id - 1) % 3 * 60,
            status: MessageStatus::Sent,
            is_outgoing: true,
            reply_to: None,
            media: None,
        }
    }

    /// What PR 8 added to memory is bounded, and this is the part of AC-22 that
    /// can be asserted rather than audited.
    ///
    /// **No RSS claim is made.** The project has no measurement harness — no
    /// `heaptrack`, no `massif` target, and `docs/memory.md` records the 50 MB
    /// ceiling as unmeasured (OQ-09) — so what is checked here is the property
    /// that makes the ceiling plausible: nothing PR 8 added grows with history
    /// beyond the window that was already bounded.
    ///
    /// Two claims, then: the layout is rebuilt every frame rather than kept, and
    /// what it holds is one entry per message plus one per day; and the read state
    /// is a single number per conversation however many acknowledgements arrive.
    #[test]
    fn what_pr_eight_holds_is_bounded_by_the_window_and_the_conversation_count() {
        let mut app = App::mock();
        app.record_body(53);

        // A full window with a day per message: the worst case for separators,
        // and still one row each.
        app.conversation
            .window
            .replace((0..CONVERSATION_WINDOW).map(|day| {
                let day = i64::try_from(day).expect("a window index fits a timestamp");
                Message {
                    id: 1_000 + day,
                    chat_id: MOCK_CHAT,
                    text: Cow::Borrowed("text"),
                    timestamp: 1_730_000_000 + day * 86_400,
                    status: MessageStatus::Sent,
                    is_outgoing: true,
                    reply_to: None,
                    media: None,
                }
            }));
        app.vim.set_total(CONVERSATION_WINDOW);

        let layout = app.row_layout();
        assert_eq!(
            layout.len(),
            CONVERSATION_WINDOW * 2,
            "one entry per message and one separator per day, and nothing else"
        );
        assert_eq!(
            rows::total_rows(&layout),
            CONVERSATION_WINDOW * 2,
            "and one row each: no message grew and no separator did"
        );

        // Read state: one number per conversation, whatever arrives.
        for chat_id in [MOCK_CHAT, MOCK_CHAT + 1] {
            for max_id in [1, 5, 3, 9] {
                let _ = app.apply_update(&read(chat_id, max_id));
            }
        }
        assert_eq!(
            app.read_receipts.borrow().len(),
            2,
            "four acknowledgements for each of two conversations, and two numbers"
        );
    }

    /// An event for a conversation the client does not hold has nowhere to go:
    /// neither window can apply it, so nothing observable moved.
    #[test]
    fn an_arrival_for_an_unknown_conversation_changes_nothing() {
        let mut app = App::mock();

        assert!(!app.apply_update(&UpdateEvent::NewMessage(unknown(11))));
        assert_eq!(app.conversation.window.len(), 10);
    }

    /// A conversation other than the one on show is still one the list holds,
    /// so the arrival reaches the list and stops there.
    #[test]
    fn an_arrival_for_another_conversation_reaches_the_list_alone() {
        let mut app = App::mock();
        let before = app.conversation.window.len();

        assert!(
            app.apply_update(&UpdateEvent::NewMessage(stranger(11))),
            "the list holds the conversation the message belongs to"
        );
        assert_eq!(
            app.conversation.window.len(),
            before,
            "but the window on show is a different conversation"
        );
    }

    #[test]
    fn an_edit_reaches_the_open_conversation() {
        let mut app = App::mock();
        let edit = UpdateEvent::MessageEdited {
            chat_id: MOCK_CHAT,
            message_id: 3,
            new_text: Cow::Borrowed("corrected"),
        };

        assert!(app.apply_update(&edit));
        assert_eq!(text_of(&app, 3), Some("corrected"));
        assert!(
            !app.apply_update(&edit),
            "the same text twice is not a change"
        );
    }

    #[test]
    fn a_deletion_takes_the_message_out_of_the_open_conversation() {
        let mut app = App::mock();

        assert!(app.apply_update(&UpdateEvent::MessagesDeleted {
            message_ids: vec![3],
        }));

        assert_eq!(text_of(&app, 3), None);
        assert_eq!(app.conversation.window.len(), 9);
        assert_eq!(
            reading(&app),
            Some(10),
            "the reader was on the newest message and still is"
        );
    }

    /// Three same-side messages a minute apart: one group, three identifiers.
    fn a_group_of_three() -> Vec<Message> {
        [0, 60, 120]
            .into_iter()
            .map(|seconds| Message {
                id: 100 + seconds,
                chat_id: MOCK_CHAT,
                text: Cow::Borrowed("text"),
                timestamp: 1_730_000_000 + seconds,
                status: MessageStatus::Sent,
                is_outgoing: true,
                reply_to: None,
                media: None,
            })
            .collect()
    }

    /// Where each message of the window stands in its group.
    fn places(app: &App) -> Vec<rows::Grouped> {
        (0..app.conversation.window.len())
            .map(|index| rows::group_of(app, index))
            .collect()
    }

    /// An edit is a new text for a message that is already there, so it moves no
    /// group boundary: membership is decided by who is talking, when, and what
    /// the message is about — none of which an edit touches.
    #[test]
    fn an_edit_inside_a_group_changes_no_group_boundary() {
        let mut app = App::mock();
        app.conversation.window.replace(a_group_of_three());
        app.record_body(53);

        assert!(app.apply_update(&UpdateEvent::MessageEdited {
            chat_id: MOCK_CHAT,
            message_id: 160,
            new_text: Cow::Borrowed("corrected"),
        }));

        assert_eq!(text_of(&app, 160), Some("corrected"));
        assert_eq!(
            places(&app),
            vec![
                rows::Grouped {
                    first: true,
                    last: false,
                    receipt: rows::Receipt::None
                },
                rows::Grouped {
                    first: false,
                    last: false,
                    receipt: rows::Receipt::None
                },
                rows::Grouped {
                    first: false,
                    last: true,
                    receipt: rows::Receipt::None
                },
            ],
            "the group is the same one it was"
        );
    }

    /// A deletion takes the message out and leaves the survivors grouped; a
    /// group with nothing left in it leaves no row behind either.
    #[test]
    fn a_deletion_inside_a_group_leaves_the_survivors_grouped() {
        let mut app = App::mock();
        app.conversation.window.replace(a_group_of_three());
        app.record_body(53);

        assert!(app.apply_update(&UpdateEvent::MessagesDeleted {
            message_ids: vec![160],
        }));

        assert_eq!(
            places(&app),
            vec![
                rows::Grouped {
                    first: true,
                    last: false,
                    receipt: rows::Receipt::None
                },
                rows::Grouped {
                    first: false,
                    last: true,
                    receipt: rows::Receipt::None
                },
            ],
            "the two that are left are still one group"
        );
        let layout = app.row_layout();
        assert_eq!(layout.len(), 3, "two messages and the day's separator");
        assert_eq!(
            rows::total_rows(&layout),
            3,
            "and no row for what was deleted"
        );

        // An emptied group leaves nothing at all: no entry of its own, and no
        // rows for the scrollbar to count.
        assert!(app.apply_update(&UpdateEvent::MessagesDeleted {
            message_ids: vec![100, 220],
        }));
        assert!(app.row_layout().is_empty());
        assert_eq!(rows::total_rows(&app.row_layout()), 0);
    }

    // ---- fetching ------------------------------------------------------

    /// The margin is what stops a fetch from being asked for at every
    /// keystroke: near an end, once, and not again while one is in flight.
    #[test]
    fn a_page_is_asked_for_near_an_end_and_not_before() {
        let mut app = App::mock();
        app.apply_latest(page(&(1..=60).collect::<Vec<_>>()));

        assert!(
            !app.wants_older(),
            "the reader is at the end, not the start"
        );
        assert!(
            !app.wants_newer(),
            "a view pinned to the newest message has nothing to catch up on"
        );

        app.handle_key(press(KeyCode::Char('k')));
        assert!(!app.wants_older());
        assert!(
            app.wants_newer(),
            "the reader has stepped away from the end"
        );

        go_to_top(&mut app);
        assert!(
            app.wants_older(),
            "the reader is at the top of what is loaded"
        );
        assert!(!app.wants_newer());
    }

    /// The margin is counted in rows, which is what a reader scrolling upwards
    /// is counting. Twenty messages that came to fill four rows each is eighty
    /// rows of conversation, and a reader on the fourth of them is nowhere near
    /// the top of it.
    #[test]
    fn a_page_is_asked_for_by_rows_rather_than_by_messages() {
        let mut app = App::mock();
        app.record_body(53);
        app.apply_latest(tall_page(10));
        app.vim.set_cursor(3);

        assert!(
            !app.wants_older(),
            "message 4 begins at row {}, and what is in front of it is a screenful of text rather than one line of window",
            rows::first_row_of_message(&app.row_layout(), 3).expect("the message is laid out")
        );

        app.vim.set_cursor(0);
        assert!(
            app.wants_older(),
            "and the reader on the first message is near the top of both"
        );
    }

    #[test]
    fn a_fetch_in_flight_is_not_asked_for_twice() {
        let mut app = App::mock();
        assert!(
            app.wants_older(),
            "a window shorter than the margin is near its start"
        );

        app.begin_fetch(FetchDirection::Older);
        assert!(app.is_fetching(FetchDirection::Older));
        assert!(!app.wants_older(), "one page per direction at a time");

        app.end_fetch(FetchDirection::Older);
        assert!(!app.is_fetching(FetchDirection::Older));
        assert!(app.wants_older(), "the direction is open again");
    }

    /// The directions are tracked apart, so a page in flight in one of them
    /// does not hold up the others.
    #[test]
    fn the_directions_are_tracked_apart() {
        let mut app = App::mock();

        app.begin_fetch(FetchDirection::Latest);
        app.begin_fetch(FetchDirection::Older);

        assert!(app.is_fetching(FetchDirection::Latest));
        assert!(app.is_fetching(FetchDirection::Older));
        assert!(!app.is_fetching(FetchDirection::Newer));

        app.end_fetch(FetchDirection::Latest);
        assert!(!app.is_fetching(FetchDirection::Latest));
        assert!(
            app.is_fetching(FetchDirection::Older),
            "releasing one says nothing about the rest"
        );
    }

    /// Opening another conversation forgets what was in flight for the old one:
    /// the page is coming for a window that is no longer on screen.
    #[test]
    fn opening_a_conversation_forgets_what_was_in_flight() {
        let mut app = App::mock();
        app.begin_fetch(FetchDirection::Latest);
        app.begin_fetch(FetchDirection::Older);

        app.select_chat(1);

        for direction in [
            FetchDirection::Latest,
            FetchDirection::Older,
            FetchDirection::Newer,
        ] {
            assert!(
                !app.is_fetching(direction),
                "{direction:?} is still in flight"
            );
        }
    }

    /// A conversation with nothing loaded has no message to count a page from,
    /// so the newest page is the only one it can be given.
    #[test]
    fn an_empty_conversation_is_near_neither_of_its_ends() {
        let mut app = App::mock();
        app.select_chat(1);

        assert!(app.conversation.window.is_empty());
        assert!(!app.wants_older());
        assert!(!app.wants_newer());
    }

    #[test]
    fn a_direction_the_conversation_has_run_out_of_is_not_asked_for() {
        let mut app = App::mock();
        assert!(app.wants_older());

        app.exhaust(FetchDirection::Older);
        assert!(
            !app.wants_older(),
            "there is nothing in front of the oldest message"
        );

        app.exhaust(FetchDirection::Newer);
        assert!(!app.wants_newer(), "nor behind the newest one");
    }

    // ---- `gg` and the unread messages ----------------------------------

    /// `gg` is Vim's top-of-buffer when there is nothing unread to be taken to,
    /// which is the conversation the reader is already in.
    #[test]
    fn gg_with_nothing_unread_is_the_top_of_the_window() {
        let mut app = App::mock();

        go_to_top(&mut app);

        assert_eq!(app.vim.cursor(), 0);
        assert_eq!(app.pending_jump(), None, "there is nowhere to be taken to");
        assert!(!app.conversation.auto_follow());
    }

    #[test]
    fn gg_with_no_conversation_open_moves_nothing() {
        let mut app = App::new();

        go_to_top(&mut app);

        assert_eq!(app.vim.cursor(), 0);
        assert_eq!(app.pending_jump(), None);
    }

    /// The unread messages are the newest ones there are, so a window that ends
    /// where the conversation does holds them: `gg` lands on the first of them
    /// without a round trip.
    #[test]
    fn gg_with_unread_loaded_lands_on_the_first_of_them() {
        let mut app = with_unread(2, 10);

        go_to_top(&mut app);

        assert_eq!(
            reading(&app),
            Some(9),
            "the newest message is 10, and two of them are unread"
        );
        assert_eq!(app.pending_jump(), None, "so no page was needed");
        assert!(
            !app.conversation.auto_follow(),
            "the reader moved off the end"
        );
    }

    /// Identifiers have gaps wherever messages were deleted, so counting back
    /// from the newest by number can name a message that does not exist. A window
    /// that ends where the conversation does is the exception: the unread
    /// messages are the newest ones there are, so they are counted back by
    /// position and land exactly.
    #[test]
    fn a_window_that_ends_the_conversation_lands_where_the_numbers_do_not() {
        let mut app = with_unread(3, 20);
        app.apply_latest(page(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 20]));

        go_to_top(&mut app);

        assert_eq!(
            reading(&app),
            Some(8),
            "the third from the end — counting back three from 20 would name 18, \
             which is not a message this conversation has"
        );
        assert_eq!(app.pending_jump(), None);
    }

    /// Counting from the end is only an answer when the window reaches the end,
    /// and only when the unread messages fit inside it.
    #[test]
    fn counting_from_the_end_needs_a_window_that_holds_the_unread_ones() {
        assert_eq!(landing_position(10, 0), None, "nothing unread");
        assert_eq!(landing_position(10, 3), Some(7));
        assert_eq!(landing_position(3, 3), Some(0), "the whole window");
        assert_eq!(
            landing_position(3, 4),
            None,
            "they reach past the window, so counting them from the end would land \
             on a message that is not one of them"
        );
        assert_eq!(landing_position(0, 1), None, "and an empty window");
    }

    /// A target the window does not hold is handed to the caller, and asking
    /// again while it is on its way produces the same intent rather than another
    /// one: holding the key must not stack requests.
    #[test]
    fn a_jump_the_window_cannot_answer_is_asked_for_once() {
        let mut app = with_unread_out_of_reach(2);

        go_to_top(&mut app);

        let expected = Jump {
            peer_id: MOCK_CHAT,
            target_id: 19,
            kind: JumpKind::Unread,
        };
        assert_eq!(
            app.pending_jump(),
            Some(expected),
            "counting back two from 20"
        );

        go_to_top(&mut app);
        assert_eq!(app.pending_jump(), Some(expected), "the same place, once");
    }

    /// The completion puts the reader on the message they jumped to, and the
    /// window it landed in is surrounded by the unknown on both sides.
    #[test]
    fn a_jump_lands_the_reader_on_the_message_it_was_for() {
        let mut app = with_unread_out_of_reach(2);
        go_to_top(&mut app);
        assert!(app.pending_jump().is_some());

        assert!(app.apply_jump(&page(&[16, 17, 18, 19, 20]), 19));

        assert_eq!(reading(&app), Some(19));
        assert_eq!(app.pending_jump(), None, "the jump is over");
        assert!(!app.conversation.auto_follow());
        assert!(
            !app.conversation.window.exhausted_older && !app.conversation.window.exhausted_newer,
            "a window that jumped has no edge the one before it can vouch for"
        );
    }

    /// A page that does not hold the target: the reader is put on the first
    /// message after it, which is the nearest the page came.
    #[test]
    fn a_jump_that_missed_its_target_lands_on_the_nearest_message_after_it() {
        let mut app = with_unread_out_of_reach(2);
        go_to_top(&mut app);

        assert!(app.apply_jump(&page(&[16, 17, 20, 21]), 19));

        assert_eq!(reading(&app), Some(20));
    }

    /// And an estimate past everything the page holds lands on the newest of it:
    /// an estimate that outran the conversation, which the nearest survivor
    /// answers honestly.
    #[test]
    fn a_jump_past_the_page_lands_on_its_newest_message() {
        let mut app = with_unread_out_of_reach(2);
        go_to_top(&mut app);

        assert!(app.apply_jump(&page(&[1, 2, 3]), 19));

        assert_eq!(reading(&app), Some(3));
    }

    /// However it ended, the jump is over: an empty page leaves the reader where
    /// they were rather than wedging the key.
    #[test]
    fn a_jump_that_came_back_empty_leaves_the_reader_where_they_were() {
        let mut app = with_unread_out_of_reach(2);
        go_to_top(&mut app);
        let before = app.conversation.window.len();

        assert!(!app.apply_jump(&[], 19));

        assert_eq!(app.pending_jump(), None, "the key is free again");
        assert_eq!(
            app.conversation.window.len(),
            before,
            "and the window is untouched"
        );
    }

    /// A page for a jump the reader has abandoned: opening another conversation
    /// is the reader saying they are no longer going there.
    #[test]
    fn a_jump_for_a_conversation_that_is_no_longer_open_is_dropped() {
        let mut app = with_unread_out_of_reach(2);
        go_to_top(&mut app);
        assert!(app.pending_jump().is_some());

        app.select_chat(1);

        assert!(!app.apply_jump(&page(&[16, 17, 18, 19, 20]), 19));
        assert_eq!(app.pending_jump(), None);
        assert!(
            app.conversation.window.is_empty(),
            "the conversation that was opened kept its empty window"
        );
    }

    /// A page naming another conversation is refused even when the target
    /// matches: a window belongs to one conversation.
    #[test]
    fn a_jump_page_for_another_conversation_is_refused() {
        let mut app = with_unread_out_of_reach(2);
        go_to_top(&mut app);
        let before = app.conversation.window.len();

        assert!(!app.apply_jump(&[stranger(19)], 19));

        assert_eq!(app.conversation.window.len(), before);
        assert_eq!(app.pending_jump(), None, "and the jump is over");
    }

    /// A page for a target nobody is waiting for: the reader asked for one
    /// place, and the fetch that comes back is for another.
    #[test]
    fn a_jump_page_for_another_target_is_refused() {
        let mut app = with_unread_out_of_reach(2);
        go_to_top(&mut app);
        let before = app.conversation.window.len();

        assert!(!app.apply_jump(&page(&[16, 17, 18, 19, 20]), 18));

        assert_eq!(app.conversation.window.len(), before);
        assert_eq!(
            app.pending_jump(),
            Some(Jump {
                peer_id: MOCK_CHAT,
                target_id: 19,
                kind: JumpKind::Unread,
            }),
            "the jump the reader did ask for is still the one being waited on"
        );
    }

    /// While a jump is on its way only `Esc` answers: a key that moved the cursor
    /// would move it out from under the page that is coming, so `G` no longer
    /// gets to say "take me to the end instead". `Esc` drops the jump and leaves
    /// the reader where they were, and the page is dropped when it lands.
    #[test]
    fn escape_is_the_only_answer_while_a_jump_is_in_flight() {
        let mut app = with_unread_out_of_reach(2);
        go_to_top(&mut app);
        assert!(app.pending_jump().is_some());
        let before = app.vim.cursor();

        app.handle_key(press(KeyCode::Char('G')));

        assert_eq!(
            app.pending_jump().map(|jump| jump.target_id),
            Some(19),
            "another key is swallowed rather than answered"
        );
        assert_eq!(app.vim.cursor(), before, "and moves nothing");

        app.handle_key(press(KeyCode::Esc));
        assert_eq!(app.pending_jump(), None, "`Esc` drops the jump");
        assert_eq!(app.vim.cursor(), before, "and leaves the reader put");
        assert!(
            !app.apply_jump(&page(&[16, 17, 18, 19, 20]), 19),
            "the page that was on its way has nobody waiting for it"
        );
    }

    /// `G` after the jump is over still means what it always meant.
    #[test]
    fn the_end_of_the_conversation_still_follows_the_reader() {
        let mut app = with_unread_out_of_reach(2);
        go_to_top(&mut app);
        go_to_top(&mut app);
        assert!(app.pending_jump().is_some());

        app.handle_key(press(KeyCode::Esc));

        app.handle_key(press(KeyCode::Char('G')));
        assert!(app.conversation.auto_follow());
    }

    /// The contrapositive of what used to hold: a window that was replaced no
    /// longer invalidates the match list, because a match is a message
    /// identifier rather than a position in the window that was on screen.
    #[test]
    fn a_jump_keeps_the_match_list() {
        let mut app = with_unread_out_of_reach(2);
        run_search_line(&mut app, "text");
        assert!(
            app.search().is_match(3),
            "the loaded window matched message 3"
        );

        go_to_top(&mut app);
        assert!(app.apply_jump(&page(&[16, 17, 18, 19, 20]), 19));

        assert!(app.search().is_active(), "the search survives the jump");
        assert!(
            app.search().is_match(3),
            "and still remembers the places it found"
        );
    }

    /// The other half: loading the newest page replaces the window, and the
    /// match list is places, which survive that too.
    #[test]
    fn a_latest_page_keeps_the_match_list() {
        let mut app = App::mock();
        run_search_line(&mut app, "benchmarks");
        assert!(app.search().is_match(7), "the sample match is message 7");

        assert!(app.apply_latest(page(&[5, 6, 7, 8, 9, 10])));

        assert!(app.search().is_active());
        assert!(app.search().is_match(7));
    }

    /// The test that fails the moment someone puts a `clear` back into
    /// `apply_jump`: `n` walks across the boundary instead of restarting.
    #[test]
    fn n_crosses_a_jump_boundary() {
        let mut app = with_unread_out_of_reach(2);
        run_search_line(&mut app, "text");
        assert!(app.apply_searched(MOCK_CHAT, "text", vec![3, 7, 19, 25], 4));
        assert_eq!(
            reading(&app),
            Some(3),
            "the walk starts at the oldest match"
        );

        app.handle_key(press(KeyCode::Char('n')));
        assert_eq!(reading(&app), Some(7), "a loaded match is a cursor move");

        app.handle_key(press(KeyCode::Char('n')));
        assert_eq!(
            app.pending_jump(),
            Some(Jump {
                peer_id: MOCK_CHAT,
                target_id: 19,
                kind: JumpKind::Unread,
            }),
            "an unloaded match is a jump, the same path `gg` takes"
        );

        assert!(app.apply_jump(&page(&[19, 20, 21, 22, 23, 24, 25]), 19));
        assert_eq!(reading(&app), Some(19));

        app.handle_key(press(KeyCode::Char('n')));
        assert_eq!(
            reading(&app),
            Some(25),
            "the walk continues from the jumped-to match, not from the top"
        );
    }

    #[test]
    fn a_jump_in_flight_is_what_the_status_line_says() {
        let mut app = with_unread_out_of_reach(2);
        app.status = "3 conversation(s)".to_string();

        assert_eq!(app.status_text(), "3 conversation(s)");

        go_to_top(&mut app);
        assert_eq!(app.status_text(), JUMP_LABEL);

        app.apply_jump(&page(&[16, 17, 18, 19, 20]), 19);
        assert_eq!(
            app.status_text(),
            "3 conversation(s)",
            "the line goes back to what it was saying once the jump is over"
        );
    }

    // ---- `gd`, the message a reply quotes ------------------------------

    /// A message of the sample conversation that quotes `reply_to`.
    fn reply(id: i64, reply_to: i64) -> Message {
        Message {
            reply_to: Some(reply_to),
            ..message(id, "text")
        }
    }

    /// A window whose middle message quotes 19, which is nowhere near it.
    fn with_a_reply_to_19() -> App {
        let mut app = App::mock();
        app.apply_latest(vec![message(1, "text"), reply(2, 19), message(3, "text")]);
        app.handle_key(press(KeyCode::Char('k')));
        app
    }

    /// `gd`, as a reader types it.
    fn go_to_reply(app: &mut App) {
        app.handle_key(press(KeyCode::Char('g')));
        app.handle_key(press(KeyCode::Char('d')));
    }

    /// A window whose middle message quotes the one before it, which is on
    /// screen: `gd` here is a cursor move and nothing else, and `Ctrl-o` is the
    /// way back.
    fn with_a_loaded_quote() -> App {
        let mut app = App::mock();
        app.apply_latest(vec![message(1, "text"), reply(2, 1), message(3, "text")]);
        app.handle_key(press(KeyCode::Char('k')));
        app
    }

    /// A quote the window already holds is a cursor move and nothing else: a
    /// round trip for a message on screen would put the reader through the same
    /// window twice to arrive where they already were.
    #[test]
    fn gd_on_a_loaded_quote_is_a_cursor_move() {
        let mut app = App::mock();
        app.apply_latest(vec![message(1, "text"), reply(2, 1), message(3, "text")]);
        app.handle_key(press(KeyCode::Char('k')));
        assert_eq!(reading(&app), Some(2), "the reply is under the cursor");

        go_to_reply(&mut app);

        assert_eq!(
            reading(&app),
            Some(1),
            "and the reader is on what it quotes"
        );
        assert_eq!(
            app.pending_jump(),
            None,
            "so nothing was asked of the network"
        );
    }

    /// A quote the window does not hold is the same jump `gg` makes, and says so.
    #[test]
    fn gd_on_a_quote_out_of_the_window_asks_for_it() {
        let mut app = with_a_reply_to_19();

        go_to_reply(&mut app);

        assert_eq!(
            app.pending_jump(),
            Some(Jump {
                peer_id: MOCK_CHAT,
                target_id: 19,
                kind: JumpKind::Reply,
            })
        );
    }

    /// A message that quotes nothing has nowhere to go, and the refusal says
    /// which key would have gone somewhere.
    #[test]
    fn gd_on_a_message_that_quotes_nothing_refuses() {
        let mut app = App::mock();
        app.apply_latest(page(&[1, 2, 3]));
        app.handle_key(press(KeyCode::Char('k')));

        go_to_reply(&mut app);

        assert_eq!(app.status_text(), NOT_A_REPLY);
        assert_eq!(app.pending_jump(), None);
        assert_eq!(reading(&app), Some(2), "and the reader stays put");
    }

    /// A quote the client does not hold at all: the fetch came back with
    /// nothing, so there is no page to land in and the reader is told why the
    /// jump ended rather than left waiting for a key that will never work again.
    #[test]
    fn a_reply_jump_that_came_back_empty_says_the_message_is_gone() {
        let mut app = with_a_reply_to_19();
        go_to_reply(&mut app);
        let before = app.conversation.window.len();

        assert!(!app.apply_jump(&[], 19));

        assert_eq!(app.status_text(), JUMP_UNAVAILABLE);
        assert_eq!(app.pending_jump(), None, "and the key is free again");
        assert_eq!(app.conversation.window.len(), before);
    }

    /// The label names where the reader is going, so it differs by jump: a jump
    /// to a reply is not a jump to the first unread message, and saying so is
    /// the difference between a fetch that was asked for and one that was not.
    #[test]
    fn the_label_says_which_jump_is_in_flight() {
        let mut app = with_unread_out_of_reach(2);

        go_to_top(&mut app);
        assert_eq!(app.status_text(), JUMP_LABEL, "`gg` is the first unread");

        app.handle_key(press(KeyCode::Esc));

        let mut app = with_a_reply_to_19();
        go_to_reply(&mut app);
        assert_eq!(app.status_text(), JUMP_REPLY_LABEL);
        assert_eq!(
            app.jump_label(),
            JUMP_REPLY_LABEL,
            "the panel says the same"
        );
    }

    // ---- `Ctrl-o` and `Ctrl-i`, back and forward -------------------------

    /// A reply jump the window can answer, remembered: `gd` on a quote that is
    /// on screen still moves the reader away from where they were standing, and
    /// `Ctrl-o` is how they get back.
    #[test]
    fn a_reply_jump_records_where_the_reader_was() {
        let mut app = with_a_loaded_quote();

        go_to_reply(&mut app);
        assert_eq!(reading(&app), Some(1), "the reader is on the quote");

        app.handle_key(press_ctrl('o'));

        assert_eq!(reading(&app), Some(2), "and back where they were standing");
        assert_eq!(app.pending_jump(), None, "which needed no page");
    }

    /// And forward again, which is the other half of the same walk.
    #[test]
    fn ctrl_i_takes_the_reader_forward_again() {
        let mut app = with_a_loaded_quote();
        go_to_reply(&mut app);
        app.handle_key(press_ctrl('o'));

        app.handle_key(press_ctrl('i'));

        assert_eq!(reading(&app), Some(1), "back to the quoted message");
        assert_eq!(app.pending_jump(), None);
    }

    /// The jump that took the reader away replaced the window, so the mark they
    /// left behind is not on screen any more: a return is then a jump of its own,
    /// on the same terms as the one they asked for, and it says so.
    #[test]
    fn a_return_across_a_replaced_window_is_a_jump_of_its_own() {
        let mut app = with_a_reply_to_19();
        go_to_reply(&mut app);
        assert!(app.apply_jump(&page(&[16, 17, 18, 19, 20]), 19));

        app.handle_key(press_ctrl('o'));

        assert_eq!(
            app.pending_jump(),
            Some(Jump {
                peer_id: MOCK_CHAT,
                target_id: 2,
                kind: JumpKind::Back,
            }),
            "message 2 is not in the window the jump replaced"
        );
        assert_eq!(app.status_text(), JUMP_BACK_LABEL);

        assert!(app.apply_jump(&page(&[1, 2, 3, 4, 5]), 2));
        assert_eq!(reading(&app), Some(2), "the reader is back where they were");

        app.handle_key(press_ctrl('i'));
        assert_eq!(
            app.pending_jump(),
            Some(Jump {
                peer_id: MOCK_CHAT,
                target_id: 19,
                kind: JumpKind::Forward,
            })
        );
        assert_eq!(app.status_text(), JUMP_FORWARD_LABEL);
    }

    /// One fetch at a time: a return asked for while a page is on its way is
    /// swallowed rather than replacing the jump the reader is still waiting for.
    #[test]
    fn a_return_asked_for_while_a_jump_is_on_its_way_is_ignored() {
        let mut app = with_a_reply_to_19();
        go_to_reply(&mut app);
        let waiting = app.pending_jump();

        app.handle_key(press_ctrl('o'));
        assert_eq!(app.pending_jump(), waiting, "the key is swallowed");

        app.jump_back();
        assert_eq!(
            app.pending_jump(),
            waiting,
            "and calling it directly does not walk the list either"
        );
    }

    /// A placeholder is a message the server has not seen, so a jump to one has
    /// nothing to fetch: the reader is told the message cannot be reached and
    /// left where they were, rather than watching a page arrive for an id that
    /// does not exist. Recorded as a known limitation rather than fixed — a
    /// placeholder has no server-side identity to fetch around.
    #[test]
    fn a_return_to_a_placeholder_says_so_and_leaves_the_reader_put() {
        let mut app = App::mock();
        // Numbered below zero, which is what an outgoing message looks like
        // before the server has given it an id. The window is in message order,
        // so the placeholder is the oldest row and two steps up from the newest.
        app.apply_latest(vec![reply(-7, 19), message(1, "text"), message(3, "text")]);
        app.handle_key(press(KeyCode::Char('k')));
        app.handle_key(press(KeyCode::Char('k')));
        go_to_reply(&mut app);
        assert!(app.apply_jump(&page(&[16, 17, 18, 19, 20]), 19));
        assert_eq!(reading(&app), Some(19));

        app.handle_key(press_ctrl('o'));
        assert_eq!(
            app.pending_jump().map(|jump| jump.target_id),
            Some(-7),
            "it was asked for: nothing here can say it will not land"
        );

        assert!(!app.apply_jump(&[], -7));
        assert_eq!(app.status_text(), JUMP_UNAVAILABLE);
        assert_eq!(reading(&app), Some(19), "and the reader is where they were");
    }

    /// Q5: `Ctrl-i` is the same byte as `Tab` on a terminal that does not report
    /// modifiers. Bare `Tab` stays the pane switch — and stays *only* that, so
    /// forward navigation is unreachable there and `Ctrl-o` carries the criterion
    /// alone.
    #[test]
    fn a_bare_tab_cycles_panes_and_is_not_forward_navigation() {
        let mut app = with_a_loaded_quote();
        go_to_reply(&mut app);
        app.handle_key(press_ctrl('o'));
        assert_eq!(app.focus, Focus::Conversation);

        app.handle_key(press(KeyCode::Tab));

        assert_eq!(
            app.focus,
            Focus::Input,
            "`Tab` is the pane switch, whatever byte a terminal sent"
        );
        assert_eq!(reading(&app), Some(2), "and it walked nothing");

        app.handle_key(press(KeyCode::BackTab));
        assert_eq!(app.focus, Focus::Conversation);
        assert_eq!(reading(&app), Some(2), "nor did the other way");

        app.handle_key(press_ctrl('i'));
        assert_eq!(reading(&app), Some(1), "a reported `Ctrl-i` still walks");
    }

    // ---- the :shortcode completion --------------------------------------

    /// An application composing `draft`, which opens a completion if it names a
    /// shortcode.
    fn typing(draft: &str) -> App {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('i')));
        type_text(&mut app, draft);
        app
    }

    #[test]
    fn a_shortcode_opens_the_completion() {
        let app = typing(":cr");

        let trigger = app.completion().expect("a completion is up");
        assert_eq!(trigger.query, "cr");
        assert_eq!(
            trigger.chosen().and_then(|emoji| emoji.shortcode()),
            Some("cry"),
            "the shortest prefix is the top candidate"
        );
    }

    #[test]
    fn a_character_keeps_filtering_and_keeps_the_popup_open() {
        let app = typing(":cry");

        let trigger = app.completion().expect("a completion is up");
        assert!(
            trigger
                .candidates
                .iter()
                .all(|emoji| emoji.shortcode().is_some_and(|code| code.contains("cry"))),
            "a candidate is on the list without matching the query"
        );
    }

    /// `j` and `k` are letters, not candidate motion: binding them would make
    /// `:joy` and `:jack_o_lantern` untypable, which is the feature refusing to
    /// work.
    #[test]
    fn j_and_k_are_still_letters_while_the_popup_is_open() {
        // `k` after `:o` still matches (`ok_hand`), so the popup is up on both
        // sides of the key — the case where a candidate motion would be reached.
        let mut app = typing(":o");
        assert!(app.completion().is_some(), "`:o` opens it");

        app.handle_key(press(KeyCode::Char('k')));

        assert_eq!(app.line.text(), ":ok", "`k` went to the draft");
        let trigger = app.completion().expect("and the list keeps filtering");
        assert_eq!(trigger.query, "ok");
        assert_eq!(trigger.selected, 0, "and `k` did not move the candidate");

        // `j` is the same key one row over. What matters is that the letter
        // reached the draft rather than being taken as motion.
        let mut app = typing(":cr");
        app.handle_key(press(KeyCode::Char('j')));

        assert_eq!(app.line.text(), ":crj", "`j` went to the draft too");
    }

    #[test]
    fn the_arrows_move_the_candidate_and_wrap_around_it() {
        let mut app = typing(":cry");
        assert_eq!(app.completion().expect("up").candidates.len(), 3);

        for _ in 0..3 {
            app.handle_key(press(KeyCode::Down));
        }
        assert_eq!(
            app.completion().expect("up").selected,
            0,
            "three downs over three candidates wrapped"
        );

        app.handle_key(press(KeyCode::Up));
        assert_eq!(app.completion().expect("up").selected, 2, "and up wrapped");
    }

    #[test]
    fn the_arrows_are_caret_motions_again_once_it_is_closed() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('i')));
        type_text(&mut app, "hello");
        app.handle_key(press_ctrl('j'));
        type_text(&mut app, ":cr");
        assert!(app.completion().is_some(), "there is one to close");

        app.handle_key(press(KeyCode::Esc));
        assert!(app.completion().is_none());

        app.handle_key(press(KeyCode::Up));

        assert_eq!(app.line.caret(), 3, "the arrow moved the caret");
        assert_eq!(
            app.line.laid_out(78).row,
            0,
            "to the first row of the draft, not to a candidate"
        );
    }

    #[test]
    fn tab_accepts_the_candidate() {
        let mut app = typing(":cr");

        app.handle_key(press(KeyCode::Tab));

        assert_eq!(app.line.text(), "😢");
        assert_eq!(app.line.caret(), 4, "after the glyph");
        assert!(app.completion().is_none(), "and the popup is away");
    }

    /// Two presses fifty milliseconds apart are accept-then-send, which is what
    /// a reader who typed `:cry` and mashed `Enter` wanted.
    #[test]
    fn enter_accepts_the_candidate_and_the_next_enter_sends() {
        let mut app = typing(":cry");

        app.handle_key(press(KeyCode::Enter));

        assert_eq!(app.take_action(), None, "accepting did not send");
        assert_eq!(app.line.text(), "😢");
        assert_eq!(
            app.focus,
            Focus::Input,
            "and the reader is still in the line"
        );

        app.handle_key(press(KeyCode::Enter));

        assert_eq!(
            app.take_action(),
            Some(Action::Send {
                chat_id: MOCK_CHAT,
                temp_id: -1,
                text: "😢".to_owned(),
                reply_to: None,
            })
        );
    }

    #[test]
    fn escape_puts_the_completion_away_and_leaves_the_draft_alone() {
        let mut app = typing(":cry");

        app.handle_key(press(KeyCode::Esc));

        assert_eq!(app.line.text(), ":cry", "the words are the reader's");
        assert!(app.completion().is_none());
    }

    /// The popup's `Esc` is a third key in front of the line's own two-stage
    /// `Esc`, and it does not shorten the rule.
    #[test]
    fn escape_twice_leaves_the_line_as_it_did_before() {
        let mut app = typing(":cry");

        app.handle_key(press(KeyCode::Esc));
        assert_eq!(app.focus, Focus::Input, "the popup's escape only closes it");
        assert!(app.completion().is_none());

        app.handle_key(press(KeyCode::Esc));
        assert_eq!(
            app.focus,
            Focus::Input,
            "the line's own escape is still the first stage"
        );

        app.handle_key(press(KeyCode::Esc));
        assert_eq!(
            app.focus,
            Focus::Conversation,
            "and the second stage leaves"
        );
    }

    #[test]
    fn backspace_shortens_the_query_and_the_list_grows() {
        let mut app = typing(":cry");
        let before = app.completion().expect("up").candidates.len();

        app.handle_key(press(KeyCode::Backspace));

        let trigger = app.completion().expect("still open on a shorter query");
        assert_eq!(trigger.query, "cr");
        assert!(
            trigger.candidates.len() >= before,
            "{} < {before}",
            trigger.candidates.len()
        );
    }

    #[test]
    fn a_newline_ends_the_shortcode() {
        let mut app = typing(":cry");

        app.handle_key(press_ctrl('j'));

        assert!(app.completion().is_none());
        assert_eq!(app.line.text(), ":cry\n");
    }

    #[test]
    fn a_space_ends_the_shortcode() {
        let mut app = typing(":cry");

        app.handle_key(press(KeyCode::Char(' ')));

        assert!(app.completion().is_none(), "the query is now `cry `");
    }

    #[test]
    fn leaving_the_line_puts_the_completion_away() {
        let mut app = typing(":cry");

        app.handle_key(press_ctrl('w'));

        assert_eq!(app.focus, Focus::Conversation);
        assert!(app.completion().is_none());
    }

    #[test]
    fn a_command_line_never_completes() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char(':')));
        assert_eq!(app.line.purpose(), PromptKind::Command);

        type_text(&mut app, "cr");

        assert!(app.completion().is_none(), "a command is not a shortcode");
    }

    #[test]
    fn a_search_line_never_completes() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('/')));
        assert_eq!(app.line.purpose(), PromptKind::Search);

        type_text(&mut app, "cr");

        assert!(app.completion().is_none(), "a search is not a shortcode");
    }

    /// The gate is `is_buffer`, not `Message`: a reply and an edit are buffers
    /// too, and a reader answering either can name an emoji.
    #[test]
    fn a_reply_and_an_edit_do_complete() {
        let mut reply = App::mock();
        reply.handle_key(press(KeyCode::Char('r')));
        assert_eq!(reply.line.purpose(), PromptKind::Reply);
        type_text(&mut reply, ":cr");
        assert!(reply.completion().is_some(), "a reply completes");

        // Only the reader's own messages can be edited, and the sample
        // conversation's newest is not one of them.
        let mut edit = App::mock();
        edit.handle_key(press(KeyCode::Char('k')));
        edit.handle_key(press(KeyCode::Char('e')));
        assert_eq!(edit.line.purpose(), PromptKind::Edit);
        type_text(&mut edit, ":cr");
        assert!(edit.completion().is_some(), "an edit completes");
    }

    // ---- the sign-in field through the command that opens it ---------------

    /// `:signin` leaves the line holding the phone field, pre-filled, and keeps
    /// the keys going there.
    ///
    /// Driven through `run_command_line` rather than `begin_signin` on purpose:
    /// the field was being emptied and unfocused by `submit`'s reset, which only
    /// runs when the command is *run*, so a test that calls `begin_signin`
    /// directly walks straight past the bug. The symptom it caused is the whole
    /// surface locking: `handle_key` routes every key to `handle_signin` once
    /// `signin` is up, and its `Conversation` arm has nothing to say about a
    /// flow — so a wiped line also meant no digits and no `q`.
    #[test]
    fn the_signin_command_leaves_the_phone_field_open_and_typed_into() {
        let mut app = App::mock();
        run_command_line(&mut app, "signin");

        assert_eq!(app.signin_field(), Some(LoginField::Phone));
        assert_eq!(app.focus, Focus::Input, "the field is what has the keys");
        assert_eq!(
            app.line.text(),
            "+44 7700 900142",
            "the number the configuration carries is still in the bar"
        );

        app.handle_key(press(KeyCode::Char('4')));

        assert_eq!(
            app.line.text(),
            "+44 7700 9001424",
            "a digit lands in the field rather than being swallowed"
        );
    }

    /// And the step after it: `login_advanced` opens the code field and the
    /// focus with it, so the next thing the reader types is a code.
    #[test]
    fn the_code_step_after_the_phone_one_is_typed_into_too() {
        let mut app = App::mock();
        run_command_line(&mut app, "signin");
        app.login_advanced(
            domain::session::SessionState::AwaitingCode {
                phone: "+44 7700 900142".to_owned(),
            },
            None,
        );

        assert_eq!(app.signin_field(), Some(LoginField::Code));
        assert_eq!(app.focus, Focus::Input);

        app.handle_key(press(KeyCode::Char('4')));

        assert_eq!(app.line.text(), "4", "the code is what they are typing");
    }

    /// `Esc` at the phone step is `cancel`, which is what the hint calls it —
    /// so it takes the flow down rather than pausing into an overlay that
    /// swallows every key, `q` among them.
    #[test]
    fn escape_at_the_phone_step_takes_the_flow_down() {
        let mut app = App::mock();
        run_command_line(&mut app, "signin");
        assert!(app.signin().is_some(), "the flow is up to begin with");

        app.handle_key(press(KeyCode::Esc));

        assert!(app.signin().is_none(), "cancelled, not paused");
        assert_eq!(
            app.focus,
            Focus::Conversation,
            "and the keys are ours again"
        );
        // The consequence of not doing this: a paused flow swallowed every key.
        app.handle_key(press(KeyCode::Char('q')));
        assert!(
            app.confirm.is_some(),
            "`q` asks to quit, so it reached the app"
        );
    }

    /// `Esc` at the code step is a *different* thing, and stays one: Telegram
    /// has sent a code, so the step restarts at the phone and the status line
    /// says what that cost. The reader asked to sign in, not to stop.
    #[test]
    fn escape_at_the_code_step_still_restarts_at_the_phone() {
        let mut app = App::mock();
        run_command_line(&mut app, "signin");
        app.login_advanced(
            domain::session::SessionState::AwaitingCode {
                phone: "+44 7700 900142".to_owned(),
            },
            None,
        );

        app.handle_key(press(KeyCode::Esc));

        assert!(app.signin().is_some(), "a code was sent, so the flow stays");
        assert_eq!(app.signin_field(), Some(LoginField::Phone));
        assert_eq!(app.focus, Focus::Input);
        assert_eq!(
            app.status,
            "cancelling discards the code Telegram sent; ⏎ asks for a new one"
        );
    }

    // ---- the flow against a client, or the lack of one -------------------

    /// A `⏎` with no client up reports itself instead of claiming to be in
    /// flight.
    ///
    /// The flag is the whole subject: with nothing to carry the request, a
    /// `waiting` of `true` is a panel saying "Checking…" for an answer that is
    /// never coming, and the guard behind it then swallows every later `⏎` as a
    /// second press. So nothing is queued, the draft stays, and the sentence is
    /// what the key earns.
    #[test]
    fn a_signin_with_no_client_reports_rather_than_waits() {
        let mut app = App::mock();
        app.set_client_available(false);
        run_command_line(&mut app, "signin");
        type_text(&mut app, "7");
        let draft = app.line.text().to_owned();

        app.handle_key(press(KeyCode::Enter));

        assert!(
            !app.signin()
                .and_then(SignIn::flow)
                .expect("the flow is up")
                .waiting,
            "nothing is on its way, so nothing is in flight"
        );
        assert!(
            app.take_action().is_none(),
            "and nothing was queued: a login fired by a client arriving later \
             is an attempt nobody asked for"
        );
        assert_eq!(app.line.text(), draft, "the draft is the reader's");
        assert_eq!(app.focus, Focus::Input, "and the field keeps the keys");
        assert_eq!(app.status, "not connected yet — the client is not up");
    }

    /// With a client up the same key is the request it always was: queued, and
    /// the flow told a request is on its way.
    #[test]
    fn a_signin_with_a_client_queues_and_waits() {
        let mut app = App::mock();
        app.set_client_available(true);
        run_command_line(&mut app, "signin");

        app.handle_key(press(KeyCode::Enter));

        assert!(
            matches!(app.take_action(), Some(Action::Login { .. })),
            "a client is there to carry it"
        );
        assert!(
            app.signin()
                .and_then(SignIn::flow)
                .expect("the flow is up")
                .waiting,
            "and the panel may say so"
        );
    }

    /// Losing the client takes the flow out of the wait, rather than leaving a
    /// flag that outlived the thing it was about.
    #[test]
    fn losing_the_client_ends_the_wait() {
        let mut app = App::mock();
        app.set_client_available(true);
        run_command_line(&mut app, "signin");
        app.handle_key(press(KeyCode::Enter));
        app.take_action();

        app.set_client_available(false);

        assert!(
            !app.signin()
                .and_then(SignIn::flow)
                .expect("the flow is up")
                .waiting,
            "no client, no in-flight request"
        );
    }
}
