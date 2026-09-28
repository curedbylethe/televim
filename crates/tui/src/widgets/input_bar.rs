//! Input bar shown above the status line.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{App, Focus, Mode};

/// The hint while the conversation has the focus in Normal mode.
///
/// A constant so its length can be checked: the hints have to fit one row of the
/// widest terminal the client assumes, and adding reply, edit and delete meant
/// shortening the mode keys rather than letting the line run past the bar and be
/// clipped.
const NORMAL_HINT: &str = " i/a: insert  r: reply  e: edit  dd: del  D: dismiss  v  /  ::  q: quit";

/// The hint while the chat list has the focus.
const CHAT_LIST_HINT: &str = " j/k: chat  Enter: open  Tab: pane  h: conversation";

/// The hint while a selection is being made over the messages.
const VISUAL_HINT: &str = " d: delete  y: yank  r: reply  Esc: cancel";

/// The hint while a deletion is waiting to be confirmed.
const CONFIRM_HINT: &str = " y: delete  n/Esc: cancel";

pub fn render(app: &App, area: Rect, frame: &mut Frame<'_>) {
    // The line is the focus rather than a pane that takes it, so being on the
    // line is what says whether the buffer or a hint is drawn.
    let composing = app.focus == Focus::Input;

    let hint = match (app.focus, app.mode) {
        (Focus::Input, _) => "",
        (Focus::ChatList, _) => CHAT_LIST_HINT,
        (Focus::Conversation, Mode::Normal) => NORMAL_HINT,
        (Focus::Conversation, Mode::Visual) => VISUAL_HINT,
        (Focus::Conversation, Mode::Confirm) => CONFIRM_HINT,
    };

    let line = if composing {
        Line::from(vec![
            Span::styled(app.prompt_prefix(), app.theme.text_dim),
            Span::styled(app.line.text(), app.theme.text),
            Span::styled("█", app.theme.text_dim), // caret
        ])
    } else {
        Line::from(Span::styled(hint, app.theme.text_dim))
    };

    // The bar does not take the focus visually, it *is* the focus, so it is the
    // one pane whose border is on exactly when the line is.
    let border = if composing {
        app.theme.border_focused
    } else {
        app.theme.border
    };

    let paragraph = Paragraph::new(line).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border)
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
        for hint in [NORMAL_HINT, CHAT_LIST_HINT, VISUAL_HINT, CONFIRM_HINT] {
            assert!(
                hint.chars().count() <= 80 - 2,
                "the hint is {} columns, and the bar has 78: {hint:?}",
                hint.chars().count()
            );
        }
    }
}
