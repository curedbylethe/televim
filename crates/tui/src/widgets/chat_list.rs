//! Chat list panel.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};

use crate::app::{App, Focus};
use crate::bidi::{self, BidiMode};
use crate::text_row::{self, Ink, TextRow};

/// `text` in `style`, in the order the terminal reads it under `app.bidi()`.
///
/// Under [`BidiMode::Visual`] the text's runs are laid out in visual order, the
/// same as a conversation row; under [`BidiMode::Terminal`] it is split per
/// cluster so no glyph is clipped. Shared with the forward picker, which draws
/// the same titles.
pub(crate) fn drawn(app: &App, text: &str, style: Style) -> Vec<Span<'static>> {
    let row = TextRow {
        text,
        range: 0..text.len(),
        matched: false,
        selected: None,
        caret: None,
        reversed: false,
        concealed: false,
        ink: Ink {
            plain: style,
            ..Ink::readonly(&app.ui.theme)
        },
    };
    let spans = match app.bidi() {
        BidiMode::Terminal => text_row::per_cluster(text_row::spans(&row)),
        BidiMode::Visual => {
            let base = bidi::base_direction(text);
            let pieces = bidi::visual_row_in(text, 0..text.len(), base);
            text_row::spans_permuted(&row, &pieces)
        }
    };
    // Owned, because the callers' rows outlive the text they were built from: a
    // preview is a local string with its newlines taken out.
    spans
        .into_iter()
        .map(|span| Span::styled(span.content.into_owned(), span.style))
        .collect()
}

pub fn render(app: &App, area: Rect, frame: &mut Frame<'_>) {
    let items: Vec<ListItem> = app
        .chats()
        .iter()
        .map(|c| {
            let unread = if c.unread_count > 0 {
                format!(" ({})", c.unread_count)
            } else {
                String::new()
            };
            // A pinned chat leads its row with the pin, so the marker says why
            // the chat is at the top as well as that it is.
            let pin = if c.pinned { "📌 " } else { "" };
            let mut spans = vec![Span::styled(pin, app.ui.theme.text)];
            spans.extend(drawn(app, &c.title, app.ui.theme.text));
            spans.push(Span::styled(unread, app.ui.theme.text_dim));
            ListItem::new(Line::from(spans))
        })
        .collect();

    let title = format!(" Chats ({}) ", app.chats().len());
    // The focused pane's border is the only thing that says where the keys go,
    // so the two panes cannot both be drawn as though they had it.
    let border = if app.ui.focus == Focus::ChatList {
        app.ui.theme.border_focused
    } else {
        app.ui.theme.border
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border)
                .title(title),
        )
        .highlight_style(app.ui.theme.selection);

    let mut state = ListState::default();
    state.select(Some(app.list.selected_chat));
    frame.render_stateful_widget(list, area, &mut state);
}

// Kept for potential future use.
#[allow(dead_code)]
fn _unused(_: Style) {}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    /// The chat list drawn into a screen of `width` by `height`.
    fn screen(app: &App, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test backend");
        terminal
            .draw(|frame| render(app, frame.area(), frame))
            .expect("the chat list draws");
        terminal.backend().buffer().clone()
    }

    /// Everything one row says, as a string.
    fn row_text(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    #[test]
    fn a_pinned_chat_leads_the_list_with_its_marker() {
        let mut app = App::mock();
        let title = app.chats()[1].title.clone();
        let chat = app.chats()[1].id;
        app.set_pinned(chat, true);

        // Row 0 is the border; row 1 is the first chat.
        let buffer = screen(&app, 40, 10);
        let first = row_text(&buffer, 1);

        assert!(first.contains("📌"), "the pinned row is marked: {first:?}");
        assert!(
            first.contains(&title),
            "and it is the pinned chat: {first:?}"
        );
    }

    #[test]
    fn an_unpinned_chat_has_no_marker() {
        let app = App::mock();

        let buffer = screen(&app, 40, 10);

        for y in 1..4 {
            assert!(
                !row_text(&buffer, y).contains("📌"),
                "row {y} is not pinned"
            );
        }
    }

    const ARABIC: &str = "سلام";

    /// The chat list with its first title set to [`ARABIC`], drawn under `mode`.
    fn arabic_title(mode: BidiMode) -> App {
        let mut app = App::mock().with_bidi(mode);
        let mut chats = app.chats().to_vec();
        chats[0].title = ARABIC.into();
        app.set_chats(chats);
        app
    }

    #[test]
    fn a_terminal_title_is_drawn_in_logical_order_in_full() {
        let app = arabic_title(BidiMode::Terminal);

        let buffer = screen(&app, 40, 10);

        assert!(
            row_text(&buffer, 1).contains(ARABIC),
            "the terminal shapes it: {:?}",
            row_text(&buffer, 1)
        );
    }

    #[test]
    fn a_visual_title_is_drawn_reversed_in_full() {
        let app = arabic_title(BidiMode::Visual);
        let reversed: String = ARABIC.chars().rev().collect();

        let buffer = screen(&app, 40, 10);

        assert!(
            row_text(&buffer, 1).contains(&reversed),
            "the title is in visual order: {:?}",
            row_text(&buffer, 1)
        );
    }
}
