//! The conversation panel.
//!
//! The window holds far more than a terminal can show, so the panel renders the
//! slice the reader is in rather than the whole of it: at most a screenful of
//! rows is built per frame, whatever the window's ceiling is. A message is as
//! tall as its text is at the width the panel gave it, and the rows it takes up
//! are worked out in [`crate::rows`] — the panel asks for them rather than
//! counting anything itself. Nothing is cached between frames: a couple of
//! hundred rows is cheap to build, and a cache would be a second thing to keep
//! in step with the window.

use std::ops::Range;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, List, ListItem, ListState, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use domain::message::Message;

use crate::app::{App, FetchDirection, JUMP_LABEL};
use crate::rows;

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
    app.record_body(body.width);

    let reserved = app.reserved();
    let above = reserved.above();
    let budget = usize::from(body.height).saturating_sub(above + reserved.below());

    let layout = app.row_layout();
    let view = app.viewport(&layout, budget);

    let mut items: Vec<ListItem> =
        Vec::with_capacity(reserved.above() + reserved.below() + view.rows);
    if reserved.older {
        items.push(loading(app, FetchDirection::Older.label()));
    }
    if reserved.jumping {
        // A jump replaces the window rather than extending it, so it is said
        // where the messages are: the page it is waiting for has no edge of the
        // window on show to sit at.
        items.push(loading(app, JUMP_LABEL));
    }
    // Said in place of the messages rather than at an edge of the window, which
    // an empty one does not have. It is not one of the reserved rows either: a
    // window that is empty is never taller than the row the announcement takes.
    if app.is_fetching(FetchDirection::Latest) && app.conversation.window.is_empty() {
        items.push(loading(app, FetchDirection::Latest.label()));
    }

    let window = &app.conversation.window;
    let mut drawn = 0;
    for span in layout.iter().skip(view.start) {
        // A message is drawn whole or not at all: half a message with no way to
        // scroll to the rest of it is a different, worse thing than a row the
        // panel did not fill.
        if drawn >= view.budget {
            break;
        }
        let Some(message) = window.get(span.index) else {
            break;
        };

        let wrapped = rows::message_rows(app, message, body.width);
        for (row, range) in wrapped.iter().enumerate().skip(view.skip) {
            if drawn >= view.budget {
                break;
            }
            let first = row == 0;
            let last = row + 1 == wrapped.len();
            items.push(message_row(app, message, range, first, last, body.width));
            drawn += 1;
        }
    }

    if reserved.newer {
        items.push(loading(app, FetchDirection::Newer.label()));
    }

    // Only a message can be the selection, so an empty window has none — the
    // indicator rows are not places the cursor can be.
    let mut state = ListState::default();
    state.select((!window.is_empty()).then(|| view.selection + above));

    let list = List::new(items).highlight_style(app.theme.selection);
    frame.render_stateful_widget(list, body, &mut state);

    if let Some(gutter) = gutter {
        render_scrollbar(app, gutter, frame, &view);
    }
}

/// The panel's title: where in what is loaded the reader is, and what a search
/// found there.
///
/// Counted in messages, because that is where in the conversation the reader is:
/// the cursor stands on a message however many rows that message is, and a
/// message number is the one position that survives a page landing.
fn conversation_title(app: &App) -> String {
    let search = search_note(app);
    let total = app.conversation.window.len();
    if total == 0 {
        return format!(" Conversation{search} ");
    }

    format!(" Conversation ({}/{total}){search} ", app.vim.cursor() + 1)
}

/// What the title says about a search, if one is running.
///
/// The count is how many matches are held, which is the number of messages that
/// can be marked; how many there are in all is the status line's to say.
fn search_note(app: &App) -> String {
    if !app.search().is_active() {
        return String::new();
    }

    format!(" · {} match(es)", app.search().len())
}

