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
//! # Rows
//!
//! A row carries the three things the design's drawing does: the display name at
//! the left, the `@username` beside it, and the **standing** at the right —
//! `chat` when a conversation with that person already exists, `new` when
//! choosing them makes one. The standing is a word rather than a colour because
//! the palette has no role for it and the design says so; it is read off the
//! chat list behind the overlay, so the two cannot disagree about who is already
//! a chat. The fragment the reader typed is inked `match` inside the name (a
//! name query) or the handle (a `@` query), the role a search match takes in a
//! message.
//!
//! On the highlighted row the quiet ink is `text` rather than `text_dim`: a dim
//! token under reverse video reverses into a dim *background*, which the design
//! names as the one thing a cursor row must never draw. The whole row is one ink
//! on that row, and the dim returns when the cursor moves off it.
//!
//! # Empty is not drawn
//!
//! A search with nothing to show draws no overlay at all. An empty, in-flight or
//! failed search is spoken for by [`UserSearchState::label`] on the status line,
//! and an empty box over the chat list would tell the reader less than the
//! sentence already does — and would cover the list they are about to go back
//! to. The overlay is for choices, and a search with no candidates has none.
//!
//! # It is not drawn while the prompt has the focus
//!
//! The prompt is a line and a submission: the query goes to the server on `⏎`
//! and the candidates are the network's answer, not a filter the prompt keeps, so
//! there is no list to draw while the line has the focus. The design model states
//! the same rule — "there is no live narrowing: the query is sent on `⏎` and the
//! candidates arrive afterwards" — and re-opening `/` hides the previous answer
//! for the same reason: a list under a query it was not asked for would say the
//! screen answers a question it is not being asked.
//!
//! [`UserSearchState::label`]: domain::user::UserSearchState::label

use std::ops::Range;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};

use crate::app::{App, Focus};
use crate::wrap::columns;
use domain::user::UserCandidate;

/// Where the overlay draws, or `None` when there is nothing to draw into.
///
/// Every field saturates: `panic = "abort"` turns a `u16` underflow into a wrap
/// in release and a panic in debug, and a short terminal must produce neither.
/// No border plus at least one candidate is below the floor — a one-row overlay
/// would be all border — so that case draws nothing rather than a frame with no
/// room in it.
fn overlay_area(app: &App, chat_list: Rect) -> Option<Rect> {
    let search = app.user_search();
    if app.ui.focus == Focus::Input || !search.is_active() || search.is_empty() {
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

    // The rows' inner width is the overlay's width less the two border columns,
    // which is what the right-aligned standing is laid out against.
    let width = usize::from(area.width).saturating_sub(2);
    let query = app.user_search().query();
    let selected = app.user_search().selected();

    let items: Vec<ListItem> = app
        .user_search()
        .candidates()
        .iter()
        .take(usize::from(area.height).saturating_sub(2))
        .enumerate()
        .map(|(index, candidate)| candidate_row(app, candidate, query, width, index == selected))
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(app.ui.theme.border)
                .title(" New chat "),
        )
        .highlight_style(app.ui.theme.selection);

    // The real index, and `List` scrolls it into view itself — the same as the
    // emoji popup, and the reason this widget has no offset arithmetic.
    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}

/// One candidate's row: the name and handle at the left, the standing at the
/// right, and the fragment the reader typed inked `match`.
///
/// The standing's columns are reserved before the label is laid out, and the
/// label is truncated to what is left — the design model does the same, so the
/// standing is always shown: a name that lost cells to a standing would hide the
/// thing the reader is choosing between. The handle gives way before the name,
/// and the name's tail last.
fn candidate_row<'a>(
    app: &'a App,
    candidate: &'a UserCandidate,
    query: Option<&str>,
    width: usize,
    selected: bool,
) -> ListItem<'a> {
    // A `@` query is a username search, so the fragment belongs in the handle;
    // anything else is a name search, and the fragment belongs in the name.
    let by_username = query.is_some_and(|q| q.starts_with('@'));
    let fragment = query
        .map(|q| q.trim_start_matches('@'))
        .filter(|f| !f.is_empty());

    // The ink the row's quieter parts take. On the cursor row it is the body ink,
    // because reverse video turns a dim foreground into a dim background.
    let quiet = if selected {
        app.ui.theme.text
    } else {
        app.ui.theme.text_dim
    };
    let base = app.ui.theme.text;
    let mark = app.ui.theme.match_hit;

    let standing = if app.chats().iter().any(|chat| chat.id == candidate.user_id) {
        "chat"
    } else {
        "new"
    };

    // One space between the label and the standing, then the standing itself.
    let budget = width.saturating_sub(columns(standing) + 1);
    let mut used = 0;

    let mut spans = Vec::new();
    used += push_marked(
        &mut spans,
        &candidate.display_name,
        if by_username { None } else { fragment },
        base,
        mark,
        budget.saturating_sub(used),
    );

    if let Some(username) = candidate.username.as_deref() {
        // The leading space and the `@` are chrome and never the match; only the
        // handle's own text can carry the fragment.
        let handle_fragment = if by_username { fragment } else { None };
        for (text, fragment) in [(" ", None), ("@", None), (username, handle_fragment)] {
            used += push_marked(
                &mut spans,
                text,
                fragment,
                quiet,
                mark,
                budget.saturating_sub(used),
            );
        }
    }

    // Fill the rest of the label's budget so the standing lands on the right
    // edge, and keep at least one space in front of it.
    let pad = budget.saturating_sub(used);
    spans.push(Span::raw(" ".repeat(pad + 1)));
    spans.push(Span::styled(standing, quiet));

    ListItem::new(Line::from(spans))
}

