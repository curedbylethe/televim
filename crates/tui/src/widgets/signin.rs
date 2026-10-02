//! The sign-in surface, drawn in the rectangle the conversation had.
//!
//! The one view that is not a chat. A box, two rows, and a third only when
//! Telegram asks for it — which is the whole rule: **a no-2FA account has two
//! rows for the whole flow**, and a password row drawn speculatively would be a
//! field the reader is not being asked for.
//!
//! It is drawn over whichever pane was there, which is why it is an overlay on
//! [`crate::app::App`] and not a [`crate::app::Pane`]: the form outlives the card
//! that names it, and a reader who pressed `:signin` from a signed-out card has
//! to still be typing the code after the card is gone.
//!
//! The rows are the design's, verbatim, and the geometry is the engine's: the
//! label at the panel's padding, the value fourteen columns along, and the
//! marker right-aligned against the box's inner edge. Nothing is cached and
//! nothing is measured twice — the panel's width is the only measurement here.
//!
//! No `Wrap` on the `Paragraph`, for the same reason the card has none: this
//! panel cuts its own lines with [`crate::wrap`] and truncates what it cannot
//! wrap, because a marker right-aligned against the edge is geometry rather than
//! text, and a renderer that wrapped as well would put it somewhere else.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{App, LoginField, SignIn, SignInFlow};
use crate::wrap;

/// The first line of the view, above every field.
const TITLE: &str = "Sign in to Telegram";

/// Why there are rows at all, under the first line.
const EXPLAIN: &str = "televim signs in to one account. Telegram asks for what it needs, in order.";

/// The sentence a machine with no `api_id` and no `api_hash` gets.
///
/// Not a form: there is nothing to type, and a phone row here would ask the
/// reader for something the program still could not do with.
pub const NO_CREDENTIALS: &str = "televim has no application credentials. It needs an api_id and an api_hash in its config file before it can sign in to anything.";

/// The offer on the phone row when the stored session is one Telegram forgot.
///
/// The one row that offers a way back rather than describing a step, because
/// there is no step to go back to: the session is gone, and the phone is the
/// whole way in again.
pub const AGAIN_ROW: &str = "[ ⏎: sign in again ]";

/// What a row the reader is past says.
const OK: &str = "[ok]";

/// What the code row says after Telegram has refused one.
const WRONG: &str = "[wrong code]";

/// What stands in for the value of the row a request is in flight for.
const IN_FLIGHT: &str = "· · ·";

/// What the notes say while a request is on its way.
pub const CHECKING: &str = "Checking…";

/// How far along its row a value sits from the label.
const VALUE_OFFSET: usize = 14;

/// Which of the three rows the flow is at.
///
/// An index rather than a `match` at every call site: the rows are the phone,
/// the code and the password in that order, and every question this panel asks
/// of the step — which label is the reader's ink, which row a marker belongs to,
/// whether the password row exists — is an index into them.
fn row_of(field: Option<LoginField>) -> usize {
    match field {
        Some(LoginField::Phone) => 0,
        Some(LoginField::Code) => 1,
        Some(LoginField::Password) | None => 2,
    }
}

/// The phone Telegram accepted, once it has said.
///
/// The pre-fill cannot answer this: the number the reader typed is not
/// necessarily the number Telegram accepted, and this is the accepted one — it
/// is what the note naming where the code went has to be right about.
fn accepted_phone(app: &App) -> Option<&str> {
    use domain::session::SessionState as State;
    match &app.signin().and_then(SignIn::flow)?.login.step {
        State::AwaitingCode { phone } | State::AwaitingPassword { phone } => Some(phone),
        State::LoggedOut | State::LoggedIn { .. } => None,
    }
}

/// One row of the form: a label, and whatever the value column holds.
struct Field<'a> {
    label: String,
    value: Option<&'a str>,
    marker: Option<&'a str>,
}

