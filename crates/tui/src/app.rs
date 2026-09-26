//! Top-level TUI state.

use std::borrow::Cow;
use std::cell::Cell;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use domain::chat::Chat;
use domain::history::ConversationView;
use domain::message::{Message, MessageStatus};
use domain::updates::UpdateEvent;
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

/// Which end of a conversation a fetch is asking for.
///
/// Named rather than a `bool`, because the two differ in what they do to the
/// window — one prepends and one appends — and a boolean would leave every call
/// site saying which end it meant by convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchDirection {
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
            Self::Older => "Loading older…",
            Self::Newer => "Loading newer…",
        }
    }
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

pub struct App {
    pub mode: Mode,
    pub prompt: PromptKind,
    pub theme: Theme,

    pub chats: Vec<Chat>,
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

    /// Whether a page in front of the window is in flight.
    fetching_older: bool,

    /// Whether a page behind the window is in flight.
    fetching_newer: bool,

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
            chats: Vec::new(),
            selected_chat: 0,
            conversation: ConversationView::new(0),
            vim: VimState::new(0),
            input: String::new(),
            status: "televim — skeleton (no network)".to_string(),
            should_quit: false,
            search_query: None,
            fetching_older: false,
            fetching_newer: false,
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
        app.chats = mock_chats();
        app.select_chat(0);
        app.apply_latest(mock_messages());
        app
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
        let Some(chat) = self.chats.get(index) else {
            return;
        };
        let chat_id = chat.id;

        self.selected_chat = index;
        self.conversation = ConversationView::new(chat_id);
        self.vim = VimState::new(0);
        self.fetching_older = false;
        self.fetching_newer = false;
        self.search_query = None;
    }

    /// Whether a conversation is open to put messages in.
    ///
    /// Telegram numbers peers from one, so a zero here is the absence of a
    /// conversation rather than a conversation with an odd identifier.
    #[must_use]
    fn has_conversation(&self) -> bool {
        self.conversation.window.chat_id != 0
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

    /// Applies an event from the feed to the open conversation.
    ///
    /// The same contract as the flat window's: `false` means nothing observable
    /// moved, so the caller owes no redraw. Both windows are fed the same
    /// events, and deduplicating by identifier is what makes the overlap
    /// between them harmless.
    #[must_use]
    pub fn apply_update(&mut self, event: &UpdateEvent) -> bool {
        let anchor = self.cursor_message_id();

        if !self.conversation.apply_event(event) {
            return false;
        }

        self.after_window_change(anchor);

        true
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

        !self.fetching_older
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

        !self.fetching_newer
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
        match direction {
            FetchDirection::Older => self.fetching_older = true,
            FetchDirection::Newer => self.fetching_newer = true,
        }
    }

    /// Records that the fetch for `direction` is over, however it ended.
    ///
    /// A failed fetch releases the direction as surely as a successful one: the
    /// alternative is a conversation that can never be paged again because one
    /// request went wrong.
    pub fn end_fetch(&mut self, direction: FetchDirection) {
        match direction {
            FetchDirection::Older => self.fetching_older = false,
            FetchDirection::Newer => self.fetching_newer = false,
        }
    }

    /// Whether a fetch for `direction` is in flight.
    #[must_use]
    pub const fn is_fetching(&self, direction: FetchDirection) -> bool {
        match direction {
            FetchDirection::Older => self.fetching_older,
            FetchDirection::Newer => self.fetching_newer,
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
                self.vim.handle_char(c);
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
                self.status = "televim — skeleton (no network)".into();
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
                    && let Some(pos) = self.chats.iter().position(|c| c.id == id)
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
        self.chats.get(self.selected_chat).map_or(0, |c| c.id)
    }

    #[must_use]
    pub fn prompt_prefix(&self) -> &'static str {
        match self.prompt {
            PromptKind::Message => "",
            PromptKind::Command => ":",
            PromptKind::Search => "/",
        }
    }
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
            unread_count: 2,
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

    // ---- the frame -----------------------------------------------------

    #[test]
    fn a_new_application_holds_nothing_it_has_not_fetched() {
        let app = App::new();

        assert!(app.chats.is_empty());
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
            .chats
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

    #[test]
    fn an_arrival_the_window_already_holds_owes_no_redraw() {
        let mut app = App::mock();

        assert!(!app.apply_update(&UpdateEvent::NewMessage(message(10, "again"))));
        assert_eq!(app.conversation.window.len(), 10);
    }

    #[test]
    fn an_arrival_for_another_conversation_changes_nothing() {
        let mut app = App::mock();

        assert!(!app.apply_update(&UpdateEvent::NewMessage(stranger(11))));
        assert_eq!(app.conversation.window.len(), 10);
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

    #[test]
    fn a_direction_the_conversation_has_run_out_of_is_not_asked_for() {
        let mut app = App::mock();
        assert!(app.wants_older());

        app.conversation.window.exhausted_older = true;
        assert!(
            !app.wants_older(),
            "there is nothing in front of the oldest message"
        );
    }
}
