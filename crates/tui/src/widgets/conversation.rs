//! Conversation view with a sliding window.
//!
//! Only the visible messages (plus a small prefetch) are materialised as
//! `ListItem`s. The `VimState` cursor is respected for highlighting.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};

use crate::app::App;

/// How many messages beyond the visible area to prefetch.
const PREFETCH: usize = 20;

pub fn render(app: &App, area: Rect, frame: &mut Frame<'_>) {
    // Naive sliding window: centre on cursor, clamp to prefetch window.
    // A proper "visible rows" calculation lands in a later PR.
    let total = app.messages.len();
    let cursor = app.vim.cursor().min(total.saturating_sub(1));
    let window_start = cursor.saturating_sub(PREFETCH / 2);
    let window_end = (window_start + PREFETCH).min(total);

    let slice = &app.messages[window_start..window_end];

    let items: Vec<ListItem> = slice
        .iter()
        .map(|m| {
            let who = if m.is_outgoing { "you" } else { "them" };
            let prefix = format!("[{who}] ");
            ListItem::new(Line::from(vec![
                Span::styled(prefix, app.theme.text_dim),
                Span::styled(m.text.to_string(), app.theme.text),
            ]))
        })
        .collect();

    let title = format!(" Conversation ({cursor}/{total}) ");
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(app.theme.border)
                .title(title),
        )
        .highlight_style(app.theme.selection);

    let mut state = ListState::default();
    // Translate absolute cursor to window-relative index.
    state.select(Some(cursor.saturating_sub(window_start)));
    frame.render_stateful_widget(list, area, &mut state);
}
