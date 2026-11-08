use tui::key::{KeyCode as TuiKeyCode, KeyEvent as TuiKeyEvent, KeyModifiers};

pub fn to_key(key: ratzilla::event::KeyEvent) -> Option<TuiKeyEvent> {
    let mut modifiers = KeyModifiers::NONE;
    if key.ctrl {
        modifiers |= KeyModifiers::CONTROL;
    }
    if key.alt {
        modifiers |= KeyModifiers::ALT;
    }
    if key.shift {
        modifiers |= KeyModifiers::SHIFT;
    }

    let code = match key.code {
        ratzilla::event::KeyCode::Char(c) => TuiKeyCode::Char(c),
        ratzilla::event::KeyCode::F(n) => TuiKeyCode::F(n),
        ratzilla::event::KeyCode::Backspace => TuiKeyCode::Backspace,
        ratzilla::event::KeyCode::Enter => TuiKeyCode::Enter,
        ratzilla::event::KeyCode::Left => TuiKeyCode::Left,
        ratzilla::event::KeyCode::Right => TuiKeyCode::Right,
        ratzilla::event::KeyCode::Up => TuiKeyCode::Up,
        ratzilla::event::KeyCode::Down => TuiKeyCode::Down,
        ratzilla::event::KeyCode::Tab if key.shift => TuiKeyCode::BackTab,
        ratzilla::event::KeyCode::Tab => TuiKeyCode::Tab,
        ratzilla::event::KeyCode::Delete => TuiKeyCode::Delete,
        ratzilla::event::KeyCode::Home => TuiKeyCode::Home,
        ratzilla::event::KeyCode::End => TuiKeyCode::End,
        ratzilla::event::KeyCode::PageUp => TuiKeyCode::PageUp,
        ratzilla::event::KeyCode::PageDown => TuiKeyCode::PageDown,
        ratzilla::event::KeyCode::Esc => TuiKeyCode::Esc,
        _ => return None,
    };
    Some(TuiKeyEvent::new(code, modifiers))
}

pub fn from_touch_key(code: &str, bits: u8) -> Option<TuiKeyEvent> {
    let mut modifiers = KeyModifiers::NONE;
    if bits & 1 != 0 {
        modifiers |= KeyModifiers::CONTROL;
    }
    if bits & 2 != 0 {
        modifiers |= KeyModifiers::ALT;
    }
    if bits & 4 != 0 {
        modifiers |= KeyModifiers::SHIFT;
    }
    let code = match code {
        "Escape" => TuiKeyCode::Esc,
        "Tab" => TuiKeyCode::Tab,
        "Enter" => TuiKeyCode::Enter,
        "Backspace" => TuiKeyCode::Backspace,
        "ArrowLeft" => TuiKeyCode::Left,
        "ArrowRight" => TuiKeyCode::Right,
        "ArrowUp" => TuiKeyCode::Up,
        "ArrowDown" => TuiKeyCode::Down,
        "Delete" => TuiKeyCode::Delete,
        "Home" => TuiKeyCode::Home,
        "End" => TuiKeyCode::End,
        "PageUp" => TuiKeyCode::PageUp,
        "PageDown" => TuiKeyCode::PageDown,
        _ => {
            let mut chars = code.chars();
            let character = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            TuiKeyCode::Char(character)
        }
    };
    Some(TuiKeyEvent::new(code, modifiers))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_keys_preserve_characters_special_keys_and_modifiers() {
        assert_eq!(
            from_touch_key("j", 0),
            Some(TuiKeyEvent::new(TuiKeyCode::Char('j'), KeyModifiers::NONE))
        );
        assert_eq!(
            from_touch_key("ArrowUp", 1),
            Some(TuiKeyEvent::new(TuiKeyCode::Up, KeyModifiers::CONTROL))
        );
        assert_eq!(from_touch_key("unknown-key", 0), None);
    }
}
