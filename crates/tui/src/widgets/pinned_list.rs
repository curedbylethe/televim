//! The pinned list: one chat's pins, newest first.
//!
//! Drawn over the conversation column the pins belong to, in the box the forward
//! picker raises, so a reader who opens it keeps their place in the messages: the
//! list is a place *in* the conversation, not a screen of its own.
//!
//! A row is a number, the sender (`you` for the account's own), one clipped line
//! of the text, and the age at the far end. The **rows are the conversation's
//! pins**, in the order it answers them, so a row and the `⌖` in the messages
//! cannot disagree about what is pinned; the list's own state holds only the
//! fetch, which is what a row whose message has left the window is drawn from.
//!
//! **`Clear` first**, bounded to the column's own rectangle, for the reason the
//! forward picker gives: without it the rows are drawn over the messages
//! underneath and the two read as one list.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};

use crate::app::App;
use crate::date;
use crate::rows;
use crate::wrap::columns;

/// The sentence the panel says when the chat has no pins.
///
/// Dim, the ink metadata uses: a chat with no pins is an answer, not a refusal of
/// the reader's key.
const EMPTY: &str = "No pinned message in this chat.";
/// The word a row shows in place of the text when the message is nowhere to be
/// read.
///
/// The same bracketed token a reply whose target is not loaded wears: the message
/// may have been deleted, or it may simply be older than the window, and the
/// reader is told the program cannot show it rather than being shown nothing.
const GONE: &str = "[message not loaded]";

/// One row of the list, as it is drawn and acted on.
pub(crate) struct Row {
    /// Whether the account wrote it.
    mine: bool,
    /// The message's own first line, or its absence.
    text: Option<String>,
    /// When it was sent, as the loader of the fetch reported it.
    at: Option<i64>,
}

/// The rows the overlay lists: the conversation's pins, in its own order.
///
/// A pin whose message the window does not hold is still a row — the pinned set
/// outlives the window, and the answer carries the message — and a pin neither
/// holds is a row with no text, because a pin removed on another device is not
/// something this list can tell from a message it cannot reach.
pub(crate) fn rows(app: &App) -> Vec<Row> {
    let view = &app.conversation.conversation;

    view.pinned_newest_first()
        .into_iter()
        .map(|id| {
            let held = view.message(id);
            let fetched = app.conversation.pinned.fetched(id);
            let message = held.or(fetched);

            Row {
                mine: message.is_some_and(|message| message.is_outgoing),
                text: message.map(|message| first_line(message.display_body())),
                at: message
                    .map(|message| message.timestamp)
                    .filter(|at| date::has_time(*at)),
            }
        })
        .collect()
}

/// The one line a row has room for: the message's text with its breaks closed up.
fn first_line(text: &str) -> String {
    text.split('\n').next().unwrap_or_default().to_owned()
}

pub fn render(app: &App, conversation: Rect, frame: &mut Frame<'_>) {
    if !app.conversation.pinned.is_active() {
        return;
    }

    frame.render_widget(Clear, conversation);
    // A kitty picture is not in the buffer, so `Clear` does not cover it: the
    // conversation recorded it this frame, under the column the list has taken.
    app.ui.placements.borrow_mut().clear();

    let rows = rows(app);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(app.ui.theme.border)
        .title(format!(" Pinned messages · {} ", rows.len()));
    let inner = block.inner(conversation);
    frame.render_widget(block, conversation);

    if rows.is_empty() {
        // An empty list is an answer once it has been answered: while the fetch
        // is on its way the panel says nothing rather than claiming the chat has
        // no pins.
        if !app.conversation.pinned.is_fetching() {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(EMPTY, app.ui.theme.text_dim))),
                inner,
            );
        }
        return;
    }

    let sender = app.open_chat_name().unwrap_or("them");
    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| item(app, index, row, sender, inner.width))
        .collect();

    let mut state = ListState::default();
    state.select(Some(app.conversation.pinned.selected()));
    let list = List::new(items).highlight_style(app.ui.theme.selection);
    frame.render_stateful_widget(list, inner, &mut state);
}

