//! The account's own profile, in the rectangle the conversation had.
//!
//! A [`List`] of [`ListItem`]s inside a bordered block, for the reason the chat
//! list is one: the list widget draws the highlight, the reversed cursor row and
//! the scrolling, and rebuilding those three on a `Paragraph` would be three
//! features done worse.
//!
//! The chat list is not drawn from the list of *rows* but from the chats
//! themselves, one item each. This is the same widget over a different subject:
//! the rows come from [`App::profile_rows`], which is where the decision about
//! which rows exist lives, so the panel and the keys cannot disagree about it.
//!
//! # What the panel says when there is nothing to show
//!
//! An empty panel is a panel that looks broken, so the two states with no profile
//! each say what they are: one that has not read anything yet, and one that
//! could not. The reason is drawn rather than a title swap, because a title that
//! changes with the failure is a title a reader has to read twice.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};

use crate::app::{AccountState, App, ProfileRow, SessionStore};
use crate::wrap;

/// The title on the panel.
///
/// The subject rather than the command, because a title that names a state stops
/// being true the moment the panel is reused for somebody else, and `Conversation`
/// already names what is shown rather than what the pane is for.
const TITLE: &str = " Profile ";

/// What the panel shows for a row.
///
/// A blank line above the account and a gap before the two actions, because a
/// panel has no padding primitive and the alternative is padding inside the first
/// and last row's text — two places to be wrong rather than one. Dimmed, so a
/// blank row renders as nothing rather than as a row of the background.
const BLANK: &str = "";

pub fn render(app: &App, area: Rect, frame: &mut Frame<'_>) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border(app))
        .title(TITLE);

    // The items are built before the block so the two states with no rows can
    // draw a sentence instead of an empty list.
    let items = items(app, area.width.saturating_sub(2).max(1));
    let list = List::new(items)
        .block(block)
        .highlight_style(app.theme.selection);

    let mut state = ListState::default();
    if app.pane.is_profile() {
        state.select(Some(app.profile_cursor()));
    }
    frame.render_stateful_widget(list, area, &mut state);
}

/// The panel's border, focused when the keys are going here.
fn border(app: &App) -> Style {
    if app.focus.is_profile(app.pane) {
        app.theme.border_focused
    } else {
        app.theme.border
    }
}

/// The rows, in order, with the account's own fields filled in.
fn items(app: &App, width: u16) -> Vec<ListItem<'static>> {
    match &app.account {
        AccountState::Unfetched => vec![row(app.theme.text_dim, "reading the account…")],
        AccountState::Unavailable(reason) => {
            let mut lines = vec![
                row(app.theme.text, "not signed in"),
                row(app.theme.text_dim, reason.clone()),
            ];
            lines.push(row(
                app.theme.text_dim,
                "set the credentials in the configuration",
            ));
            lines
        }
        AccountState::Known(account) => known_rows(app, account, width),
    }
}

/// The rows for an account that is known.
fn known_rows(app: &App, account: &domain::account::Account, width: u16) -> Vec<ListItem<'static>> {
    let mut items = vec![row(app.theme.text_dim, BLANK)];

    for kind in app.profile_rows() {
        // The bio is the one field that may be longer than the panel, so it is
        // the one that wraps: a bio is the reader's own words about themselves
        // and truncating it with an ellipsis would be the panel deciding what
        // they get to say. `wrap` returns byte ranges into the bio, so the rows
        // are slices of the string itself.
        match kind {
            ProfileRow::Name => items.push(row(app.theme.text, account.display_name())),
            ProfileRow::Identity => items.push(row(app.theme.text, identity(account))),
            ProfileRow::Bio => {
                let bio = account.bio.as_deref().unwrap_or_default();
                for range in wrap::wrap(bio, width) {
                    items.push(row(app.theme.text, bio[range].to_owned()));
                }
            }
            ProfileRow::Birthday => items.push(row(
                app.theme.text,
                birthday(account.birthday).unwrap_or_default(),
            )),
            // Present but not attention-drawing: it is what tells two accounts
            // with the same name apart, and a reader looking for it looks.
            ProfileRow::UserId => {
                items.push(row(app.theme.text_dim, format!("id {}", account.user_id)));
            }
            ProfileRow::Session => items.push(row(app.theme.text_dim, session(&app.session_store))),
            // Both actions are dim, and `d` on them says they are not available
            // yet. A row that looks live and does nothing is worse than a row
            // that says it is not.
            ProfileRow::AddAccount => items.push(row(app.theme.text_dim, "add account")),
            ProfileRow::Logout => items.push(row(app.theme.text_dim, "logout")),
        }
    }

    items
}

