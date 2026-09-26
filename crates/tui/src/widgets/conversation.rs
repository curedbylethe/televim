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

use crate::app::{App, FetchDirection, JUMP_LABEL};

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
    let jumping = app.pending_jump().is_some();
    // A page that replaces an empty window has no edge to be announced at, so
    // it is announced in place of the messages: there is nothing else to say
    // while a conversation is being opened.
    let opening = app.is_fetching(FetchDirection::Latest) && app.conversation.window.is_empty();
    // Every row above the messages, which is what the cursor's own row has to
    // be counted past.
    let above = usize::from(older) + usize::from(jumping);
    let reserved = above + usize::from(newer);
    let budget = usize::from(body.height).saturating_sub(reserved);

    let window = &app.conversation.window;
    let start = app.viewport_start(budget);

    let mut items: Vec<ListItem> = Vec::with_capacity(budget + reserved);
    if older {
        items.push(loading(app, FetchDirection::Older.label()));
    }
    if jumping {
        // A jump replaces the window rather than extending it, so it is said
        // where the messages are: the page it is waiting for has no edge of the
        // window on show to sit at.
        items.push(loading(app, JUMP_LABEL));
    }
    if opening {
        items.push(loading(app, FetchDirection::Latest.label()));
    }
    items.extend(
        window
            .iter()
            .skip(start)
            .take(budget)
            .map(|message| message_line(app, message)),
    );
    if newer {
        items.push(loading(app, FetchDirection::Newer.label()));
    }

    // Only a message can be the selection, so an empty window has none — the
    // indicator rows are not places the cursor can be.
    let mut state = ListState::default();
    state.select((!window.is_empty()).then(|| app.vim.cursor().saturating_sub(start) + above));

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

