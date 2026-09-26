//! Input bar shown above the status line.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{App, Mode};

/// The normal-mode hint.
///
/// A constant so its length can be checked: the hints have to fit one row of the
/// widest terminal the client assumes, and adding reply, edit and delete meant
/// shortening the mode keys rather than letting the line run past the bar and be
/// clipped.
const NORMAL_HINT: &str = " i/a: insert  r: reply  e: edit  dd: del  D: dismiss  v  /  ::  q: quit";

pub fn render(app: &App, area: Rect, frame: &mut Frame<'_>) {
    let prefix = if app.mode == Mode::Insert {
        app.prompt_prefix()
    } else {
        ""
    };

    let hint = match app.mode {
        Mode::Normal => NORMAL_HINT,
        Mode::Insert => "",
        Mode::Visual => " d: delete  y: yank  r: reply  Esc: cancel",
        Mode::Confirm => " y: delete  n/Esc: cancel",
    };

    let line = if app.mode == Mode::Insert {
        Line::from(vec![
            Span::styled(prefix, app.theme.text_dim),
            Span::styled(app.input.clone(), app.theme.text),
            Span::styled("█", app.theme.text_dim), // caret
        ])
    } else {
        Line::from(Span::styled(hint, app.theme.text_dim))
    };

    let paragraph = Paragraph::new(line).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(app.theme.border)
            .title(" Input "),
    );
    frame.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An eighty-column terminal is what the client assumes everywhere else, and
    /// the bar's two borders come out of it. A hint longer than this is clipped
    /// mid-word, which reads as a bug rather than as a hint.
    #[test]
    fn the_normal_hint_fits_one_row_of_the_widest_assumed_terminal() {
        assert!(
            NORMAL_HINT.chars().count() <= 80 - 2,
            "the hint is {} columns, and the bar has 78: {NORMAL_HINT:?}",
            NORMAL_HINT.chars().count()
        );
    }
}
