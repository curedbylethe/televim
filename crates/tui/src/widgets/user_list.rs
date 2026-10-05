//! The new-conversation results overlay.
//!
//! It answers `/` on the chat list: the reader names a person, the server offers
//! whoever matches, and this is the short list they pick from. It is modelled on
//! [`crate::widgets::emoji_popup`] because the shape is the same one — a
//! transient list drawn over a pane the reader is not editing — and it keeps the
//! same two rules.
//!
//! **`Clear` first**, and bounded to the chat list's own rectangle. Without it
//! the overlay's rows are drawn over the chat list that was already there, and a
//! `Clear`-less overlay is a smear of two lists. A `Clear` larger than the
//! overlay paints out things it does not own, which is why the rectangle is the
//! widget's own and not the pane's whole area.
//!
//! **The highlight is `theme.selection`**, the reverse video the chat list and
//! the conversation use, and there is no `highlight_symbol`: nothing else in
//! this workspace draws one, and reverse video belongs to the cursor and to
//! nothing else.
//!
//! It grows **upward** from the bottom of the chat-list column into that column,
//! so it sits directly above the input bar the query was typed into, and it is
//! capped to the column's own height: it cannot spill into the conversation to
//! its right or the bar below it.
//!
//! # Empty is not drawn
//!
//! A search with nothing to show draws no overlay at all. An empty, in-flight or
//! failed search is spoken for by [`UserSearchState::label`] on the status line,
//! and an empty box over the chat list would tell the reader less than the
//! sentence already does — and would cover the list they are about to go back
//! to. The overlay is for choices, and a search with no candidates has none.
//!
//! [`UserSearchState::label`]: domain::user::UserSearchState::label

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};

use crate::app::{App, Focus};