/// One row of one message: the slice of its text `range` names, which the
/// panel's width has already made room for.
///
/// The decorations belong to the message rather than to the row. Who it is
/// from and what it quotes say which message this is, so they are drawn once,
/// on the first row — the row the reader reaches the message by. A pending or
/// failed send is a fact about the whole message, so it goes on the last row,
/// which is the one with room for it; a continuation row repeats neither, and
/// a reader scrolling into the middle of a long message is reading the same
/// speaker they were a moment ago.
///
/// The row was cut at a width that already made room for both, in
/// [`rows::message_rows`], so nothing here is clipped by the terminal and lost.
///
/// A message a search matched has its spans patched with
/// [`Theme::match_style`](crate::theme::Theme::match_style) rather than given a
/// style of their own, so that the cursor's `REVERSED` selection composes on top
/// of it instead of replacing it.
fn message_row(
    app: &App,
    message: &Message,
    range: &Range<usize>,
    first: bool,
    last: bool,
    width: u16,
) -> ListItem<'static> {
    let mut spans = Vec::new();

    if first {
        let who = if message.is_outgoing { "you" } else { "them" };
        spans.push(Span::styled(format!("[{who}] "), app.theme.text_dim));

        if let Some(reply_to) = message.reply_to {
            spans.push(Span::styled(
                rows::reply_prefix(app, reply_to, width),
                app.theme.text_dim,
            ));
        }
    }

    spans.push(Span::styled(
        message.text[range.clone()].to_owned(),
        app.theme.text,
    ));

    if last && let Some(suffix) = rows::status_suffix(app, message) {
        spans.push(Span::styled(suffix, app.theme.text_dim));
    }

    if app.search().is_match(message.id) {
        for span in &mut spans {
            span.style = span.style.patch(app.theme.match_style);
        }
    }

    ListItem::new(Line::from(spans))
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
/// The bar measures the rows the window was laid out in, and so does the slice
/// beside it: both come from one layout, so the thumb always describes what is
/// on the screen. The bar measures the window rather than the conversation: the
/// client holds a window, not a history, so the only extent it can honestly
/// show is the one it has.
fn render_scrollbar(app: &App, area: Rect, frame: &mut Frame<'_>, view: &rows::Slice) {
    if view.total <= view.budget {
        return;
    }

    let mut state = ScrollbarState::new(view.total)
        .position(view.start_row)
        .viewport_content_length(view.budget);

    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .thumb_style(app.theme.text)
        .track_style(app.theme.text_dim);

    frame.render_stateful_widget(scrollbar, area, &mut state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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

    /// Sends one keystroke to the application, as the reader would.
    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    /// Types `text` one character at a time.
    fn type_text(app: &mut App, text: &str) {
        for character in text.chars() {
            press(app, KeyCode::Char(character));
        }
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

    /// One cell of the screen.
    fn cell(buffer: &Buffer, x: u16, y: u16) -> &Cell {
        let width = usize::from(buffer.area.width);

        &buffer.content()[usize::from(y) * width + usize::from(x)]
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

    /// The rows the messages are drawn on, in the panel's body.
    fn message_rows(buffer: &Buffer) -> Vec<u16> {
        let height = usize::from(buffer.area.height);
        (1..height - 5).map(|y| y as u16).collect()
    }

    /// One row of the panel's body: from its first message column to the last,
    /// without the borders and the chat list beside it.
    fn body_row(buffer: &Buffer, y: u16) -> String {
        let last = usize::from(buffer.area.width) - 2;

        (usize::from(BODY_X)..last)
            .map(|x| cell(buffer, x as u16, y).symbol())
            .collect()
    }

    /// Whether a row of the panel's body has anything on it.
    fn drawn(buffer: &Buffer, y: u16) -> bool {
        let last = usize::from(buffer.area.width) - 2;
        (usize::from(BODY_X)..last).any(|x| cell(buffer, x as u16, y).symbol() != " ")
    }

    /// The lowest row the panel drew a message on, if it drew one.
    fn last_drawn(buffer: &Buffer) -> Option<u16> {
        message_rows(buffer)
            .into_iter()
            .rev()
            .find(|y| drawn(buffer, *y))
    }

    /// The rows the bar's thumb covers, counted from the top of the track.
    fn thumb(buffer: &Buffer) -> Vec<usize> {
        let width = usize::from(buffer.area.width);
        let x = width - 2;

        (1..usize::from(buffer.area.height) - 5)
            .filter(|y| buffer.content()[y * width + x].symbol() == "█")
            .collect()
    }

    /// The panel's own account of itself: the rows it filled, and what the bar
    /// beside them says about where the reader is.
    ///
    /// The slice and the bar are two answers to one question, and the failure
    /// this guards is them disagreeing: nothing fails and nothing panics when
    /// they do, and it reads as a scrollbar in the wrong place, which is a
    /// cosmetic complaint about a geometry computed twice. Read off the screen,
    /// because a slice drawn a row out from the one that was computed is still
    /// a slice.
    fn assert_one_answer(app: &App, buffer: &Buffer) {
        let reserved = app.reserved();
        let panel_rows = message_rows(buffer).len();
        let budget = panel_rows - reserved.above() - reserved.below();
        let view = app.viewport(&app.row_layout(), budget);
        let filled = message_rows(buffer)
            .into_iter()
            .filter(|y| drawn(buffer, *y))
            .count();

        assert_eq!(
            filled,
            view.rows + reserved.above() + reserved.below(),
            "the panel drew the rows the layout says, and no others"
        );
        assert_eq!(
            !thumb(buffer).is_empty(),
            view.total > view.budget,
            "the bar is there exactly when there is somewhere to scroll"
        );

        if view.start_row == 0 && view.total > view.budget {
            assert_eq!(
                thumb(buffer).first().copied(),
                Some(1),
                "the thumb is at the top of the track, below the arrow"
            );
        }
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

    /// The same message, from the reader's side of it, and one a send is still
    /// on its way for.
    fn long_message_sending(chat_id: i64) -> domain::message::Message {
        use domain::message::MessageStatus;

        domain::message::Message {
            status: MessageStatus::Sending,
            is_outgoing: true,
            ..long_message(chat_id)
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
    ///
    /// The panel is tall enough for the whole message, because the question is
    /// what is drawn where and not whether there is anything to scroll.
    #[test]
    fn a_long_message_stops_at_the_column_the_bar_was_given() {
        let mut app = App::mock();
        let chat_id = app.conversation.window.chat_id;
        app.apply_latest(vec![long_message(chat_id)]);

        let screen = screen(&app, 80, 24);
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

    // ---- a message of more than one row ---------------------------------

    /// A message is as tall as its text is at the panel's width, and every row
    /// of it is drawn.
    #[test]
    fn a_long_message_is_as_many_rows_as_it_needs() {
        let mut app = App::mock();
        let chat_id = app.conversation.window.chat_id;
        app.apply_latest(vec![long_message(chat_id)]);

        let screen = screen(&app, 80, 24);

        assert_eq!(
            app.row_layout()[0].len,
            8,
            "47 columns on the first row, 53 on the seven after it"
        );
        assert!(
            row(&screen, 1).contains("[them] xxx"),
            "the first row: {}",
            row(&screen, 1)
        );
        assert!(
            message_rows(&screen)
                .into_iter()
                .take(8)
                .all(|y| drawn(&screen, y)),
            "all nine rows are on the screen: {:?}",
            message_rows(&screen)
                .into_iter()
                .map(|y| row(&screen, y))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            last_drawn(&screen),
            Some(8),
            "and the ninth is the panel's own padding, not a message"
        );
        assert_one_answer(&app, &screen);
    }

    /// A continuation row names nobody: it is the same message as the row above
    /// it, and a reader scrolling into the middle of a long one is reading the
    /// speaker they were a moment ago.
    #[test]
    fn a_continuation_row_names_nobody() {
        let mut incoming = App::mock();
        let chat_id = incoming.conversation.window.chat_id;
        incoming.apply_latest(vec![long_message(chat_id)]);
        let theirs = screen(&incoming, 80, 24);

        assert!(row(&theirs, 1).contains("[them]"), "the first row names it");
        assert!(
            !row(&theirs, 2).contains('['),
            "and a continuation row does not: {}",
            row(&theirs, 2)
        );

        let mut outgoing = App::mock();
        let chat_id = outgoing.conversation.window.chat_id;
        outgoing.apply_latest(vec![long_message_sending(chat_id)]);
        let yours = screen(&outgoing, 80, 24);

        assert!(row(&yours, 1).contains("[you]"), "the first row names it");
        assert!(
            !row(&yours, 2).contains('['),
            "and a continuation row does not: {}",
            row(&yours, 2)
        );
    }

    /// A page of rows moves the reader by rows and leaves the slice and the bar
    /// beside it in step, which is the one thing they must never stop agreeing
    /// about.
    #[test]
    fn a_page_of_rows_leaves_the_slice_and_the_bar_in_step() {
        let mut app = App::mock();

        let pinned = screen(&app, 80, 10);
        assert_one_answer(&app, &pinned);
        assert!(
            row(&pinned, 1).contains("And the benchmarks."),
            "pinned to the end of the window: {}",
            row(&pinned, 1)
        );

        app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        let paged = screen(&app, 80, 10);

        assert_one_answer(&app, &paged);
        assert!(
            row(&paged, 1).contains("1.85.0, edition 2024."),
            "a screenful of rows up: {}",
            row(&paged, 1)
        );
    }

    /// A send on its way says so on the last row of its message, which is the
    /// one with room for it: the first row of a long message is full.
    #[test]
    fn a_pending_send_says_so_on_the_last_row_of_its_message() {
        let mut app = App::mock();
        let chat_id = app.conversation.window.chat_id;
        app.apply_latest(vec![long_message_sending(chat_id)]);

        let screen = screen(&app, 80, 24);
        let last = last_drawn(&screen).expect("the message is on the screen");

        assert!(
            !row(&screen, 1).contains("[sending…]"),
            "not on the first row, which is full: {}",
            row(&screen, 1)
        );
        assert!(
            row(&screen, last).contains("[sending…]"),
            "but on the last: {}",
            row(&screen, last)
        );
        assert_one_answer(&app, &screen);
    }

    /// A reply quotes a share of the row it shares with its body, so that the
    /// body the reply carries still has room. A prefix that filled the row would
    /// push the thing it is a prefix to off it, and the terminal would clip
    /// without saying so.
    #[test]
    fn a_wrapped_reply_leaves_its_body_room_on_the_first_row() {
        use std::borrow::Cow;

        use domain::message::{Message, MessageStatus};

        let mut app = App::mock();
        let chat_id = app.conversation.window.chat_id;
        app.apply_latest(vec![
            Message {
                id: 90,
                chat_id,
                text: Cow::Owned("q".repeat(200)),
                timestamp: 0,
                status: MessageStatus::Received,
                is_outgoing: false,
                reply_to: None,
            },
            Message {
                id: 91,
                chat_id,
                text: Cow::Borrowed("sure"),
                timestamp: 0,
                status: MessageStatus::Received,
                is_outgoing: true,
                reply_to: Some(90),
            },
        ]);

        let screen = screen(&app, 80, 24);
        let on = message_rows(&screen)
            .into_iter()
            .find(|y| row(&screen, *y).contains("sure"))
            .expect("the reply is on the screen");
        let first = body_row(&screen, on);

        assert!(first.contains("> qqq"), "the target is quoted: {first:?}");
        assert!(
            first.contains("‖ sure"),
            "and the body is on the same row: {first:?}"
        );
        assert!(
            first.chars().count() <= 53,
            "the row is the panel's width and no more: {} columns",
            first.chars().count()
        );
    }

    /// A search marks every row of the message it matched, and the cursor
    /// stands on the first of them.
    #[test]
    fn a_wrapped_match_is_marked_throughout_and_the_cursor_is_on_its_first_row() {
        use ratatui::style::{Color, Modifier};

        use std::borrow::Cow;

        use domain::message::{Message, MessageStatus};

        let mut app = App::mock();
        let chat_id = app.conversation.window.chat_id;
        app.apply_latest(vec![Message {
            id: 90,
            chat_id,
            text: Cow::Owned(format!("benchmarks and {}", "x".repeat(200))),
            timestamp: 0,
            status: MessageStatus::Received,
            is_outgoing: false,
            reply_to: None,
        }]);
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "benchmarks");
        press(&mut app, KeyCode::Enter);

        let screen = screen(&app, 80, 24);
        let first = cell(&screen, BODY_X, 1);
        let second = cell(&screen, BODY_X, 2);

        assert_eq!(first.fg, Color::Yellow, "the first row is marked");
        assert_eq!(second.fg, Color::Yellow, "and so is the row after it");
        assert!(
            first.modifier.contains(Modifier::REVERSED),
            "the cursor is on the first of them: {:?}",
            first.modifier
        );
        assert!(
            !second.modifier.contains(Modifier::REVERSED),
            "and not on the rest: {:?}",
            second.modifier
        );
    }

    // ---- sending, replying and confirming ------------------------------

    /// A reply is drawn as a prefix on the message's own row, never as a row of
    /// its own: the panel's geometry assumes one message is one row.
    #[test]
    fn a_reply_is_drawn_as_a_prefix_on_the_message_s_row() {
        let mut app = App::mock();
        press(&mut app, KeyCode::Char('k'));
        press(&mut app, KeyCode::Char('r'));
        type_text(&mut app, "sure");
        press(&mut app, KeyCode::Enter);

        let screen = screen(&app, 80, 24);

        assert!(
            row(&screen, 11).contains("> No pressure then :) ‖ sure"),
            "the reply quotes its target before its own body: {}",
            row(&screen, 11)
        );
    }

    /// A reply whose target the window does not hold says so rather than looking
    /// unanchored.
    #[test]
    fn a_reply_to_a_message_that_is_not_loaded_says_so() {
        let mut app = App::mock();
        app.conversation.queue_send("orphan", Some(999));

        let screen = screen(&app, 80, 24);

        assert!(
            row(&screen, 11).contains("> [message not loaded] ‖ orphan"),
            "{}",
            row(&screen, 11)
        );
    }

    #[test]
    fn a_send_on_its_way_and_a_failed_one_say_so_on_their_row() {
        let mut app = App::mock();
        press(&mut app, KeyCode::Char('i'));
        type_text(&mut app, "hello");
        press(&mut app, KeyCode::Enter);
        let id = app.sending.expect("the send is in flight");

        let sending = screen(&app, 80, 24);
        assert!(
            row(&sending, 11).contains("[sending…]"),
            "{}",
            row(&sending, 11)
        );

        app.fail_send(id, "no route".to_owned());
        let failed = screen(&app, 80, 24);
        assert!(
            row(&failed, 11).contains("[failed: no route]"),
            "{}",
            row(&failed, 11)
        );
    }

    /// The two wordings are the whole of what the confirm says about scope, so
    /// both are checked on the screen the reader sees.
    #[test]
    fn the_confirm_prompt_names_the_side_it_is_about() {
        // Outgoing: the message before the newest, which is one of theirs.
        let mut app = App::mock();
        press(&mut app, KeyCode::Char('k'));
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('d'));
        let outgoing = screen(&app, 80, 24);
        assert!(
            row(&outgoing, 23).contains("Delete your message from both sides? (y/n)"),
            "{}",
            row(&outgoing, 23)
        );

        // Incoming: the newest sample message.
        let mut app = App::mock();
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('d'));
        let incoming = screen(&app, 80, 24);
        assert!(
            row(&incoming, 23).contains("Delete their message from both sides? (y/n)"),
            "{}",
            row(&incoming, 23)
        );
    }

    // ---- what a search marks -------------------------------------------

    /// The sample conversation's seventh message is the only one containing
    /// "benchmarks"; the panel draws the whole window, so it is the seventh
    /// message row.
    const MATCH_ROW: u16 = 7;

    /// An application with a search whose match is on screen.
    fn searched() -> App {
        let mut app = App::mock();
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "benchmarks");
        press(&mut app, KeyCode::Enter);
        app
    }

    /// The first column of the conversation panel's body.
    ///
    /// The panel is the right two-thirds of the frame, so its border is at a
    /// fixed column and the body starts one past it.
    const BODY_X: u16 = 25;

    #[test]
    fn a_matched_message_row_is_marked_and_an_unmatched_one_is_not() {
        use ratatui::style::Color;

        let screen = screen(&searched(), 80, 24);

        assert_eq!(
            cell(&screen, BODY_X, MATCH_ROW).fg,
            Color::Yellow,
            "the matched row carries the match colour"
        );
        assert_ne!(
            cell(&screen, BODY_X, 1).fg,
            Color::Yellow,
            "and an ordinary row does not: {}",
            row(&screen, 1)
        );
    }

    /// The cursor can stand on a match, so the two styles have to compose: the
    /// row is both marked and selected, not one instead of the other.
    #[test]
    fn the_cursor_row_on_a_match_still_reads_as_the_cursor() {
        use ratatui::style::{Color, Modifier};

        let screen = screen(&searched(), 80, 24);
        let cursor = cell(&screen, BODY_X, MATCH_ROW);

        assert_eq!(cursor.fg, Color::Yellow, "the match marking is still there");
        assert!(
            cursor.modifier.contains(Modifier::REVERSED),
            "and so is the selection: {:?}",
            cursor.modifier
        );
    }

    #[test]
    fn the_match_count_is_in_the_panel_title() {
        let screen = screen(&searched(), 80, 24);

        assert!(
            row(&screen, 0).contains("1 match(es)"),
            "the title carries what the search found: {}",
            row(&screen, 0)
        );
    }
}
