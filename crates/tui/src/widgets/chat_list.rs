//! Chat list panel.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};

use crate::app::App;

pub fn render(app: &App, area: Rect, frame: &mut Frame<'_>) {
    let items: Vec<ListItem> = app
        .chats()
        .iter()
        .map(|c| {
            let unread = if c.unread_count > 0 {
                format!(" ({})", c.unread_count)
            } else {
                String::new()
            };
            ListItem::new(Line::from(vec![
                Span::styled(c.title.clone(), app.theme.text),
                Span::styled(unread, app.theme.text_dim),
            ]))
        })
        .collect();

    let title = format!(" Chats ({}) ", app.chats().len());
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(app.theme.border_focused)
                .title(title),
        )
        .highlight_style(app.theme.selection);

    let mut state = ListState::default();
    state.select(Some(app.selected_chat));
    frame.render_stateful_widget(list, area, &mut state);
}

// Kept for potential future use.
#[allow(dead_code)]
fn _unused(_: Style) {}
