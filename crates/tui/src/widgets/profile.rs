//! A profile card in the rectangle the conversation had.
//!
//! One widget over two subjects — the signed-in account, and somebody the reader
//! is talking to — and one interaction model shared with the conversation, because
//! a row here is the same kind of object a message is. What lives in [`crate::card`]
//! is the row model and the drawing; this file is the panel.
//!
//! # Why it is a `Paragraph` and not a `List`
//!
//! The chat list and this panel were both `List`s, and a `List` cannot draw a
//! caret *inside* one of its items. A value wraps, and a wrapped value is one row
//! the reader moves over with one `j` whose highlight covers all its lines — which
//! a `List` can only do by making each drawn line its own item, and then `j` moves
//! a third of the way through a field. So the reversed cursor row is patched onto
//! the spans here, by hand, in the order the theme's module doc states.
//!
//! # What the panel says when there is nothing to show
//!
//! An empty panel is a panel that looks broken, so the two states with no profile
//! each say what they are: one that has not read anything yet, and one that could
//! not. The reason is drawn rather than swapped into the title, because a title
//! that changes with the failure is a title a reader has to read twice.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{AccountState, App};
use crate::card;
use crate::wrap;

/// Where to put the credentials back, as the signed-out card says it.
///
/// A command rather than a place: the card has no idea whether the reader's
/// configuration file is somewhere they can reach, and `:signin` is a key this
/// program answers wherever it is typed.
const SET_CREDENTIALS: &str = "Set the credentials again with :signin.";

pub fn render(app: &App, area: Rect, frame: &mut Frame<'_>) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border(app))
        .title(card::title(app));

    // The rows are built before the block so the two states with no rows can draw
    // a sentence instead of an empty card. They are borrowed for the frame rather
    // than copied into owned items, which is the same reason the card is a
    // `Paragraph`: a value is a slice of a string this program already holds.
    let rows = card::rows(app);
    let lines: Vec<Line<'_>> = if rows.is_empty() {
        shell(app, area.width.saturating_sub(2).max(1))
    } else {
        card::lines(app, &rows, area.width.saturating_sub(2).max(1))
            .into_iter()
            .map(|(_, line)| line)
            .collect()
    };

    // No `Wrap` here, and that is load-bearing rather than an omission. The card
    // wraps its own values with `wrap::wrap` and hands over finished lines, because
    // the rows a value occupies are the geometry `j`, the highlight and the caret
    // all count — and a `Paragraph` that wrapped as well would cut a second set of
    // lines out of lines that are already cut, at whatever width it liked. The
    // double wrap is visible in a birthday: `born 10 Dec` and `1815` are one row
    // and were being drawn as two.
    let paragraph = Paragraph::new(lines).block(block);

    frame.render_widget(paragraph, area);
}

/// The panel's border, focused when the keys are going here.
fn border(app: &App) -> Style {
    if app.focus.is_profile(app.pane) {
        app.theme.border_focused
    } else {
        app.theme.border
    }
}

/// The lines for a card with no rows: the two states in which that happens.
///
/// **A contact's two states are the account's with their own words.** The
/// account's first line is `not signed in` because its problem is credentials;
/// putting that on a failed profile read would send a reader to check something
/// that is not the problem. Neither borrows the other's wording, and the
/// difference is the only thing that tells a reader which of the two they are
/// looking at.
fn shell(app: &App, width: u16) -> Vec<Line<'_>> {
    if let Some(contact) = app.contact() {
        return match &contact.state {
            AccountState::Unfetched => vec![Line::from(Span::styled(
                "reading their profile…",
                app.theme.text_dim,
            ))],
            // The reason for the same reason the account's is, and more so here: a
            // clipped reason is a reason the reader cannot act on, and the whole
            // point of drawing it rather than an empty card is that they can.
            AccountState::Unavailable(reason) => {
                let mut lines = vec![Line::from(Span::styled(
                    "could not read this profile",
                    app.theme.text,
                ))];
                lines.extend(wrapped(&app.theme.text_dim, reason, width));
                lines
            }
            // Known, so `card::rows` had rows and this was never called.
            AccountState::Known(_) => Vec::new(),
        };
    }

    match &app.account {
        AccountState::Unfetched => vec![Line::from(Span::styled(
            "reading the account…",
            app.theme.text_dim,
        ))],
        // The reason wraps for the same reason a bio does, and it matters more
        // that it does: a reason clipped at the panel's edge is a reason the
        // reader cannot act on, and the whole point of drawing it rather than an
        // empty profile is that they can. A bring-up failure carries a chain, and
        // the chain is where the answer usually is.
        //
        // **An empty reason draws no line**, which is the one answer that is not
        // a failure: a machine with no session sends `""`, and the signed-out
        // card is two lines — that there is no session, and where to put the
        // credentials back. A blank row between them is a row saying nothing.
        AccountState::Unavailable(reason) => {
            let mut lines = vec![Line::from(Span::styled("not signed in", app.theme.text))];
            if !reason.is_empty() {
                lines.extend(wrapped(&app.theme.text_dim, reason, width));
            }
            lines.extend(wrapped(&app.theme.text_dim, SET_CREDENTIALS, width));
            lines
        }
        AccountState::Known(_) => Vec::new(),
    }
}

/// Wrapped text, as byte ranges sliced back out of the string itself.
///
/// The same `wrap` the card's values use, so a reason wraps by the same rules a
/// bio does and the two cannot drift apart.
fn wrapped<'a>(style: &Style, text: &'a str, width: u16) -> Vec<Line<'a>> {
    wrap::wrap(text, width)
        .into_iter()
        .map(|range| Line::from(Span::styled(&text[range], *style)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The card for an account in one state, at the panel's width.
    fn card(account: Result<domain::account::Account, String>) -> Vec<String> {
        let mut app = App::mock();
        // The account's own card, which is the only card that draws these lines.
        app.set_account(account);

        shell(&app, 50)
            .into_iter()
            .map(|line| line.to_string())
            .collect()
    }

    /// A machine with no session has no reason to give, and its card says so in
    /// two lines rather than three.
    ///
    /// `wrap` gives an empty string a row of its own, and the empty `Err` a
    /// signed-out launch sends would otherwise be drawn as a blank line between
    /// the only two sentences that mean anything.
    #[test]
    fn a_signed_out_session_with_no_reason_draws_no_blank_line() {
        let rows = card(Err(String::new()));

        assert_eq!(rows.len(), 2, "no blank row between them: {rows:#?}");
        assert!(rows[0].contains("not signed in"), "{}", rows[0]);
        assert!(rows[1].contains(SET_CREDENTIALS), "{}", rows[1]);
    }

    /// A reason is a failure's own words, and there is no reason to refuse one:
    /// dropping it would send a reader to look for something that is not wrong.
    #[test]
    fn a_failed_profile_read_keeps_its_reason() {
        let rows = card(Err("the network is unreachable".to_owned()));

        assert_eq!(rows.len(), 3, "the reason is between the two: {rows:#?}");
        assert!(
            rows[1].contains("the network is unreachable"),
            "{}",
            rows[1]
        );
    }
}
