//! Input bar shown above the status line.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{App, Mode};

pub fn render(app: &App, area: Rect, frame: &mut Frame<'_>) {
    let prefix = if app.mode == Mode::Insert {
        app.prompt_prefix()
    } else {
        ""
    };

    let hint = match app.mode {
        Mode::Normal => {
            " i/a: insert  r: reply  e: edit  dd: delete  D: dismiss  v: visual  /: search  :: cmd  q: quit"
        }
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