/// The three rows, and the fourth's absence.
///
/// The phone and the code are there from the start; the password row exists only
/// when Telegram answered `SESSION_PASSWORD_NEEDED`, which is the account's own
/// doing rather than a step every flow passes through.
///
/// A value is drawn beside a row only once the reader is past it: the one in
/// front of them is in the bar, which is where they are typing it. So a row with
/// nothing beside it is a row being asked for, and its dim label is saying so.
fn rows<'a>(app: &'a App, flow: &'a SignInFlow, at: usize) -> Vec<Field<'a>> {
    let mut out = vec![
        Field {
            label: "Phone".to_owned(),
            value: accepted_phone(app).or_else(|| flow.stale.then_some(&app.phone)),
            marker: match (flow.stale, at) {
                (true, 0) => Some(AGAIN_ROW),
                (_, 1 | 2) => Some(OK),
                _ => None,
            },
        },
        Field {
            label: "Login code".to_owned(),
            // The refused code is not kept: it is the one value in this program
            // nobody can use, because Telegram will not take it twice.
            value: None,
            marker: match (at, flow.login.refusal.is_some()) {
                (1, true) => Some(WRONG),
                (2, _) => Some(OK),
                _ => None,
            },
        },
    ];

    if at == 2 {
        out.push(Field {
            label: password_label(flow.used),
            value: None,
            marker: None,
        });
    }

    out
}

/// The password row's label, counting down from the same three every refusal
/// spends.
///
/// [`domain::session::attempts_left`] rather than a count of this panel's own,
/// so the row and the refusal written with the same number cannot disagree.
fn password_label(used: u8) -> String {
    let left = domain::session::attempts_left(used);
    format!(
        "two-factor password ({} attempt{} left)",
        left,
        if left == 1 { "" } else { "s" }
    )
}

pub fn render(app: &App, signin: &SignIn, area: Rect, frame: &mut Frame<'_>) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border(app, signin))
        .title(" Sign in ");

    // One column of padding inside the border, so a label is not against the
    // frame the way a card's cue is beside it. The padding is applied to the
    // finished lines rather than to every measurement, because a row's geometry
    // — the value's column and the marker's edge — is the design's and is not the
    // panel's to re-derive.
    let width = area.width.saturating_sub(3).max(1);

    let mut lines: Vec<Line<'_>> = vec![Line::from(Span::styled(TITLE, app.theme.text))];

    let flow = match signin {
        SignIn::NoCredentials => {
            lines.push(Line::from(""));
            lines.extend(wrapped(&app.theme.text_dim, NO_CREDENTIALS, width));
            pad_all(&mut lines);
            frame.render_widget(Paragraph::new(lines).block(block), area);
            return;
        }
        SignIn::Flow(flow) => flow,
    };

    lines.extend(wrapped(&app.theme.text_dim, EXPLAIN, width));
    lines.push(Line::from(""));

    let at = row_of(app.signin_field());
    for (index, row) in rows(app, flow, at).iter().enumerate() {
        lines.push(field_row(app, flow, row, index == at, width));
    }

    lines.extend(notes(app, flow, at, width));
    pad_all(&mut lines);

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// The padding column, in front of every line.
///
/// A `Paragraph` cannot be given one left margin, so it is put on the lines:
/// the alternative is every measurement in here having to remember it, and a row
/// whose marker forgot the margin is a marker in the wrong place.
fn pad_all(lines: &mut [Line<'_>]) {
    for line in lines {
        line.spans.insert(0, Span::raw(" "));
    }
}

/// One row, laid out by columns rather than by spans.
///
/// Three columns and two of them are fixed by the design — the label at the
/// padding, the value fourteen along, the marker against the inner edge — so
/// the row is built as a line of known width and each part is padded into its
/// place. That is what keeps the marker against the edge: it is measured from
/// the edge, not from whatever text happens to precede it.
fn field_row<'a>(
    app: &App,
    flow: &SignInFlow,
    row: &Field<'a>,
    asked: bool,
    width: u16,
) -> Line<'a> {
    let width = usize::from(width).max(1);
    let mut columns = 0;
    let mut spans: Vec<Span<'a>> = Vec::new();

    // The row being asked for is the reader's ink and the others are dim, which
    // is how the reader knows which field the keys are for.
    let label_style = if asked && !flow.waiting {
        app.theme.text
    } else {
        app.theme.text_dim
    };
    let label = clip(&row.label, width);
    spans.push(Span::styled(label.clone(), label_style));
    columns += label.chars().count();

    // A request in flight takes the value's place rather than the value: the
    // reader has to see that something is happening, and what they sent is
    // already in the bar.
    let waiting = flow.waiting && asked;
    if let Some(value) = row.value.filter(|_| !waiting) {
        columns = pad(&mut spans, columns, VALUE_OFFSET);
        let value = clip(value, width.saturating_sub(VALUE_OFFSET));
        columns += value.chars().count();
        spans.push(Span::styled(value, app.theme.text));
    } else if waiting {
        columns = pad(&mut spans, columns, VALUE_OFFSET);
        columns += IN_FLIGHT.chars().count();
        spans.push(Span::styled(IN_FLIGHT, app.theme.text_dim));
    }

    if let Some(marker) = row.marker {
        let marker = clip(marker, width);
        if columns + marker.chars().count() < width {
            pad(&mut spans, columns, width - marker.chars().count());
        }
        spans.push(Span::styled(marker, app.theme.text));
    }

    Line::from(spans)
}

