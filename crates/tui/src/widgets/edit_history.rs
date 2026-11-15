//! The edit-history overlay: the revisions of one message's text, newest first.
//!
//! Drawn over the conversation column, in the box `:pinned` and the forward picker
//! raise, because it is about a message in the conversation and the reader should
//! keep their place in it while reading what changed.
//!
//! **A block is a heading and the text that revision carried**: `2d  current` for
//! the text the message shows now, then `…  revision` and `…  original` for the
//! ones the session journalled. The heading of the newest block is the only one
//! with a time, because it is the only one this build can know: an edit update
//! carries no timestamp, and a local arrival time would be a different fact wearing
//! the same label — so the older blocks wear `…` and say nothing they cannot prove.
//!
//! The body of a block is wrapped to the panel with two columns of indent and
//! **never truncated**: a revision a reader cannot read is one that hides the very
//! change it exists to show. The selected block takes reverse video whole, heading
//! and body, which is how a block is told from the one above it when both are
//! prose.
//!
//! **`Clear` first**, bounded to the column's own rectangle, for the reason the
//! forward picker gives: without it the blocks are drawn over the messages
//! underneath and the two read as one column of text.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};

use crate::app::App;
use crate::date;
use crate::wrap::wrap_decorated;

/// The sentence the panel says for a message no revision was seen for.
///
/// Dim, and a sentence rather than a refusal: an unedited message is not a
/// mistake, and the panel that opened is the answer to the key.
const EMPTY: &str = "No recorded revision: this text is the only version.";

/// How far a block's text is indented under its heading.
///
/// The heading marks where a block starts and the body is the quoted evidence, so
/// the body is set in from it: two columns is the indent the design gives a
/// revision, and it is what makes a wrapped block read as one thing.
const INDENT: usize = 2;

/// One block of the overlay: a heading and the text it is read under.
struct Revision {
    /// `2d  current`, `…  revision`, `…  original`.
    head: String,
    /// What that revision said.
    text: String,
}

/// The blocks the overlay draws, newest first.
///
/// The current text is block zero and the journal's revisions follow it, so the
/// first block is what the message says and the last is the text its sender first
/// wrote. A message no revision was recorded for has **no blocks**: the panel says
/// so in a sentence, because the current text alone is not a history of anything.
/// A message the window no longer holds keeps its revisions and loses the current
/// block, since there is no text left to call current.
fn blocks(app: &App, message_id: i64) -> Vec<Revision> {
    let view = &app.conversation.conversation;
    let revisions = view.revisions(message_id);
    if revisions.is_empty() {
        return Vec::new();
    }

    let mut blocks = Vec::with_capacity(1 + revisions.len());

    if let Some(message) = view.message(message_id) {
        let age = if date::has_time(message.timestamp) {
            date::age(message.timestamp, app.now())
        } else {
            "…".to_owned()
        };
        blocks.push(Revision {
            head: format!("{age}  current"),
            text: message.display_body().to_owned(),
        });
    }

    for (index, text) in revisions.iter().enumerate() {
        // The last revision is the text the sender first wrote, because the
        // journal keeps them newest first and drops the oldest at the bound.
        let label = if index + 1 == revisions.len() {
            "original"
        } else {
            "revision"
        };
        blocks.push(Revision {
            head: format!("…  {label}"),
            text: text.to_string(),
        });
    }

    blocks
}

