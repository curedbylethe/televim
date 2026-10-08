//! The forward picker: the chats a selection can be forwarded into.
//!
//! It is drawn over the conversation column, not the chat list, because the
//! selection it is forwarding is in the conversation: the reader keeps their
//! place in the messages while they choose. One row per chat, the title first,
//! then the newest message's text, with a `📌` in front of a pinned chat.
//!
//! **`Clear` first**, bounded to the column's own rectangle, for the reason the
//! new-conversation overlay gives: without it the rows are drawn over the
//! messages underneath and the two read as one list.
//!
//! The highlight is `theme.selection`, the reverse video the chat list uses, so
//! the cursor reads the same wherever it is.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};

use crate::app::App;

/// The header line for `count` messages, in the words the design gives.
fn header(count: usize) -> String {
    let noun = if count == 1 { "message" } else { "messages" };
    format!("Forward {count} {noun} to…")
}

pub fn render(app: &App, conversation: Rect, frame: &mut Frame<'_>) {
    let Some(pick) = app.conversation.picking() else {
        return;
    };

    frame.render_widget(Clear, conversation);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(app.ui.theme.border_focused);
    let inner = block.inner(conversation);
    frame.render_widget(block, conversation);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            header(pick.message_ids.len()),
            app.ui.theme.text,
        ))),
        rows[0],
    );

    let items: Vec<ListItem> = app
        .chats()
        .iter()
        .map(|chat| {
            let pin = if chat.pinned { "📌 " } else { "" };
            let preview = chat
                .last_message
                .as_deref()
                .unwrap_or_default()
                .replace('\n', " ");
            ListItem::new(Line::from(vec![
                Span::styled(pin, app.ui.theme.text),
                Span::styled(chat.title.clone(), app.ui.theme.text),
                Span::styled("  ", app.ui.theme.text_dim),
                Span::styled(preview, app.ui.theme.text_dim),
            ]))
        })
        .collect();

    let list = List::new(items).highlight_style(app.ui.theme.selection);

    let mut state = ListState::default();
    state.select(Some(pick.selected));
    frame.render_stateful_widget(list, rows[1], &mut state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Modifier;

    use crate::app::Mode;
    use crate::state::conversation::Forwarding;
    use domain::selection::Selection;

    /// The whole panel drawn, so the picker is judged where the reader sees it.
    fn screen(app: &App, width: u16, height: u16) -> Buffer {
        let mut terminal =
            Terminal::new(TestBackend::new(width, height)).expect("the test backend builds");
        terminal
            .draw(|frame| app.render(frame))
            .expect("the frame draws");

        terminal.backend().buffer().clone()
    }

    /// Everything one row says, as a string.
    fn row_text(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    /// The first column the conversation column is drawn from: the chat list
    /// takes the left 30 per cent of the screen.
    fn conversation_column(buffer: &Buffer) -> u16 {
        buffer.area.width * 30 / 100
    }

    /// What the conversation column says on row `y`, which is the only part of
    /// the row the picker is drawn in: the chat list beside it has its own rows,
    /// with the same titles.
    fn picker_text(buffer: &Buffer, y: u16) -> String {
        (conversation_column(buffer)..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    /// The first row of the conversation column that says `needle`, if one does.
    fn row_of(buffer: &Buffer, needle: &str) -> Option<u16> {
        (0..buffer.area.height).find(|&y| picker_text(buffer, y).contains(needle))
    }

    /// An app with the picker up over the first message.
    fn picking() -> App {
        let mut app = App::mock();
        app.set_selection(Selection::at(1, None));
        app.ui.set_mode(Mode::Visual);
        app.conversation.open_forward(Forwarding {
            ids: vec![1],
            skipped: 0,
        });
        app
    }

    #[test]
    fn the_header_names_how_many_messages_and_pluralises_for_one() {
        assert_eq!(header(1), "Forward 1 message to…");
        assert_eq!(header(3), "Forward 3 messages to…");
    }

    #[test]
    fn the_header_is_drawn_above_the_chats() {
        let app = picking();

        let buffer = screen(&app, 80, 24);

        let header_row = row_of(&buffer, "Forward 1 message to…").expect("the header is drawn");
        let chat_row = row_of(&buffer, &app.chats()[0].title).expect("the first chat is listed");
        assert!(chat_row > header_row, "the chats sit under the header");
    }

    #[test]
    fn the_picker_is_drawn_over_the_conversation_column_only() {
        let app = picking();
        let buffer = screen(&app, 80, 24);

        let header_row = row_of(&buffer, "Forward 1 message to…").expect("the header is drawn");
        let left = (0..conversation_column(&buffer))
            .map(|x| buffer[(x, header_row)].symbol())
            .collect::<String>();
        assert!(
            !left.contains("Forward"),
            "the header is not drawn over the chat list: {left:?}"
        );
    }

    #[test]
    fn the_selected_chat_is_drawn_in_reverse_video_and_the_others_are_not() {
        let mut app = picking();
        app.conversation
            .forward
            .as_mut()
            .expect("the picker is up")
            .selected = 1;

        let buffer = screen(&app, 80, 24);

        let reversed = |y: u16| {
            (conversation_column(&buffer)..buffer.area.width)
                .any(|x| buffer[(x, y)].modifier.contains(Modifier::REVERSED))
        };
        let second = row_of(&buffer, &app.chats()[1].title).expect("the second chat is listed");
        let first = row_of(&buffer, &app.chats()[0].title).expect("the first chat is listed");
        assert!(reversed(second), "the selected chat is reverse video");
        assert!(!reversed(first), "and the others are not");
    }

    #[test]
    fn a_pinned_chat_is_marked_in_the_picker() {
        let mut app = picking();
        let pinned = app.chats()[2].id;
        let title = app.chats()[2].title.clone();
        app.set_pinned(pinned, true);

        let buffer = screen(&app, 80, 24);

        let y = row_of(&buffer, &title).expect("the pinned chat is listed");
        assert!(
            row_text(&buffer, y).contains("📌"),
            "the pinned chat carries its marker"
        );
    }

    #[test]
    fn nothing_is_drawn_when_no_forward_is_being_directed() {
        let app = App::mock();

        let buffer = screen(&app, 80, 24);

        assert!(
            row_of(&buffer, "Forward").is_none(),
            "no header without a picker"
        );
    }
}
