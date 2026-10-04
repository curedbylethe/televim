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
//!
//! One thing about a row is answered here rather than in [`crate::rows`]: the
//! order its pieces are *drawn* in. Rows are broken logically, so a message is
//! the same height in either mode and the bar counts the same rows — but a
//! right-to-left row reaches the terminal in a different order than it is stored
//! when the reader has asked this program to permute it ([`App::bidi`]). The
//! panel asks [`crate::bidi`] for that order and hands it to [`text_row`], which
//! paints each piece of the row and clips the selection to it.

use std::ops::Range;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, List, ListItem, ListState, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use domain::message::Message;

use crate::app::{App, FetchDirection, Focus};
use crate::bidi::{self, BidiMode};
use crate::rows::{self, RowSpan};
use crate::text_row;
use crate::wrap::columns;

/// How many columns the messages keep for themselves before a scrollbar is
/// worth showing beside them.
///
/// A narrow panel has no room to give: the bar would cost more than it tells
/// the reader.
const MIN_BODY_WIDTH: u16 = 8;

/// The panel, drawn from a layout the caller has.
///
/// The layout is [`App::row_layout`]'s, and is passed in rather than asked for a
/// second time here so that the rows drawn, the rows sliced and the rows the
/// scrollbar counts are one answer, laid out once per frame by the one owner of
/// the geometry.
pub fn render(app: &App, area: Rect, frame: &mut Frame<'_>, layout: &[RowSpan]) {
    // The focused pane's border is the only thing on the screen that says where
    // a keystroke goes, so the two panes cannot both be drawn as though they had
    // it.
    let border = if app.focus == Focus::Conversation {
        app.theme.border_focused
    } else {
        app.theme.border
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
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

    let view = app.viewport(layout, budget);

    let mut items: Vec<ListItem> =
        Vec::with_capacity(reserved.above() + reserved.below() + view.rows);
    if reserved.older {
        items.push(loading(app, FetchDirection::Older.label()));
    }
    if reserved.jumping {
        // A jump replaces the window rather than extending it, so it is said
        // where the messages are: the page it is waiting for has no edge of the
        // window on show to sit at. Which jump it is comes from the request
        // itself, so the row and the status line cannot disagree.
        items.push(loading(app, app.jump_label()));
    }
    // Said in place of the messages rather than at an edge of the window, which
    // an empty one does not have. It is not one of the reserved rows either: a
    // window that is empty is never taller than the row the announcement takes.
    if app.is_fetching(FetchDirection::Latest) && app.conversation.window.is_empty() {
        items.push(loading(app, FetchDirection::Latest.label()));
    }

    let window = &app.conversation.window;
    let mut drawn = 0;
    // The slice begins at a row rather than at a message, so what is drawn is
    // every entry the layout says has a row on the screen — which is not the
    // same set as the entries from the slice's first message onward, since a row
    // that names no message sits in the layout too.
    let mut on_the_first_row = true;
    for span in layout
        .iter()
        .filter(|span| span.first + span.len > view.start_row)
    {
        // A message is drawn whole or not at all: half a message with no way to
        // scroll to the rest of it is a different, worse thing than a row the
        // panel did not fill.
        if drawn >= view.budget {
            break;
        }
        // Only the entry the slice starts inside has rows above the panel; every
        // other one begins on it.
        let skip = if on_the_first_row { view.skip } else { 0 };
        on_the_first_row = false;

        let Some(index) = span.kind.index() else {
            // A row that names no message: a day separator. It takes its row of
            // the panel's own width, and the cursor can never be on it, because
            // the cursor names a message.
            items.push(separator(
                app,
                span.kind.label().unwrap_or_default(),
                body.width,
            ));
            drawn += 1;
            continue;
        };
        let Some(message) = window.get(index) else {
            break;
        };

        let grouped = rows::group_of(app, index);
        let wrapped = rows::message_rows(app, message, grouped, body.width);
        // Which window positions the selection covers is one question, and its
        // answer does not change from one message to the next, so it is worked out
        // once here rather than per message.
        let covered = app.covered(app.selection());
        let coverage = coverage(app, message, index, &covered);
        for (row, range) in wrapped.iter().enumerate().skip(skip) {
            if drawn >= view.budget {
                break;
            }
            let place = Place {
                first: row == 0,
                last: row + 1 == wrapped.len(),
                group: grouped,
            };
            items.push(message_row(
                app,
                message,
                &place,
                range,
                coverage.as_ref(),
                body.width,
            ));
            drawn += 1;
        }
    }

    if reserved.newer {
        items.push(loading(app, FetchDirection::Newer.label()));
    }

    // Only a message can be the selection, so an empty window has none — the
    // indicator rows are not places the cursor can be, and neither is a row of
    // the layout that names no message.
    let mut state = ListState::default();
    state.select((!window.is_empty()).then(|| view.selection + above));

    let list = List::new(items).highlight_style(app.theme.selection);
    frame.render_stateful_widget(list, body, &mut state);

    if let Some(gutter) = gutter {
        render_scrollbar(app, gutter, frame, &view);
    }
}

/// The panel's title: where in what is loaded the reader is, what a search found
/// there, and what they have selected.
///
/// Counted in messages, because that is where in the conversation the reader is:
/// the cursor stands on a message however many rows that message is, and a
/// message number is the one position that survives a page landing.
fn conversation_title(app: &App) -> String {
    let notes = format!("{}{}", search_note(app), selection_note(app));
    let total = app.conversation.window.len();
    if total == 0 {
        return format!(" Conversation{notes} ");
    }

    format!(" Conversation ({}/{total}){notes} ", app.vim.cursor() + 1)
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

/// What the title says about a selection, if there is one.
///
/// A bare count, because the title is one row wide and the status line beside it
/// is where the sentence lives — including which unit the count is in, which
/// matters because a selection can be three characters or three messages.
fn selection_note(app: &App) -> String {
    app.selection_len()
        .map_or_else(String::new, |selected| format!(" · {selected} selected"))
}

/// What a selection covers of one message, in the panel's own units.
///
/// Byte offsets, because that is what a row's range is: the mark itself is
/// counted in characters, and [`rows::byte_span`] is where the two meet. Worked
/// out once per message rather than once per row, because the conversion walks
/// the text.
enum Coverage {
    /// These bytes of the message's text, in a selection inside it.
    Text(Range<usize>),

    /// All of it, decorations included — a selection of whole messages, where
    /// "the message" is what the reader selected and `[you]` is part of it.
    Whole,
}

/// What a selection covers of the message at `index`, or nothing if the selection
/// does not reach it.
fn coverage(
    app: &App,
    message: &Message,
    index: usize,
    covered: &Range<usize>,
) -> Option<Coverage> {
    let selection = app.selection()?;

    // One rule decides it, and it is the rule every operation follows: a
    // selection inside a single message is a text selection, and anything else is
    // a set of messages. `text_range` being `None` *is* the second case.
    match selection.text_range() {
        Some((id, range)) if id == message.id => Some(Coverage::Text(rows::byte_span(
            message.display_body(),
            range,
        ))),
        Some(_) => None,
        None => covered.contains(&index).then_some(Coverage::Whole),
    }
}

/// Where a row stands: in its message, and in its message's group.
///
/// The two are not the same question and are kept apart deliberately — a row is
/// the first or the last of a *message*, and a message is the first or the last
/// of a *group*, and most rows and most messages are neither. What follows from
/// that is all the drawing: the tag and the quoted target go on a message's
/// first row, the status and the time on a message's last row, and each only
/// once per group.
struct Place {
    /// The row the reader reaches the message by.
    first: bool,

    /// The row with room for what stands behind the message.
    last: bool,

    /// What the message's group says of it.
    group: rows::Grouped,
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
/// What belongs to the **group** rather than to the message is thinner still:
/// the sender is named on the message that opens the group and on no other, so a
/// message that follows one into its group is given the same blank tag of
/// [`rows::WHO_WIDTH`] columns and its text begins in the same column. The time
/// is shown on the message that closes the group and on no other. Neither the
/// tag nor the time is taken away from a message that carries its own status:
/// a group's last message shows both.
///
/// The row was cut at a width that already made room for all of it, in
/// [`rows::message_rows`], so nothing here is clipped by the terminal and lost.
///
/// A message a search matched has its spans patched with
/// [`Theme::match_hit`](crate::theme::Theme::match_hit) rather than given a
/// style of their own, so that the cursor's `REVERSED` selection composes on top
/// of it instead of replacing it. The same goes for a selection: a selected
/// substring is split out of the row and given
/// [`Theme::selection_bg`](crate::theme::Theme::selection_bg), and a selected
/// message has its whole row patched, so the cursor still composes on top.
///
/// The match is applied **before** the selection, which is the order the theme's
/// module doc states. It matters because both set a foreground: the selection's
/// ink wins on a cell that is both, so a match inside a selection is painted in
/// the selection's colour and keeps only its `BOLD` rather than its own colour,
/// which on a selection is close to invisible.
fn message_row<'m>(
    app: &App,
    message: &'m Message,
    place: &Place,
    range: &Range<usize>,
    covered: Option<&Coverage>,
    width: u16,
) -> ListItem<'m> {
    let mut spans = Vec::new();

    if place.first {
        if place.group.first {
            let who = if message.is_outgoing { "you" } else { "them" };
            spans.push(Span::styled(format!("[{who}] "), app.theme.text_dim));
        } else {
            // The tag is blank rather than absent, so this message's text begins
            // in the same column as the one that opened the group.
            spans.push(Span::raw(" ".repeat(rows::WHO_WIDTH)));
        }

        if let Some(reply_to) = message.reply_to {
            spans.push(Span::styled(
                rows::reply_prefix(app, reply_to, width),
                app.theme.text_dim,
            ));
        }
    }

    // The decorations are the message's, not the text's, so they are patched with
    // the match here rather than inside the row — a reader who is told a row is a
    // hit by its text alone is being told the same thing.
    let matched = app.search().is_match(message.id);
    if matched {
        for span in &mut spans {
            span.style = span.style.patch(app.theme.match_hit);
        }
    }

    // The text's own three steps — the text, a match on it, a selection split out
    // of it — are [`text_row`]'s, because the line and a card row want them too.
    //
    // The row itself is `&message.display_body()[range]` and stays that way: which of its
    // pieces reach the terminal first is a question about the order they are
    // *drawn* in, and [`crate::bidi`] answers that with logical slices of the very
    // same bytes. In [`BidiMode::Terminal`] there is no answer to ask for — the
    // terminal's shaper reverses a right-to-left run for us, and permuting here
    // would reverse it twice.
    let row = text_row::TextRow {
        ink: text_row::Ink::readonly(&app.theme),
        text: message.display_body(),
        range: range.clone(),
        matched,
        selected: match covered {
            Some(Coverage::Text(selected)) => Some(selected.clone()),
            _ => None,
        },
        // A message is a row of reverse video and the cursor is the row; there is
        // no position *within* one, which is what a card row adds.
        caret: None,
        reversed: false,
        concealed: false,
    };
    match app.bidi() {
        BidiMode::Terminal => spans.extend(text_row::spans(&row)),
        BidiMode::Visual => {
            // One direction for the whole message, one permutation per row: the
            // base level is the message's, and a row of an all-neutral message
            // carries no evidence of its own.
            let base = bidi::base_direction(message.display_body());
            let pieces = bidi::visual_row(&message.display_body()[range.clone()], base);
            spans.extend(text_row::spans_permuted(&row, &pieces));
        }
    }

    if place.last
        && let Some(note) = rows::trailing_note(app, message, place.group)
    {
        // The note is right-aligned: the status column is the far end of the row,
        // so a group's time is at the same column on every group rather than
        // hanging after whichever text happened to end last.
        let prefix = if place.first {
            rows::WHO_WIDTH
                + message.reply_to.map_or(0, |reply_to| {
                    columns(&rows::reply_prefix(app, reply_to, width))
                })
        } else {
            0
        };
        let drawn = prefix + columns(&message.display_body()[range.clone()]);
        let gap = usize::from(width).saturating_sub(drawn + columns(&note));
        if gap > 0 {
            spans.push(Span::raw(" ".repeat(gap)));
        }
        spans.push(Span::styled(note, app.theme.text_dim));
        if matched {
            let last_span = spans.len() - 1;
            spans[last_span].style = spans[last_span].style.patch(app.theme.match_hit);
        }
    }

    // A selected *message* is styled in one pass, which is what puts the
    // decorations and the trailing note inside the selection with the text. A
    // selection inside one message is not this: the text was already split, and
    // splitting it again would be splitting a row that is no longer one span.
    if let Some(Coverage::Whole) = covered {
        for span in &mut spans {
            span.style = span.style.patch(app.theme.selection_bg);
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

/// A day separator: a rule across the panel with the day set into it.
///
/// A row of its own, the full width of the text — the tag column included,
/// because this is not a message and does not live inside one. The rule is in the
/// border's ink and the label in the dim ink a decoration is drawn in, so the row
/// reads as furniture rather than as something said: no tag, no sender, no quoted
/// target, no status, and nothing a selection or a match could touch.
///
/// The label is set into the middle with a space either side, and an odd column
/// goes to the right of it rather than the left — a rule a column wider on one
/// side than the other is one nobody can read, and a reader looking at one is
/// looking at the day in it.
///
/// `ponytail:` drawn as one line rather than as a rule above a label. The design
/// artifact's row is this rule with the day in it; a second row would change the
/// count the scrollbar shows for the same window.
fn separator(app: &App, label: &str, width: u16) -> ListItem<'static> {
    let room = usize::from(width);
    // Truncated rather than clipped: a day longer than a narrow panel's row is
    // said as much of as fits, and the row is still the panel's width.
    let label = rows::truncate(label, room.saturating_sub(2));
    let rule = room.saturating_sub(columns(&label) + 2);
    let left = rule / 2;

    ListItem::new(Line::from(vec![
        Span::styled("─".repeat(left), app.theme.border),
        Span::styled(format!(" {label} "), app.theme.text_dim),
        Span::styled("─".repeat(rule - left), app.theme.border),
    ]))
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
    use crate::app::JumpKind;
    use crate::theme::Theme;
    use crate::wrap::columns;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use domain::message::MediaKind;
    use domain::selection::Mark;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::style::Color;

    /// The theme's own values, so a palette change does not have to touch a test.
    ///
    /// `App::mock()` builds on `Theme::default()`, so reading a colour here and
    /// reading it off an app are reading the same one.
    fn theme() -> Theme {
        Theme::default()
    }

    /// The ink a search match is painted in.
    fn match_fg() -> Color {
        theme().match_hit.fg.expect("a match is a foreground")
    }

    /// The background a selection paints, and the ink its text takes.
    fn selection_bg() -> Color {
        theme()
            .selection_bg
            .bg
            .expect("a selection is a background")
    }

    fn selection_fg() -> Color {
        theme()
            .selection_bg
            .fg
            .expect("a selection carries the text's own ink")
    }

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

    /// The same, from a layout the test holds rather than the panel's own.
    ///
    /// Which is how a row that names no message reaches the screen: nothing in
    /// the program emits one yet, and the panel's whole job — the rows drawn,
    /// the slice, and the bar beside them — is what has to hold when one does.
    fn screen_of_layout(app: &App, layout: &[RowSpan], width: u16, height: u16) -> Buffer {
        let mut terminal =
            Terminal::new(TestBackend::new(width, height)).expect("the test backend builds");
        terminal
            .draw(|frame| app.render_layout(layout, frame))
            .expect("the frame draws");

        terminal.backend().buffer().clone()
    }

    /// The panel's layout with a row that names no message put in front of the
    /// message at `index`, and every row below it moved down to make room.
    fn layout_with_other_row(app: &App, index: usize) -> Vec<RowSpan> {
        let mut layout = app.row_layout();
        let first = layout[index].first;

        layout.insert(
            index,
            RowSpan {
                kind: rows::RowKind::Other {
                    label: "── test ──".to_owned(),
                },
                message_id: None,
                first,
                len: 1,
                text: 0..0,
            },
        );
        for span in &mut layout[index + 1..] {
            span.first += 1;
        }

        layout
    }

    // ---- a conversation to group ----------------------------------------

    /// A moment in the middle of a day: 2024-11-14 22:13:20 UTC.
    const AT: i64 = 1_730_000_000;

    /// The sample conversation's identifier, which a fixture message has to carry.
    fn mock_chat_id() -> i64 {
        App::mock().conversation.window.chat_id
    }

    /// A short message of the sample conversation, `seconds` after [`AT`].
    fn at(id: i64, seconds: i64, outgoing: bool, text: &'static str) -> Message {
        Message {
            id,
            chat_id: mock_chat_id(),
            text: text.into(),
            timestamp: AT + seconds,
            status: domain::message::MessageStatus::Received,
            is_outgoing: outgoing,
            reply_to: None,
            media: None,
        }
    }

    /// An application holding exactly these messages, oldest first, with the
    /// reader on the newest of them.
    ///
    /// The window is replaced rather than extended so that what a test groups is
    /// what it built: the sample conversation alternates direction, so every one
    /// of its messages is already a group of its own.
    fn showing(messages: Vec<Message>) -> App {
        let mut app = App::mock();
        let count = messages.len();
        app.conversation.window.replace(messages);
        app.vim.set_total(count);
        app.vim.set_cursor(count.saturating_sub(1));

        app
    }

    /// How many of the panel's rows carry `needle`.
    fn occurrences(buffer: &Buffer, needle: &str) -> usize {
        message_rows(buffer)
            .into_iter()
            .filter(|y| row(buffer, *y).contains(needle))
            .count()
    }

    /// The rows each entry of a layout begins on.
    fn firsts_of(layout: &[RowSpan]) -> Vec<usize> {
        layout.iter().map(|span| span.first).collect()
    }

    /// The `HH:MM` a message `seconds` after [`AT`] shows.
    fn clock_at(seconds: i64) -> String {
        crate::date::clock(AT + seconds).expect("a moment in a day has a clock")
    }

    /// The day a message `seconds` after [`AT`] is named by, with no clock
    /// recorded: a date, which is a fact about the message rather than about when
    /// it is being read.
    fn day_at(seconds: i64) -> String {
        rows::separator_label(AT + seconds, 0).into_owned()
    }

    /// The cursor is on a message, and the panel's own selection is on the row the
    /// layout says that message begins on — a separator is a row of the panel
    /// without ever being a place the cursor can be.
    fn assert_cursor_stands_on_a_message(app: &App, screen: &Buffer) {
        use ratatui::style::Modifier;

        let layout = app.row_layout();
        let view = app.viewport(&layout, message_rows(screen).len());
        let at_row = drawn_at(view.selection);

        assert!(
            cell(screen, BODY_X, at_row)
                .modifier
                .contains(Modifier::REVERSED),
            "the selection is on the row the panel drew for message {}: {}",
            app.vim.cursor(),
            row(screen, at_row)
        );
        assert_eq!(
            view.start_row + view.selection,
            rows::first_row_of_message(&layout, app.vim.cursor())
                .expect("the cursor names a message the window holds"),
            "and that row is the message's own first row, not a separator's"
        );
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
        assert_one_answer_about(app, &app.row_layout(), buffer);
    }

    /// The same, for a layout the caller holds — the panel draws the layout it is
    /// given, and the panel and the layout must still agree about every row.
    ///
    /// A day separator counts like any other row: it is in the layout, it is in
    /// `view.total`, and it is drawn, so it is in the sum without being special.
    fn assert_one_answer_about(app: &App, layout: &[RowSpan], buffer: &Buffer) {
        let reserved = app.reserved();
        let panel_rows = message_rows(buffer).len();
        let budget = panel_rows - reserved.above() - reserved.below();
        let view = app.viewport(layout, budget);
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
            media: None,
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

    // ---- right-to-left fixtures -----------------------------------------

    /// Hebrew, in the order it is stored: the word a reader reads first comes
    /// first in the string.
    ///
    /// Logical order, which is what the message and the layout both hold. Whether
    /// the screen shows it in this order is a separate question, and one only the
    /// exact-column assertions below can answer — see [`body_cells`].
    const HEBREW: &str = "שלום עולם";

    /// Arabic, likewise logical: one word, four glyphs.
    const ARABIC: &str = "سلام";

    /// An application showing the Hebrew fixture and then the Arabic one, so the
    /// two sit in one conversation and the screen has to carry both.
    ///
    /// The direction changes between them, so each opens its own group and each
    /// carries its own sender tag — which is what makes the two rows comparable
    /// at all: `[them] ` is seven columns and `[you] ` is six, so the body text
    /// does not begin in the same column on both.
    fn showing_rtl() -> App {
        showing(vec![at(1, 0, false, HEBREW), at(2, 60, true, ARABIC)])
    }

    /// The same conversation, drawing right-to-left rows itself.
    ///
    /// The mode is chosen at construction, which is the whole of what makes a row
    /// the same height either way: nothing here mutates an application and asks
    /// the screen to come out differently.
    fn showing_rtl_visual() -> App {
        showing_rtl().with_bidi(BidiMode::Visual)
    }

    /// The glyphs of `text` as the screen shows them left to right, which is
    /// `text` itself in [`BidiMode::Terminal`] and its reverse in
    /// [`BidiMode::Visual`] for a fixture with no embedded left-to-right run.
    ///
    /// Both fixtures are one-directional words with a space between them, so the
    /// whole row is one reversed run and the drawn order is the reverse of the
    /// stored one. That is a property of these two strings, not a claim about
    /// right-to-left text in general — an embedded number would stay as it is
    /// written, which is what [`crate::bidi`]'s own tests pin.
    fn in_drawing_order(text: &str, visual: bool) -> String {
        if visual {
            text.chars().rev().collect()
        } else {
            text.to_owned()
        }
    }

    /// One row of the panel's body as `(column, symbol)` for every cell from
    /// [`BODY_X`] to the scrollbar, in ascending column order.
    ///
    /// This exists because [`body_row`] is a blind spot rather than a helper: it
    /// concatenates the cells in ascending column index, so `contains` on it
    /// passes on a row whose glyphs are in the wrong columns — a reordered row
    /// and an unreordered one read identically as a substring. Only a column
    /// pinned to a symbol can tell them apart, which is the whole question for
    /// right-to-left text.
    fn body_cells(buffer: &Buffer, y: u16) -> Vec<(u16, String)> {
        let last = usize::from(buffer.area.width) - 2;

        (usize::from(BODY_X)..last)
            .map(|x| (x as u16, cell(buffer, x as u16, y).symbol().to_owned()))
            .collect()
    }

    /// The whole-screen column of the first cell of `y` holding `symbol`.
    ///
    /// A whole-screen column rather than an offset into the row, so that the
    /// gutter and the text after it are pinned on one scale and can be compared.
    fn column_holding(buffer: &Buffer, y: u16, symbol: &str) -> u16 {
        body_cells(buffer, y)
            .into_iter()
            .find(|(_, drawn)| drawn == symbol)
            .map_or_else(
                || {
                    panic!(
                        "{symbol:?} is nowhere on row {y}: {:?}",
                        body_row(buffer, y)
                    )
                },
                |(column, _)| column,
            )
    }

    /// The column the body text of a row begins in: one past the sender tag
    /// `who` draws, as a whole-screen column.
    ///
    /// The tag is the message's own text, so its width is its own: `[them] ` is
    /// seven columns and `[you] ` is six, and the layout reserves
    /// [`rows::WHO_WIDTH`] for both — which is why the two kinds of row begin
    /// their body text in different columns. That difference is the thing to pin,
    /// not a constant to work around.
    fn text_column(buffer: &Buffer, y: u16, who: &str) -> u16 {
        let tag = format!("[{who}] ");
        let tag_x = column_holding(buffer, y, &tag[..1]);
        let tag_w = u16::try_from(columns(&tag)).expect("a tag's width fits a u16");

        for (offset, drawn) in tag.chars().enumerate() {
            let column = tag_x + u16::try_from(offset).expect("a tag is seven wide");
            assert_eq!(
                cell(buffer, column, y).symbol(),
                drawn.to_string(),
                "the tag is drawn whole before the text: {:?}",
                body_row(buffer, y)
            );
        }

        tag_x + tag_w
    }

    /// The glyph of `text` that is `n`th in the string, as it must appear on the
    /// screen when the row is drawn in logical order.
    ///
    /// Returns the whole-screen column it belongs in and the symbol that belongs
    /// there, so a caller can pin both and a reorder that moves one without the
    /// other cannot pass.
    fn nth_glyph(text: &str, n: usize, buffer: &Buffer, y: u16, who: &str) -> (u16, String) {
        let column = text_column(buffer, y, who)
            + u16::try_from(columns(&text.chars().take(n).collect::<String>()))
                .expect("a glyph count fits a u16");

        (
            column,
            text.chars()
                .nth(n)
                .expect("the glyph is in the string")
                .into(),
        )
    }

    /// The cells one row of the panel's body spells its body text in, from the
    /// first text column onwards, for as many cells as the text has clusters.
    ///
    /// Reads the cells rather than the string, because a permuted row and an
    /// unpermuted one have the same letters in them and are told apart only by
    /// which column holds which letter. The extent is the message's own rather than
    /// a run of non-blank cells: a right-to-left sentence has a space in the middle
    /// of it, and stopping at the first one would read half a word.
    ///
    /// Clusters rather than [`columns`], because both fixtures are one cell per
    /// cluster and [`columns`] says otherwise about the Arabic — `سلام` holds the
    /// lam-alef pair, which it measures as one cell narrower than the terminal
    /// draws it. Counting what was drawn rather than what was measured keeps the
    /// assertion about the order of the glyphs, which is the whole question here.
    fn spelled(buffer: &Buffer, y: u16, first: u16, text: &str) -> Vec<(u16, String)> {
        body_cells(buffer, y)
            .into_iter()
            .skip(usize::from(first - BODY_X))
            .take(crate::grapheme::clusters(text).count())
            .collect()
    }

    /// The first `n` glyphs of `text`, as they are read when the row is drawn in
    /// `visual` order.
    ///
    /// Reversed rather than re-permuted, because a run of one direction comes back
    /// reversed — see [`in_drawing_order`], which says why that holds for these
    /// fixtures.
    fn leading(text: &str, n: usize, visual: bool) -> String {
        let glyphs: Vec<char> = text.chars().take(n).collect();

        if visual {
            glyphs.into_iter().rev().collect()
        } else {
            glyphs.into_iter().collect()
        }
    }

    /// The letters in the cells at `columns`, read left to right.
    fn letters_at(buffer: &Buffer, y: u16, columns: &[u16]) -> String {
        columns
            .iter()
            .map(|column| cell(buffer, *column, y).symbol())
            .collect()
    }

    // ---- right-to-left: visual order -------------------------------------

    /// The claim the whole of `BidiMode::Visual` exists for: a right-to-left row
    /// is drawn with its logical-first glyph at the right of its run.
    ///
    /// Every glyph is pinned to its own column rather than the row being
    /// `contains`-ed, and both fixtures are checked — the Hebrew the reader is
    /// sent and the Arabic they sent, because they sit on rows that begin their
    /// text in different columns and a row that reordered only one of them would
    /// pass on the other.
    #[test]
    fn a_right_to_left_row_is_drawn_with_its_first_glyph_at_the_right() {
        let screen = screen(&showing_rtl_visual(), 80, 24);

        for (text, who, y) in [(HEBREW, "them", FIRST), (ARABIC, "you", FIRST + 1)] {
            let first = text_column(&screen, y, who);
            let columns = spelled(&screen, y, first, text);

            assert_eq!(
                letters_at(
                    &screen,
                    y,
                    &columns.iter().map(|(x, _)| *x).collect::<Vec<_>>()
                ),
                in_drawing_order(text, true),
                "every glyph of the {who} row is drawn, in the order it is read: {:?}",
                body_row(&screen, y)
            );

            // The glyph a reader reads first is the rightmost of the run, and the
            // last is the leftmost: the two ends of the claim, pinned.
            assert_eq!(
                cell(&screen, columns.last().expect("the row has text").0, y).symbol(),
                text.chars()
                    .next()
                    .expect("the fixture has a glyph")
                    .to_string(),
                "the first glyph of {text:?} is at the right of its run"
            );
            assert_eq!(
                cell(&screen, columns[0].0, y).symbol(),
                text.chars()
                    .last()
                    .expect("the fixture has a glyph")
                    .to_string(),
                "and the last is at the left"
            );
        }
    }

    /// The default is unchanged: the same two rows, drawn as they are stored.
    ///
    /// The [`BidiMode::Terminal`] half of the claim above, and the reason the
    /// fixture is pinned twice: a reorder that reached the default path would
    /// corrupt every right-to-left message on a terminal that shapes, which is
    /// where most of the readers of this text are.
    #[test]
    fn the_default_mode_is_still_logical_and_the_mode_is_a_construction_choice() {
        let app = showing_rtl();
        assert_eq!(
            app.bidi(),
            BidiMode::Terminal,
            "a fresh application hands the row to the terminal"
        );

        let screen = screen(&app, 80, 24);
        for (text, who, y) in [(HEBREW, "them", FIRST), (ARABIC, "you", FIRST + 1)] {
            let first = text_column(&screen, y, who);

            assert_eq!(
                cell(&screen, first, y).symbol(),
                text.chars().next().expect("a glyph").to_string(),
                "{text:?} still begins at the left of its body text: {:?}",
                body_row(&screen, y)
            );
        }
    }

    /// A permuted row is the same row: same height, same geometry, same trailing
    /// note in the same column.
    ///
    /// Everything a reader scrolls by is worked out in [`crate::rows`] from the
    /// window and the panel's width, before anything is permuted — so a mode that
    /// changed a row's height would change the number of rows between two messages
    /// and every selection and motion with it. The note is the last check because
    /// its gap is computed from the row's width, and a permutation that moved a
    /// glyph a column would move the note with it.
    #[test]
    fn a_permuted_row_is_the_same_height_and_its_note_stands_where_it_did() {
        let logical = showing_rtl();
        let visual = showing_rtl_visual();

        assert_eq!(
            logical.row_layout(),
            visual.row_layout(),
            "the layout is a pure function of the window and the width, so the mode \
             cannot be in it"
        );

        let before = screen(&logical, 80, 24);
        let after = screen(&visual, 80, 24);

        for y in message_rows(&after) {
            assert_eq!(
                row(&before, y).trim_end().is_empty(),
                row(&after, y).trim_end().is_empty(),
                "row {y} is drawn in one mode and empty in the other"
            );
        }

        // The note is on the last row of each group, and its gap is worked out from
        // the row's own width — which a permutation cannot change, because it
        // moves the cells and not the count of them.
        //
        // The Hebrew row, and not the Arabic: `سلام` holds the lam-alef pair, which
        // [`columns`] measures as one cell narrower than a terminal draws it, so on
        // the default path the renderer offsets the next span by a column short and
        // the note lands one column early. That is this tree's width of a lam-alef
        // and nothing to do with the reorder, so pinning it here would pin a defect.
        let theirs = FIRST;
        let hour = clock_at(0);
        assert_eq!(
            column_holding(&before, theirs, &hour[..1]),
            column_holding(&after, theirs, &hour[..1]),
            "the note stands in the same column: the row is as wide either way"
        );
        assert_eq!(
            spelled(
                &before,
                theirs,
                text_column(&before, theirs, "them"),
                HEBREW
            )
            .len(),
            spelled(&after, theirs, text_column(&after, theirs, "them"), HEBREW).len(),
            "and so is the text in front of it"
        );
    }

    /// A selection follows its own characters, not its columns.
    ///
    /// The selection is a range of bytes in the message and stays one: the paint
    /// clips it to each permuted piece, so the letters carrying the selection's
    /// background are the same letters whichever order the row is drawn in. A
    /// paint that clipped once and then permuted would move the highlight onto
    /// the wrong letters without changing its width — which no width assertion
    /// can see, and which is why the letters themselves are read back.
    #[test]
    fn a_selection_covers_the_same_characters_after_the_reorder() {
        // The first two characters of the Hebrew: `של`, which the reader reads
        // first and which sit rightmost in [`BidiMode::Visual`].
        const CHARS: std::ops::Range<usize> = 0..2;

        let mut logical = showing_rtl();
        selecting_chars(&mut logical, 1, CHARS.start, CHARS.end);
        let mut visual = showing_rtl_visual();
        selecting_chars(&mut visual, 1, CHARS.start, CHARS.end);
        let y = FIRST;

        let before = screen(&logical, 80, 24);
        let after = screen(&visual, 80, 24);

        let read = |buffer: &Buffer| -> String {
            selected_text_columns(buffer, y, FIRST)
                .iter()
                .map(|x| cell(buffer, BODY_X + SENDER as u16 + *x as u16, y).symbol())
                .collect()
        };
        let logical_letters = read(&before);
        let visual_letters = read(&after);

        assert_eq!(
            logical_letters,
            leading(HEBREW, CHARS.end, false),
            "the selection covers the first two glyphs, in logical order"
        );
        assert_eq!(
            visual_letters,
            leading(HEBREW, CHARS.end, true),
            "and the same two glyphs in visual order — the highlight followed the \
             characters rather than the columns"
        );
    }

    /// A row that is not right-to-left is untouched by the mode.
    ///
    /// The permutation is a no-op on left-to-right text — it comes back as one
    /// chunk covering the whole row — so the panel is not asked to draw anything
    /// different, and this is what proves the mode did not become a general
    /// reordering.
    #[test]
    fn a_left_to_right_message_is_drawn_alike_in_both_modes() {
        let logical = showing(vec![at(1, 0, false, "hello world")]);
        let visual = showing(vec![at(1, 0, false, "hello world")]).with_bidi(BidiMode::Visual);

        assert_eq!(
            row(&screen(&logical, 80, 24), FIRST),
            row(&screen(&visual, 80, 24), FIRST),
            "one mode to draw it in"
        );
    }

    // ---- right-to-left: the baseline ------------------------------------

    /// A right-to-left message is laid out left-to-right, because no reorder
    /// happens yet — so the glyph that comes first in the string is on the left.
    ///
    /// This is the baseline the reorder stages are measured against: it names the
    /// exact column of the first glyph of the fixture, and it names three of them
    /// rather than one, so a row that is half reordered does not pass.
    #[test]
    fn a_right_to_left_message_is_drawn_in_logical_order_and_the_baseline_pins_it() {
        let app = showing_rtl();
        let screen = screen(&app, 80, 24);

        // Both fixtures name a sender that opens its own group, so each row is the
        // message's first row and each carries the whole of its text.
        let theirs = FIRST;
        let yours = FIRST + 1;

        // The Hebrew is drawn left-to-right, so the glyph that comes first in the
        // string is on the left. Three of them, each pinned to its own column:
        // a row that reordered only its tail would still pass on the first glyph,
        // and the substring of the string is the same either way.
        for (n, glyph) in ["ש", "ל", "ו"].into_iter().enumerate() {
            let (column, expected) = nth_glyph(HEBREW, n, &screen, theirs, "them");

            assert_eq!(
                column_holding(&screen, theirs, glyph),
                column,
                "glyph {n} of the Hebrew is at column {column}, and the screen has it there"
            );
            assert_eq!(expected, glyph, "the fixture's own {n}th glyph");
        }

        // And the Arabic, on the reader's side, the same way.
        for (n, glyph) in ["س", "ل", "ا"].into_iter().enumerate() {
            let (column, _) = nth_glyph(ARABIC, n, &screen, yours, "you");

            assert_eq!(
                column_holding(&screen, yours, glyph),
                column,
                "glyph {n} of the Arabic is at column {column}"
            );
        }

        // The two kinds of row do not begin their text in the same column, and
        // that is not the test's arithmetic to correct: `[them] ` is seven
        // columns of tag and `[you] ` is six, so the body text starts one earlier
        // on the reader's own messages. Both are pinned against the screen, and
        // against each other, because a gutter that is off by one here shifts
        // every glyph after it.
        let their_text = text_column(&screen, theirs, "them");
        let your_text = text_column(&screen, yours, "you");

        assert_eq!(
            (your_text, their_text),
            (31, 32),
            "`[you] ` is one column narrower than `[them] `, so its text begins one \
             column earlier — the row above is {:?} and the row below is {:?}",
            body_row(&screen, yours),
            body_row(&screen, theirs)
        );
        assert_eq!(
            column_holding(&screen, yours, "س"),
            your_text,
            "and the Arabic's first glyph is the leftmost of its own body text"
        );

        // The columns are contiguous, so the whole word is where the first glyph
        // is rather than three scattered glyphs that happen to be present.
        assert_eq!(
            body_cells(&screen, theirs)[usize::from(their_text - BODY_X)..]
                .iter()
                .take(3)
                .map(|(_, symbol)| symbol.as_str())
                .collect::<String>(),
            "שלו",
            "the first three columns of the Hebrew body are its own first three glyphs"
        );
    }

    /// A substring assertion cannot see a reordered row, which is why the
    /// baseline above pins columns. This is the same screen read the other way,
    /// kept because it is the mistake the helper exists to prevent: `body_row`
    /// concatenates the cells in ascending column order, so this passes on a row
    /// whose glyphs are in the wrong columns.
    #[test]
    fn the_substring_of_a_reordered_row_would_still_pass() {
        let screen = screen(&showing_rtl(), 80, 24);
        let theirs = FIRST;

        assert_eq!(
            column_holding(&screen, theirs, "ש"),
            text_column(&screen, theirs, "them"),
            "today the logical-first glyph is the leftmost"
        );
        assert!(
            body_row(&screen, theirs).contains(HEBREW),
            "and the substring assertion — the one that cannot see order — also passes, \
             which is why it is not what the baseline rests on: {:?}",
            body_row(&screen, theirs)
        );
    }

    /// An application whose conversation has unread messages in front of what is
    /// loaded, with a jump asked for.
    ///
    /// Built through the application's own interface rather than by reaching into
    /// it, because what is under test is what a reader's keystroke puts on the
    /// screen.
    fn jumping() -> App {
        let mut app = unread_out_of_reach();

        for _ in 0..2 {
            app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        }

        app
    }

    /// The application [`jumping`] starts from: the conversation runs to 20 and
    /// the window stops at 5, so there is something to be taken to and nothing
    /// has asked to be taken to it yet.
    fn unread_out_of_reach() -> App {
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
                    media: None,
                })
                .collect(),
        );

        app
    }

    /// The same, for a jump to the message a reply quotes: the row in the
    /// messages says which jump this is, because `Jumping to first unread…`
    /// would be a claim about a fetch the reader never asked for.
    #[test]
    fn a_reply_jump_is_announced_as_a_reply_jump() {
        let app = reply_jumping();
        assert_eq!(
            app.pending_jump().map(|jump| jump.kind),
            Some(JumpKind::Reply)
        );

        let screen = screen(&app, 80, 10);

        assert!(
            row(&screen, 1).contains("Jumping to the quoted message"),
            "the panel's first row: {}",
            row(&screen, 1)
        );
    }

    /// An application with a reply whose quote is nowhere near what is loaded,
    /// with the reader asking to be taken to it.
    fn reply_jumping() -> App {
        use domain::message::Message;

        let mut app = unread_out_of_reach();
        // The reply is the newest message on show and it quotes 19, which the
        // window stops well short of.
        app.apply_latest(
            app.conversation
                .window
                .iter()
                .map(|message| Message {
                    reply_to: Some(19),
                    ..message.clone()
                })
                .collect(),
        );
        app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

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

    /// The panel's first message row, for a window whose messages are all of one
    /// day.
    ///
    /// Row 0 is the frame's own, and row 1 is the separator that anchors the day
    /// the window starts in — a row of the panel's own, drawn before the first
    /// message of the window whatever else is true of it.
    const FIRST: u16 = 2;

    /// One of the reader's own messages as the server left it: numbered by it and
    /// accepted, which is the only state a receipt can be about.
    fn mine(id: i64, seconds: i64) -> Message {
        Message {
            status: domain::message::MessageStatus::Sent,
            is_outgoing: true,
            ..at(id, seconds, true, "text")
        }
    }

    /// The frame row a row of the layout is drawn on: the panel's body begins one
    /// row below the frame's own.
    fn drawn_at(layout_row: usize) -> u16 {
        u16::try_from(layout_row + 1).expect("a row of the layout fits a frame")
    }

    /// A message that carries something and says nothing: the shape the
    /// placeholder exists for.
    fn attachment(id: i64, seconds: i64, outgoing: bool, media: MediaKind) -> Message {
        Message {
            media: Some(media),
            ..at(id, seconds, outgoing, "")
        }
    }

    #[test]
    fn a_message_that_only_carries_something_draws_its_placeholder() {
        let app = showing(vec![attachment(1, 0, false, MediaKind::Photo)]);

        let screen = screen(&app, 80, 24);

        assert!(
            row(&screen, FIRST).contains("[image]"),
            "a photo with no caption still says what it is: {}",
            row(&screen, FIRST)
        );
        assert_cursor_stands_on_a_message(&app, &screen);
    }

    #[test]
    fn every_kind_draws_its_own_placeholder() {
        for (media, label) in [
            (MediaKind::Photo, "[image]"),
            (MediaKind::Video, "[video]"),
            (MediaKind::Gif, "[gif]"),
            (MediaKind::Voice, "[voice]"),
            (MediaKind::File, "[file]"),
        ] {
            let app = showing(vec![attachment(1, 0, false, media)]);

            let screen = screen(&app, 80, 24);

            assert!(
                row(&screen, FIRST).contains(label),
                "{media:?} draws {label}: {}",
                row(&screen, FIRST)
            );
        }
    }

    /// A placeholder that cannot be selected is not body text, it is decoration.
    #[test]
    fn a_placeholder_can_be_selected_and_yanked() {
        let mut app = showing(vec![attachment(1, 0, false, MediaKind::File)]);

        press(&mut app, KeyCode::Char('V'));
        press(&mut app, KeyCode::Char('y'));

        assert_eq!(
            app.register().lines(),
            std::slice::from_ref(&"[file]".to_owned()),
            "the message yanks what it shows, label and all"
        );
    }

    #[test]
    fn a_placeholder_never_replaces_a_caption() {
        let mut media = at(1, 0, false, "the pier at six");
        media.media = Some(MediaKind::Photo);
        let app = showing(vec![media]);

        let screen = screen(&app, 80, 24);

        assert!(
            row(&screen, FIRST).contains("the pier at six"),
            "the caption is the body: {}",
            row(&screen, FIRST)
        );
        assert!(
            !row(&screen, FIRST).contains("[image]"),
            "and the placeholder is not drawn beside it"
        );
    }

    #[test]
    fn an_empty_message_with_no_media_still_draws_nothing() {
        let app = showing(vec![at(1, 0, false, "")]);

        let screen = screen(&app, 80, 24);

        assert!(
            !["[image]", "[video]", "[gif]", "[voice]", "[file]"]
                .iter()
                .any(|label| row(&screen, FIRST).contains(label)),
            "there is nothing to stand in for nothing: {}",
            row(&screen, FIRST)
        );
    }

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
            row(&screen, FIRST).contains("Hey, is the build green?"),
            "the whole window fits: {}",
            row(&screen, FIRST)
        );
        assert!(row(&screen, FIRST + 9).contains("See you at the demo."));
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
            row(&screen, 3).contains("text"),
            "and the messages start behind it: {}",
            row(&screen, 3)
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
        // The newest entry is the long message: the day's separator sits above
        // the window's first message, so the entries are not the messages.
        let long = app
            .row_layout()
            .last()
            .expect("the window's last entry is the long message")
            .clone();
        let first = drawn_at(long.first);

        assert_eq!(
            long.len, 8,
            "47 columns on the first row, 53 on the seven after it"
        );
        assert!(
            row(&screen, first).contains("[them] xxx"),
            "the first row: {}",
            row(&screen, first)
        );
        assert!(
            message_rows(&screen)
                .into_iter()
                .skip(usize::from(first) - 1)
                .take(long.len)
                .all(|y| drawn(&screen, y)),
            "all eight rows are on the screen: {:?}",
            message_rows(&screen)
                .into_iter()
                .map(|y| row(&screen, y))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            last_drawn(&screen),
            Some(first + u16::try_from(long.len - 1).expect("eight rows fits a u16")),
            "and the row after it is the panel's own padding, not a message"
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

    /// A row that names no message sits between two messages, takes a row of the
    /// screen, and is counted by the scrollbar beside them — and the cursor does
    /// not land on it, because the cursor stands on messages.
    #[test]
    fn a_row_that_names_no_message_is_drawn_and_never_stands_the_cursor() {
        use ratatui::style::Modifier;

        let app = App::mock();
        // In front of the window's fifth message, so the panel shows the whole of
        // it — the day's own separator above the window's first message is entry
        // zero, so the entries are not the messages.
        let layout = layout_with_other_row(&app, 5);
        let inserted = layout[5].clone();
        assert!(
            inserted.kind.index().is_none(),
            "the entry names no message"
        );
        let newest = layout.last().expect("the window has an entry");
        let newest_last = drawn_at(newest.first + newest.len - 1);

        let screen = screen_of_layout(&app, &layout, 80, 24);
        let y = drawn_at(inserted.first);

        assert!(
            body_row(&screen, y).contains("── test ──"),
            "the row says what it is: {:?}",
            body_row(&screen, y)
        );
        assert!(
            row(&screen, y - 1).contains("1.85.0, edition 2024."),
            "the message above it is where it was: {}",
            row(&screen, y - 1)
        );
        assert!(
            row(&screen, y + 1).contains("Perfect. Let's meet tomorrow."),
            "and the one below it is a row further down: {}",
            row(&screen, y + 1)
        );
        assert!(
            !cell(&screen, BODY_X, y)
                .modifier
                .contains(Modifier::REVERSED),
            "and nothing about the row says the cursor is on it"
        );
        assert!(
            cell(&screen, BODY_X, newest_last)
                .modifier
                .contains(Modifier::REVERSED),
            "the cursor is on the newest message: {}",
            row(&screen, newest_last)
        );
        assert_one_answer_about(&app, &layout, &screen);
    }

    // ---- what a group says once ------------------------------------------

    /// Three messages from one side a minute apart are one group: the sender is
    /// named once, on the first row, and the time shown once, on the last. The
    /// message in the middle is given the same blank tag so its text begins in
    /// the same column as the rest.
    #[test]
    fn a_group_names_the_sender_once_and_shows_the_time_once() {
        let app = showing(vec![
            at(1, 0, false, "one"),
            at(2, 60, false, "two"),
            at(3, 120, false, "three"),
        ]);

        let screen = screen(&app, 80, 24);

        assert_eq!(occurrences(&screen, "[them]"), 1, "one name for the group");
        assert_eq!(occurrences(&screen, "[you]"), 0);
        assert_eq!(
            occurrences(&screen, &clock_at(120)),
            1,
            "and one time, on the row that ends it"
        );
        assert!(
            body_row(&screen, FIRST).starts_with("[them] one"),
            "the first row names it: {:?}",
            body_row(&screen, FIRST)
        );
        assert!(
            body_row(&screen, FIRST + 1).starts_with("       two"),
            "the one after it is a blank tag and its own text: {:?}",
            body_row(&screen, FIRST + 1)
        );
        assert!(
            body_row(&screen, FIRST + 2).starts_with("       three"),
            "and so is the last: {:?}",
            body_row(&screen, FIRST + 2)
        );
        assert!(
            body_row(&screen, FIRST + 2)
                .trim_end()
                .ends_with(&clock_at(120)),
            "with the time at the end of the row: {:?}",
            body_row(&screen, FIRST + 2)
        );
        assert_eq!(
            body_row(&screen, FIRST + 2).trim_end().chars().count(),
            53,
            "and the row is full to its last column, which is what right-aligned means: {:?}",
            body_row(&screen, FIRST + 2)
        );
        assert!(
            !body_row(&screen, FIRST + 1).contains(&clock_at(120)),
            "and on no other row of the group: {:?}",
            body_row(&screen, FIRST + 1)
        );
        assert_one_answer(&app, &screen);
    }

    /// A change of direction is a change of who is talking, which ends the group:
    /// the sender is named again and each group carries its own time.
    #[test]
    fn a_change_of_direction_names_the_sender_again() {
        let app = showing(vec![
            at(1, 0, false, "one"),
            at(2, 60, true, "two"),
            at(3, 120, false, "three"),
        ]);

        let screen = screen(&app, 80, 24);

        assert_eq!(occurrences(&screen, "[them]"), 2, "the two of theirs");
        assert_eq!(occurrences(&screen, "[you]"), 1, "and the one of yours");
        assert_eq!(occurrences(&screen, &clock_at(0)), 1);
        assert_eq!(occurrences(&screen, &clock_at(120)), 1);
        assert_eq!(occurrences(&screen, &clock_at(60)), 1);
        assert_one_answer(&app, &screen);
    }

    /// The time belongs to the group and the status to the message, so a send on
    /// its way in the middle of a group says so and does not take the group's
    /// time with it.
    #[test]
    fn a_send_on_its_way_inside_a_group_still_says_so_on_its_own_row() {
        let mut pending = at(2, 60, false, "two");
        pending.status = domain::message::MessageStatus::Sending;
        let app = showing(vec![
            at(1, 0, false, "one"),
            pending,
            at(3, 120, false, "three"),
        ]);

        let screen = screen(&app, 80, 24);

        assert!(
            body_row(&screen, FIRST + 1).contains("[sending…]"),
            "the middle row says what its send is doing: {:?}",
            body_row(&screen, FIRST + 1)
        );
        assert!(
            !body_row(&screen, FIRST + 1).contains(&clock_at(120)),
            "and the group's time is not on it: {:?}",
            body_row(&screen, FIRST + 1)
        );
        assert!(
            body_row(&screen, FIRST + 2).contains(&clock_at(120)),
            "it is on the row that ends the group: {:?}",
            body_row(&screen, FIRST + 2)
        );
        assert_one_answer(&app, &screen);
    }

    /// A reply opens its group, so its target and the sender's name are both on
    /// the row the reader reaches it by, and the group's time is on its last row.
    #[test]
    fn a_reply_names_the_sender_and_quotes_its_target_on_the_groups_first_row() {
        let mut reply = at(2, 60, true, "two");
        reply.reply_to = Some(1);
        let app = showing(vec![at(1, 0, false, "one"), reply]);

        let screen = screen(&app, 80, 24);

        assert!(
            body_row(&screen, FIRST + 1).contains("[you] > one ‖ two"),
            "the reply names and quotes on its first row: {:?}",
            body_row(&screen, FIRST + 1)
        );
        assert!(
            body_row(&screen, FIRST + 1).contains(&clock_at(60)),
            "and the group it opens ends on it: {:?}",
            body_row(&screen, FIRST + 1)
        );
        assert_one_answer(&app, &screen);
    }

    // ---- the day separators ---------------------------------------------

    /// A separator is a row of the panel's own width: a rule with the day set into
    /// its middle. It is not a message — no tag column, no sender, no quoted
    /// target, no status — and it is inked differently from one, so a reader can
    /// see at a glance which rows are conversation and which are furniture.
    #[test]
    fn a_separator_is_a_row_of_its_own_and_names_no_one() {
        let app = showing(vec![
            at(1, 0, false, "one"),
            at(2, 86_400, false, "two"),
            at(3, 86_460, false, "three"),
        ]);

        let screen = screen(&app, 80, 24);
        let rule = body_row(&screen, 1);

        assert!(rule.starts_with('─'), "the row is the rule: {rule:?}");
        assert!(
            rule.contains(&day_at(0)),
            "with the first day's name set into it: {rule:?}"
        );
        assert!(
            rule.trim_end().ends_with('─'),
            "and it runs to the panel's last column: {rule:?}"
        );
        assert_eq!(
            rule.trim_end().chars().count(),
            53,
            "which is the whole width of the text"
        );
        for absent in ["[you]", "[them]", "‖", "[sending", "[failed"] {
            assert!(
                !rule.contains(absent),
                "and nothing of a message's is on it: {rule:?}"
            );
        }

        let rule_before = rule.chars().take_while(|c| *c == '─').count();
        assert_eq!(
            cell(&screen, BODY_X, 1).fg,
            theme().border.fg.expect("the border has an ink"),
            "the rule is in the border's ink"
        );
        assert_eq!(
            cell(
                &screen,
                BODY_X + u16::try_from(rule_before).expect("a column fits a u16"),
                1
            )
            .fg,
            theme().text_dim.fg.expect("dim text has an ink"),
            "and the label in the dim ink a decoration is drawn in"
        );

        // The messages either side of it are on their own rows, which is the
        // whole of "sitting exactly between the two days".
        assert!(body_row(&screen, 2).contains("[them] one"));
        assert!(body_row(&screen, 4).contains("[them] two"));
        assert!(
            body_row(&screen, 3).contains(&day_at(86_400)),
            "and the second separator is between them: {:?}",
            body_row(&screen, 3)
        );
        assert_one_answer(&app, &screen);
    }

    /// `Today` is a claim about the reader's clock, so it is made only once a
    /// clock has been recorded; without one the row says the date.
    #[test]
    fn a_separator_says_today_only_when_a_clock_has_been_recorded() {
        let app = showing(vec![at(1, -86_400, false, "old"), at(2, 0, false, "new")]);
        assert_eq!(
            occurrences(&screen(&app, 80, 24), "Today"),
            0,
            "no clock, no claim"
        );

        app.record_now(AT);
        let screen = screen(&app, 80, 24);

        assert_eq!(occurrences(&screen, "Yesterday"), 1);
        assert_eq!(occurrences(&screen, "Today"), 1);
        assert_eq!(
            occurrences(&screen, &day_at(0)),
            0,
            "and no date where a name fits"
        );
        assert_one_answer(&app, &screen);
    }

    /// A separator is a row of the panel and nothing more: no motion lands on it,
    /// and every way of moving leaves the cursor on a message.
    #[test]
    fn no_separator_is_where_the_cursor_can_stand() {
        let mut app = showing(
            (1..=9)
                .map(|id| at(id, (id - 1) * 43_200, id % 2 == 0, "text"))
                .collect(),
        );

        let shown = screen(&app, 80, 24);
        assert_cursor_stands_on_a_message(&app, &shown);

        for _ in 0..3 {
            press(&mut app, KeyCode::Char('j'));
            let stepped = screen(&app, 80, 24);
            assert_cursor_stands_on_a_message(&app, &stepped);
        }

        app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        let up = screen(&app, 80, 24);
        assert_cursor_stands_on_a_message(&app, &up);
        assert!(
            up.content.iter().any(|cell| cell.symbol() == "─"),
            "and the separators are still there either side of it"
        );

        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        let down = screen(&app, 80, 24);
        assert_cursor_stands_on_a_message(&app, &down);
        assert_one_answer(&app, &down);
    }

    /// Twelve messages in four days of three, so the window has days in it and is
    /// taller than a short panel.
    fn four_days() -> App {
        showing(
            (1..=12)
                .map(|id| {
                    let day = (id - 1) / 3;
                    at(id, day * 86_400 + (id - 1) % 3 * 60, false, "text")
                })
                .collect(),
        )
    }

    /// A separator takes a row of the window, so the slice counts it and the
    /// scrollbar counts the slice: the bar describes the rows beside it, separator
    /// rows included.
    #[test]
    fn a_separator_counts_toward_the_slice_and_the_bar_beside_it() {
        let app = four_days();
        let layout = app.row_layout();

        let screen = screen(&app, 80, 10);
        let budget = message_rows(&screen).len();
        let view = app.viewport(&layout, budget);

        assert_eq!(
            layout.len(),
            16,
            "twelve messages and four separators: {:?}",
            firsts_of(&layout)
        );
        assert_eq!(
            view.total,
            rows::total_rows(&layout),
            "and the slice counts every row of it"
        );
        assert_eq!(
            occurrences(&screen, "─"),
            layout
                .iter()
                .filter(|span| {
                    !span.kind.is_message()
                        && span.first >= view.start_row
                        && span.first < view.start_row + view.rows
                })
                .count(),
            "a separator row is drawn for each row of the slice that is one"
        );
        assert!(
            !thumb(&screen).is_empty(),
            "and the bar is there to describe them"
        );
        assert_one_answer(&app, &screen);
    }

    /// Landing on the first message of a day — `gg`, a search, any move that puts
    /// the cursor there — shows the separator above it, because the slice is
    /// centred by rows and the separator is the row above the cursor's.
    #[test]
    fn landing_on_a_days_first_message_shows_the_separator_above_it() {
        let mut app = four_days();
        // The window's seventh message, which is the first of its third day, and
        // well inside a window too tall for the panel. Landing on it leaves the
        // pinned view, as `gg` and a search landing do.
        let target = 6;
        app.vim.set_cursor(target);
        app.conversation.unfollow();

        let screen = screen(&app, 80, 10);
        let layout = app.row_layout();
        let view = app.viewport(&layout, message_rows(&screen).len());
        let cursor = drawn_at(view.selection);

        assert_eq!(
            view.start_row + view.selection,
            rows::first_row_of_message(&layout, target).expect("the window holds it"),
            "the cursor is on the day's first message"
        );
        assert!(
            body_row(&screen, cursor - 1).contains(&day_at(2 * 86_400)),
            "and the row above it names that day: {:?}",
            body_row(&screen, cursor - 1)
        );
        assert_cursor_stands_on_a_message(&app, &screen);
        assert_one_answer(&app, &screen);
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
                media: None,
            },
            Message {
                id: 91,
                chat_id,
                text: Cow::Borrowed("sure"),
                timestamp: 0,
                status: MessageStatus::Received,
                is_outgoing: true,
                reply_to: Some(90),
                media: None,
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
            columns(&first) <= 53,
            "the row is the panel's width and no more: {} columns",
            columns(&first)
        );
        assert_one_answer(&app, &screen);
    }

    // ---- what a group has earned ----------------------------------------

    /// A read receipt is drawn once per group, on the row of the group's newest
    /// message, in front of the time — and nowhere else in the group.
    #[test]
    fn a_read_receipt_is_shown_once_on_the_newest_row_of_its_group() {
        let mut app = showing(vec![mine(1, 0), mine(2, 60)]);
        app.conversation.set_read_watermark(2);

        let screen = screen(&app, 80, 24);
        let newest = body_row(&screen, FIRST + 1);

        assert_eq!(
            occurrences(&screen, "[read]"),
            1,
            "one receipt for the group, however many messages are in it"
        );
        assert_eq!(occurrences(&screen, "[delivered]"), 0);
        assert!(
            newest.contains("[read]") && newest.contains(&clock_at(60)),
            "on the newest message's row, before its time: {newest:?}"
        );
        assert!(
            matches!(
                (newest.find("[read]"), newest.find(&clock_at(60))),
                (Some(word), Some(time)) if word < time
            ),
            "and the word comes before the time: {newest:?}"
        );
        assert!(
            !body_row(&screen, FIRST).contains("[read]"),
            "and not on the row that only continued it: {:?}",
            body_row(&screen, FIRST)
        );
        assert_one_answer(&app, &screen);
    }

    /// Before the peer's read watermark reaches the group's newest message, what
    /// it shows is that the server has it — and no more than that.
    #[test]
    fn an_outgoing_group_the_peer_has_not_read_says_delivered() {
        let mut app = showing(vec![mine(1, 0), mine(2, 60)]);
        app.conversation.set_read_watermark(1);

        let screen = screen(&app, 80, 24);

        assert_eq!(
            occurrences(&screen, "[delivered]"),
            1,
            "one word for the group"
        );
        assert_eq!(occurrences(&screen, "[read]"), 0);
        assert!(
            body_row(&screen, FIRST + 1).contains("[delivered]"),
            "on the newest message's row: {:?}",
            body_row(&screen, FIRST + 1)
        );
        assert_one_answer(&app, &screen);
    }

    /// A group the peer has said nothing about claims nothing: the time, and the
    /// time alone. An incoming group is in the same position for a different
    /// reason, and neither of them gets a word it was not told.
    #[test]
    fn no_group_is_given_a_receipt_it_was_not_told() {
        let silent = showing(vec![mine(1, 0), mine(2, 60)]);
        let without_a_read = screen(&silent, 80, 24);
        assert_eq!(
            occurrences(&without_a_read, "[read]"),
            0,
            "the peer has said nothing"
        );
        assert_eq!(occurrences(&without_a_read, "[delivered]"), 0);
        assert!(
            body_row(&without_a_read, FIRST + 1).contains(&clock_at(60)),
            "so only the time: {:?}",
            body_row(&without_a_read, FIRST + 1)
        );

        let mut theirs = showing(vec![at(1, 0, false, "one"), at(2, 60, false, "two")]);
        theirs.conversation.set_read_watermark(99);
        let screen = screen(&theirs, 80, 24);
        assert_eq!(
            occurrences(&screen, "[read]") + occurrences(&screen, "[delivered]"),
            0,
            "and an incoming group is never given one at all"
        );
        assert_one_answer(&theirs, &screen);
    }

    /// A failed send says why, and never also claims a receipt: it did not reach
    /// the server, so it has earned nothing (AC-14).
    #[test]
    fn a_failed_send_says_why_and_claims_no_receipt() {
        let mut app = showing(vec![mine(1, 0), mine(2, 60)]);
        app.conversation.set_read_watermark(99);
        let failed = app.conversation.queue_send("on its way", None);
        assert!(app.conversation.fail_send(failed, "no route".to_owned()));

        let screen = screen(&app, 80, 24);
        let last = message_rows(&screen)
            .into_iter()
            .rev()
            .find(|y| body_row(&screen, *y).contains("[failed"))
            .expect("the failure is on the screen");

        let note = body_row(&screen, last);
        assert!(
            note.contains("[you] on its way"),
            "the reader's own send, named like any other: {note:?}"
        );
        assert!(
            note.trim_end().ends_with("[failed: no route]"),
            "and it ends on why it failed, with no time and no receipt after it: {note:?}"
        );
        assert_eq!(
            occurrences(&screen, "[delivered]"),
            0,
            "and nothing claims a delivery it was not told about"
        );
        assert_eq!(
            occurrences(&screen, "[read]"),
            1,
            "while the group the failure is not in still shows what it earned"
        );
        assert_one_answer(&app, &screen);
    }

    /// A search marks every row of the message it matched, and the cursor
    /// stands on the first of them.
    #[test]
    fn a_wrapped_match_is_marked_throughout_and_the_cursor_is_on_its_first_row() {
        use ratatui::style::Modifier;

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
            media: None,
        }]);
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "benchmarks");
        press(&mut app, KeyCode::Enter);

        let screen = screen(&app, 80, 24);
        let first = cell(&screen, BODY_X, 1);
        let second = cell(&screen, BODY_X, 2);

        assert_eq!(first.fg, match_fg(), "the first row is marked");
        assert_eq!(second.fg, match_fg(), "and so is the row after it");
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
        assert_one_answer(&app, &screen);
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
            row(&screen, FIRST + 10).contains("> No pressure then :) ‖ sure"),
            "the reply quotes its target before its own body: {}",
            row(&screen, FIRST + 10)
        );
        assert_one_answer(&app, &screen);
    }

    /// A reply whose target the window does not hold says so rather than looking
    /// unanchored.
    #[test]
    fn a_reply_to_a_message_that_is_not_loaded_says_so() {
        let mut app = App::mock();
        app.conversation.queue_send("orphan", Some(999));

        let screen = screen(&app, 80, 24);

        assert!(
            row(&screen, FIRST + 10).contains("> [message not loaded] ‖ orphan"),
            "{}",
            row(&screen, FIRST + 10)
        );
        assert_one_answer(&app, &screen);
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
            row(&sending, FIRST + 10).contains("[sending…]"),
            "{}",
            row(&sending, FIRST + 10)
        );

        app.fail_send(id, "no route".to_owned());
        let failed = screen(&app, 80, 24);
        assert!(
            row(&failed, FIRST + 10).contains("[failed: no route]"),
            "{}",
            row(&failed, FIRST + 10)
        );
        assert_one_answer(&app, &sending);
        assert_one_answer(&app, &failed);
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
    /// message row — the eighth row of the panel, because the day it all falls in
    /// is named above the first of them.
    const MATCH_ROW: u16 = FIRST + 6;

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
        let app = searched();
        let screen = screen(&app, 80, 24);

        assert_eq!(
            cell(&screen, BODY_X, MATCH_ROW).fg,
            match_fg(),
            "the matched row carries the match colour"
        );
        assert_ne!(
            cell(&screen, BODY_X, FIRST).fg,
            match_fg(),
            "and an ordinary row does not: {}",
            row(&screen, FIRST)
        );
        assert_one_answer(&app, &screen);
    }

    /// The cursor can stand on a match, so the two styles have to compose: the
    /// row is both marked and selected, not one instead of the other.
    #[test]
    fn the_cursor_row_on_a_match_still_reads_as_the_cursor() {
        use ratatui::style::Modifier;

        let app = searched();
        let screen = screen(&app, 80, 24);
        let cursor = cell(&screen, BODY_X, MATCH_ROW);

        assert_eq!(cursor.fg, match_fg(), "the match marking is still there");
        assert!(
            cursor.modifier.contains(Modifier::REVERSED),
            "and so is the selection: {:?}",
            cursor.modifier
        );
        assert_one_answer(&app, &screen);
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

    // ---- what a selection marks ------------------------------------------

    /// How many columns a message's text has on its first row at this terminal.
    ///
    /// The panel is the right 70% of eighty columns, less its two borders and the
    /// scrollbar's, and the first row also gives up seven columns to the sender's
    /// name. A message of nothing but `x` has no whitespace to break at, so its
    /// rows are cut at the edge and these are exact.
    const FIRST_ROW: usize = 46;
    const LATER_ROW: usize = 53;

    /// The body width the rows are laid out at, and the sender's name that comes
    /// off its first row.
    const BODY: u16 = 53;
    const SENDER: usize = 7;

    /// One application holding one long unbreakable message.
    fn one_long_message() -> App {
        use std::borrow::Cow;

        use domain::chat::{Chat, ChatKind};
        use domain::message::{Message, MessageStatus};

        const CHAT: i64 = 3;

        let mut app = App::new();
        app.set_chats(vec![Chat {
            id: CHAT,
            title: "Ada".into(),
            kind: ChatKind::Private,
            last_message: None,
            unread_count: 0,
            last_message_id: Some(1),
            last_timestamp: Some(0),
        }]);
        app.select_chat(0);
        app.apply_latest(vec![Message {
            id: 1,
            chat_id: CHAT,
            text: Cow::Owned("x".repeat(FIRST_ROW * 3)),
            timestamp: 0,
            status: MessageStatus::Received,
            is_outgoing: false,
            reply_to: None,
            media: None,
        }]);

        app
    }

    /// A selection over a span of one message's characters, placed directly.
    fn selecting_chars(app: &mut App, id: i64, from: usize, to: usize) {
        assert!(
            app.select(id, Some(from)),
            "the mark is on a loaded message"
        );
        let mut selection = *app.selection().expect("a selection");
        selection.focus = Mark::text(id, to);
        app.set_selection(selection);
    }

    /// The same, with a selection over two of the long message's characters.
    fn selecting(from: usize, to: usize) -> App {
        let mut app = one_long_message();
        selecting_chars(&mut app, 1, from, to);
        app
    }

    /// One cell of a message's *text*, counting columns from the start of the
    /// text rather than from the start of the body.
    ///
    /// The assertions below are about characters a reader can see themselves, and
    /// a selection counts characters, so every one of them is read in those units.
    /// Only `first` — the row the message begins on — carries the sender's name,
    /// and which row that is depends on the fixture: a window of one message with
    /// no day separator begins on the panel's first row, and one inside the
    /// sample conversation begins below the separator that names its day.
    fn text_cell(buffer: &Buffer, x: usize, y: u16, first: u16) -> &Cell {
        let prefix = if y == first { SENDER as u16 } else { 0 };
        cell(buffer, BODY_X + prefix + x as u16, y)
    }

    /// Which of a message's text columns carry the selection's background.
    fn selected_text_columns(buffer: &Buffer, y: u16, first: u16) -> Vec<usize> {
        (0..usize::from(BODY))
            .filter(|x| text_cell(buffer, *x, y, first).bg == selection_bg())
            .collect()
    }

    /// The first row of a message in a window that has nothing but it: no day
    /// separator, so the panel's first row is the message's own.
    const ONLY: u16 = 1;

    #[test]
    fn a_selected_substring_is_styled_and_the_rest_of_its_row_is_not() {
        let screen = screen(&selecting(0, 5), 80, 10);

        assert_eq!(
            selected_text_columns(&screen, ONLY, ONLY),
            (0..5).collect::<Vec<usize>>(),
            "five characters, and nothing in front of them: the sender's name is not text"
        );
        assert_eq!(
            text_cell(&screen, 5, ONLY, ONLY).bg,
            Color::Reset,
            "the sixth character of the same row is plain text"
        );
    }

    /// A selection that spans rows highlights the leading row's suffix and the
    /// trailing row's prefix, which is what a reader expects from Vim and is what
    /// intersecting the selection with each row gives.
    #[test]
    fn a_selection_across_wrapped_rows_highlights_the_right_slices() {
        let text = "x".repeat(FIRST_ROW * 3);
        let rows = crate::wrap::wrap_decorated(&text, SENDER, 0, BODY);
        assert_eq!(rows[0], 0..FIRST_ROW, "the fixture wraps where this says");
        assert_eq!(rows[1], FIRST_ROW..FIRST_ROW + LATER_ROW);

        let screen = screen(&selecting(40, 60), 80, 10);

        assert_eq!(
            selected_text_columns(&screen, ONLY, ONLY),
            (40..FIRST_ROW).collect::<Vec<usize>>(),
            "the first row's suffix, from where the selection began"
        );
        assert_eq!(
            selected_text_columns(&screen, ONLY + 1, ONLY),
            (0..60 - FIRST_ROW).collect::<Vec<usize>>(),
            "and the second row's prefix, up to where the selection ends"
        );
        assert_eq!(
            selected_text_columns(&screen, ONLY + 2, ONLY),
            vec![],
            "a row the selection never reached is left alone"
        );
    }

    /// A selection of whole messages covers the decorations too: the reader
    /// selected the message, and the sender's name is part of the message's row.
    #[test]
    fn a_selected_message_is_styled_whole_decorations_and_all() {
        let mut app = App::mock();
        for _ in 0..2 {
            press(&mut app, KeyCode::Char('g'));
        }
        assert!(app.select(1, None));

        let screen = screen(&app, 80, 10);

        assert_eq!(
            cell(&screen, BODY_X, FIRST).bg,
            selection_bg(),
            "the `[` of the sender is inside the selection"
        );
        assert_eq!(
            cell(&screen, BODY_X, FIRST).fg,
            selection_fg(),
            "and the row's text follows the selection, not the terminal"
        );
        assert_eq!(
            cell(&screen, BODY_X, FIRST + 1).bg,
            Color::Reset,
            "and the message below it, which is not in the selection, is not"
        );
    }

    /// The cursor can stand inside a selection, and the two have to compose for
    /// that cell: the reverse video is the cursor's and stays, the background is
    /// the selection's and stays.
    #[test]
    fn the_cursor_row_inside_a_selection_still_reads_as_the_cursor() {
        use ratatui::style::Modifier;

        let screen = screen(&selecting(0, 5), 80, 10);
        let cursor = text_cell(&screen, 0, ONLY, ONLY);

        assert_eq!(cursor.bg, selection_bg(), "the selection is still there");
        assert!(
            cursor.modifier.contains(Modifier::REVERSED),
            "and so is the cursor: {:?}",
            cursor.modifier
        );
    }

    /// A match under a selection is the later of the theme's two, and the two have
    /// to be legible together rather than one replacing the other.
    #[test]
    fn a_match_under_a_selection_keeps_both_of_its_marks() {
        use ratatui::style::Modifier;

        let mut app = App::mock();
        for _ in 0..2 {
            press(&mut app, KeyCode::Char('g'));
        }
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "build");
        press(&mut app, KeyCode::Enter);

        // Over "build", which starts at character 12 of "Hey, is the build green?".
        selecting_chars(&mut app, 1, 12, 17);

        let screen = screen(&app, 80, 10);
        let marked = text_cell(&screen, 12, FIRST, FIRST);

        assert_eq!(
            marked.bg,
            selection_bg(),
            "the selection's background stays"
        );
        assert_eq!(
            marked.fg,
            selection_fg(),
            "and the cell's text follows the selection, not the match's colour"
        );
        assert!(
            marked.modifier.contains(Modifier::BOLD),
            "the match is still marked, by weight: {:?}",
            marked.modifier
        );
    }

    #[test]
    fn the_selection_count_is_in_the_panel_title() {
        let screen = screen(&selecting(0, 2), 80, 10);

        assert!(
            row(&screen, 0).contains("· 2 selected"),
            "the title carries the count: {}",
            row(&screen, 0)
        );
    }

    // ---- where the keys go ----------------------------------------------

    /// Which of the three borders is drawn focused, given where the focus is.
    ///
    /// The border is the only thing on the screen that says which pane a
    /// keystroke goes to, so two panes drawn alike would be two panes the reader
    /// has to guess between. Read off the screen rather than off the widget,
    /// because a border styled correctly and drawn everywhere is still a border
    /// that says nothing.
    fn lit_borders(focus: crate::app::Focus) -> [bool; 3] {
        let mut app = App::mock();
        app.focus = focus;
        let screen = screen(&app, 80, 24);
        let focused = app
            .theme
            .border_focused
            .fg
            .expect("the focused border has an ink");

        [
            // The chat list's top-left corner, the conversation's, and the input
            // bar's. The bar is the full width of the frame and sits below the
            // two panes, so its corner is at the frame's own left edge.
            (0, 0),
            (24, 0),
            (0, 20),
        ]
        .map(|(x, y)| cell(&screen, x, y).fg == focused)
    }

    #[test]
    fn the_focused_pane_is_the_one_with_the_focused_border() {
        use crate::app::Focus;

        assert_eq!(
            lit_borders(Focus::Conversation),
            [false, true, false],
            "the conversation is on show, so the conversation is lit"
        );
        assert_eq!(lit_borders(Focus::ChatList), [true, false, false]);
        assert_eq!(
            lit_borders(Focus::Input),
            [false, false, true],
            "the bar is not a pane that takes the focus visually, it is the focus"
        );
    }
}
