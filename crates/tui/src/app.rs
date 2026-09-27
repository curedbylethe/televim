//! Top-level TUI state.

use std::cell::Cell;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[cfg(test)]
use std::borrow::Cow;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use domain::chat::Chat;
use domain::history::{CONVERSATION_WINDOW, ConversationView, ConversationWindow, unread_target};
use domain::message::{Message, MessageStatus};
use domain::search::{SearchState, word_prefix_match};
use domain::updates::{ChatList, UpdateEvent};
use domain::vim::{Motion, VimState};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};

use crate::rows::{self, Reserved, RowSpan, Slice};
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

/// The word the status line shows while the input line holds the focus.
///
/// A constant rather than a label on [`Focus`]: the other two panes are both in
/// [`Mode::Normal`], and which of them is on show is what the border is for.
pub const INSERT_LABEL: &str = "INSERT";

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

/// The most characters a composed message may hold.
///
/// Telegram's own limit. Repeated here rather than reached for: `tui` may not
/// name `telegram-framework`, and the cap is what stops a key held down from
/// growing the buffer without bound. The framework checks the same number before
/// the request, so the two cannot disagree about what is sendable.
const MESSAGE_LIMIT: usize = 4096;

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

/// What the input bar represents when in Insert mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Message,
    Command,
    Search,
    Reply,
    Edit,
}

/// A destructive action waiting for the reader's `y`.
///
/// The message's direction is captured here, when the prompt is raised, rather
/// than looked up again when `y` arrives: an arrival can evict the message while
/// the prompt is up, and the wording has to keep describing what was asked
/// about even then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmKind {
    /// Delete one message. `id` is the message and `is_outgoing` says whose it
    /// is, which is what the prompt's wording turns on.
    DeleteMessage {
        /// Identifier of the message to delete.
        id: i64,

        /// Whether the reader's own account sent it.
        is_outgoing: bool,
    },
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

pub struct App {
    pub mode: Mode,
    pub focus: Focus,
    pub prompt: PromptKind,
    pub theme: Theme,

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
    pub input: String,
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

    /// Whether a `d` was just pressed and a second one would delete.
    ///
    /// The latch is what makes `dd` two presses: any other key, and any motion,
    /// clears it, so `jd` does not delete.
    pub pending_d: bool,

    /// The placeholder of the send in flight, if one is.
    ///
    /// An `Option` rather than a flag so that a result is matched to the send it
    /// answers: releasing the gate for a send that is no longer in flight is a
    /// no-op, and a duplicate result cannot release a later send's gate.
    pub sending: Option<i64>,

    /// The deletion waiting to be confirmed, if one is.
    pub confirm: Option<ConfirmKind>,

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

    /// The conversation the highlight has moved onto but has not been taken to.
    ///
    /// The same hand-over as [`App::pending_jump`] — recorded here because
    /// `tui` cannot reach the network — and for the same reason it carries a
    /// time: a reader holding `j` would otherwise fetch every conversation they
    /// scrolled past, and one page per chat as fast as a key repeats is how a
    /// scroll through the list becomes a flood wait.
    pending_chat: Option<ChatChoice>,