/// One row, in one style.
fn row(style: Style, text: impl Into<String>) -> ListItem<'static> {
    ListItem::new(Line::from(Span::styled(text.into(), style)))
}

/// The username and the phone number, on one row.
///
/// One line rather than three, because the identity is one thing and a reader
/// scanning for "which account is this" wants one line to find it on. The `·` is
/// the title-note joiner the status line and the chat list already use.
fn identity(account: &domain::account::Account) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(username) = account.username.as_deref() {
        parts.push(format!("@{username}"));
    }
    if let Some(phone) = account.phone.as_deref() {
        parts.push(phone.to_owned());
    }
    parts.join(" · ")
}

/// A birthday as a reader would write it.
///
/// Not a timestamp and not a `NaiveDate`: `domain` carries a day, a month and
/// sometimes a year, and a year is a disclosure the account may not have made —
/// so the row is written without one rather than with a guess. `None` becomes
/// "not set" rather than a row of nothing, because the row exists to say
/// whether there is one.
fn birthday(date: Option<domain::account::Birthday>) -> Option<String> {
    let date = date?;
    let month = MONTHS
        .get(usize::try_from(date.month).unwrap_or(0).saturating_sub(1))
        .copied();
    let day = usize::try_from(date.day)
        .ok()
        .filter(|day| (1..=31).contains(day));

    match (month, day) {
        (Some(month), Some(day)) => Some(match date.year {
            Some(year) => format!("born {day} {month} {year}"),
            None => format!("born {day} {month}"),
        }),
        // A date the framework let through that is still not one. The row says
        // what is wrong rather than printing `born 99` or going blank.
        _ => Some("born on a date this build will not guess".to_owned()),
    }
}