pub fn render(app: &App, conversation: Rect, frame: &mut Frame<'_>) {
    let Some(message_id) = app.conversation.edit_history.message_id() else {
        return;
    };

    frame.render_widget(Clear, conversation);
    // A kitty picture is not in the buffer, so `Clear` does not cover it: the
    // conversation recorded it this frame, under the column the overlay has taken.
    app.ui.placements.borrow_mut().clear();

    let blocks = blocks(app, message_id);
    let panel = Block::default()
        .borders(Borders::ALL)
        .border_style(app.ui.theme.border)
        .title(format!(" Edit history · {} ", blocks.len()));
    let inner = panel.inner(conversation);
    frame.render_widget(panel, conversation);

    if blocks.is_empty() {
        // No revision was recorded for this message, so there is nothing to
        // compare the text with: the panel says so in a sentence rather than
        // drawing the current text as a block of one, which would be a history of
        // nothing.
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(EMPTY, app.ui.theme.text_dim))),
            inner,
        );
        return;
    }

    // The selection is the block, the cursor is a text position, and the list
    // above needs one of its *lines*: the block's heading is the line the reader
    // is looking at, so it is the one the list is scrolled to.
    let selected = app
        .conversation
        .edit_history
        .selected()
        .min(blocks.len() - 1);
    let mut items: Vec<ListItem> = Vec::new();
    let mut heading = 0;
    for (index, block) in blocks.iter().enumerate() {
        let on = index == selected;
        if on {
            heading = items.len();
        }

        // The heading is metadata and the body is the evidence; a selected block
        // brightens both, because the reverse video is the block's and a dim
        // heading inside it would read as a line of its own.
        let ink = if on {
            app.ui.theme.text
        } else {
            app.ui.theme.text_dim
        };
        let mut lines = vec![Line::from(Span::styled(block.head.clone(), ink))];
        // The body, indented and wrapped: `wrap_decorated` reserves the first
        // row's prefix and the later rows' suffix, and a block's rows are all
        // indented the same, so both are asked for the same two columns.
        for range in wrap_decorated(&block.text, INDENT, INDENT, inner.width) {
            let text = block.text.get(range).unwrap_or_default();
            lines.push(Line::from(Span::styled(
                format!("{}{text}", " ".repeat(INDENT)),
                app.ui.theme.text,
            )));
        }

        // A block is selected whole — heading and body — because a block is what
        // the reader is choosing between, and an outline round one line of it
        // would look like a line selection. The item's own style is what fills the
        // row, so the reverse video covers the panel rather than the text.
        let style = if on {
            app.ui.theme.selection
        } else {
            Style::default()
        };
        items.extend(
            lines
                .into_iter()
                .map(|line| ListItem::new(line).style(style)),
        );
    }

    let mut state = ListState::default();
    state.select(Some(heading));
    // The block's own style is the selection; a highlight laid on top of it would
    // be a second answer to the same question.
    let list = List::new(items).highlight_style(Style::default());
    frame.render_stateful_widget(list, inner, &mut state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::history::ConversationView;
    use domain::message::{MediaKind, Message, MessageStatus};
    use domain::updates::UpdateEvent;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Modifier;
    use std::borrow::Cow;

    const AT: i64 = 1_730_000_000;

    fn chat() -> i64 {
        App::mock().conversation.conversation.window.chat_id
    }

    fn message(id: i64, text: &'static str) -> Message {
        Message {
            id,
            chat_id: chat(),
            text: Cow::Borrowed(text),
            timestamp: AT,
            status: MessageStatus::Received,
            is_outgoing: true,
            reply_to: None,
            media: None::<MediaKind>,
            is_service: false,
            media_id: None,
        }
    }

    /// An application showing one message, edited to `edits` in order.
    fn showing(edits: &[&'static str]) -> App {
        let mut app = App::mock();
        let mut view = ConversationView::new(chat());
        view.window.replace([message(5, "the first version")]);
        for text in edits {
            let _ = view.apply_event(&UpdateEvent::MessageEdited {
                chat_id: chat(),
                message_id: 5,
                new_text: Cow::Borrowed(text),
            });
        }
        app.conversation.conversation = view;
        app.conversation.vim.set_total(1);
        app
    }

    fn screen(app: &App, width: u16, height: u16) -> Buffer {
        let mut terminal =
            Terminal::new(TestBackend::new(width, height)).expect("the test backend builds");
        terminal
            .draw(|frame| app.render(frame))
            .expect("the frame draws");

        terminal.backend().buffer().clone()
    }

    /// The conversation column's first column: the chat list takes the left 30 per
    /// cent of the screen.
    fn conversation_column(buffer: &Buffer) -> usize {
        usize::from(buffer.area.width) * 30 / 100
    }

    /// What the conversation column says on row `y`.
    fn column_text(buffer: &Buffer, y: u16) -> String {
        (conversation_column(buffer)..usize::from(buffer.area.width))
            .map(|x| buffer[(u16::try_from(x).expect("a column"), y)].symbol())
            .collect()
    }

    fn row_of(buffer: &Buffer, needle: &str) -> Option<u16> {
        (0..buffer.area.height).find(|&y| column_text(buffer, y).contains(needle))
    }

    fn blocks_of(app: &App, id: i64) -> Vec<(String, String)> {
        blocks(app, id)
            .into_iter()
            .map(|block| (block.head, block.text))
            .collect()
    }

    #[test]
    fn the_blocks_are_the_current_text_then_the_revisions_newest_first() {
        let app = showing(&["the middle version", "the last version"]);

        assert_eq!(
            blocks_of(&app, 5),
            vec![
                ("now  current".to_owned(), "the last version".to_owned()),
                ("…  revision".to_owned(), "the middle version".to_owned()),
                ("…  original".to_owned(), "the first version".to_owned()),
            ]
        );
    }

    /// Only the newest block has an age: the journal records no time, and an
    /// invented one would be a fact the reader would read as the edit's.
    #[test]
    fn only_the_current_block_carries_a_time() {
        let app = showing(&["after"]);

        let heads: Vec<String> = blocks_of(&app, 5)
            .into_iter()
            .map(|(head, _)| head)
            .collect();
        assert!(heads[0].ends_with("current"), "{heads:?}");
        assert!(
            !heads[0].starts_with('…'),
            "the current block says when: {heads:?}"
        );
        assert_eq!(heads[1], "…  original", "and the revision does not");
    }

    #[test]
    fn a_message_no_revision_was_seen_for_says_so_in_the_dim_ink() {
        let mut app = showing(&[]);
        app.conversation.edit_history.begin(5);

        let buffer = screen(&app, 80, 24);

        let y = row_of(&buffer, EMPTY).expect("the empty sentence is drawn");
        let x = column_text(&buffer, y).find(EMPTY).expect("its column");
        let cell = &buffer[(
            u16::try_from(conversation_column(&buffer) + x).expect("a column"),
            y,
        )];
        assert_eq!(
            cell.fg,
            app.ui.theme.text_dim.fg.expect("dim is a foreground"),
            "it is the ink metadata uses"
        );
        assert!(
            blocks_of(&app, 5).is_empty(),
            "the current text alone is not a block: the sentence stands for it"
        );
        assert_eq!(
            app.conversation
                .edit_history
                .len(&app.conversation.conversation),
            0
        );
    }

    #[test]
    fn the_panel_is_titled_with_the_number_of_blocks() {
        let mut app = showing(&["middle", "last"]);
        app.conversation.edit_history.begin(5);

        let buffer = screen(&app, 80, 24);

        assert!(
            row_of(&buffer, "Edit history · 3").is_some(),
            "the title counts the current text too"
        );
    }

    /// The selected block is reverse video whole — heading and body — so a reader
    /// can tell which block is which when both are prose.
    #[test]
    fn the_selected_block_is_reverse_video_and_the_others_are_not() {
        let mut app = showing(&["middle", "last"]);
        app.conversation.edit_history.begin(5);

        let buffer = screen(&app, 80, 24);

        let reversed = |y: u16| {
            (conversation_column(&buffer)..usize::from(buffer.area.width)).any(|x| {
                buffer[(u16::try_from(x).expect("a column"), y)]
                    .modifier
                    .contains(Modifier::REVERSED)
            })
        };
        let current = row_of(&buffer, "current").expect("the current heading");
        let original = row_of(&buffer, "original").expect("the original heading");
        assert!(
            reversed(current + 1),
            "the selected block's body is reversed"
        );
        assert!(!reversed(original + 1), "and the other block is not");
    }

    /// A revision is wrapped, never clipped: the change is what the block exists to
    /// show, and half of it is not the change.
    #[test]
    fn a_long_revision_wraps_rather_than_being_cut() {
        let long = "one two three four five six seven eight nine ten eleven twelve \
                    thirteen fourteen fifteen sixteen";
        let mut app = showing(&[long]);
        app.conversation.edit_history.begin(5);

        let buffer = screen(&app, 80, 40);

        let head = row_of(&buffer, "one two three").expect("the revision's start");
        let tail = row_of(&buffer, "sixteen").expect("and its end");
        assert_ne!(
            head, tail,
            "the revision wrapped: it is on more than one row rather than cut"
        );
        assert!(
            !column_text(&buffer, tail).contains('…'),
            "and nothing in it was elided: {:?}",
            column_text(&buffer, tail)
        );
    }

    #[test]
    fn nothing_is_drawn_when_the_overlay_is_not_open() {
        let app = showing(&["after"]);

        let buffer = screen(&app, 80, 24);

        assert!(row_of(&buffer, "Edit history").is_none());
    }
}
