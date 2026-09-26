//! The conversation panel.
//!
//! The window holds far more than a terminal can show, so the panel renders the
//! slice the reader is in rather than the whole of it: at most a screenful of
//! rows is built per frame, whatever the window's ceiling is. Nothing is cached
//! between frames — a couple of hundred rows is cheap to build, and a cache
//! would be a second thing to keep in step with the window.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, List, ListItem, ListState, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use crate::app::{App, FetchDirection};

/// How many columns the messages keep for themselves before a scrollbar is
/// worth showing beside them.
///
/// A narrow panel has no room to give: the bar would cost more than it tells
/// the reader.
const MIN_BODY_WIDTH: u16 = 8;

pub fn render(app: &App, area: Rect, frame: &mut Frame<'_>) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(app.theme.border)
        .title(conversation_title(app));

    // Drawn apart from the list so that the bar can have a column of its own
    // rather than being painted over the end of a message.
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (body, gutter) = split_gutter(inner);
    app.record_rows(usize::from(body.height));

    let older = app.is_fetching(FetchDirection::Older);
    let newer = app.is_fetching(FetchDirection::Newer);
    let reserved = usize::from(older) + usize::from(newer);
    let budget = usize::from(body.height).saturating_sub(reserved);

    let window = &app.conversation.window;
    let start = app.viewport_start(budget);

    let mut items: Vec<ListItem> = Vec::with_capacity(budget + reserved);
    if older {
        items.push(loading(app, FetchDirection::Older));
    }
    items.extend(
        window
            .iter()
            .skip(start)
            .take(budget)
            .map(|message| message_line(app, message)),
    );
    if newer {
        items.push(loading(app, FetchDirection::Newer));
    }

    // Only a message can be the selection, so an empty window has none — the
    // indicator rows are not places the cursor can be.
    let mut state = ListState::default();
    state.select(
        (!window.is_empty()).then(|| app.vim.cursor().saturating_sub(start) + usize::from(older)),
    );

    let list = List::new(items).highlight_style(app.theme.selection);
    frame.render_stateful_widget(list, body, &mut state);

    if let Some(gutter) = gutter {
        render_scrollbar(app, gutter, frame, start, budget);
    }
}

/// The panel's title: where in what is loaded the reader is.
///
/// Counted in messages rather than in lines, because that is the only measure
/// the client has — how many lines a message wraps onto is a question about the
/// width of a terminal.
fn conversation_title(app: &App) -> String {
    let total = app.conversation.window.len();
    if total == 0 {
        return " Conversation ".to_string();
    }

    format!(" Conversation ({}/{total}) ", app.vim.cursor() + 1)
}

/// One message as a row.
fn message_line(app: &App, message: &domain::message::Message) -> ListItem<'static> {
    let who = if message.is_outgoing { "you" } else { "them" };

    ListItem::new(Line::from(vec![
        Span::styled(format!("[{who}] "), app.theme.text_dim),
        Span::styled(message.text.to_string(), app.theme.text),
    ]))
}

/// A row saying which way a page is being fetched.
fn loading(app: &App, direction: FetchDirection) -> ListItem<'static> {
    ListItem::new(Line::from(Span::styled(
        direction.label(),
        app.theme.text_dim,
    )))
}

/// Splits the panel's inside into the messages and a column for the scrollbar.
fn split_gutter(inner: Rect) -> (Rect, Option<Rect>) {
    if inner.width < MIN_BODY_WIDTH + 1 {
        return (inner, None);
    }

    let gutter = Rect {
        x: inner.x + inner.width - 1,
        width: 1,
        ..inner
    };
    let body = Rect {
        width: inner.width - 1,
        ..inner
    };

    (body, Some(gutter))
}

/// Draws how far into the loaded messages the reader has scrolled.
///
/// The bar measures the window rather than the conversation: the client holds a
/// window, not a history, so the only extent it can honestly show is the one it
/// has.
fn render_scrollbar(app: &App, area: Rect, frame: &mut Frame<'_>, start: usize, budget: usize) {
    let total = app.conversation.window.len();
    if total <= budget {
        return;
    }

    let mut state = ScrollbarState::new(total)
        .position(start)
        .viewport_content_length(budget);

    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .thumb_style(app.theme.text)
        .track_style(app.theme.text_dim);

    frame.render_stateful_widget(scrollbar, area, &mut state);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(width: u16, height: u16) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    #[test]
    fn a_wide_panel_gives_the_scrollbar_a_column_of_its_own() {
        let (body, gutter) = split_gutter(area(40, 10));

        assert_eq!(body, area(39, 10), "the messages lose one column");
        assert_eq!(
            gutter,
            Some(Rect {
                x: 39,
                width: 1,
                ..area(1, 10)
            }),
            "the bar sits in the column that was given up"
        );
    }

    #[test]
    fn a_narrow_panel_keeps_its_columns_for_the_messages() {
        let (body, gutter) = split_gutter(area(MIN_BODY_WIDTH, 10));

        assert_eq!(body, area(MIN_BODY_WIDTH, 10));
        assert_eq!(gutter, None, "there is no room to give away");
    }
}
