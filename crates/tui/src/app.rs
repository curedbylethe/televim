//! Top-level TUI state.

use std::borrow::Cow;
use std::cell::Cell;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use domain::chat::Chat;
use domain::history::{ConversationView, unread_target};
use domain::message::{Message, MessageStatus};
use domain::updates::{ChatList, UpdateEvent};
use domain::vim::{Motion, VimState};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};

use crate::theme::Theme;
use crate::widgets;

/// How close to an end of the loaded messages the cursor has to get before the
/// page beyond it is worth asking for.
///
/// A margin rather than the edge itself, because a fetch costs a round trip:
/// asking a screenful early means the reader reaches the end of what is loaded
/// with the next page already on its way.
const FETCH_MARGIN: usize = 20;

/// How many message rows the conversation panel is assumed to have before it
/// has been drawn once.
///
/// Only the panel knows the real number, and only during a frame. This is what
/// the key handling falls back on in between, and it is deliberately a normal
/// size rather than a small one: a page that overshoots is clamped.
const ASSUMED_ROWS: usize = 20;

/// What the status line shows before anything has happened.
const IDLE_STATUS: &str = "televim";

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
}

impl Mode {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
        }
    }
}

/// What the input bar represents when in Insert mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Message,
    Command,
    Search,
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

    /// Set by `/` search: the query text.
    pub search_query: Option<String>,

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

    /// How many message rows the conversation panel had room for as of the last
    /// frame.
    ///
    /// A cell rather than a field because a frame is drawn from a shared
    /// reference, and the panel is the only place that knows how tall the
    /// terminal made it. It is a measurement rather than state anything decides,
    /// so recording it late is the same as recording it at all.
    rows: Cell<usize>,
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
            prompt: PromptKind::Message,
            theme: Theme::default(),
            list: ChatList::default(),
            selected_chat: 0,
            conversation: ConversationView::new(0),
            vim: VimState::new(0),
            input: String::new(),
            status: IDLE_STATUS.to_string(),
            should_quit: false,
            search_query: None,
            fetching: Fetching::default(),
            pending_jump: None,
            rows: Cell::new(ASSUMED_ROWS),
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
        self.search_query = None;
    }

    // ---- what is on show ------------------------------------------------

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
        self.select_chat_none();
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
    fn cursor_message_id(&self) -> Option<i64> {
        self.conversation
            .window
            .get(self.vim.cursor())
            .map(|message| message.id)
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
        // Search matches are positions in the window that was just replaced.
        self.vim.set_matches(Vec::new());
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
    /// and it is the common case — see [`App::landing_position`].
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
        if self.pending_jump.map(|jump| jump.target_id) != Some(target_id) {
            return false;
        }

        self.pending_jump = None;

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
        // Search matches are positions in the window that was just replaced.
        self.vim.set_matches(Vec::new());

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
    #[must_use]
    pub fn wants_older(&self) -> bool {
        let window = &self.conversation.window;

        !self.fetching.is_in_flight(FetchDirection::Older)
            && !window.is_empty()
            && !window.exhausted_older
            && self.vim.cursor() < FETCH_MARGIN
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
            && self.vim.cursor().saturating_add(FETCH_MARGIN) >= window.len()
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

    // ---- key handling --------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) {
        // Ctrl-C always quits.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        match self.mode {
            Mode::Normal => self.handle_normal(key),
            Mode::Insert => self.handle_insert(key),
            Mode::Visual => self.handle_visual(key),
        }
    }

    fn handle_normal(&mut self, key: KeyEvent) {
        // A screenful at a time, which is what a terminal scrolls by. Bound here
        // rather than in the motion table because how much a page is depends on
        // how tall the panel turned out to be.
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('d') => self.page(true),
                KeyCode::Char('u') => self.page(false),
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char(c) if matches!(c, 'j' | 'k' | 'g' | 'G' | 'n' | 'N') => {
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

                    _ => {}
                }

                self.settle_follow();
            }
            KeyCode::Char('i' | 'a') => {
                self.mode = Mode::Insert;
                self.prompt = PromptKind::Message;
                self.input.clear();
            }
            KeyCode::Char('v') => {
                self.mode = Mode::Visual;
                self.status = "VISUAL: d=delete y=yank r=reply (stubs)".into();
            }
            KeyCode::Char('/') => {
                self.mode = Mode::Insert;
                self.prompt = PromptKind::Search;
                self.input.clear();
            }
            KeyCode::Char(':') => {
                self.mode = Mode::Insert;
                self.prompt = PromptKind::Command;
                self.input.clear();
            }
            KeyCode::Char('q') => self.should_quit = true,
            _ => {}
        }
    }

    /// Moves the cursor a screenful, which is what `Ctrl+d` and `Ctrl+u` mean.
    ///
    /// Landing on the newest message re-engages following and moving away from
    /// it disengages, on the same rule as `j` and `k`, so a page and a line
    /// cannot disagree about whether the view is pinned.
    fn page(&mut self, down: bool) {
        let step = self.rows.get().max(1);
        let last = self.conversation.window.len().saturating_sub(1);

        let cursor = if down {
            self.vim.cursor().saturating_add(step).min(last)
        } else {
            self.vim.cursor().saturating_sub(step)
        };

        self.vim.set_cursor(cursor);
        self.settle_follow();
    }

    fn handle_insert(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.prompt = PromptKind::Message;
                self.input.clear();
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Enter => self.submit(),
            KeyCode::Char(c) => self.input.push(c),
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

    fn submit(&mut self) {
        match self.prompt {
            PromptKind::Message => {
                // A message belongs to a conversation, so there has to be one
                // open for it to belong to. Nothing is sent yet; the message is
                // shown because the reader typed it.
                if !self.input.is_empty() && self.has_conversation() {
                    let message = Message {
                        id: self.next_message_id(),
                        chat_id: self.conversation.window.chat_id,
                        text: Cow::Owned(std::mem::take(&mut self.input)),
                        timestamp: 0,
                        status: MessageStatus::Sent,
                        is_outgoing: true,
                    };
                    self.apply_newer(vec![message]);
                }
            }
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
        self.mode = Mode::Normal;
        self.prompt = PromptKind::Message;
    }

    /// The identifier a locally composed message gets.
    ///
    /// Counting on from the newest identifier the conversation holds keeps it
    /// distinct from everything on screen, which is all the window needs of it
    /// while nothing is sent.
    fn next_message_id(&self) -> i64 {
        self.conversation
            .window
            .newest_id()
            .map_or(1, |id| id.saturating_add(1))
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

    fn run_search(&mut self, query: &str) {
        if query.is_empty() {
            self.vim.set_matches(vec![]);
            return;
        }
        let matches: Vec<usize> = self
            .conversation
            .window
            .iter()
            .enumerate()
            .filter_map(|(i, m)| {
                if m.text.contains(query) {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();
        let n = matches.len();
        self.vim.set_matches(matches);
        self.search_query = Some(query.to_owned());
        self.status = format!("/{query} — {n} match(es)");
        if n > 0 {
            self.vim.handle_char('n');
            self.settle_follow();
        }
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

    /// How many message rows the conversation panel had room for, as of the last
    /// frame.
    #[must_use]
    pub fn visible_rows(&self) -> usize {
        self.rows.get()
    }

    /// Records how many message rows the conversation panel has room for.
    ///
    /// Called from the panel, which is the only place the terminal's height has
    /// been turned into a rectangle. Zero is not a measurement anything can act
    /// on, so it is stored as one row: a page that moves nowhere is worse than a
    /// page that moves too little.
    pub fn record_rows(&self, rows: usize) {
        self.rows.set(rows.max(1));
    }

    /// The first message the conversation panel shows, given `rows` of room.
    ///
    /// While the view is pinned to the newest message the slice ends at it;
    /// otherwise it is centred on the cursor, which is the reader's place, and
    /// then pulled back inside the window so that the slice is always exactly as
    /// tall as the panel and never starts past the end.
    #[must_use]
    pub fn viewport_start(&self, rows: usize) -> usize {
        let total = self.conversation.window.len();
        if total == 0 {
            return 0;
        }

        let rows = rows.clamp(1, total);
        if self.conversation.auto_follow() {
            return total - rows;
        }

        self.vim.cursor().saturating_sub(rows / 2).min(total - rows)
    }

    // ---- helpers -------------------------------------------------------

    #[must_use]
    pub fn current_chat_id(&self) -> i64 {
        self.list.chats.get(self.selected_chat).map_or(0, |c| c.id)
    }

    #[must_use]
    pub fn prompt_prefix(&self) -> &'static str {
        match self.prompt {
            PromptKind::Message => "",
            PromptKind::Command => ":",
            PromptKind::Search => "/",
        }
    }

    /// What the status line shows.
    ///
    /// A jump outranks whatever the status line was last told: it is what the
    /// reader has just asked for, and it is over as soon as its page lands.
    /// Derived from the jump rather than written into the status, so that the two
    /// cannot come apart — a status that outlived its fetch would be a line
    /// saying "jumping" over a reader who had already arrived.
    #[must_use]
    pub fn status_text(&self) -> &str {
        if self.pending_jump.is_some() {
            JUMP_LABEL
        } else {
            &self.status
        }
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
        }
    }

    /// Messages of the sample conversation, with these identifiers.
    fn page(ids: &[i64]) -> Vec<Message> {
        ids.iter().map(|id| message(*id, "text")).collect()
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
        assert_eq!(app.mode, Mode::Insert);

        type_text(&mut app, "hello");
        assert_eq!(app.input, "hello");
    }

    #[test]
    fn escape_returns_to_normal_and_clears_input() {
        let mut app = App::mock();
        app.handle_key(press(KeyCode::Char('i')));
        type_text(&mut app, "hi");
        app.handle_key(press(KeyCode::Esc));

        assert_eq!(app.mode, Mode::Normal);
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

    #[test]
    fn enter_submits_the_typed_message() {
        let mut app = App::mock();
        let before = app.conversation.window.len();

        app.handle_key(press(KeyCode::Char('i')));
        type_text(&mut app, "ping");
        app.handle_key(press(KeyCode::Enter));

        assert_eq!(app.conversation.window.len(), before + 1);
        assert_eq!(text_of(&app, 11), Some("ping"));
        assert_eq!(
            reading(&app),
            Some(11),
            "a message just typed is the one on screen"
        );
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn typing_with_no_conversation_open_composes_nothing() {
        let mut app = App::new();
        app.handle_key(press(KeyCode::Char('i')));
        type_text(&mut app, "ping");
        app.handle_key(press(KeyCode::Enter));

        assert!(app.conversation.window.is_empty());
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

        assert_eq!(app.search_query.as_deref(), Some("benchmarks"));
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

    // ---- where the reader is -------------------------------------------

    #[test]
    fn opening_a_conversation_starts_pinned_to_the_newest_message() {
        let app = App::mock();

        assert!(app.conversation.auto_follow());
        assert_eq!(
            app.conversation.anchor_id(),
            None,
            "a pinned view has no place to keep"
        );
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

    #[test]
    fn the_viewport_is_a_windowful_ending_at_a_pinned_view() {
        let app = App::mock();

        assert_eq!(
            app.viewport_start(4),
            6,
            "pinned to the bottom, the slice is the last screenful"
        );
        assert_eq!(app.viewport_start(99), 0, "a panel taller than the window");
        assert_eq!(
            app.viewport_start(0),
            9,
            "a panel with no room still shows the newest row"
        );
    }

    #[test]
    fn the_viewport_centres_on_a_cursor_that_is_not_pinned() {
        let mut app = App::mock();
        go_to_top(&mut app);

        assert_eq!(
            app.viewport_start(4),
            0,
            "the top of the window is the top of the slice"
        );

        app.vim.set_cursor(5);
        assert_eq!(
            app.viewport_start(4),
            3,
            "half a panel either side of the cursor"
        );

        app.vim.set_cursor(9);
        assert_eq!(
            app.viewport_start(4),
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

    /// A window that was replaced is one the search indices no longer describe:
    /// they are positions, and the messages they pointed at are gone.
    #[test]
    fn a_jump_clears_the_search_matches() {
        let mut app = with_unread_out_of_reach(2);
        run_search_line(&mut app, "text");
        go_to_top(&mut app);
        assert!(app.apply_jump(&page(&[16, 17, 18, 19, 20]), 19));

        let landing = app.vim.cursor();
        app.handle_key(press(KeyCode::Char('n')));

        assert_eq!(
            app.vim.cursor(),
            landing,
            "there is nothing to search for until the next `/`"
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