/// Append `text` to `spans`, with `fragment` inked `mark`, taking at most
/// `budget` columns; return how many columns were written.
///
/// The text is cut at a character boundary — never inside a cluster a terminal
/// would draw as one cell — so a name too long for the column loses whole
/// characters rather than half of one.
fn push_marked<'a>(
    spans: &mut Vec<Span<'a>>,
    text: &'a str,
    fragment: Option<&str>,
    base: Style,
    mark: Style,
    budget: usize,
) -> usize {
    if budget == 0 || text.is_empty() {
        return 0;
    }

    let cut = fit_columns(text, budget);
    let slice = &text[..cut];
    spans.extend(marked(slice, fragment, base, mark));
    columns(slice)
}

/// The byte index of the longest prefix of `text` that fits `max` columns.
fn fit_columns(text: &str, max: usize) -> usize {
    let mut used = 0;
    for (at, character) in text.char_indices() {
        let cell = columns(&text[at..at + character.len_utf8()]);
        if used + cell > max {
            return at;
        }
        used += cell;
    }
    text.len()
}

/// `text` split into spans, with every case-insensitive occurrence of `fragment`
/// inked `mark` and the rest in `base`.
///
/// An absent or empty fragment is the whole string in `base`, which is the
/// common case: most rows carry no match at all.
fn marked<'a>(text: &'a str, fragment: Option<&str>, base: Style, mark: Style) -> Vec<Span<'a>> {
    let Some(fragment) = fragment.filter(|f| !f.is_empty()) else {
        return vec![Span::styled(text, base)];
    };

    let ranges = match_ranges(text, fragment);
    if ranges.is_empty() {
        return vec![Span::styled(text, base)];
    }

    let mut spans = Vec::with_capacity(ranges.len() * 2 + 1);
    let mut at = 0;
    for range in ranges {
        if range.start > at {
            spans.push(Span::styled(&text[at..range.start], base));
        }
        spans.push(Span::styled(&text[range.start..range.end], mark));
        at = range.end;
    }
    if at < text.len() {
        spans.push(Span::styled(&text[at..], base));
    }
    spans
}

/// The byte ranges in `text` where `fragment` occurs, case-insensitively.
///
/// Matched character by character rather than on a lowercased copy, because a
/// lowercased string is not the same string: `İ` lowercases to two code points,
/// and a byte offset into the copy would then name a cell in a row that is not
/// the one drawn. Names and handles carry non-ASCII, so the fold is per
/// character and the offsets stay the original string's.
fn match_ranges(text: &str, fragment: &str) -> Vec<Range<usize>> {
    let needle: Vec<char> = fragment.chars().collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let haystack: Vec<(usize, char)> = text.char_indices().collect();

    let mut ranges = Vec::new();
    let mut at = 0;
    while at + needle.len() <= haystack.len() {
        let hit = needle
            .iter()
            .zip(&haystack[at..])
            .all(|(want, (_, got))| same_letter(*want, *got));
        if hit {
            let start = haystack[at].0;
            let last = haystack[at + needle.len() - 1];
            ranges.push(start..last.0 + last.1.len_utf8());
            at += needle.len();
        } else {
            at += 1;
        }
    }
    ranges
}

/// Whether two characters are the same letter, ignoring case.
fn same_letter(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::style::Modifier;

    use crate::theme::Theme;
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

    /// Everything one row says, as a string.
    fn row_text(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
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

    /// The standing says whether choosing a person opens their chat or makes one,
    /// read off the chat list behind the overlay.
    #[test]
    fn a_row_stands_chat_or_new_for_whether_a_conversation_exists() {
        let existing = App::mock().chats()[0].id;
        let app = showing(vec![
            candidate(existing, "Existing Person", Some("ep")),
            candidate(9001, "Fresh Person", Some("fp")),
        ]);

        let buffer = screen(&app, 80, 24);
        let known = row_with(&buffer, "Existing Person").expect("the first row is drawn");
        let fresh = row_with(&buffer, "Fresh Person").expect("the second row is drawn");

        assert!(
            row_text(&buffer, known).contains("chat│"),
            "a person with a chat stands `chat` at the row's right edge: {:?}",
            row_text(&buffer, known)
        );
        assert!(
            row_text(&buffer, fresh).contains("new│"),
            "a person without one stands `new` there: {:?}",
            row_text(&buffer, fresh)
        );
    }

    /// The fragment the reader typed is inked `match`, the same role a search hit
    /// takes in a message.
    #[test]
    fn the_typed_fragment_is_inked_match_inside_the_name() {
        let app = showing(vec![candidate(7, "Noor Haddad", Some("noorh"))]);
        let match_fg = Theme::default()
            .match_hit
            .fg
            .expect("a match is a foreground");

        let buffer = screen(&app, 80, 24);
        let y = row_with(&buffer, "Noor Haddad").expect("the row is drawn");
        let x = (0..buffer.area.width)
            .find(|&x| buffer[(x, y)].symbol() == "N")
            .expect("the name's first letter");

        assert_eq!(
            buffer[(x, y)].fg,
            match_fg,
            "`no` inside `Noor` is the match ink"
        );
        assert!(
            buffer[(x, y)].modifier.contains(Modifier::BOLD),
            "a match is bold as well as coloured"
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
        assert_eq!(app.ui.focus, Focus::Input, "the prompt is open again");

        let column = chat_list(&app, 80, 24);
        assert!(
            overlay_area(&app, column).is_none(),
            "the old list is put away while the new query is typed"
        );
        let shown = flat(&screen(&app, 80, 24));
        assert!(!shown.contains("Noor Haddad"), "{shown}");
    }
}