/// Where the overlay draws, or `None` when there is nothing to draw into.
///
/// Every field saturates: `panic = "abort"` turns a `u16` underflow into a wrap
/// in release and a panic in debug, and a short terminal must produce neither.
/// No border plus at least one candidate is below the floor — a one-row overlay
/// would be all border — so that case draws nothing rather than a frame with no
/// room in it.
///
/// It is drawn only while the line does **not** have the focus: re-opening `/`
/// is the reader editing the question, and the previous answer steps aside
/// rather than sitting under a query it does not belong to.
fn overlay_area(app: &App, chat_list: Rect) -> Option<Rect> {
    let search = app.user_search();
    if app.focus == Focus::Input || !search.is_active() || search.is_empty() {
        return None;
    }

    let height = (search.len() + 2).min(usize::from(chat_list.height));
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

    let items: Vec<ListItem> = app
        .user_search()
        .candidates()
        .iter()
        .take(usize::from(area.height).saturating_sub(2))
        .map(|candidate| {
            // The display name is what the reader is reading; the handle is a
            // second, dimmer fact about the same person, exactly as the unread
            // count is beside a chat's title. `@` is drawn as stored, so the
            // handle reads as a handle and stays searchable by eye.
            let handle = candidate
                .username
                .as_ref()
                .map_or_else(String::new, |name| format!(" @{name}"));
            ListItem::new(Line::from(vec![
                Span::styled(candidate.display_name.clone(), app.theme.text),
                Span::styled(handle, app.theme.text_dim),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(app.theme.border)
                .title(" New chat "),
        )
        .highlight_style(app.theme.selection);

    // The real index, and `List` scrolls it into view itself — the same as the
    // emoji popup, and the reason this widget has no offset arithmetic.
    let mut state = ListState::default();
    state.select(Some(app.user_search().selected()));
    frame.render_stateful_widget(list, area, &mut state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use domain::user::UserCandidate;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::style::Modifier;

    use crate::widgets::input_bar;

    /// A person the search offered.
    fn candidate(user_id: i64, display_name: &str, username: Option<&str>) -> UserCandidate {
        UserCandidate {
            user_id,
            display_name: display_name.to_owned(),
            username: username.map(str::to_owned),
        }
    }

    /// Sends one keystroke to the application, as the reader would.
    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    /// An application that has run a search for `no` and been answered with these
    /// candidates, as the caller's [`App::apply_users`] would.
    ///
    /// Driven through `/` on the chat list rather than a test-only setter: the
    /// query has to be the one the search is *for* before a result is accepted,
    /// and only the real route establishes that.
    fn showing(candidates: Vec<UserCandidate>) -> App {
        let mut app = App::mock();
        press(&mut app, KeyCode::Char('h'));
        press(&mut app, KeyCode::Char('/'));
        for ch in "no".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press(&mut app, KeyCode::Enter);

        assert!(app.user_search().is_active(), "the search was started");
        assert!(
            app.apply_users("no", candidates),
            "the fixture's answer is for the query on show"
        );
        app
    }

    /// The whole frame, drawn into an in-memory terminal.
    fn screen(app: &App, width: u16, height: u16) -> Buffer {
        let mut terminal =
            Terminal::new(TestBackend::new(width, height)).expect("the test backend builds");
        terminal
            .draw(|frame| app.render(frame))
            .expect("the frame draws");

        terminal.backend().buffer().clone()
    }

    /// Everything the screen says, as one string.
    fn flat(buffer: &Buffer) -> String {
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The row of the screen carrying `needle`, if any.
    fn row_with(buffer: &Buffer, needle: &str) -> Option<u16> {
        (0..buffer.area.height).find(|&y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .contains(needle)
        })
    }

    /// The chat-list rectangle, laid out the way `App::render` lays it out.
    fn chat_list(app: &App, width: u16, height: u16) -> Rect {
        let full = Rect::new(0, 0, width, height);
        let inner = width.saturating_sub(2).max(1);
        let input = 2 + input_bar::content_rows(app, inner);
        let input = u16::try_from(input).unwrap_or(u16::MAX);

        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(input),
                Constraint::Length(1),
            ])
            .split(full);

        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(vertical[0])[0]
    }

    #[test]
    fn the_overlay_draws_the_name_and_the_handle_of_every_candidate() {
        let app = showing(vec![
            candidate(7, "Noor Haddad", Some("noorh")),
            candidate(8, "Ravi Menon", Some("ravim")),
        ]);

        let shown = flat(&screen(&app, 80, 24));

        assert!(shown.contains("Noor Haddad"), "the first name is drawn");
        assert!(shown.contains("@noorh"), "and its handle");
        assert!(shown.contains("Ravi Menon"), "the second name is drawn");
        assert!(shown.contains("@ravim"), "and its handle");
        assert!(shown.contains("New chat"), "under a title that names it");
    }

    /// A person who has not set a handle draws as a name and nothing else: no
    /// empty `@`, which would read as a handle that failed to load.
    #[test]
    fn a_candidate_without_a_handle_draws_only_their_name() {
        let app = showing(vec![candidate(9, "Solo Person", None)]);

        let shown = flat(&screen(&app, 80, 24));

        assert!(shown.contains("Solo Person"));
        assert!(!shown.contains('@'), "no handle is drawn at all: {shown}");
    }

    #[test]
    fn the_highlighted_row_is_the_selected_candidate() {
        let mut app = showing(vec![
            candidate(7, "Noor Haddad", Some("noorh")),
            candidate(8, "Ravi Menon", Some("ravim")),
        ]);
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

        let buffer = screen(&app, 80, 24);
        let y = row_with(&buffer, "Ravi Menon").expect("the selected name is drawn");
        let x = (0..buffer.area.width)
            .find(|&x| buffer[(x, y)].symbol() == "R")
            .expect("its first column");

        assert!(
            buffer[(x, y)].modifier.contains(Modifier::REVERSED),
            "the row the cursor is on is the highlighted one"
        );
    }

    /// Nothing to show means nothing drawn: no `Clear`, no border, no title over
    /// the chat list. The status line's label says why.
    #[test]
    fn a_search_with_no_candidates_draws_no_overlay() {
        let app = showing(Vec::new());

        let rect = chat_list(&app, 80, 24);
        assert!(
            overlay_area(&app, rect).is_none(),
            "there is no list to draw"
        );

        let shown = flat(&screen(&app, 80, 24));
        assert!(
            !shown.contains("New chat"),
            "and no empty box is left over the list: {shown}"
        );
    }

    /// The overlay covers the column it was asked from and stops at that column's
    /// bottom: it never reaches the conversation to its right or the bar below.
    #[test]
    fn the_overlay_stays_inside_the_chat_list_column() {
        let app = showing(vec![candidate(7, "Noor Haddad", Some("noorh"))]);
        let column = chat_list(&app, 80, 24);

        let rect = overlay_area(&app, column).expect("there is a candidate to draw");

        assert_eq!(rect.x, column.x, "it starts where the list does");
        assert_eq!(rect.width, column.width, "and is as wide as the column");
        assert!(
            rect.bottom() <= column.bottom(),
            "it cannot grow past the column's own bottom: {rect:?} vs {column:?}"
        );
    }

    /// The status line names the query and the count, which is the one place an
    /// ambiguous set of results is quantified.
    #[test]
    fn the_status_line_names_the_query_and_the_count() {
        let app = showing(vec![
            candidate(7, "Noor Haddad", Some("noorh")),
            candidate(8, "Ravi Menon", Some("ravim")),
        ]);

        assert_eq!(app.status_text(), "/no — 2 candidates");
    }

    /// Re-opening `/` hides the previous answer: the reader is editing the
    /// question, and a list under a query it does not belong to would be a lie
    /// about which question is on screen.
    #[test]
    fn the_overlay_hides_while_a_new_query_is_being_typed() {
        let mut app = showing(vec![candidate(7, "Noor Haddad", Some("noorh"))]);

        press(&mut app, KeyCode::Char('h'));
        press(&mut app, KeyCode::Char('/'));
        assert_eq!(app.focus, Focus::Input, "the prompt is open again");

        let column = chat_list(&app, 80, 24);
        assert!(
            overlay_area(&app, column).is_none(),
            "the old list is put away while the new query is typed"
        );
        let shown = flat(&screen(&app, 80, 24));
        assert!(!shown.contains("Noor Haddad"), "{shown}");
    }
}
