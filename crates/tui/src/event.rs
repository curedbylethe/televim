//! Maps crossterm key events to app actions. Kept deliberately small — the
//! `App` owns the actual dispatch today.

use crate::app::Mode;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppAction {
    Quit,
    Char(char),
    Enter,
    Escape,
    Backspace,
    SetMode(Mode),
    Noop,
}

#[must_use]
pub fn key_to_action(key: KeyEvent, _mode: Mode) -> AppAction {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return AppAction::Quit;
    }
    match key.code {
        KeyCode::Esc => AppAction::Escape,
        KeyCode::Enter => AppAction::Enter,
        KeyCode::Backspace => AppAction::Backspace,
        KeyCode::Char(c) => AppAction::Char(c),
        _ => AppAction::Noop,
    }
}
