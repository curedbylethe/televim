//! The archived-chats overlay.
//!
//! It answers `:archived`: the conversations the account has archived, drawn over
//! the chat column. `Enter` puts the row under the cursor back in the visible
//! list, which is why the overlay exists at all — an archive with no way back
//! would be a deletion with a longer name.
//!
//! The same panel shape as the contacts and new-conversation overlays. An empty
//! list draws nothing and the status line says `archived — 0`; the state closes
//! itself when the last row leaves.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};

use crate::app::{App, Focus};
use crate::rows::truncate;
use crate::wrap::columns;

use super::chat_list::drawn;

/// Where the overlay draws, or `None` when there is nothing to draw into.
fn overlay_area(app: &App, chat_list: Rect) -> Option<Rect> {
    if app.ui.focus == Focus::Input
        || !app.list.archived_is_open()
        || app.list.archived().is_empty()
    {
        return None;
    }

    let height = (app.list.archived().len() + 2).min(usize::from(chat_list.height));
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

    let selected = app.list.archived_cursor();
    let width = usize::from(area.width).saturating_sub(2);
    let items: Vec<ListItem> = app
        .list
        .archived()
        .iter()
        .take(usize::from(area.height).saturating_sub(2))
        .enumerate()
        .map(|(index, chat)| {
            let quiet = if index == selected {
                app.ui.theme.text
            } else {
                app.ui.theme.text_dim
            };
            // The unread count at the right edge, so a chat with news reads
            // differently from one without; the title gives way to it.
            let unread = (chat.unread_count > 0).then(|| chat.unread_count.to_string());
            let reserved = unread.as_ref().map_or(0, |count| columns(count) + 1);
            let title = truncate(&chat.title, width.saturating_sub(reserved));
            let mut spans = drawn(app, &title, app.ui.theme.text);
            if let Some(count) = unread {
                let gap = width.saturating_sub(columns(&title) + columns(&count));
                spans.push(Span::raw(" ".repeat(gap)));
                spans.push(Span::styled(count, quiet));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(app.ui.theme.border)
                .title(format!(" Archived · {} ", app.list.archived().len())),
        )
        .highlight_style(app.ui.theme.selection);

    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::app::App;
    use crate::bidi::BidiMode;

    fn screen(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("the backend builds");
        terminal
            .draw(|frame| app.render(frame))
            .expect("the frame draws");
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn an_open_archive_draws_its_rows_under_a_counted_title() {
        let mut app = App::mock();
        let mut chat = app.chats()[0].clone();
        chat.id = 99;
        "Old Friend".clone_into(&mut chat.title);
        app.list.install_archived(vec![chat]);
        assert!(!screen(&app).contains("Archived ·"), "closed draws nothing");

        app.list.open_archived();
        let shown = screen(&app);

        assert!(shown.contains("Archived · 1"), "{shown}");
        assert!(shown.contains("Old Friend"), "{shown}");
        assert_eq!(app.status_text(), "archived — 1");
    }

    #[test]
    fn a_right_to_left_title_follows_the_bidi_mode() {
        const TITLE: &str = "سلام";
        let reversed: String = TITLE.chars().rev().collect();
        for (mode, expected, absent) in [
            (BidiMode::Visual, reversed.as_str(), TITLE),
            (BidiMode::Terminal, TITLE, reversed.as_str()),
        ] {
            let mut app = App::mock().with_bidi(mode);
            let mut chat = app.chats()[0].clone();
            chat.id = 99;
            TITLE.clone_into(&mut chat.title);
            app.list.install_archived(vec![chat]);
            app.list.open_archived();

            let shown = screen(&app);

            assert!(shown.contains(expected), "{mode:?}:\n{shown}");
            assert!(!shown.contains(absent), "{mode:?}:\n{shown}");
        }
    }

    #[test]
    fn an_archived_chat_with_news_shows_its_unread_count() {
        let mut app = App::mock();
        let mut quiet = app.chats()[0].clone();
        quiet.id = 98;
        "Quiet One".clone_into(&mut quiet.title);
        quiet.unread_count = 0;
        let mut loud = app.chats()[0].clone();
        loud.id = 99;
        "Loud One".clone_into(&mut loud.title);
        loud.unread_count = 7;
        app.list.install_archived(vec![quiet, loud]);
        app.list.open_archived();

        let shown = screen(&app);
        let row = |needle: &str| {
            shown
                .lines()
                .find(|line| line.contains(needle))
                .unwrap_or_else(|| panic!("{needle} is drawn:\n{shown}"))
                .to_owned()
        };

        assert!(row("Loud One").contains(" 7"), "{shown}");
        assert!(!row("Quiet One").contains(" 0"), "{shown}");
    }
}