/// One row: the number, the sender, the clipped line, and the age at the far end.
///
/// The number and the age are the dim ink metadata takes, so the line of text is
/// what the eye lands on; the selection reverses the whole row, number included.
fn item<'a>(app: &'a App, index: usize, row: &'a Row, sender: &'a str, width: u16) -> ListItem<'a> {
    let selected = index == app.conversation.pinned.selected();
    let quiet = if selected {
        app.ui.theme.selection
    } else {
        app.ui.theme.text_dim
    };

    let number = format!("{}.", index + 1);
    let who = if row.mine { "you" } else { sender };
    let age = row
        .at
        .map(|at| date::age(at, app.now()))
        .unwrap_or_default();

    let head = format!(" {who}  ");
    let tail = if age.is_empty() {
        String::new()
    } else {
        format!("  {age}")
    };
    let room = usize::from(width)
        .saturating_sub(columns(&number) + columns(&head) + columns(&tail))
        .max(1);
    let text = row
        .text
        .as_deref()
        .map_or_else(|| GONE.to_owned(), |text| rows::truncate(text, room));

    let mut spans = vec![
        Span::styled(number.clone(), quiet),
        Span::styled(head.clone(), app.ui.theme.text),
        Span::styled(text.clone(), app.ui.theme.text),
    ];
    // The age is right-aligned: it is the same column on every row, so the ages
    // can be read down the list without looking for them.
    let used = columns(&number) + columns(&head) + columns(&text) + columns(&tail);
    let gap = usize::from(width).saturating_sub(used);
    spans.push(Span::raw(" ".repeat(gap)));
    spans.push(Span::styled(tail, quiet));

    ListItem::new(Line::from(spans))
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::message::{MediaKind, Message, MessageStatus};
    use domain::updates::UpdateEvent;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Modifier;
    use std::borrow::Cow;

    const AT: i64 = 1_730_000_000;

    /// The sample conversation's identifier, which the pins and the messages have
    /// to carry, and whose name the rows print.
    fn chat() -> i64 {
        App::mock().conversation.conversation.window.chat_id
    }

    /// The conversation column's first column: the chat list takes the left 30
    /// per cent of the screen.
    fn conversation_column(buffer: &Buffer) -> u16 {
        buffer.area.width * 30 / 100
    }

    /// What the conversation column says on row `y`.
    fn column_text(buffer: &Buffer, y: u16) -> String {
        (conversation_column(buffer)..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    /// The first row of the conversation column that says `needle`, if one does.
    fn row_of(buffer: &Buffer, needle: &str) -> Option<u16> {
        (0..buffer.area.height).find(|&y| column_text(buffer, y).contains(needle))
    }

    fn screen(app: &App, width: u16, height: u16) -> Buffer {
        let mut terminal =
            Terminal::new(TestBackend::new(width, height)).expect("the test backend builds");
        terminal
            .draw(|frame| app.render(frame))
            .expect("the frame draws");

        terminal.backend().buffer().clone()
    }

    fn message(id: i64, seconds: i64, outgoing: bool, text: &'static str) -> Message {
        Message {
            id,
            chat_id: chat(),
            text: Cow::Borrowed(text),
            timestamp: AT + seconds,
            status: MessageStatus::Received,
            is_outgoing: outgoing,
            reply_to: None,
            media: None::<MediaKind>,
            is_service: false,
            media_id: None,
        }
    }

    /// An application showing one conversation, with the overlay open over it.
    fn showing(messages: Vec<Message>) -> App {
        let mut app = App::mock();
        app.conversation.conversation.window.replace(messages);
        app.conversation.vim.set_total(1);
        app
    }

    /// Marks `ids` as pinned in the conversation, as an update from the server
    /// does.
    fn pins(app: &mut App, ids: &[i64]) {
        let _ = app
            .conversation
            .conversation
            .apply_event(&UpdateEvent::PinChanged {
                chat_id: chat(),
                message_ids: ids.to_vec(),
                pinned: true,
            });
    }

    /// Opens the overlay and takes `fetched` as its answer.
    fn listing(app: &mut App, fetched: Vec<Message>) {
        let chat = chat();
        app.conversation.pinned.begin(chat);
        let took = app.conversation.pinned.adopt(chat, fetched);
        assert!(took, "the answer is for the chat the list was opened in");
    }

    #[test]
    fn the_panel_is_titled_with_the_number_of_pins() {
        let mut app = showing(vec![message(1, 0, true, "And the 12-inch ruler.")]);
        pins(&mut app, &[1]);
        listing(&mut app, Vec::new());

        let buffer = screen(&app, 80, 24);

        assert!(
            row_of(&buffer, "Pinned messages · 1").is_some(),
            "the title counts the pins"
        );
    }

    /// A row is the number, the sender, the line and the age, and the age is at
    /// the far end so the ages can be read down the list.
    #[test]
    fn a_row_is_numbered_and_names_its_sender_and_its_age() {
        let mut app = showing(vec![
            message(1, 0, true, "And the 12-inch ruler."),
            message(2, 60, false, "The nanosecond wire is on my desk."),
        ]);
        pins(&mut app, &[1, 2]);
        listing(&mut app, Vec::new());
        // Two minutes after the older pin and one after the newer, so the ages
        // are a fact of the frame rather than of when the test ran.
        app.record_now(AT + 120, 0);

        let buffer = screen(&app, 80, 24);

        let newest = row_of(&buffer, "The nanosecond wire").expect("the newest pin is listed");
        let older = row_of(&buffer, "And the 12-inch ruler.").expect("the older pin is listed");
        assert!(newest < older, "newest first");
        assert!(
            column_text(&buffer, newest).contains("1. Ada Lovelace  The nanosecond wire"),
            "the other side is named by the chat's name: {:?}",
            column_text(&buffer, newest)
        );
        assert!(
            column_text(&buffer, older).contains("2. you  And the 12-inch ruler."),
            "{:?}",
            column_text(&buffer, older)
        );
        // The panel's right border is the last column, so the age is read back
        // without it.
        let age = |y: u16| {
            column_text(&buffer, y)
                .trim_end_matches([' ', '│'])
                .to_owned()
        };
        assert!(
            age(newest).ends_with("1m"),
            "the age ends the row, and it is the newest one's: {:?}",
            age(newest)
        );
        assert!(
            age(older).ends_with("2m"),
            "and the older pin's is older: {:?}",
            age(older)
        );
    }

    #[test]
    fn a_chat_with_no_pins_says_so_in_the_dim_ink() {
        let mut app = showing(vec![message(1, 0, true, "hello")]);
        listing(&mut app, Vec::new());

        let buffer = screen(&app, 80, 24);

        let y = row_of(&buffer, EMPTY).expect("the empty sentence is drawn");
        let x = column_text(&buffer, y).find(EMPTY).expect("its column");
        let cell = &buffer[(
            conversation_column(&buffer) + u16::try_from(x).expect("a column"),
            y,
        )];
        assert_eq!(
            cell.fg,
            app.ui.theme.text_dim.fg.expect("dim is a foreground"),
            "it is the ink metadata uses"
        );
    }

    /// While the fetch is still on its way the panel says nothing rather than
    /// telling the reader a chat they just asked about has no pins.
    #[test]
    fn a_fetch_in_flight_does_not_claim_there_are_no_pins() {
        let mut app = showing(vec![message(1, 0, true, "hello")]);
        app.conversation.pinned.begin(chat());

        let buffer = screen(&app, 80, 24);

        assert!(
            row_of(&buffer, EMPTY).is_none(),
            "the empty sentence waits for the answer"
        );
    }

    #[test]
    fn the_selected_row_is_reverse_video_and_the_others_are_not() {
        let mut app = showing(vec![
            message(1, 0, true, "first"),
            message(2, 60, true, "second"),
        ]);
        pins(&mut app, &[1, 2]);
        listing(&mut app, Vec::new());
        app.conversation.pinned.move_selection(1, 2);

        let buffer = screen(&app, 80, 24);

        let reversed = |y: u16| {
            (conversation_column(&buffer)..buffer.area.width)
                .any(|x| buffer[(x, y)].modifier.contains(Modifier::REVERSED))
        };
        let first = row_of(&buffer, "first").expect("the first row");
        let second = row_of(&buffer, "second").expect("the second row");
        assert!(reversed(first), "the selected row is reverse video");
        assert!(!reversed(second), "and the other is not");
    }

    /// A pin whose message has left the window still lists: the answer carries
    /// it, and the reader is told they cannot be taken there.
    #[test]
    fn a_pin_the_window_does_not_hold_is_still_a_row() {
        let mut app = showing(vec![message(1, 0, true, "in the window")]);
        pins(&mut app, &[1, 9]);
        listing(&mut app, vec![message(9, 600, false, "far behind")]);

        let buffer = screen(&app, 80, 24);

        assert!(
            row_of(&buffer, "far behind").is_some(),
            "the fetched text is drawn"
        );
    }

    /// A pin neither the window nor the answer holds is a row with the program's
    /// word for a message it cannot reach.
    #[test]
    fn a_pin_whose_message_is_gone_says_so_instead_of_nothing() {
        let mut app = showing(vec![message(1, 0, true, "in the window")]);
        pins(&mut app, &[1, 9]);
        listing(&mut app, Vec::new());

        let buffer = screen(&app, 80, 24);

        assert!(
            row_of(&buffer, GONE).is_some(),
            "the row says the message is not loaded"
        );
    }

    #[test]
    fn nothing_is_drawn_when_the_list_is_not_open() {
        let mut app = showing(vec![message(1, 0, true, "And the 12-inch ruler.")]);
        pins(&mut app, &[1]);

        let buffer = screen(&app, 80, 24);

        assert!(row_of(&buffer, "Pinned messages").is_none());
    }
}
