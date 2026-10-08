//! Maps crossterm key events to the keys that answer before any focus or mode
//! branch. Everything else is `Noop`, and the `App` passes it on to
//! `coordinate::handle_key`, which owns the context-dependent keys.

use crate::app::Mode;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppAction {
    Quit,
    Noop,
}

#[must_use]
pub fn key_to_action(key: KeyEvent, _mode: Mode) -> AppAction {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return AppAction::Quit;
    }
    AppAction::Noop
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODES: [Mode; 3] = [Mode::Normal, Mode::Visual, Mode::Confirm];

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn press_ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn ctrl_c_quits_in_every_mode() {
        for mode in MODES {
            assert_eq!(key_to_action(press_ctrl('c'), mode), AppAction::Quit);
        }
    }

    // Focus- and mode-dependent keys stay out of the map: each one means
    // something different on the pane that answers it, so the map says `Noop`
    // and `coordinate::handle_key` decides.
    #[test]
    fn context_dependent_keys_pass_through_in_every_mode() {
        let keys = [
            press(KeyCode::Esc),
            press(KeyCode::Enter),
            press(KeyCode::Backspace),
            press(KeyCode::Tab),
            press(KeyCode::BackTab),
            press(KeyCode::Char('j')),
            press(KeyCode::Char('c')),
            press_ctrl('w'),
        ];
        for mode in MODES {
            for key in keys {
                assert_eq!(
                    key_to_action(key, mode),
                    AppAction::Noop,
                    "{key:?} in {mode:?}"
                );
            }
        }
    }
}