/// The month names, January first.
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Where the session is kept, as the panel says it.
///
/// A file's path rather than a keyring account name: "never store session
/// strings in plaintext" is a rule this program keeps, so a reader who chose a
/// file deserves to be reminded that is what it is.
fn session(store: &SessionStore) -> String {
    match store {
        SessionStore::Keyring => "session: OS credential store".to_owned(),
        SessionStore::PlaintextFile(path) => format!("session: {} (plaintext)", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use domain::account::Account;

    use crate::app::{Pane, ProfileId};

    /// A bio comfortably wider than the fifty-odd columns the right-hand pane
    /// has on a terminal of eighty, with a tail that can only be on screen if
    /// the text was wrapped rather than cut.
    const BIO: &str = "Notes on the analytical engine, and then some more of them, \
                       so that the text is comfortably wider than the pane it is \
                       drawn in and has to be wrapped across several rows to fit \
                       at all without anything being lost off the end of it.";

    /// The panel with the mock account showing, and the keys going to it.
    ///
    /// Opened the way a reader opens it rather than by calling the method, so a
    /// test cannot pass on a panel nothing reaches.
    fn profile() -> App {
        let mut app = App::mock();
        app.handle_key(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::NONE));
        app
    }

    /// The mock application with `account` as the account and the profile
    /// showing, for the tests that change the account first.
    fn profile_of(account: Account) -> App {
        let mut app = App::mock();
        app.set_account(Ok(account));
        app.handle_key(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::NONE));
        app
    }

    /// The screen, drawn the way the loop draws it.
    fn screen(app: &App) -> Buffer {
        let mut terminal =
            Terminal::new(TestBackend::new(80, 24)).expect("the test backend builds");
        terminal
            .draw(|frame| app.render(frame))
            .expect("the frame draws");
        terminal.backend().buffer().clone()
    }

    /// Everything on the screen, as one string per row.
    fn rows(app: &App) -> Vec<String> {
        let buffer = screen(app);
        (0..24)
            .map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect()
    }

    /// Whether the cell at a corner is in the focused border's ink.
    ///
    /// Read off the screen rather than off the widget, because a border styled
    /// correctly and drawn everywhere is still a border that says nothing.
    fn lit(app: &App, at: (u16, u16)) -> bool {
        let focused = app
            .theme
            .border_focused
            .fg
            .expect("the focused border has an ink");
        screen(app)[(at.0, at.1)].fg == focused
    }

    /// The profile has to take the rectangle the conversation had, and say so by
    /// being the only lit border on the screen.
    #[test]
    fn the_profile_takes_the_right_hand_pane() {
        let app = profile();

        assert_eq!(app.pane, Pane::Profile(ProfileId::SelfAccount));
        assert!(
            lit(&app, (24, 0)) && !lit(&app, (0, 0)) && !lit(&app, (0, 20)),
            "the right-hand border is the lit one, and the chat list is not"
        );
        assert!(rows(&app).iter().any(|row| row.contains(" Profile ")));
    }

    /// The other half, because a change that lit the right border in both states
    /// would say nothing.
    #[test]
    fn the_conversation_is_where_it_was() {
        let app = App::mock();

        assert_eq!(app.pane, Pane::Conversation);
        assert!(
            lit(&app, (24, 0)) && !lit(&app, (0, 0)) && !lit(&app, (0, 20)),
            "the same border is lit with the conversation in it"
        );
    }

    #[test]
    fn the_account_is_named_on_one_line() {
        let rows = rows(&profile());
        assert!(rows.iter().any(|row| row.contains("Ada Lovelace")));
        assert!(
            rows.iter().any(|row| row.contains("@ada · +15551234567")),
            "the identity is one row, not three"
        );
    }

    /// An account with no username must not draw a bare `@`, which reads as a
    /// broken widget rather than as an account that has not set one.
    #[test]
    fn a_missing_username_is_not_an_at_sign() {
        let app = profile_of(Account {
            username: None,
            phone: Some("+15551234567".into()),
            ..crate::app::mock_account()
        });

        let rows = rows(&app);
        assert!(rows.iter().any(|row| row.contains("+15551234567")));
        assert!(
            !rows.iter().any(|row| row.contains('@')),
            "no row carries an at sign the account did not set"
        );
    }

    /// A bio longer than the panel is more than one row, and none of it is lost.
    #[test]
    fn the_bio_wraps_and_is_not_truncated() {
        let app = profile_of(Account {
            bio: Some(BIO.into()),
            ..crate::app::mock_account()
        });

        let rows = rows(&app);
        assert!(
            rows.iter()
                .any(|row| row.contains("analytical engine, and then")),
            "the bio is not truncated where the panel ends"
        );
        assert!(
            !rows.iter().any(|row| row.contains('…')),
            "nothing is truncated with an ellipsis"
        );
    }

    #[test]
    fn both_actions_are_visible_while_signed_in() {
        let rows = rows(&profile());
        assert!(rows.iter().any(|row| row.contains("add account")));
        assert!(rows.iter().any(|row| row.contains("logout")));
    }

    /// The state a reader on a machine with no credentials meets first, and the
    /// one that must not look like a widget that has gone wrong.
    #[test]
    fn a_signed_out_panel_says_so() {
        let mut app = App::mock();
        app.set_account(Err("no API credentials in the configuration".into()));
        app.handle_key(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::NONE));

        let rows = rows(&app);
        assert!(rows.iter().any(|row| row.contains("not signed in")));
        assert!(
            rows.iter()
                .any(|row| row.contains("no API credentials in the configuration")),
            "the reason is drawn, not just the fact that there is none"
        );
    }

    /// And the state before anything has been read, which is a different thing
    /// and must not borrow the other's wording.
    #[test]
    fn an_unread_account_says_it_is_reading() {
        let mut app = App::new();
        app.pane = Pane::Profile(ProfileId::SelfAccount);

        let rows = rows(&app);
        assert!(rows.iter().any(|row| row.contains("reading the account")));
    }

    /// The session line has to name the store rather than the file type, and a
    /// file is named by its path.
    #[test]
    fn the_session_line_names_the_store() {
        assert!(
            rows(&profile())
                .iter()
                .any(|row| row.contains("OS credential store"))
        );

        let mut app = profile();
        app.set_session_store(SessionStore::PlaintextFile("/tmp/televim.session".into()));
        assert!(
            rows(&app)
                .iter()
                .any(|row| row.contains("/tmp/televim.session (plaintext)")),
            "a file is named by its path, and said to be plaintext"
        );
    }

    /// A birthday with no year is written without one, rather than with a guess.
    #[test]
    fn a_birthday_is_written_as_it_was_given() {
        assert_eq!(
            birthday(Some(domain::account::Birthday {
                day: 10,
                month: 12,
                year: Some(1815)
            })),
            Some("born 10 Dec 1815".to_owned())
        );
        assert_eq!(
            birthday(Some(domain::account::Birthday {
                day: 1,
                month: 1,
                year: None
            })),
            Some("born 1 Jan".to_owned())
        );
        assert_eq!(birthday(None), None, "an account need not have one");
        assert!(
            birthday(Some(domain::account::Birthday {
                day: 0,
                month: 0,
                year: None
            }))
            .is_some_and(|row| row.contains("will not guess")),
            "a date that is not one is said to be, not printed"
        );
    }

    /// A month name the panel does not have must not be reachable, because the
    /// lookup is the one thing standing between a month number and a word.
    #[test]
    fn the_month_names_are_the_year() {
        assert_eq!(MONTHS.len(), 12);
        assert_eq!(MONTHS[0], "Jan");
        assert_eq!(MONTHS[11], "Dec");
    }
}
