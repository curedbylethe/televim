//! Chat list panel.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};

use crate::app::{App, Focus};

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
                Span::styled(c.title.clone(), app.ui.theme.text),
                Span::styled(unread, app.ui.theme.text_dim),
            ]))
        })
        .collect();

    let title = format!(" Chats ({}) ", app.chats().len());
    // The focused pane's border is the only thing that says where the keys go,
    // so the two panes cannot both be drawn as though they had it.
    let border = if app.ui.focus == Focus::ChatList {
        app.ui.theme.border_focused
    } else {
        app.ui.theme.border
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border)
                .title(title),
        )
        .highlight_style(app.ui.theme.selection);

    let mut state = ListState::default();
    state.select(Some(app.list.selected_chat));
    frame.render_stateful_widget(list, area, &mut state);
}

// Kept for potential future use.
#[allow(dead_code)]
fn _unused(_: Style) {}
