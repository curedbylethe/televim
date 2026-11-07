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
