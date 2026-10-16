//! The session passphrase box, drawn in the rectangle the conversation had.
//!
//! One sentence says what is asked and why, the state and any refusal follow
//! it, and a refusal stays on screen until the next `⏎` rather than flashing.
//! The sentences are the design's, verbatim. The typed text is never drawn
//! here: it is in the bar below, as bullets.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{App, PASS_ASK, PassMode, PassPrompt};
use crate::wrap;

/// What the box says while the key of an unlock is derived.
const DERIVING: &str = "Deriving the key from the passphrase; ⏎ is refused while it runs.";

/// What the box says while a change is sealed.
const ENCRYPTING: &str =
    "Encrypting the session file with the new passphrase; ⏎ is refused while it runs.";

/// The change's explanation, above the first entry.
const CHANGE_ASK: &str = "The session file is encrypted with the current passphrase. Enter the new one twice. A typo would lock the file, because nothing else holds the key.";

/// The second entry's prompt.
const AGAIN: &str = "Enter the same passphrase again.";

/// The first sentence of a change that worked.
const CHANGED: &str =
    "The session passphrase has been changed. The session file is encrypted with the new one.";

/// What to update after a change that worked.
const REMINDER: &str = "Update TELEVIM_SESSION_PASSPHRASE or session_passphrase in the config file to the new one, if that is where the old passphrase was set. Otherwise the next launch will not open the file.";

/// The status word, shared with the sign-in view.
const CHECKING: &str = "Checking…";

pub fn render(app: &App, area: Rect, frame: &mut Frame<'_>) {
    let Some(pass) = app.session.pass.as_ref() else {
        return;
    };
    let title = match pass.mode {
        PassMode::Unlock => " Session file ",
        PassMode::Change => " Session passphrase ",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(app.ui.theme.border)
        .title(title);
    let width = area.width.saturating_sub(3).max(1);
    let mut lines: Vec<Line<'_>> = Vec::new();
    for (text, ink) in paragraphs(pass) {
        let style = if ink {
            app.ui.theme.text
        } else {
            app.ui.theme.text_dim
        };
        for range in wrap::wrap(text.as_str(), width) {
            lines.push(Line::from(vec![
                Span::raw(" "),
                Span::styled(text[range].to_owned(), style),
            ]));
        }
        lines.push(Line::from(""));
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// The box's paragraphs in order, each with whether it is in the reader's ink
/// (`true`) or the dim one.
fn paragraphs(pass: &PassPrompt) -> Vec<(String, bool)> {
    let dim = |text: &str| (text.to_owned(), false);
    let ink = |text: &str| (text.to_owned(), true);
    match (&pass.done, pass.mode) {
        (Some(Ok(())), _) => vec![ink(CHANGED), dim(REMINDER)],
        (Some(Err(reason)), _) => vec![ink(&format!(
            "The passphrase was not changed: {reason}. The session file was left as it was, so the old passphrase still opens the file."
        ))],
        (None, PassMode::Unlock) => {
            let mut out = vec![dim(PASS_ASK)];
            if pass.checking {
                out.push(dim(DERIVING));
                out.push(ink(CHECKING));
            } else if let Some(bad) = &pass.bad {
                out.push(ink(bad));
            }
            out
        }
        (None, PassMode::Change) => {
            let mut out = vec![dim(CHANGE_ASK)];
            if pass.first.is_some() && !pass.checking {
                out.push(dim(AGAIN));
            }
            if pass.checking {
                out.push(dim(ENCRYPTING));
                out.push(ink(CHECKING));
            } else if let Some(bad) = &pass.bad {
                out.push(ink(bad));
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn rows(app: &App) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("a test backend");
        terminal.draw(|frame| app.render(frame)).expect("draws");
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    fn key(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn flat(app: &App) -> String {
        rows(app).join("\n")
    }

    #[test]
    fn the_question_is_drawn_over_the_right_column_beside_the_cached_list() {
        let mut app = App::mock();
        app.session_key_required(false);
        let screen = flat(&app);

        assert!(screen.contains("Session file"));
        assert!(
            screen.contains("so televim needs the passphrase"),
            "{screen}"
        );
        assert!(
            screen.contains("· cached"),
            "the list is the cache: {screen}"
        );
        assert!(screen.contains("Passphrase"), "the bar is titled: {screen}");
    }

    #[test]
    fn the_typed_passphrase_is_bullets_on_every_row() {
        let mut app = App::mock();
        app.session_key_required(false);
        for c in "lantern".chars() {
            key(&mut app, KeyCode::Char(c));
        }
        let screen = flat(&app);

        assert!(!screen.contains("lantern"), "{screen}");
        assert!(screen.contains("•••••••"), "{screen}");
    }

    #[test]
    fn checking_is_shown_and_a_refusal_stays_until_the_next_try() {
        let mut app = App::mock();
        app.session_key_required(false);
        key(&mut app, KeyCode::Char('x'));
        key(&mut app, KeyCode::Enter);
        let screen = flat(&app);
        assert!(screen.contains("Checking…"), "{screen}");
        assert!(screen.contains("Deriving the key"), "{screen}");

        app.session_key_required(true);
        let screen = flat(&app);
        assert!(screen.contains("that passphrase did not open"), "{screen}");
        assert!(screen.contains("left as it was"), "{screen}");
    }

    #[test]
    fn escape_returns_to_the_conversation_and_the_box_is_gone() {
        let mut app = App::mock();
        app.session_key_required(false);
        key(&mut app, KeyCode::Esc);
        let screen = flat(&app);

        assert!(!screen.contains("Session file"), "{screen}");
        assert!(screen.contains("· cached"), "still locked: {screen}");
    }

    #[test]
    fn an_empty_list_has_no_cached_note() {
        let mut app = App::mock();
        app.set_chats(Vec::new());
        app.session_key_required(false);

        assert!(!flat(&app).contains("· cached"));
    }
}