/// Pads the row with spaces until it is `at` columns wide.
fn pad(spans: &mut Vec<Span<'_>>, from: usize, at: usize) -> usize {
    let spaces = at.saturating_sub(from);
    if spaces > 0 {
        spans.push(Span::raw(" ".repeat(spaces)));
    }
    at.max(from)
}

/// What Telegram sends beside a two-step password, if it sends anything.
///
/// An `Option` and not a string because **most accounts have no hint**: the row
/// is absent rather than a `Password hint:` with nothing after it, which would be
/// a question this panel cannot answer and the reader cannot either. That is the
/// card's row rule applied to a row of the sign-in view.
fn password_hint(flow: &SignInFlow) -> Option<String> {
    flow.hint
        .as_ref()
        .map(|hint| format!("Password hint: {hint}"))
}

/// The rows under the form, saying what the step means.
///
/// A refusal is *not* one of them. A refusal is state the reader must not lose,
/// and the status line is where such state lives — a sentence drawn inside the
/// panel is a sentence that scrolls out of view.
fn notes<'a>(app: &'a App, flow: &'a SignInFlow, at: usize, width: u16) -> Vec<Line<'a>> {
    let dim = app.theme.text_dim;
    let mut out = vec![Line::from("")];

    if flow.waiting {
        out.push(Line::from(Span::styled(CHECKING, app.theme.text)));
        out.push(Line::from(Span::styled(
            "The request is in flight; ⏎ is refused.",
            dim,
        )));
        return out;
    }

    let said: String = match at {
        0 if app.phone.is_empty() => "Include the country code.".to_owned(),
        0 => "Include the country code.\nThe configured number is filled in.".to_owned(),
        1 => match accepted_phone(app) {
            Some(phone) => format!("Telegram sent a login code to {phone}."),
            None => String::new(),
        },
        _ => "Two-step verification is on.".to_owned(),
    };

    for line in said.lines() {
        out.push(Line::from(Span::styled(
            clip(line, usize::from(width)),
            dim,
        )));
    }

    // The hint is not dim, because it is not the program's own commentary: it is
    // the one thing on this row the reader is here to read.
    if let Some(hint) = password_hint(flow) {
        out.push(Line::from(Span::styled(
            clip(&hint, usize::from(width)),
            app.theme.text,
        )));
    }

    out
}

