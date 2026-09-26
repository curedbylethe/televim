//! Single-line status bar showing mode + message.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, Mode};

pub fn render(app: &App, area: Rect, frame: &mut Frame<'_>) {
    let (label, style) = match app.mode {
        Mode::Normal => (Mode::Normal.label(), app.theme.mode_normal),
        Mode::Insert => (Mode::Insert.label(), app.theme.mode_insert),
        Mode::Visual => (Mode::Visual.label(), app.theme.mode_visual),
        Mode::Confirm => (Mode::Confirm.label(), app.theme.mode_confirm),
    };

    let line = Line::from(vec![
        Span::styled(format!(" {label} "), style),
        Span::raw(" "),
        Span::styled(app.status_text(), app.theme.text_dim),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}
