//! Single-line status bar showing mode + message.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, Focus, INSERT_LABEL, Mode};

pub fn render(app: &App, area: Rect, frame: &mut Frame<'_>) {
    // The label is the mode, and the mode belongs to the conversation — except
    // that having the line at all is its insert mode, and a confirmation is a
    // question about the whole screen. Which *pane* has the focus is the
    // border's to say, not this row's.
    let (label, style) = match (app.focus, app.mode) {
        (_, Mode::Visual) => (Mode::Visual.label(), app.theme.mode_visual),
        (_, Mode::Confirm) => (Mode::Confirm.label(), app.theme.mode_confirm),
        (Focus::Input, _) => (INSERT_LABEL, app.theme.mode_insert),
        (Focus::ChatList, _) | (Focus::Conversation, Mode::Normal) => {
            (Mode::Normal.label(), app.theme.mode_normal)
        }
    };

    let line = Line::from(vec![
        Span::styled(format!(" {label} "), style),
        Span::raw(" "),
        Span::styled(app.status_text(), app.theme.text_dim),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}