/// A row saying what is being fetched.
///
/// The label is a `&'static str` rather than a borrow of anything: the row
/// outlives the frame's own borrows, and every label here is a constant.
fn loading(app: &App, label: &'static str) -> ListItem<'static> {
    ListItem::new(Line::from(Span::styled(label, app.theme.text_dim)))
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
    use crate::app::App;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::{Buffer, Cell};

    fn area(width: u16, height: u16) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    /// The whole frame, drawn into an in-memory terminal.
    ///
    /// The panel's arithmetic is testable on its own; what this adds is that the
    /// arithmetic reaches the screen. A slice computed correctly and drawn a row
    /// out is still wrong, and only a screen says so.
    fn screen(app: &App, width: u16, height: u16) -> Buffer {
        let mut terminal =
            Terminal::new(TestBackend::new(width, height)).expect("the test backend builds");
        terminal
            .draw(|frame| app.render(frame))
            .expect("the frame draws");

        terminal.backend().buffer().clone()
    }

    /// One row of the screen, as the text on it.
    fn row(buffer: &Buffer, y: u16) -> String {
        let width = usize::from(buffer.area.width);
        let start = usize::from(y) * width;

        buffer.content()[start..start + width]
            .iter()
            .map(Cell::symbol)
            .collect()
    }

    /// One column of the screen, over the rows the messages are drawn on.
    ///
    /// The panel's own borders cross every column, so reading one whole would
    /// find them and mistake them for content. The panel is the top block of the
    /// frame: a title row, the messages, a border, then the input bar and the
    /// status line below it.
    fn body_column(buffer: &Buffer, x: usize) -> String {
        let width = usize::from(buffer.area.width);
        let rows = 1..(usize::from(buffer.area.height) - 5);

        rows.map(|y| buffer.content()[y * width + x].symbol())
            .collect()
    }

    /// The scrollbar's column, which is the last one before the panel's edge.
    fn gutter_content(buffer: &Buffer) -> String {
        body_column(buffer, usize::from(buffer.area.width) - 2)
    }

    /// A message long enough to reach the edge of any panel.
    fn long_message(chat_id: i64) -> domain::message::Message {
        use std::borrow::Cow;

        use domain::message::MessageStatus;

        domain::message::Message {
            id: 1,
            chat_id,
            text: Cow::Owned("x".repeat(400)),
            timestamp: 0,
            status: MessageStatus::Received,
            is_outgoing: false,
            reply_to: None,
        }
    }

    /// An application whose conversation has unread messages in front of what is
    /// loaded, with a jump asked for.
    ///
    /// Built through the application's own interface rather than by reaching into
    /// it, because what is under test is what a reader's keystroke puts on the
    /// screen.
    fn jumping() -> App {
        use std::borrow::Cow;

        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use domain::chat::{Chat, ChatKind};
        use domain::message::{Message, MessageStatus};

        const CHAT: i64 = 7;

        let mut app = App::new();
        app.set_chats(vec![Chat {
            id: CHAT,
            title: "Ada Lovelace".into(),
            kind: ChatKind::Private,
            last_message: Some("See you at the demo.".into()),
            unread_count: 2,
            // The conversation runs to 20; the page below stops well short of it.
            last_message_id: Some(20),
            last_timestamp: Some(1_730_000_000),
        }]);
        app.select_chat(0);
        app.apply_latest(
            (1..=5)
                .map(|id| Message {
                    id,
                    chat_id: CHAT,
                    text: Cow::Borrowed("text"),
                    timestamp: 1_730_000_000 + id,
                    status: MessageStatus::Received,
                    is_outgoing: false,
                    reply_to: None,
                })
                .collect(),
        );

        for _ in 0..2 {
            app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        }

        app
    }

    // ---- the panel's own geometry ---------------------------------------

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

    // ---- what reaches the screen ----------------------------------------

    #[test]
    fn an_empty_window_draws_a_panel_with_nothing_in_it() {
        let screen = screen(&App::new(), 80, 24);

        assert!(row(&screen, 0).contains("Conversation"));
        assert!(
            !screen.content.iter().any(|cell| cell.symbol() == "["),
            "a conversation with nothing loaded has no message rows"
        );
    }

    #[test]
    fn a_pinned_conversation_draws_the_end_of_the_window() {
        let screen = screen(&App::mock(), 80, 10);

        assert!(
            row(&screen, 0).contains("Conversation (10/10)"),
            "the title says where the reader is: {}",
            row(&screen, 0)
        );
        assert!(
            row(&screen, 1).contains("[you] And the benchmarks."),
            "the panel is as tall as the terminal allows: {}",
            row(&screen, 1)
        );
        assert!(
            row(&screen, 4).contains("[them] See you at the demo."),
            "and ends at the newest message: {}",
            row(&screen, 4)
        );
        assert!(
            !row(&screen, 1).contains("Hey, is the build green?"),
            "the top of the window is off the screen"
        );
    }

    #[test]
    fn a_taller_panel_shows_more_of_the_window() {
        let screen = screen(&App::mock(), 80, 24);

        assert!(
            row(&screen, 1).contains("Hey, is the build green?"),
            "the whole window fits: {}",
            row(&screen, 1)
        );
        assert!(row(&screen, 10).contains("See you at the demo."));
    }

    #[test]
    fn the_title_says_where_the_reader_is() {
        let mut app = App::mock();
        app.vim.set_cursor(8);

        let screen = screen(&app, 80, 24);

        assert!(
            row(&screen, 0).contains("Conversation (9/10)"),
            "{}",
            row(&screen, 0)
        );
    }

    #[test]
    fn a_page_in_flight_is_announced_at_the_edge_it_is_coming_from() {
        let mut older = App::mock();
        older.begin_fetch(FetchDirection::Older);
        let with_older = screen(&older, 80, 10);

        assert!(
            row(&with_older, 1).contains("Loading older…"),
            "the row in front of the messages: {}",
            row(&with_older, 1)
        );

        let mut newer = App::mock();
        newer.begin_fetch(FetchDirection::Newer);
        let with_newer = screen(&newer, 80, 10);

        assert!(
            row(&with_newer, 4).contains("Loading newer…"),
            "the row behind them: {}",
            row(&with_newer, 4)
        );
    }

    /// A page that replaces an empty window has no edge to sit at, so it is
    /// announced where the messages would be: a conversation being opened says
    /// something rather than showing nothing.
    #[test]
    fn a_conversation_being_opened_says_so() {
        let mut app = App::mock();
        app.select_chat(1);
        app.begin_fetch(FetchDirection::Latest);

        let screen = screen(&app, 80, 10);

        assert!(
            row(&screen, 1).contains("Loading…"),
            "the panel's first row: {}",
            row(&screen, 1)
        );
        assert!(
            !screen.content.iter().any(|cell| cell.symbol() == "["),
            "and no message rows, because there are none"
        );
    }

    /// A jump replaces the window rather than extending it, so there is no edge
    /// of the window on show for its row to sit at: it is said where the messages
    /// are, and the messages follow it.
    #[test]
    fn a_jump_in_flight_is_announced_where_the_messages_would_be() {
        let app = jumping();
        assert!(
            app.pending_jump().is_some(),
            "the fixture has asked for a jump"
        );

        let screen = screen(&app, 80, 10);

        assert!(
            row(&screen, 1).contains("Jumping"),
            "the panel's first row: {}",
            row(&screen, 1)
        );
        assert!(
            row(&screen, 2).contains("text"),
            "and the messages start behind it: {}",
            row(&screen, 2)
        );
    }

    #[test]
    fn the_scrollbar_appears_only_when_there_is_somewhere_to_scroll() {
        // Ten messages in a four-row panel: the window is taller than the panel.
        let short = screen(&App::mock(), 80, 10);
        assert!(
            !gutter_content(&short).trim().is_empty(),
            "a window taller than the panel has a position worth showing: {:?}",
            gutter_content(&short)
        );

        // The same window in a panel that shows all of it: nothing to show.
        let tall = screen(&App::mock(), 80, 24);
        assert!(
            gutter_content(&tall).trim().is_empty(),
            "a panel that shows the whole window has no scrollbar to draw: {:?}",
            gutter_content(&tall)
        );
    }

    /// The column the bar sits in is one the messages gave up, so a long message
    /// has to stop before it rather than be written under it.
    #[test]
    fn a_long_message_stops_at_the_column_the_bar_was_given() {
        let mut app = App::mock();
        let chat_id = app.conversation.window.chat_id;
        app.apply_latest(vec![long_message(chat_id)]);

        let screen = screen(&app, 80, 10);
        let last_message_column = usize::from(screen.area.width) - 3;

        assert!(
            !body_column(&screen, last_message_column).trim().is_empty(),
            "the messages reach the edge they were given: {:?}",
            body_column(&screen, last_message_column)
        );
        assert!(
            gutter_content(&screen).trim().is_empty(),
            "and stop there: {:?}",
            gutter_content(&screen)
        );
    }
}
