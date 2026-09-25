//! Top-level TUI state.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use domain::chat::{Chat, ChatKind};
use domain::message::{Message, MessageStatus};
use domain::vim::VimState;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};

use crate::theme::Theme;
use crate::widgets;

/// Converts a `usize` (e.g. a length or index) into the `i64` id space.
///
/// Panics if the value exceeds `i64::MAX`. In practice `Vec`/`slice`
/// lengths and indices can never approach this on any real machine,
/// so a panic here would indicate a genuine bug.
fn to_id(n: usize) -> i64 {
    i64::try_from(n).expect("usize value does not fit in i64")
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
    pub messages: Vec<Message>,
    pub vim: VimState,
    pub input: String,
    pub status: String,
    pub should_quit: bool,

    /// Set by `/` search: the query text.
    pub search_query: Option<String>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    #[must_use]
    pub fn new() -> Self {
        let chats = mock_chats();
        let messages = mock_messages();
        let mut vim = VimState::new(messages.len());
        // Start on the newest message.
        vim.apply_motion(domain::vim::Motion::Last);

        Self {
            mode: Mode::Normal,
            prompt: PromptKind::Message,
            theme: Theme::default(),
            chats,
            selected_chat: 0,
            messages,
            vim,
            input: String::new(),
            status: "televim — skeleton (no network)".to_string(),
            should_quit: false,
            search_query: None,
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
        match key.code {
            KeyCode::Char('j') => self.vim.handle_char('j'),
            KeyCode::Char('k') => self.vim.handle_char('k'),
            KeyCode::Char('g') => self.vim.handle_char('g'),
            KeyCode::Char('G') => self.vim.handle_char('G'),
            KeyCode::Char('n') => self.vim.handle_char('n'),
            KeyCode::Char('N') => self.vim.handle_char('N'),
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
                if !self.input.is_empty() {
                    let msg = Message {
                        id: to_id(self.messages.len()) + 1,
                        chat_id: self.current_chat_id(),
                        text: std::borrow::Cow::Owned(std::mem::take(&mut self.input)),
                        timestamp: 0,
                        status: MessageStatus::Sent,
                        is_outgoing: true,
                    };
                    self.messages.push(msg);
                    self.vim.set_total(self.messages.len());
                    self.vim.apply_motion(domain::vim::Motion::Last);
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

    fn run_command(&mut self, cmd: &str) {
        match cmd {
            "q" | "quit" => self.should_quit = true,
            _ if cmd.starts_with("chat ") => {
                if let Ok(id) = cmd[5..].trim().parse::<i64>()
                    && let Some(pos) = self.chats.iter().position(|c| c.id == id)
                {
                    self.selected_chat = pos;
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
            .messages
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

// ---- mock data ---------------------------------------------------------

fn mock_chats() -> Vec<Chat> {
    vec![
        Chat {
            id: 1,
            title: "Ada Lovelace".into(),
            kind: ChatKind::Private,
            last_message: Some("See you at the demo.".into()),
            unread_count: 2,
            last_timestamp: Some(1_730_000_000),
        },
        Chat {
            id: 2,
            title: "Grace Hopper".into(),
            kind: ChatKind::Private,
            last_message: Some("The compiler is ready.".into()),
            unread_count: 0,
            last_timestamp: Some(1_729_999_000),
        },
        Chat {
            id: 3,
            title: "Alan Turing".into(),
            kind: ChatKind::Private,
            last_message: Some("Halting problem again…".into()),
            unread_count: 1,
            last_timestamp: Some(1_729_998_000),
        },
    ]
}

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
            chat_id: 1,
            text: std::borrow::Cow::Borrowed(*t),
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

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(app: &mut App, text: &str) {
        for ch in text.chars() {
            app.handle_key(press(KeyCode::Char(ch)));
        }
    }

    /// Regression: every keystroke must be applied exactly once. Previously
    /// the reader thread in `runtime.rs` dropped every other event, so typing
    /// `s` then `q` produced only `q`.
    #[test]
    fn entering_insert_mode_then_typing_records_every_key() {
        let mut app = App::new();
        app.handle_key(press(KeyCode::Char('i')));
        assert_eq!(app.mode, Mode::Insert);

        type_text(&mut app, "hello");
        assert_eq!(app.input, "hello");
    }

    #[test]
    fn escape_returns_to_normal_and_clears_input() {
        let mut app = App::new();
        app.handle_key(press(KeyCode::Char('i')));
        type_text(&mut app, "hi");
        app.handle_key(press(KeyCode::Esc));

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.input.is_empty());
    }

    #[test]
    fn backspace_removes_exactly_one_char_per_press() {
        let mut app = App::new();
        app.handle_key(press(KeyCode::Char('i')));
        type_text(&mut app, "abc");
        app.handle_key(press(KeyCode::Backspace));

        assert_eq!(app.input, "ab");
    }

    #[test]
    fn enter_submits_the_typed_message() {
        let mut app = App::new();
        let before = app.messages.len();

        app.handle_key(press(KeyCode::Char('i')));
        type_text(&mut app, "ping");
        app.handle_key(press(KeyCode::Enter));

        assert_eq!(app.messages.len(), before + 1);
        assert_eq!(app.messages[before].text, "ping");
        assert_eq!(app.mode, Mode::Normal);
    }

    fn run_command_line(app: &mut App, command: &str) {
        app.handle_key(press(KeyCode::Char(':')));
        type_text(app, command);
        app.handle_key(press(KeyCode::Enter));
    }

    #[test]
    fn chat_command_selects_the_matching_chat() {
        let mut app = App::new();
        run_command_line(&mut app, "chat 2");

        let expected = app
            .chats
            .iter()
            .position(|c| c.id == 2)
            .expect("chat 2 is part of the mock data");
        assert_eq!(app.selected_chat, expected);
    }

    /// Both halves of the `chat <id>` guard must hold: a malformed id and a
    /// well-formed-but-unknown id must both leave the selection untouched.
    #[test]
    fn chat_command_ignores_unparseable_or_unknown_ids() {
        let mut app = App::new();
        let before = app.selected_chat;

        run_command_line(&mut app, "chat not-a-number");
        assert_eq!(app.selected_chat, before);

        run_command_line(&mut app, "chat 999");
        assert_eq!(app.selected_chat, before);
    }

    #[test]
    fn unknown_command_sets_the_status_line() {
        let mut app = App::new();
        run_command_line(&mut app, "frobnicate");

        assert!(app.status.contains("unknown command"));
        assert_eq!(app.mode, Mode::Normal);
    }
}
