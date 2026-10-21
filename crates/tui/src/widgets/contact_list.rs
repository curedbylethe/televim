//! The contacts directory overlay.
//!
//! It answers `:contacts`: the account's own address book, drawn over the chat
//! column the way the new-conversation results are. The two lists are the same
//! shape — a bordered panel anchored to the bottom of that column, reverse video
//! on the cursor, no highlight symbol — and differ only in what they hold: a
//! search shows the answer to a query, this shows everybody.
//!
//! **Empty draws nothing.** The status line already says `contacts — 0`, and an
//! empty box over the chat list would cover the list the reader is about to go
//! back to while telling them less than the sentence does. The same rule the
//! new-conversation overlay follows, for the same reason.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};

use crate::app::{App, Focus};
use crate::wrap::columns;

/// Where the overlay draws, or `None` when there is nothing to draw into.
fn overlay_area(app: &App, chat_list: Rect) -> Option<Rect> {
    let contacts = &app.conversation.contacts;
    if app.ui.focus == Focus::Input || !contacts.is_active() || contacts.items().is_empty() {
        return None;
    }

    let height = (contacts.items().len() + 2).min(usize::from(chat_list.height));
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

    let contacts = &app.conversation.contacts;
    let width = usize::from(area.width).saturating_sub(2);
    let selected = contacts.selected();
    let items: Vec<ListItem> = contacts
        .items()
        .iter()
        .take(usize::from(area.height).saturating_sub(2))
        .map(|candidate| {
            let mut label = candidate.display_name.clone();
            if let Some(username) = candidate.username.as_deref() {
                label.push_str("  @");
                label.push_str(username);
            }
            ListItem::new(Line::from(Span::raw(truncate(&label, width))))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(app.ui.theme.border)
                .title(format!(" Contacts · {} ", contacts.items().len())),
        )
        .highlight_style(app.ui.theme.selection);

    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}

/// Cuts a label to `width` columns, counting a wide character as two.
fn truncate(label: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for ch in label.chars() {
        let cells = columns(&ch.to_string());
        if used + cells > width {
            break;
        }
        out.push(ch);
        used += cells;
    }
    out
}
