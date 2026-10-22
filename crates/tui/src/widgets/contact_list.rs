//! The contacts directory overlay.
//!
//! It answers `:contacts`: the account's own address book, drawn over the chat
//! column the way the new-conversation results are. The two lists are the same
//! shape — a bordered panel anchored to the bottom of that column, reverse video
//! on the cursor, no highlight symbol — and differ only in what they hold: a
//! search shows the answer to a query, this shows everybody.
//!
//! **Empty draws nothing.** The status line already says `contacts — 0`, and an
//! empty box over the chat list would cover the list the reader is about to go
//! back to while telling them less than the sentence does. The same rule the
//! new-conversation overlay follows, for the same reason.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};

use crate::app::{App, Focus};
use crate::wrap::columns;

/// Where the overlay draws, or `None` when there is nothing to draw into.
fn overlay_area(app: &App, chat_list: Rect) -> Option<Rect> {
    let contacts = &app.conversation.contacts;
    if app.ui.focus == Focus::Input || !contacts.is_active() || contacts.items().is_empty() {
        return None;
    }

    let height = (contacts.items().len() + 2).min(usize::from(chat_list.height));
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

    let contacts = &app.conversation.contacts;
    let width = usize::from(area.width).saturating_sub(2);
    let selected = contacts.selected();
    let items: Vec<ListItem> = contacts
        .items()
        .iter()
        .take(usize::from(area.height).saturating_sub(2))
        .enumerate()
        .map(|(index, candidate)| {
            // On the cursor row the quiet ink is the body ink: reverse video
            // turns a dim foreground into a dim background.
            let quiet = if index == selected {
                app.ui.theme.text
            } else {
                app.ui.theme.text_dim
            };
            // `chat` when a conversation already exists, in the list or the
            // archive, `new` when it does not. Reserved first, like the
            // new-chat list, so the name gives way and the standing never does.
            let standing = if app
                .chats()
                .iter()
                .chain(app.list.archived())
                .any(|chat| chat.id == candidate.user_id)
            {
                "chat"
            } else {
                "new"
            };
            let budget = width.saturating_sub(columns(standing) + 1);
            let name = truncate(&candidate.display_name, budget);
            let mut used = columns(&name);
            let mut spans = vec![Span::raw(name)];
            // The handle only when it fits whole: a cut `@name` is one the
            // reader cannot search for.
            if let Some(username) = candidate.username.as_deref() {
                let handle = format!("  @{username}");
                if used + columns(&handle) <= budget {
                    used += columns(&handle);
                    spans.push(Span::styled(handle, quiet));
                }
            }
            spans.push(Span::raw(" ".repeat(budget.saturating_sub(used) + 1)));
            spans.push(Span::styled(standing, quiet));
            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(app.ui.theme.border)
                .title(format!(" Contacts · {} ", contacts.items().len())),
        )
        .highlight_style(app.ui.theme.selection);

    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}

/// Cuts a label to `width` columns, counting a wide character as two.
pub(super) fn truncate(label: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for ch in label.chars() {
        let cells = columns(&ch.to_string());
        if used + cells > width {
            break;
        }
        out.push(ch);
        used += cells;
    }
    out
}

#[cfg(test)]
mod tests {
    use domain::user::UserCandidate;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::app::App;

    fn candidate(user_id: i64, display_name: &str, username: Option<&str>) -> UserCandidate {
        UserCandidate {
            user_id,
            display_name: display_name.to_owned(),
            username: username.map(str::to_owned),
        }
    }

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
    fn the_directory_draws_a_row_per_person_under_a_counted_title() {
        let mut app = App::mock();
        app.conversation.contacts.begin();
        app.conversation.contacts.adopt(vec![
            candidate(1, "Zed Zeta", Some("zed")),
            candidate(2, "Yan Yoon", None),
        ]);

        let shown = screen(&app);

        assert!(shown.contains("Contacts · 2"), "{shown}");
        assert!(shown.contains("Zed Zeta  @zed"), "{shown}");
        assert!(shown.contains("Yan Yoon"), "{shown}");
    }

    /// A row ends in `chat` when a conversation exists and `new` when it does
    /// not, so the directory can say which people have been messaged.
    #[test]
    fn a_row_says_whether_the_person_already_has_a_conversation() {
        let mut app = App::mock();
        let known = app.chats()[0].id;
        app.conversation.contacts.begin();
        app.conversation.contacts.adopt(vec![
            candidate(known, "Zed Zeta", Some("zed")),
            candidate(9_999, "Yan Yoon", None),
        ]);

        let shown = screen(&app);
        let row = |needle: &str| {
            shown
                .lines()
                .find(|line| line.contains(needle))
                .unwrap_or_else(|| panic!("{needle} is drawn:\n{shown}"))
                .to_owned()
        };

        assert!(row("Zed Zeta").contains("@zed") && row("Zed Zeta").contains("chat"));
        assert!(row("Yan Yoon").contains("new"));
    }

    #[test]
    fn an_empty_directory_draws_nothing_and_leaves_the_sentence_to_the_status_line() {
        let mut app = App::mock();
        app.conversation.contacts.begin();
        app.conversation.contacts.adopt(Vec::new());

        assert!(!screen(&app).contains("Contacts ·"));
        assert_eq!(app.status_text(), "contacts — 0");
    }
}