    /// Whether a `g` was just pressed in the chat list and a second one would
    /// take the reader to the top of it.
    ///
    /// The same latch as `dd` and for the same reason: `gg` is two presses in
    /// Vim, and a key held down is not two of them.
    pending_g: bool,

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
            prompt: PromptKind::Message,
            theme: Theme::default(),
            list: ChatList::default(),
            selected_chat: 0,
            conversation: ConversationView::new(0),
            vim: VimState::new(0),
            input: String::new(),
            status: IDLE_STATUS.to_string(),
            should_quit: false,
            search: SearchState::default(),
            reply_to: None,
            editing: None,
            pending_d: false,
            sending: None,
            confirm: None,
            actions: VecDeque::new(),
            status_until: None,
            fetching: Fetching::default(),
            pending_jump: None,
            pending_chat: None,
            pending_g: false,
            rows: Cell::new(ASSUMED_ROWS),
            body_width: Cell::new(ASSUMED_BODY_WIDTH),
        }
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

    /// Closes the conversation on show.
    fn select_chat_none(&mut self) {
        self.conversation = ConversationView::new(0);
        self.vim = VimState::new(0);
        self.fetching.clear();
        self.pending_jump = None;
        self.search.clear();
        self.reply_to = None;
        self.editing = None;
        self.pending_d = false;
        self.confirm = None;
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
    /// Reports whether anything was shown.
    pub fn apply_latest(&mut self, page: Vec<Message>) -> bool {
        if !self.page_belongs_to_open_chat(&page) {
            return false;
        }

        self.conversation.window.replace(page);
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
        })
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
    pub fn apply_jump(&mut self, page: &[Message], target_id: i64) -> bool {
        if !self.clear_jump(target_id) {
            return false;
        }

        if !self.page_belongs_to_open_chat(page) {
            return false;
        }

        // Copied into the window rather than moved: a page that replaces a
        // window is the caller's to report to the cursor it keeps, and that
        // cursor is counted from the same messages.
        self.conversation.window.replace(page.iter().cloned());

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
    /// The event is copied rather than shared because the list moves an arrival
    /// into its own window, so it needs one of its own. One copy per event is
    /// the price of a single event reaching both.
    #[must_use]
    pub fn apply_update(&mut self, event: &UpdateEvent) -> bool {
        let listed = self.list.apply_update(event.clone());

        let anchor = self.cursor_message_id();
        let windowed = self.conversation.apply_event(event);
        if windowed {
            self.after_window_change(anchor);
        }

        listed || windowed
    }

    /// Puts the reader back where they were, now that the window has moved.
    ///
    /// The position is carried as a message identifier rather than as an index,
    /// because an index means a different message on either side of a page. An
    /// identifier that is no longer in the window has no position to restore, so
    /// the clamp stands — the reader's message was evicted, and the nearest
    /// survivor is the honest answer.
    fn after_window_change(&mut self, anchor: Option<i64>) {
        self.vim.set_total(self.conversation.window.len());

        let cursor = self.vim.cursor();
        let restored = anchor
            .and_then(|id| self.conversation.window.position_of(id))
            .unwrap_or(cursor);
        self.vim.set_cursor(restored);

        if self.conversation.auto_follow() {
            // A view pinned to the end stays pinned: what arrived is what the
            // reader asked to see.
            self.vim.apply_motion(Motion::Last);
        } else {
            self.settle_follow();
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
        let first = layout.get(self.vim.cursor()).map_or(0, |span| span.first);

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
    /// `Ctrl+w` is Vim's other idiom for this, and the one to bind for a reader
    /// stepping between panes: `Esc` is the destructive way back, because `Esc`
    /// in Vim abandons what is being composed. This one only looks away.
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
            _ if key.modifiers.contains(KeyModifiers::CONTROL)
                && key.code == KeyCode::Char('w') =>
            {
                self.leave_line();
                return;
            }
            _ => {}
        }

        match self.focus {
            Focus::ChatList => self.handle_chat_list(key),
            Focus::Conversation => match self.mode {
                Mode::Normal => self.handle_normal(key),
                Mode::Visual => self.handle_visual(key),
                Mode::Confirm => self.handle_confirm(key),
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
        // A control key is not `d`, so the latch is cleared here rather than per
        // arm: `Ctrl+d` is a page, not the first half of a deletion.
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('d') => self.page(true),
                KeyCode::Char('u') => self.page(false),
                _ => self.pending_d = false,
            }
            return;
        }

        match key.code {
            KeyCode::Char(c) if matches!(c, 'j' | 'k' | 'g' | 'G' | 'n' | 'N') => {
                // Any motion moves the cursor off whatever a first `d` was about.
                self.pending_d = false;
                let motion = self.vim.handle_char(c);

                match motion {
                    // `gg` is where the unread messages start when there are
                    // any, and the top of what is loaded when there are not. A
                    // jump the window can answer is taken here; one it cannot is
                    // left for the caller to fetch. Either way the reader has
                    // asked for something, so whatever they asked for before is
                    // replaced by it.
                    Some(Motion::First) => self.pending_jump = self.jump_to_unread(),

                    // `G` is the reader overriding a jump with "take me to the
                    // end". The page on its way is for a place they no longer
                    // want to be, and it is dropped when it lands.
                    Some(Motion::Last) => self.pending_jump = None,

                    // `n` and `N` walk the search's matches. `VimState` reports
                    // the motion but cannot answer it, because a match is a
                    // place in a conversation and the list of them lives here.
                    Some(Motion::NextMatch) => self.walk_search(true),
                    Some(Motion::PrevMatch) => self.walk_search(false),

                    _ => {}
                }

                self.settle_follow();
            }
            KeyCode::Char('i' | 'a') => {
                self.pending_d = false;
                self.start_compose();
            }
            KeyCode::Char('r') => {
                self.pending_d = false;
                self.start_reply();
            }
            KeyCode::Char('e') => {
                self.pending_d = false;
                self.start_edit();
            }
            KeyCode::Char('d') => self.pending_delete(),
            KeyCode::Char('D') => {
                self.pending_d = false;
                self.dismiss_failed_at_cursor();
            }
            KeyCode::Char('v') => {
                self.pending_d = false;
                self.mode = Mode::Visual;
                self.status = "VISUAL: d=delete y=yank r=reply (stubs)".into();
            }
            // The list is beside the conversation, so `h` is how the reader gets
            // to it. `l` has nothing to move to from here and is left unbound
            // rather than made to wrap.
            KeyCode::Char('h') => {
                self.pending_d = false;
                self.set_focus(Focus::ChatList);
            }
            KeyCode::Char('/') => {
                self.pending_d = false;
                self.focus = Focus::Input;
                self.prompt = PromptKind::Search;
                self.input.clear();
            }
            KeyCode::Char(':') => {
                self.pending_d = false;
                self.focus = Focus::Input;
                self.prompt = PromptKind::Command;
                self.input.clear();
            }
            KeyCode::Char('q') => {
                self.pending_d = false;
                self.should_quit = true;
            }
            // Any other key is not the second `d`, so it clears the latch.
            _ => self.pending_d = false,
        }
    }

    /// Opens the buffer for a new message, with no reply and no edit.
    fn start_compose(&mut self) {
        self.focus = Focus::Input;
        self.prompt = PromptKind::Message;
        self.reply_to = None;
        self.editing = None;
        self.input.clear();
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
        self.prompt = PromptKind::Reply;
        self.reply_to = Some(id);
        self.editing = None;
        self.input.clear();
    }

    /// Opens the buffer with the cursor's own message in it, for editing.
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
        self.prompt = PromptKind::Edit;
        self.editing = Some(id);
        self.reply_to = None;
        self.input = text;
    }

    /// Handles a `d`: the first latches, the second asks to delete.
    ///
    /// Deletion is allowed on any real message, incoming included: Telegram
    /// permits it, and a private chat does remove the other side's words. A
    /// placeholder is refused because it has no identifier the server knows:
    /// an in-flight one is still on its way, and a failed one is the reader's to
    /// dismiss with `D` instead.
    fn pending_delete(&mut self) {
        let Some((id, is_outgoing, status)) = self
            .cursor_message()
            .map(|message| (message.id, message.is_outgoing, message.status))
        else {
            self.pending_d = false;
            return;
        };

        if id <= 0 {
            self.pending_d = false;
            // The two refusals differ because a failed message has a `D` to
            // offer and one still in flight does not: pointing at `D` for a
            // message on its way would be wrong.
            self.flash(if matches!(status, MessageStatus::Failed) {
                "that message never left — D dismisses it"
            } else {
                "that message is still on its way"
            });
            return;
        }

        if !self.pending_d {
            self.pending_d = true;
            return;
        }

        self.pending_d = false;
        self.mode = Mode::Confirm;
        self.confirm = Some(ConfirmKind::DeleteMessage { id, is_outgoing });
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
    fn handle_confirm(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') => {
                if let Some(ConfirmKind::DeleteMessage { id, .. }) = self.confirm {
                    let chat_id = self.conversation.window.chat_id;
                    self.queue_action(Action::Delete {
                        chat_id,
                        message_ids: vec![id],
                    });
                }
                self.confirm = None;
                self.mode = Mode::Normal;
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                self.confirm = None;
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
    ///
    /// A page is a motion, so a `d` waiting for its second press is forgotten
    /// here as surely as it is by `j`.
    fn page(&mut self, down: bool) {
        self.pending_d = false;
        let step = self.rows.get().max(1);
        let layout = self.row_layout();
        let total = rows::total_rows(&layout);
        let here = layout.get(self.vim.cursor()).map_or(0, |span| span.first);

        let target = if down {
            here.saturating_add(step).min(total.saturating_sub(1))
        } else {
            here.saturating_sub(step)
        };

        if let Some(cursor) = rows::message_at_row(&layout, target) {
            self.vim.set_cursor(cursor);
        }
        self.settle_follow();
    }

    fn handle_line(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                // The destructive way back: `Esc` in Vim abandons what is being
                // composed, and so does this. `Ctrl+w` is the one that keeps it.
                self.focus = Focus::Conversation;
                self.prompt = PromptKind::Message;
                self.input.clear();
                self.reply_to = None;
                self.editing = None;
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Enter => self.submit(),
            KeyCode::Char(c) => {
                // Capped in characters, which is the unit Telegram counts, so a
                // key held down cannot grow the buffer past what can be sent.
                if self.input.chars().count() < MESSAGE_LIMIT {
                    self.input.push(c);
                } else {
                    self.flash("message is too long");
                }
            }
            _ => {}
        }
    }

    fn handle_visual(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.status = IDLE_STATUS.into();
            }
            KeyCode::Char('d') => {
                self.status = "visual: delete (not implemented)".into();
                self.mode = Mode::Normal;
            }
            KeyCode::Char('y') => {
                self.status = "visual: yank (not implemented)".into();
                self.mode = Mode::Normal;
            }
            KeyCode::Char('r') => {
                self.status = "visual: reply (not implemented)".into();
                self.mode = Mode::Normal;
            }
            _ => {}
        }
    }

    /// Handles `Enter` in the input line.
    ///
    /// Which prompt it is decides what is handed over; the buffer and the reply
    /// context are cleared either way, because the work leaves here rather than
    /// happening here. The focus goes back to the conversation for the same
    /// reason: the line has given up what it was for.
    fn submit(&mut self) {
        match self.prompt {
            PromptKind::Message | PromptKind::Reply => self.submit_message(),
            PromptKind::Edit => self.submit_edit(),
            PromptKind::Command => {
                let cmd = self.input.trim().to_owned();
                self.input.clear();
                self.run_command(&cmd);
            }
            PromptKind::Search => {
                let q = self.input.trim().to_owned();
                self.input.clear();
                self.run_search(&q);
            }
        }

        self.focus = Focus::Conversation;
        self.prompt = PromptKind::Message;
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
        if !self.has_conversation() || self.input.trim().is_empty() {
            return;
        }

        let anchor = self.cursor_message_id();
        let chat_id = self.conversation.window.chat_id;
        let text = std::mem::take(&mut self.input);
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
        if !self.has_conversation() || self.input.trim().is_empty() {
            return;
        }

        let chat_id = self.conversation.window.chat_id;
        let text = std::mem::take(&mut self.input);
        self.queue_action(Action::Edit {
            chat_id,
            message_id,
            text,
        });
    }

    fn run_command(&mut self, cmd: &str) {
        match cmd {
            "q" | "quit" => self.should_quit = true,
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
            .filter(|message| word_prefix_match(&message.text, &query))
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

    // ---- rendering -----------------------------------------------------

    pub fn render(&self, frame: &mut Frame<'_>) {
        let area = frame.area();

        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(area);

        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(vertical[0]);

        widgets::chat_list::render(self, horizontal[0], frame);
        widgets::conversation::render(self, horizontal[1], frame);
        widgets::input_bar::render(self, vertical[1], frame);
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

    /// The rows every message in the window occupies, laid out at the panel's
    /// width.
    ///
    /// The single source of truth for the panel's geometry. Everything that
    /// needs to know how tall something is — the viewport, the scrollbar, the
    /// paging keys, the fetch triggers — asks here rather than working it out
    /// again, because two measurements of one thing is a bug waiting for the
    /// case where they disagree.
    ///
    /// A pure function of the window's messages and [`App::body_width`], and of
    /// nothing else: not the cursor, not the mode, not when it was asked. A
    /// layout worked out before a page lands is thrown away rather than kept,
    /// which is why a [`RowSpan`] is named by message id.
    #[must_use]
    pub fn row_layout(&self) -> Vec<RowSpan> {
        let width = self.body_width();
        let mut laid_out: Vec<RowSpan> = Vec::with_capacity(self.conversation.window.len());
        let mut first = 0;

        for (index, message) in self.conversation.window.iter().enumerate() {
            let text = 0..message.text.len();
            let len = rows::message_rows(self, message, width).len();

            laid_out.push(RowSpan {
                message_id: message.id,
                index,
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

    #[must_use]
    pub fn prompt_prefix(&self) -> &'static str {
        match self.prompt {
            PromptKind::Message | PromptKind::Reply | PromptKind::Edit => "",
            PromptKind::Command => ":",
            PromptKind::Search => "/",
        }
    }

    /// What the status line shows.
    ///
    /// A confirmation outranks everything: it is a question waiting for an
    /// answer, and it is over as soon as one is given. A search's label comes
    /// next, and outranks a transient status, because it describes state the
    /// reader must not lose: it is not a `flash`, so `expire_status` must not be
    /// able to take it away. Below both, a jump in flight — what the reader has
    /// just asked for — and then the full reason a failed message failed while
    /// the cursor is on it, and finally whatever was written to the status.
    #[must_use]
    pub fn status_text(&self) -> String {
        if let Some(ConfirmKind::DeleteMessage { is_outgoing, .. }) = self.confirm {
            return if is_outgoing {
                DELETE_OUTGOING_PROMPT.to_owned()
            } else {
                DELETE_INCOMING_PROMPT.to_owned()
            };
        }
        if self.search.is_active() {
            return self.search.label();
        }
        if self.pending_jump.is_some() {
            return JUMP_LABEL.to_owned();
        }
        if let Some(message) = self.cursor_message()
            && let Some(reason) = self.conversation.failure(message.id)
        {
            return reason.to_owned();
        }

        self.status.clone()
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
        }
    }

    /// Messages of the sample conversation, with these identifiers.
    fn page(ids: &[i64]) -> Vec<Message> {
        ids.iter().map(|id| message(*id, "text")).collect()
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

    /// Regression: every keystroke must be applied exactly once. Previously
    /// the reader thread in `runtime.rs` dropped every other event, so typing
    /// `s` then `q` produced only `q`.
    #[test]
    fn entering_insert_mode_then_typing_records_every_key() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('i')));
        assert_eq!(app.focus, Focus::Input);

        type_text(&mut app, "hello");
        assert_eq!(app.input, "hello");
    }

    #[test]
    fn escape_returns_to_normal_and_clears_input() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('i')));
        type_text(&mut app, "hi");
        app.handle_key(press(KeyCode::Esc));

        assert_eq!(app.focus, Focus::Conversation);
        assert!(app.input.is_empty());
    }

    #[test]
    fn backspace_removes_exactly_one_char_per_press() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('i')));
        type_text(&mut app, "abc");
        app.handle_key(press(KeyCode::Backspace));

        assert_eq!(app.input, "ab");
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
        assert_eq!(app.input, "half a th", "the line is not thrown away");

        // And it is still there to come back to, rather than lost.
        app.handle_key(press(KeyCode::Tab));
        assert_eq!(app.input, "half a th");
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

    // ---- dd and the confirm --------------------------------------------

    #[test]
    fn dd_asks_to_delete_the_message_under_the_cursor() {
        let mut app = App::mock();
        // The newest sample message is one of theirs.
        assert_eq!(reading(&app), Some(10));

        app.handle_key(press(KeyCode::Char('d')));
        assert!(app.pending_d, "one d latches");
        assert_eq!(app.mode, Mode::Normal);

        app.handle_key(press(KeyCode::Char('d')));
        assert!(!app.pending_d, "the second d spends the latch");
        assert_eq!(app.mode, Mode::Confirm);
        assert_eq!(
            app.confirm,
            Some(ConfirmKind::DeleteMessage {
                id: 10,
                is_outgoing: false
            })
        );
        assert_eq!(app.status_text(), DELETE_INCOMING_PROMPT);
    }

    /// The asymmetry the plan is built on: `dd` accepts the other side's
    /// message, and the prompt says which side it is about.
    #[test]
    fn dd_accepts_an_outgoing_message_and_the_prompt_names_that_side() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('k')));
        assert_eq!(reading(&app), Some(9), "an outgoing message");

        app.handle_key(press(KeyCode::Char('d')));
        app.handle_key(press(KeyCode::Char('d')));

        assert_eq!(
            app.confirm,
            Some(ConfirmKind::DeleteMessage {
                id: 9,
                is_outgoing: true
            })
        );
        assert_eq!(app.status_text(), DELETE_OUTGOING_PROMPT);
    }

    /// The classic way a `dd` binding ships a bug: `j` and `d` must not add up
    /// to a deletion.
    #[test]
    fn a_motion_between_the_ds_clears_the_latch() {
        let mut app = App::mock();

        app.handle_key(press(KeyCode::Char('d')));
        assert!(app.pending_d);
        app.handle_key(press(KeyCode::Char('k')));
        assert!(!app.pending_d, "the motion moves off what the d was about");

        app.handle_key(press(KeyCode::Char('d')));
        assert_eq!(
            app.mode,
            Mode::Normal,
            "the second d latches rather than deletes"
        );
        assert!(app.confirm.is_none());
    }

    #[test]
    fn any_other_key_clears_the_latch() {
        let mut app = App::mock();

        app.handle_key(press(KeyCode::Char('d')));
        app.handle_key(press(KeyCode::Char('x')));

        assert!(!app.pending_d);
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn a_page_clears_the_latch() {
        let mut app = App::mock();

        app.handle_key(press(KeyCode::Char('d')));
        app.handle_key(press_ctrl('d'));

        assert!(!app.pending_d);
        assert_eq!(app.mode, Mode::Normal);
    }

    /// A send that has not been acknowledged has no identifier the server knows,
    /// so neither deletion nor editing can touch it — and the two refusals for
    /// deletion differ, because only a failed message has a `D`.
    #[test]
    fn deleting_a_pending_message_is_refused_until_it_has_failed() {
        let mut app = App::mock();
        submit(&mut app, "hi");
        let id = app.sending.expect("the send is in flight");

        app.handle_key(press(KeyCode::Char('d')));
        app.handle_key(press(KeyCode::Char('d')));
        assert_eq!(app.mode, Mode::Normal, "no confirm is raised");
        assert!(
            app.status.contains("still on its way"),
            "got {:?}",
            app.status
        );

        app.fail_send(id, "boom".to_owned());
        app.handle_key(press(KeyCode::Char('d')));
        app.handle_key(press(KeyCode::Char('d')));
        assert_eq!(app.mode, Mode::Normal);
        assert!(
            app.status.contains("D dismisses"),
            "a failed message points at the key that clears it: {:?}",
            app.status
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
        assert_eq!(app.prompt, PromptKind::Reply);
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
        assert_eq!(app.prompt, PromptKind::Edit);
        assert_eq!(app.editing, Some(9));
        assert!(
            app.input.starts_with("No pressure then :)"),
            "the buffer opens with the message's text: {:?}",
            app.input
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
            app.row_layout()[3].first
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
            }),
            "the jump the reader did ask for is still the one being waited on"
        );
    }

    /// `G` is the reader overriding a jump with "take me to the end": the page on
    /// its way is for a place they no longer want to be, and it is dropped when
    /// it lands.
    #[test]
    fn the_end_of_the_conversation_cancels_a_jump() {
        let mut app = with_unread_out_of_reach(2);
        go_to_top(&mut app);
        assert!(app.pending_jump().is_some());

        app.handle_key(press(KeyCode::Char('G')));

        assert_eq!(app.pending_jump(), None);
        assert!(
            app.conversation.auto_follow(),
            "and the view is pinned to the newest message"
        );
        assert!(
            !app.apply_jump(&page(&[16, 17, 18, 19, 20]), 19),
            "the page that was on its way has nobody waiting for it"
        );
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
}
