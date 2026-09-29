//! Single-line status bar showing mode + message.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, Mode};
use crate::widgets::input_bar;

pub fn render(app: &App, area: Rect, frame: &mut Frame<'_>) {
    // The label is the mode, and it belongs to whichever pane the key is going
    // to — so it is the line's own when the line has the focus, because a line
    // has a mode of its own and being on it is no longer the whole of what it is
    // doing. A confirmation is a question about the whole screen. Which *pane*
    // has the focus is the border's to say, not this row's.
    let label = input_bar::mode_label(app);

    let style = match (app.focus == crate::app::Focus::Input, app.mode) {
        // Named rather than wildcarded, for the same reason `mode_label` names
        // them: a mode that only the conversation can be in should say so.
        (false, Mode::Visual) => app.theme.mode_visual,
        (false, Mode::Confirm) => app.theme.mode_confirm,
        // The line's insert is the one mode that is not the conversation's, and
        // it is the one the bar's border is on as well — a reader who cannot
        // see where the caret is should at least be able to see which mode the
        // keys they are about to press will mean.
        (true, Mode::Normal) if label != Mode::Normal.label() => app.theme.mode_insert,
        _ => app.theme.mode_normal,
    };

    let line = Line::from(vec![
        Span::styled(format!(" {label} "), style),
        Span::raw(" "),
        Span::styled(app.status_text(), app.theme.text_dim),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}
