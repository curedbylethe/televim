//! The archived-chats overlay.
//!
//! It answers `:archived`: the conversations the account has archived, drawn over
//! the chat column. `Enter` puts the row under the cursor back in the visible
//! list, which is why the overlay exists at all — an archive with no way back
//! would be a deletion with a longer name.
//!
//! The same panel shape as the contacts and new-conversation overlays. An empty
//! list draws nothing and the status line says `archived — 0`; the state closes
//! itself when the last row leaves.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};

use crate::app::{App, Focus};

/// Where the overlay draws, or `None` when there is nothing to draw into.
fn overlay_area(app: &App, chat_list: Rect) -> Option<Rect> {
    if app.ui.focus == Focus::Input
        || !app.list.archived_is_open()
        || app.list.archived().is_empty()
    {
        return None;
    }

    let height = (app.list.archived().len() + 2).min(usize::from(chat_list.height));
    let Ok(height) = u16::try_from(height) else {
        return None;
    };
    if height < 3 {
        return None;
    }

    let y = chat_list.bottom().saturating_sub(height);
    Some(Rect::new(chat_list.x, y, chat_list.width, height))
}

pub fn render(app: &App, chat_list: Rect, frame: &mut Frame<'_>) {
    let Some(area) = overlay_area(app, chat_list) else {
        return;
    };

    frame.render_widget(Clear, area);

    let selected = app.list.archived_cursor();
    let items: Vec<ListItem> = app
        .list
        .archived()
        .iter()
        .take(usize::from(area.height).saturating_sub(2))
        .map(|chat| ListItem::new(Line::from(Span::raw(chat.title.clone()))))
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(app.ui.theme.border)
                .title(format!(" Archived · {} ", app.list.archived().len())),
        )
        .highlight_style(app.ui.theme.selection);

    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}