/// The box's border.
///
/// Only the no-credentials sentence takes it, because only that state has no
/// field: everywhere else the keys are at the line, and the bar's border is what
/// says so. Two lit borders would be two places claiming the same key.
fn border(app: &App, signin: &SignIn) -> Style {
    if matches!(signin, SignIn::NoCredentials) {
        app.theme.border_focused
    } else {
        app.theme.border
    }
}

/// Wrapped text, as byte ranges sliced back out of the string itself.
///
/// The same `wrap` the card's values use, so a sentence wraps by the same rules a
/// bio does.
fn wrapped<'a>(style: &Style, text: &'a str, width: u16) -> Vec<Line<'a>> {
    wrap::wrap(text, width)
        .into_iter()
        .map(|range| Line::from(Span::styled(&text[range], *style)))
        .collect()
}

/// `text` cut to `width` columns, on a character boundary.
fn clip(text: &str, width: usize) -> String {
    text.chars().take(width).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Action, Focus, PromptKind};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use domain::session::SessionState;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    /// The whole frame, drawn into an in-memory terminal at the design's size.
    fn screen(app: &App) -> Buffer {
        let mut terminal =
            Terminal::new(TestBackend::new(80, 24)).expect("the test backend builds");
        terminal
            .draw(|frame| app.render(frame))
            .expect("the frame draws");
        terminal.backend().buffer().clone()
    }

    fn rows_of(buffer: &Buffer) -> Vec<String> {
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    fn flat(rows: &[String]) -> String {
        rows.join("\n")
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn type_text(app: &mut App, text: &str) {
        for character in text.chars() {
            press(app, KeyCode::Char(character));
        }
    }

    /// The row carrying `needle`, or a panic saying what was on the screen.
    fn row_with(rows: &[String], needle: &str) -> String {
        rows.iter()
            .find(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("no row says {needle:?} in {rows:#?}"))
            .clone()
    }

    /// The right-hand column's rows, with the chat list and its border off the
    /// left, so an assertion is about this panel and not about the screen.
    fn column(buffer: &Buffer) -> Vec<String> {
        rows_of(buffer)
            .into_iter()
            .map(|row| row.chars().skip(24).collect::<String>())
            .collect()
    }

    /// A shell card for a machine with no session, which is what `:signin` is
    /// typed at.
    fn signed_out() -> App {
        let mut app = App::mock();
        app.set_account(Err("no session".to_owned()));
        // The shell is a card, and a card is opened rather than shown: `S` is the
        // account's own card from the conversation and from the chat list.
        press(&mut app, KeyCode::Char('S'));
        app
    }

    // ---- the scenes -----------------------------------------------------

    /// `profile` / `not signed in`: the shell card, and where to put the
    /// credentials back.
    #[test]
    fn signedout() {
        let rows = column(&screen(&signed_out()));

        assert!(row_with(&rows, "not signed in").contains("not signed in"));
        assert!(
            row_with(&rows, ":signin.").contains("Set the credentials again with :signin."),
            "the card says where to put them back: {rows:#?}"
        );
        assert!(
            rows_of(&screen(&signed_out()))
                .iter()
                .any(|row| row.contains(" ::signin  q:quit")),
            "and the hint names the command"
        );
    }

    /// The same card with `:signin` typed at it — the flow a launch with no
    /// session takes, through the one command that starts it.
    #[test]
    fn signedout_then_signin_reaches_the_form() {
        let mut app = signed_out();
        press(&mut app, KeyCode::Char(':'));
        type_text(&mut app, "signin");
        press(&mut app, KeyCode::Enter);

        assert_eq!(app.signin_field(), Some(LoginField::Phone));
        let rows = column(&screen(&app));
        assert!(row_with(&rows, "Sign in").contains("Sign in"), "{rows:#?}");
        assert!(flat(&rows).contains("Sign in to Telegram"));
    }

    /// `signin` / `Phone, pre-filled`: two rows, and the number in the bar.
    #[test]
    fn signin_phone() {
        let mut app = App::mock();
        app.begin_signin();

        let rows = column(&screen(&app));
        assert!(row_with(&rows, "│ Phone").contains("│ Phone"), "{rows:#?}");
        assert!(
            rows.iter().any(|row| row.contains("│ Login code")),
            "{rows:#?}"
        );
        // No password row: an account nobody has asked for a password on does
        // not get one drawn.
        assert!(
            !flat(&rows).contains("two-factor password"),
            "the password row exists only once Telegram asks: {rows:#?}"
        );
        assert!(flat(&rows).contains("Include the country code."));
        assert!(flat(&rows).contains("The configured number is filled in."));
        assert!(
            rows_of(&screen(&app))
                .iter()
                .any(|row| row.contains("+44·7700·900142")),
            "the number is in the bar, dotted: {rows:#?}"
        );
    }

    /// `signin` / `Checking…`: the request in flight, and `⏎` refused.
    #[test]
    fn signin_checking() {
        let mut app = App::mock();
        app.begin_signin();
        press(&mut app, KeyCode::Enter);

        let rows = column(&screen(&app));
        assert!(row_with(&rows, "│ Phone").contains(IN_FLIGHT), "{rows:#?}");
        assert!(flat(&rows).contains("The request is in flight; ⏎ is refused."));
        assert!(row_with(&rows, CHECKING).contains(CHECKING));
        assert!(
            rows_of(&screen(&app))
                .iter()
                .any(|row| row.contains("Checking… — the request is in flight")),
            "the status line says it too: {rows:#?}"
        );
    }

    /// `signin` / `Login code`: the phone accepted, the code being asked for.
    #[test]
    fn signin_login_code() {
        let mut app = App::mock();
        app.begin_signin();
        press(&mut app, KeyCode::Enter);
        app.take_action();
        app.login_advanced(
            SessionState::AwaitingCode {
                phone: "+44 7700 900142".to_owned(),
            },
            None,
        );

        let rows = column(&screen(&app));
        let phone = row_with(&rows, "│ Phone");
        assert!(phone.contains("+44 7700 900142"), "{phone}");
        assert!(phone.contains(OK), "{phone}");
        assert!(
            flat(&rows).contains("Telegram sent a login code to +44 7700 900142."),
            "{rows:#?}"
        );
        // The code row is being asked for, so it carries no value and no marker.
        let code = row_with(&rows, "│ Login code");
        assert!(!code.contains(OK), "{code}");
        assert!(
            rows_of(&screen(&app))
                .iter()
                .any(|row| row.contains("┌ Login code")),
            "the bar names the field: {rows:#?}"
        );
    }

    /// `signin` / `Wrong code`: a marker on the row and a sentence on the status
    /// line, and neither one replacing the other.
    #[test]
    fn signin_wrong_code() {
        let mut app = App::mock();
        app.begin_signin();
        press(&mut app, KeyCode::Enter);
        app.take_action();
        app.login_advanced(
            SessionState::AwaitingCode {
                phone: "+44 7700 900142".to_owned(),
            },
            None,
        );
        app.login_refused("that code is not the one Telegram sent".to_owned(), 0);

        let rows = column(&screen(&app));
        assert!(row_with(&rows, "│ Login code").contains(WRONG), "{rows:#?}");
        assert!(
            flat(&rows_of(&screen(&app))).contains("that code is not the one Telegram sent"),
            "the refusal is the status line's: {rows:#?}"
        );
        // One wrong code is not a wrong phone.
        assert!(row_with(&rows, "│ Phone").contains(OK));
    }

    /// `signin` / `Password`: the third row, counted off the same three every
    /// refusal spends.
    #[test]
    fn signin_password() {
        let mut app = App::mock();
        app.begin_signin();
        app.login_advanced(
            SessionState::AwaitingPassword {
                phone: "+44 7700 900142".to_owned(),
            },
            None,
        );

        let rows = column(&screen(&app));
        assert!(
            flat(&rows).contains("two-factor password (3 attempts left)"),
            "{rows:#?}"
        );
        assert!(flat(&rows).contains("Two-step verification is on."));

        // A refusal spends one attempt, and the row says so rather than the
        // sentence alone.
        app.login_refused("that password is not right (2 attempts left)".to_owned(), 1);
        assert!(
            flat(&column(&screen(&app))).contains("two-factor password (2 attempts left)"),
            "{rows:#?}"
        );
    }

    /// An account that has a two-step hint: the row carries it, because a
    /// password the reader cannot recall is a password they will get wrong.
    #[test]
    fn signin_password_with_a_hint() {
        let mut app = App::mock();
        app.begin_signin();
        app.login_advanced(
            SessionState::AwaitingPassword {
                phone: "+44 7700 900142".to_owned(),
            },
            Some("street I grew up on".to_owned()),
        );

        let rows = column(&screen(&app));
        let hint = row_with(&rows, "Password hint:");
        assert!(
            hint.contains("Password hint: street I grew up on"),
            "{hint}"
        );
        // Under the step's own line, and in the reader's ink rather than the
        // panel's commentary: it is the thing the reader is here to read.
        let said = flat(&rows_of(&screen(&app)));
        assert!(
            said.find("Two-step verification is on.") < said.find("Password hint:"),
            "the hint comes after what the step is: {said}"
        );
    }

    /// An account with no hint: **no row at all**, because a `Password hint:`
    /// with nothing after it is a question neither the reader nor this panel can
    /// answer. The card's row rule, applied to a row of the sign-in view.
    #[test]
    fn signin_password_without_a_hint_draws_no_row() {
        let mut app = App::mock();
        app.begin_signin();
        app.login_advanced(
            SessionState::AwaitingPassword {
                phone: "+44 7700 900142".to_owned(),
            },
            None,
        );

        let rows = column(&screen(&app));
        assert!(flat(&rows).contains("Two-step verification is on."));
        assert!(
            !flat(&rows_of(&screen(&app))).contains("Password hint"),
            "an absent row, not an empty one: {rows:#?}"
        );
    }

    /// A hint belongs to the password Telegram is asking about now: a step
    /// change drops it, so a stale hint cannot be drawn beside a step it is not
    /// about.
    #[test]
    fn a_hint_is_dropped_when_the_step_moves_on() {
        let mut app = App::mock();
        app.begin_signin();
        app.login_advanced(
            SessionState::AwaitingPassword {
                phone: "+44 7700 900142".to_owned(),
            },
            Some("street I grew up on".to_owned()),
        );
        assert!(
            flat(&column(&screen(&app))).contains("Password hint: street I grew up on"),
            "it is there to begin with"
        );

        app.login_advanced(
            SessionState::AwaitingCode {
                phone: "+44 7700 900142".to_owned(),
            },
            Some("street I grew up on".to_owned()),
        );
        assert!(
            !flat(&column(&screen(&app))).contains("Password hint"),
            "and gone with the step that earned it: {rows:#?}",
            rows = column(&screen(&app))
        );
    }

    /// `nocreds`: a sentence and no field, because there is nothing to type.
    #[test]
    fn nocreds() {
        let mut app = App::mock();
        app.begin_no_credentials();

        let rows = column(&screen(&app));
        assert!(
            row_with(&rows, "no application credentials").contains("no application credentials."),
            "{rows:#?}"
        );
        assert!(
            !flat(&rows).contains("Login code"),
            "a field the reader cannot use is worse than no field: {rows:#?}"
        );
        assert!(
            rows_of(&screen(&app))
                .iter()
                .any(|row| row.contains(" q:quit")),
            "{rows:#?}"
        );
    }

    /// `signin` / `Stored session unregistered`: the row offers the way back,
    /// because there is no step to go back to.
    #[test]
    fn signin_stale_offers_the_way_back() {
        let mut app = App::mock();
        app.begin_stale_signin();

        let rows = column(&screen(&app));
        let phone = row_with(&rows, "│ Phone");
        assert!(phone.contains(AGAIN_ROW), "{phone}");
        assert!(phone.contains("+44 7700 900142"), "{phone}");
        // The reason is said once, on the status line. It is a refusal like any
        // other: the row carries the way back and the sentence carries the why,
        // and it is the row that survives — a key pressed here writes over it.
        assert_eq!(
            app.status,
            "the stored session is no longer valid — sign in again"
        );
        app.focus = Focus::ChatList;
        assert!(
            flat(&rows_of(&screen(&app)))
                .contains("the stored session is no longer valid — sign in again"),
            "{rows:#?}"
        );

        // `⏎` on the offer asks again, and asks once.
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        assert!(
            matches!(app.take_action(), Some(Action::Login { .. })),
            "the offer is a request"
        );
        assert!(app.take_action().is_none(), "and not two of them");
    }

    // ---- the flow's own rules -------------------------------------------

    /// A machine with no application credentials gets the sentence, not a form.
    ///
    /// `:signin` is answered wherever it is typed, so the one place it can be
    /// answered is a machine that cannot use the answer — and a field the reader
    /// cannot finish is worse than the sentence saying why.
    #[test]
    fn signin_without_credentials_says_so_instead_of_asking() {
        let mut app = App::mock();
        app.credentials_configured = false;

        app.begin_signin();

        assert_eq!(app.signin(), Some(&SignIn::NoCredentials));
        assert_eq!(app.signin_field(), None, "and there is no field to fill in");
    }

    /// What the configuration carried is in the row when the step opens — and out
    /// of it again when the step is refused.
    ///
    /// A pre-fill on the way in saves a reader typing; a pre-fill after a
    /// refusal would put back the answer Telegram just turned down.
    #[test]
    fn a_prefilled_code_is_restored_once_and_never_after_a_refusal() {
        let mut app = App::mock();
        app.code_prefill = "42424".to_owned();
        app.begin_signin();
        press(&mut app, KeyCode::Enter);
        app.take_action();

        app.login_advanced(
            SessionState::AwaitingCode {
                phone: "+44 7700 900142".to_owned(),
            },
            None,
        );
        assert_eq!(app.line.text(), "42424", "the configuration filled it in");

        app.login_refused("that code is not the one Telegram sent".to_owned(), 0);
        assert_eq!(app.line.text(), "", "a refused code is not put back");
    }

    /// An empty field asks for nothing, because nothing can be done with it.
    ///
    /// A blank phone number is a request Telegram throttles, a blank code spends
    /// a login attempt, and a blank password says nothing — none of which is
    /// worth a round trip.
    #[test]
    fn an_empty_phone_asks_telegram_for_nothing() {
        let mut app = App::mock();
        app.phone = String::new();
        app.begin_signin();

        press(&mut app, KeyCode::Enter);

        assert!(
            app.take_action().is_none(),
            "and no action leaves for a caller"
        );
        assert_eq!(
            app.status,
            "there is no phone number to ask for a code with"
        );
    }

    /// A second `⏎` while a request is on its way fires no second request.
    ///
    /// Telegram counts a login attempt per request, so a reader pressing twice
    /// is asking whether the first went out. The first press earns a sentence
    /// and every press after it is silence.
    #[test]
    fn a_second_enter_does_nothing_while_waiting() {
        let mut app = App::mock();
        app.begin_signin();
        press(&mut app, KeyCode::Enter);
        assert!(
            matches!(app.take_action(), Some(Action::Login { .. })),
            "the first ⏎ asks"
        );

        // Straight back to the field, as a caller reporting the request on every
        // pass would leave it: the guard is the point, not the focus.
        app.focus = Focus::Input;
        press(&mut app, KeyCode::Enter);
        assert!(app.take_action().is_none(), "a second ⏎ must not ask twice");
        assert_eq!(app.status, "still checking — the answer is on its way");

        press(&mut app, KeyCode::Enter);
        assert!(app.take_action().is_none(), "and it says it once");
        assert_eq!(app.status, "still checking — the answer is on its way");
    }

    /// The step survives a trip away from the line; the code does not.
    #[test]
    fn the_flow_survives_leaving_the_panel() {
        let mut app = App::mock();
        app.begin_signin();
        press(&mut app, KeyCode::Enter);
        app.take_action();
        app.login_advanced(
            SessionState::AwaitingCode {
                phone: "+44 7700 900142".to_owned(),
            },
            None,
        );
        type_text(&mut app, "42424");

        // Away: paused, and the flow says so rather than vanishing.
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::ChatList);
        assert_eq!(app.signin_field(), Some(LoginField::Code));
        assert_eq!(app.status, "sign-in paused; Tab brings it back");
        assert_eq!(app.line.text(), "", "the code is dropped on the way");

        // And back: the step is still the code's, and the return says what went.
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::Input);
        assert_eq!(app.signin_field(), Some(LoginField::Code));
        assert_eq!(app.line.purpose(), PromptKind::Code);
        assert_eq!(app.status, "the code did not survive; ⏎ asks for a new one");
    }

    /// `Esc` at the code asks for a new one and says what that cost.
    #[test]
    fn esc_at_the_code_discards_it() {
        let mut app = App::mock();
        app.begin_signin();
        app.login_advanced(
            SessionState::AwaitingCode {
                phone: "+44 7700 900142".to_owned(),
            },
            None,
        );
        type_text(&mut app, "42424");
        press(&mut app, KeyCode::Esc);

        assert_eq!(app.signin_field(), Some(LoginField::Phone));
        assert_eq!(app.line.purpose(), PromptKind::Phone);
        assert!(matches!(app.take_action(), Some(Action::LoginCancelled)));
        assert_eq!(
            app.status,
            "cancelling discards the code Telegram sent; ⏎ asks for a new one"
        );
    }

    /// The paused flow's own hint, once the sentence has expired — and `q` is
    /// unbound, because a reader typing a phone number should not be able to
    /// quit the program from it.
    #[test]
    fn a_paused_flow_names_the_key_that_brings_it_back() {
        let mut app = App::mock();
        app.begin_signin();
        app.login_advanced(
            SessionState::AwaitingCode {
                phone: "+44 7700 900142".to_owned(),
            },
            None,
        );
        press(&mut app, KeyCode::Tab);
        // Once the pause sentence has expired, the row names the way back.
        app.expire_status(std::time::Instant::now() + std::time::Duration::from_secs(10));

        assert_eq!(
            crate::widgets::input_bar::hint(&app),
            " sign-in paused; Tab brings it back"
        );

        press(&mut app, KeyCode::Char('q'));
        assert!(app.confirm.is_none(), "q is unbound while the flow is up");
    }

    /// A password is painted as bullets, and nothing else about it changes.
    #[test]
    fn a_password_draft_paints_bullets_and_not_its_characters() {
        let mut app = App::mock();
        app.begin_signin();
        app.login_advanced(
            SessionState::AwaitingPassword {
                phone: "+44 7700 900142".to_owned(),
            },
            None,
        );
        type_text(&mut app, "hunter2!");

        let rows = rows_of(&screen(&app));
        let bar = row_with(&rows, "•••");
        assert!(bar.contains("••••••••"), "{bar}");
        assert!(
            !flat(&rows).contains("hunter2"),
            "and nothing on the screen has it: {rows:#?}"
        );
        // The text itself is untouched, which is why the caret can still move
        // over it: concealment is paint and nothing else.
        assert_eq!(app.line.text(), "hunter2!");
        assert_eq!(app.line.caret(), 8);
    }
}
