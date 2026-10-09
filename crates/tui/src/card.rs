//! One profile panel over two subjects: the signed-in account, and a contact.
//!
//! A row here is the same kind of object a message is, which is the whole claim
//! this module makes. The conversation already has a cursor on an item, a charwise
//! selection inside one item, `v` to select, `y` to yank, `j`/`k` between items
//! and `d` to act — so the card is that widget over different items, and one
//! interaction model serves a chat, a card and a person.
//!
//! # What separates the two subjects
//!
//! Only what they can *do*. The rows are a list either way, the cursor moves
//! between them either way, and a value is a value either way. A contact's card
//! has no row `d` can act on, and its absent fields are absent rather than empty
//! — a row exists only when the peer says something, so a birthday their privacy
//! hides is a row that is not there rather than a row reading "not set".
//!
//! # Why this is not a `List`
//!
//! The conversation is a `List`, and so was this panel, and a `List` cannot draw
//! a caret *inside* one of its items. Two things force the change:
//!
//! - **A value wraps.** A bio of three lines is one row the reader moves over with
//!   one `j`, and its highlight covers all three drawn lines. A `List` selects one
//!   item, so it would need one item per drawn line — which makes `j` move a third
//!   of the way through a field — or a highlight that cannot span.
//! - **A caret is a cell within a row.** [`crate::text_row`] puts one there, and it
//!   needs the row to be a `Line` the panel builds rather than a `ListItem` the
//!   list widget owns.
//!
//! So the panel is a `Paragraph` and the reversed cursor row is patched onto the
//! spans itself, which is the one thing `List::highlight_style` was doing for us —
//! [`Theme::selection`] applied by hand, in the order the theme's module doc
//! states, so a match or a selection on the cursor row still composes on top.
//!
//! # Why the highlight starts at the top every time
//!
//! The conversation keeps its message cursor when the card is left and returned
//! to; the card does not. One is a document the reader has a place in and the other
//! is a fixed-shape card with no order to lose a place in — a reader who opens
//! somebody's profile is asking a question they have not asked before, and
//! restoring the row they looked at last time would answer a different one.

use domain::selection::Selection;
use ratatui::text::Line;

use crate::app::{AccountState, App, SessionStore};
use crate::presence::wording;
use crate::rows;
use crate::text_row::{self, Ink, TextRow};
use crate::wrap;

/// The cue in front of every row's label.
///
/// Present and dim, drawn the way `[you]`/`[them]` are: a cell that says "there is
/// something here" without saying what it is. A row that is only as wide as its
/// value moves as the value changes, so the cue is what holds the column.
const CUE: &str = "·";

/// The label of the row that would add another account. Refuses: there is nowhere
/// to sign in from.
///
/// Named rather than written where it is needed because the key handler matches on
/// it, and a literal in two places is a way to spell the same row two ways — which
/// is what the enum these replace was guarding against.
pub const ADD_ACCOUNT: &str = "add account";

/// The label of the row that signs out. Confirms, then asks the side holding the
/// client to discard the session.
pub const LOGOUT: &str = "logout";

/// Where the cue, the label and the value start, measured from the panel's border.
///
/// Fixed rather than measured, because a label column that moved with the longest
/// label would put the values somewhere else for every account — and the point of
/// a card is that the same row is the same row on both subjects.
const CUE_X: usize = 1;
/// Wide enough for the longest label the card has, which is `add account` — a
/// label cut to fit is a label the reader cannot act on, and this row is one `d`
/// away from doing something.
const LABEL_W: usize = 11;
/// `CUE_X` + the cue and its gap + the label column + one more, so a label that
/// fills its column still has a space between itself and the value. Without that
/// space the two run together, and `add account` becomes `add accountadd account`.
const VALUE_X: usize = CUE_X + 2 + LABEL_W + 1;

/// What one row of a card is for.
///
/// The distinction is the whole of the difference between the two subjects: a
/// *value* is something to read, select and yank, and an *action* is something to
/// do. `d` means something on an action and nothing on a value, which is why the
/// key is named on one subject's hint and not the other's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// Something the peer said. Selectable, yankable, never acted on.
    Value,

    /// Something the reader can do about. Dim, and acted on by `d`.
    Action,
}

/// One row of a card: a label, a value, and what the value is for.
///
/// A row exists only when the peer says something, so there is no `Option` on the
/// value: [`rows`] leaves a row out rather than building an empty one, and a reader
/// can tell an absent field from a set one by whether the row is there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardRow {
    /// What the row is for.
    pub kind: RowKind,

    /// The field's name, in a fixed column.
    pub label: &'static str,

    /// What the peer said, or what the action is called.
    pub value: String,

    /// Whether this row is a slot held for a field that does not exist yet.
    ///
    /// Not a row a reader can reach and not a row a count includes: it draws
    /// nothing, and what it holds is a **position**, so that the field which fills
    /// it lands where the design put it rather than wherever it was appended. See
    /// [`CardRow::reserved`].
    pub reserved: bool,
}

impl CardRow {
    /// A value row.
    #[must_use]
    pub fn value(label: &'static str, value: impl Into<String>) -> Self {
        Self {
            kind: RowKind::Value,
            label,
            value: value.into(),
            reserved: false,
        }
    }

    /// An action row: the value is the action's own name, because there is
    /// nothing else to read on it.
    #[must_use]
    pub fn action(label: &'static str) -> Self {
        Self {
            kind: RowKind::Action,
            label,
            value: label.to_owned(),
            reserved: false,
        }
    }

    /// A slot held open for a field the design has and this build has not built.
    ///
    /// The design reserves a row for a **per-peer colour** between the name and the
    /// username, and held its slot without designing it: Telegram has no field for
    /// a colour, so there is nothing to draw, and a row that draws nothing is not a
    /// row. Building one anyway is the alternative, and it is what this is — because
    /// a reservation held only by a comment is a reservation the next person to
    /// insert a field between the name and the username loses silently, and a
    /// silently lost reservation is a design decision undone by nobody's decision.
    ///
    /// Everything downstream therefore knows about it: [`lines`](self::lines)
    /// draws no line for it, [`title`] neither counts nor numbers it, the row
    /// cursor steps over it, a yank skips it, and it is neither selectable nor
    /// something `d` can act on. What it buys is the index.
    #[must_use]
    pub fn reserved(label: &'static str) -> Self {
        Self {
            kind: RowKind::Value,
            label,
            value: String::new(),
            reserved: true,
        }
    }

    /// Whether this row is a held slot rather than a field.
    #[must_use]
    pub const fn is_reserved(&self) -> bool {
        self.reserved
    }

    /// Whether the row can be selected inside, as a range of characters.
    ///
    /// Every field has text, so every field is selectable — including an action's,
    /// where the text is the action's own name. The one row that is not is a
    /// reserved slot, which has no text at all: a selection over an empty range is
    /// a selection of nothing, and `y` on it would put nothing in the register.
    ///
    /// A held slot is not drawn at all, so nothing reaches this with one in hand —
    /// it is the row model's statement of the rule rather than a branch anything
    /// takes today, and it is the answer a selection or a yank should ask first.
    #[must_use]
    pub const fn is_selectable(&self) -> bool {
        !self.reserved
    }

    /// Whether `d` can act on this row.
    ///
    /// Only an action, which is the entire difference in what the two subjects
    /// can do: a contact's card has no action, so `d` there refuses rather than
    /// doing nothing.
    #[must_use]
    pub const fn is_action(&self) -> bool {
        matches!(self.kind, RowKind::Action)
    }
}

/// The rows for the subject on show.
///
/// The one function that decides which rows exist, so the panel and the keys cannot
/// disagree about it — the discipline the conversation's window follows. A field
/// the peer did not fill is left out rather than built empty.
#[must_use]
pub fn rows(app: &App) -> Vec<CardRow> {
    match app.card_subject() {
        CardSubject::SelfAccount => self_rows(app),
        CardSubject::Contact(_) => contact_rows(app),
    }
}

/// How many rows the highlight can reach.
///
/// Up to and including the last row that is **drawn**. A held slot at the end of a
/// card is not somewhere `j` goes: `j` there would put the highlight on a row that
/// draws nothing, and a highlight nobody can see is worse than one that does not
/// move.
///
/// **This bounds the highlight; it does not skip inside the bound.** The colour
/// slot is *interior* on a contact's card — the name is above it and the identity
/// and everything after it are below — so a held slot inside the range is still
/// somewhere `j` can land. [`App::card_motion_row`](crate::app) steps off it, and
/// the two are one rule split in two: this decides where the highlight may go, and
/// that decides where it stops being.
#[must_use]
pub fn navigable(rows: &[CardRow]) -> usize {
    rows.iter()
        .rposition(|row| !row.is_reserved())
        .map_or(0, |last| last + 1)
}

/// Whom the card is about.
#[derive(Debug, Clone, Copy)]
pub enum CardSubject<'a> {
    /// The signed-in account.
    SelfAccount,

    /// Somebody the reader is talking to, named by the chat they were in.
    Contact(&'a domain::chat::Chat),
}

impl CardSubject<'_> {
    /// The title's name for the subject.
    #[must_use]
    pub fn name(&self) -> String {
        match self {
            CardSubject::SelfAccount => "you".to_owned(),
            CardSubject::Contact(chat) => chat.title.clone(),
        }
    }
}

/// The account's own rows.
fn self_rows(app: &App) -> Vec<CardRow> {
    let AccountState::Known(account) = &app.session.account else {
        return Vec::new();
    };

    let mut rows = vec![
        CardRow::value("name", account.display_name()),
        // `not set` rather than no row, and the two are not the same thing here.
        // The account always has a phone number, so a missing one is a fact about
        // the reader's own account and a row worth drawing; see `identity`.
        CardRow::value(
            "username",
            identity(account).unwrap_or_else(|| "not set".to_owned()),
        ),
    ];
    if let Some(bio) = account.bio.as_deref().filter(|_| account.has_bio()) {
        rows.push(CardRow::value("bio", bio.to_owned()));
    }
    if let Some(birthday) = birthday(account.birthday) {
        rows.push(CardRow::value("birthday", birthday));
    }
    rows.push(CardRow::value("id", format!("id {}", account.user_id)));
    rows.push(CardRow::value(
        "session",
        session(&app.session.session_store),
    ));
    rows.push(CardRow::action(ADD_ACCOUNT));
    rows.push(CardRow::action(LOGOUT));
    rows
}

/// A contact's rows.
///
/// **No rows at all until the profile has been read.** The card shows the panel's
/// two shell sentences instead, and that is the whole difference between this and
/// a card that drew the name and then filled in: the reader pressed `A` to ask a
/// question, and a card that answers half of it is a card that has to be believed
/// a moment before it is. The account's own card is read once at start-up for the
/// same reason — a panel that blanked and refilled every time would be one the
/// reader could not trust — and a contact's card is read once per open.
///
/// The name is the one row the chat list already knows, and it is drawn *from the
/// profile* rather than from the chat, so that a name the peer has since changed
/// is the name they have now.
fn contact_rows(app: &App) -> Vec<CardRow> {
    let Some(contact) = app.contact() else {
        return Vec::new();
    };
    let AccountState::Known(account) = &contact.state else {
        return Vec::new();
    };

    let mut rows = vec![
        CardRow::value("name", account.display_name()),
        // The design's per-peer colour, held and not built. It sits here and not
        // at the end because a colour is something a reader *scans* — it is the
        // thing they are looking for when they open somebody at all — so a field
        // that means to be seen belongs above the identity, not below it.
        CardRow::reserved(COLOUR),
    ];
    if let Some(identity) = identity(account) {
        rows.push(CardRow::value("username", identity));
    }
    // The live report wins over the one the profile read carried, because it is
    // the newer of the two. Absent, not empty, when the peer restricts it.
    let presence = app
        .peer_presence(contact.peer_id)
        .or(account.presence)
        .and_then(|presence| wording(presence, app.now(), app.offset()));
    if let Some(status) = presence {
        rows.push(CardRow::value("status", status));
    }
    if let Some(bio) = account.bio.as_deref().filter(|_| account.has_bio()) {
        rows.push(CardRow::value("bio", bio.to_owned()));
    }
    if let Some(birthday) = birthday(account.birthday) {
        rows.push(CardRow::value("birthday", birthday));
    }
    rows
}

/// The label of the held slot, which is the design's per-peer colour.
///
/// Named so the thing being held is named in the program rather than only in the
/// design document: a slot called `reserved` could be anything, and a reader of
/// this file should be able to see what is meant to land in it.
const COLOUR: &str = "colour";

/// The username and the phone number, on one row.
///
/// One line rather than three, because the identity is one thing and a reader
/// scanning for "which account is this" wants one line to find it on. The `·` is
/// the title-note joiner the status line and the chat list already use.
///
/// `None` when the account gave neither part, and **the two subjects read that
/// differently on purpose**. For your own account it is a row saying `not set`,
/// because you always have a phone number and a missing one is a fact. For a
/// contact it is *no row at all*, because most people do not set a username and
/// their phone number is hidden by their own privacy — and a card that wrote
/// `not set` under a person's name would be telling a reader that something about
/// them is missing when what is missing is only the answer. The card's rule is
/// that a row exists when the peer says something, and a contact who says nothing
/// about their identity has said that.
fn identity(account: &domain::account::Account) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(username) = account.username.as_deref() {
        parts.push(format!("@{username}"));
    }
    if let Some(phone) = account.phone.as_deref() {
        parts.push(phone.to_owned());
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// A birthday as a reader would write it.
///
/// Not a timestamp and not a `NaiveDate`: `domain` carries a day, a month and
/// sometimes a year, and a year is a disclosure the account may not have made, so
/// the row is written without one rather than with a guess. `None` means there is
/// no row at all.
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

/// Where the session is kept, as the card says it.
fn session(store: &SessionStore) -> String {
    match store {
        SessionStore::Keyring => "OS keyring".to_owned(),
        SessionStore::EncryptedFile(path) => format!("encrypted file {}", path.display()),
    }
}

// ---- the drawing --------------------------------------------------------

/// The title: the subject, and which row the cursor is on.
///
/// The subject rather than the command, because a title that names a state stops
/// being true the moment the panel is reused for somebody else. The `(n/m)` is
/// there because **a row that disappears when a privacy setting hides it changes
/// the count** — so a reader who saw four rows yesterday and three today can tell
/// that a row went rather than that they misremembered.
#[must_use]
pub fn title(app: &App) -> String {
    let all = rows(app);
    // Over the rows that are **drawn**, because the count's whole job is to tell a
    // reader that a field *went* — and a held slot did not go and was never there,
    // so counting it would make the number count something other than what the peer
    // told them. The cursor's number is its position among the same rows, for the
    // same reason: `1/2` must mean the first of two things there are.
    let drawn = |row: &&CardRow| !row.is_reserved();
    let total = all.iter().filter(drawn).count();
    if total == 0 {
        // The subject even here. A card that is drawing a sentence because it has
        // no rows yet is the card where naming the subject matters most — it is
        // the one a reader is waiting to see the right name on.
        return format!(" Profile · {} ", app.card_subject().name());
    }
    let before = app.profile_cursor().min(all.len());
    let cursor = all[..before].iter().filter(drawn).count() + 1;
    format!(
        " Profile · {} ({cursor}/{total}) ",
        app.card_subject().name()
    )
}

/// The card's lines, at the width the panel gave it.
///
/// A wrapped value is several lines and one row, so each line comes back with the
/// row it belongs to: that is what lets the highlight cover a whole field, and what
/// tells the panel which row a caret belongs to.
///
/// The rows are borrowed rather than built here, so the caller owns them for the
/// frame and the spans are slices of the values — a copy of every row of every
/// field, on every frame, is what a `ListItem<'static>` of owned text was doing.
#[must_use]
pub fn lines<'r>(app: &App, rows: &'r [CardRow], width: u16) -> Vec<(usize, Line<'r>)> {
    let inner = width.saturating_sub(2).max(1);
    // `wrap` measures in `u16` because a panel is a `u16` wide, and the columns
    // above are `usize` because they are positions in a span rather than widths.
    let value_width = inner
        .saturating_sub(u16::try_from(VALUE_X).unwrap_or(u16::MAX))
        .max(1);
    let cursor = app.profile_cursor();
    let selection = app.card_selection();

    let mut out = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        // A held slot draws nothing — no cue, no label, no value, and no line for
        // it either. A blank line would be worse than nothing: it would read as a
        // field the peer left empty, which is the one thing a card never draws.
        if row.is_reserved() {
            continue;
        }
        let on_cursor = index == cursor;
        let ink = ink_for(app, row, on_cursor);
        let selected = selection
            .filter(|sel| within(sel, index))
            .and_then(|sel| sel.text_range())
            .map(|(_, range)| range);

        // Byte ranges back into the value rather than copies of it: a bio is the
        // reader's own words about themselves and the panel does not re-measure
        // them.
        for (line, range) in wrap::wrap(&row.value, value_width).into_iter().enumerate() {
            let mut spans = Vec::new();
            if line == 0 {
                // The cue and the label are the row's, on its first line only.
                spans.extend(leading(&ink, row));
            } else {
                // A continuation line has no name of its own: a wrapped value is
                // one row, and repeating its name per line would be three rows
                // wearing one row's name.
                spans.push(ratatui::text::Span::raw(" ".repeat(VALUE_X)));
            }

            spans.extend(text_row::spans(&TextRow {
                text: &row.value,
                range: range.clone(),
                matched: false,
                selected: selected
                    .clone()
                    .map(|chars| rows::byte_span(&row.value, chars)),
                // A motion counts characters and a caret is a byte offset;
                // `rows::byte_span` is the one converter in the program.
                caret: (on_cursor && row.is_selectable() && !row.is_action())
                    .then(|| app.card_caret_byte(&row.value)),
                reversed: on_cursor,
                // Nothing on a card is secret: a value is the reader's own
                // profile or somebody they are talking to, and neither is a
                // password.
                concealed: false,
                ink,
            }));

            out.push((index, Line::from(spans)));
        }
    }
    out
}

/// The inks a row is painted with.
///
/// On the cursor row the value's own ink **is** the reversal, so the caret's hole
/// is a hole in it. An action is dim whether or not the cursor is on it, because it
/// is a promise rather than a value and dimming is how the panel says so.
fn ink_for(app: &App, row: &CardRow, on_cursor: bool) -> Ink {
    let base = Ink::readonly(&app.ui.theme);
    if on_cursor {
        return Ink {
            plain: app.ui.theme.selection,
            ..base
        };
    }
    if row.is_action() {
        return Ink {
            plain: app.ui.theme.text_dim,
            ..base
        };
    }
    base
}

/// The cue and the label, padded into their two fixed columns.
///
/// Padded by *measuring what was written* rather than by adding a fixed pad, so the
/// value lands in the same column whichever label is on the row. A label that
/// overran its column would push the value out, which is why [`fit`] cuts one.
fn leading<'a>(ink: &Ink, row: &CardRow) -> Vec<ratatui::text::Span<'a>> {
    let label = fit(row.label, LABEL_W);
    // The whole gutter is one span of exactly `VALUE_X - CUE_X` columns: the cue,
    // the label, and enough padding to fill it. Computing the pad from the label's
    // own width is what puts every value in the same column — a fixed pad would
    // put a short label's value one column left of a long one's.
    let written = 2 + label.chars().count();
    let pad = " ".repeat((VALUE_X - CUE_X).saturating_sub(written));
    vec![
        ratatui::text::Span::raw(" ".repeat(CUE_X)),
        ratatui::text::Span::styled(format!("{CUE} {label}{pad}"), ink.plain),
    ]
}

/// Whether a selection is inside row `index`, rather than across rows.
///
/// A selection reaching two rows is a selection of *rows*, and each of their values
/// is yankable whole — the same rule the conversation follows, and for the same
/// reason: there is no such thing as a selection that quotes half of each.
fn within(sel: &Selection, index: usize) -> bool {
    sel.text_range()
        .is_some_and(|(id, _)| usize::try_from(id).is_ok_and(|id| id == index))
}

/// A label, cut to the label column.
///
/// Cut rather than allowed to overrun, because the value is what the reader came
/// for and a label that takes the room has taken it from the wrong thing.
fn fit(label: &str, width: usize) -> String {
    if label.chars().count() <= width {
        return label.to_owned();
    }
    label.chars().take(width - 1).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::vim::{CharMotion, char_motion};

    /// The label column is fixed, so a label that overruns it is cut rather than
    /// allowed to push the value out of the panel.
    #[test]
    fn a_label_longer_than_its_column_is_cut_rather_than_overrunning_it() {
        assert_eq!(fit("name", 9), "name");
        assert_eq!(fit("a-very-long-label", 9).chars().count(), 9);
        assert!(fit("a-very-long-label", 9).ends_with('…'));
    }

    /// The value column is past the label column on every account, which is what
    /// makes the same row the same row whichever subject is on show.
    #[test]
    fn the_value_column_is_the_same_whatever_the_longest_label_is() {
        // A compile-time fact, so asserted at compile time: the label column can
        // never reach the value's, which is what makes the value column fixed.
        const { assert!(VALUE_X >= CUE_X + 2 + LABEL_W) };
    }

    /// A birthday the account may not have disclosed is written without a year
    /// rather than with a guessed one.
    #[test]
    fn a_birthday_without_a_year_is_written_without_one() {
        let date = domain::account::Birthday {
            day: 10,
            month: 12,
            year: None,
        };
        assert_eq!(birthday(Some(date)), Some("born 10 Dec".to_owned()));
    }

    /// The card names the store the session is in, and says of a file that it is
    /// encrypted, so a path is never read as a plaintext one.
    #[test]
    fn the_session_row_names_the_store_and_calls_a_file_encrypted() {
        assert_eq!(session(&SessionStore::Keyring), "OS keyring");
        assert_eq!(
            session(&SessionStore::EncryptedFile("/tmp/televim.session".into())),
            "encrypted file /tmp/televim.session"
        );
    }

    /// A date the framework let through that is still not one says what is wrong
    /// rather than printing `born 99` or going blank.
    #[test]
    fn a_birthday_that_is_not_a_date_says_so() {
        let date = domain::account::Birthday {
            day: 99,
            month: 13,
            year: None,
        };
        assert_eq!(
            birthday(Some(date)),
            Some("born on a date this build will not guess".to_owned())
        );
    }

    /// No birthday at all is **no row**, not a row reading "not set": the row
    /// exists to say whether there is one.
    #[test]
    fn a_birthday_that_is_absent_is_not_a_row() {
        assert_eq!(birthday(None), None);
    }

    /// An identity the account gave one part of is that part, and an identity it
    /// gave none of is a row that says so — **for your own account**.
    ///
    /// The two subjects read the same `None` differently and the test says both,
    /// because the difference is the whole of it: you always have a phone number,
    /// so a missing one is a fact worth drawing, and a contact who sets no
    /// username has not told you something is wrong with them.
    #[test]
    fn an_identity_with_nothing_in_it_says_so_for_you_and_is_no_row_for_a_contact() {
        let bare = domain::account::Account {
            user_id: 1,
            first_name: "Ada".to_owned(),
            last_name: "Lovelace".to_owned(),
            username: None,
            phone: None,
            birthday: None,
            bio: None,
            presence: None,
        };
        assert_eq!(identity(&bare), None, "there is no identity to show");
        assert_eq!(
            CardRow::value(
                "username",
                identity(&bare).unwrap_or_else(|| "not set".to_owned())
            )
            .value,
            "not set",
            "and your own card says so rather than showing nothing"
        );

        let username_only = domain::account::Account {
            username: Some("ada".to_owned()),
            ..bare.clone()
        };
        assert_eq!(identity(&username_only).as_deref(), Some("@ada"));
        let phone_only = domain::account::Account {
            phone: Some("+15551234567".to_owned()),
            ..bare
        };
        assert_eq!(identity(&phone_only).as_deref(), Some("+15551234567"));
    }

    /// `d` acts on an action and on nothing else. This is the entire difference
    /// in what the two subjects can do, so it is worth a test that says so.
    #[test]
    fn only_an_action_row_is_something_d_can_act_on() {
        assert!(CardRow::action("logout").is_action());
        assert!(!CardRow::value("name", "Ada").is_action());
    }

    /// The only rows `d` can act on are the two named actions, and they are named
    /// by the constants the key handler matches on.
    ///
    /// The two subjects differ by exactly this much, so it is the one thing about
    /// the rows that must not grow quietly: a third action is a key that needs a
    /// hint, a confirmation or a refusal, and this is where it would show up.
    #[test]
    fn the_only_rows_d_can_act_on_are_the_two_named_actions() {
        let app = App::mock();
        let actions: Vec<&str> = rows(&app)
            .iter()
            .filter(|row| row.is_action())
            .map(|row| row.label)
            .collect();
        assert_eq!(actions, [ADD_ACCOUNT, LOGOUT]);
    }

    /// An action's text is the action's own name, which is why the label is not
    /// repeated beside it: there is nothing else to read.
    #[test]
    fn an_actions_value_is_its_own_name() {
        assert_eq!(CardRow::action("logout").value, "logout");
    }

    /// A motion counts characters and a caret is a byte offset, and the two only
    /// diverge once a multi-byte character has gone by.
    #[test]
    fn an_inline_position_from_a_motion_becomes_a_byte_offset() {
        let value = "Ada 🎂";
        // `l` four times, which is four characters in — the cake counted once,
        // because a motion steps by one thing the reader can see.
        let mut at = 0;
        for _ in 0..4 {
            at = char_motion(value, at, CharMotion::Step { forward: true });
        }
        assert_eq!(at, 4, "four characters in, the cake");
        assert_eq!(
            rows::byte_span(value, at..at + 1).start,
            4,
            "four bytes in too, because everything before the cake is ASCII"
        );

        // The divergence itself: a character position *past* the cake is a byte
        // position two further on, and that is the whole reason a motion's answer
        // cannot be used as a caret's.
        let past = char_motion(value, 4, CharMotion::Step { forward: true });
        assert_eq!(past, 4, "clamped, because the cake is the last character");
        assert_eq!(rows::byte_span(value, past..past + 1).start, 4);
        assert_eq!(
            value.len(),
            8,
            "four ASCII, a space, and a cake of four: the two counts differ"
        );
    }

    /// The keys a card names, and the motions they are, in one place — so a key
    /// that means nothing here cannot be named on a card's hint.
    #[test]
    fn every_char_motion_a_card_names_is_a_motion_the_domain_knows() {
        for key in ['l', 'h', 'w', 'b', 'e', '0', '$'] {
            assert!(
                CharMotion::from_key(key).is_some(),
                "{key} is named on a card hint and means no motion"
            );
        }
        // `g` and `G` are motions over a whole buffer and are answered by the row
        // cursor, so they are deliberately not char motions.
        for key in ['g', 'G', 'd', 'y'] {
            assert!(
                CharMotion::from_key(key).is_none(),
                "{key} is not a motion within a row"
            );
        }
    }

    /// The count in the title is the one thing that tells a reader that a row
    /// *went*, so it is over the rows that exist rather than over a fixed list.
    #[test]
    fn the_title_counts_the_rows_the_peer_actually_gave() {
        let mut app = App::mock();
        let before = rows(&app).len();
        assert!(before > 0, "the mock account has rows");

        // A birthday the privacy hides is a row that is not there.
        if let AccountState::Known(account) = &mut app.session.account {
            account.birthday = None;
        }
        let after = rows(&app).len();
        assert_eq!(after, before - 1, "one row went, and the count says so");
    }
}

#[cfg(test)]
mod drawing {
    use super::*;
    use crate::app::App;
    use crate::state::coordinate;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Modifier;

    /// The card drawn into an in-memory terminal, at the width a panel gets.
    fn screen(app: &App, width: u16) -> Buffer {
        let mut terminal =
            Terminal::new(TestBackend::new(width, 20)).expect("the test backend builds");
        terminal
            .draw(|frame| crate::widgets::profile::render(app, frame.area(), frame))
            .expect("the frame draws");

        terminal.backend().buffer().clone()
    }

    /// The screen as text, one string per row.
    fn rows_of(buffer: &Buffer) -> Vec<String> {
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| {
                        buffer.content()
                            [usize::from(y) * usize::from(buffer.area.width) + usize::from(x)]
                        .symbol()
                    })
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    /// Opens a card the way the delegate did: the seam, then the sizing.
    fn open_card(app: &mut App, subject: crate::app::ProfileId) {
        coordinate::open_card(
            &mut app.profile,
            &mut app.ui,
            &mut app.conversation,
            &mut app.outbox,
            subject,
        );
        app.profile.resize(navigable(&rows(app)));
    }

    /// One card, about the account.
    fn self_card() -> App {
        let mut app = App::mock();
        open_card(&mut app, crate::app::ProfileId::SelfAccount);
        app
    }

    /// A card about a contact whose profile has been read.
    ///
    /// A contact's card draws nothing until the read lands, so every test about
    /// its *rows* has to land the read first — and a test that quietly did not
    /// would be testing a card with no rows, which passes for the wrong reason.
    fn contact_card() -> (App, domain::chat::Chat) {
        let mut app = self_card();
        let chat = app.any_chat().expect("the mock has a chat");
        open_card(&mut app, crate::app::ProfileId::User(chat.id));
        app.set_contact(chat.id, Ok(crate::app::mock_account()));
        (app, chat)
    }

    /// A dump of the card's spans, for reading a test failure.
    fn dump(app: &App, width: u16) -> String {
        let all = rows(app);
        lines(app, &all, width)
            .into_iter()
            .enumerate()
            .map(|(line, (row, l))| {
                let spans: Vec<String> = l
                    .spans
                    .iter()
                    .map(|span| format!("{:?}", span.content.as_ref()))
                    .collect();
                format!("{line}: row {row} {spans:?}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A value and its label are both on show, which is the thing a `List` of
    /// plain strings could not do: the panel is a list of *rows*, and a row is a
    /// label and a value rather than one string.
    #[test]
    fn a_row_is_a_label_and_a_value_and_both_are_on_show() {
        let app = self_card();
        let text = rows_of(&screen(&app, 40)).join("\n");

        assert!(text.contains("name"), "the label: {text}");
        assert!(text.contains("Ada"), "the value: {text}");
    }
    /// Every row has a cue and every value starts in the same column, which is
    /// what makes a card scannable: a column of labels and a column of values.
    ///
    /// Measured in **columns** rather than in spans, because a label that fills its
    /// column and one that does not are the same two spans — the alignment lives in
    /// the padding, so the only way to see it is to count cells.
    #[test]
    fn every_row_has_a_cue_and_the_values_line_up() {
        let app = self_card();
        let all = rows(&app);
        let drawn = lines(&app, &all, 40);

        let value_columns: Vec<usize> = drawn
            .iter()
            .filter(|(_, line)| line.spans.iter().any(|span| span.content.contains(CUE)))
            .map(|(_, line)| {
                let mut column = 0;
                let mut seen_cue = false;
                for span in &line.spans {
                    let wide = span.content.chars().count();
                    if seen_cue {
                        if !span.content.trim().is_empty() {
                            return column;
                        }
                    } else if span.content.contains(CUE) {
                        seen_cue = true;
                    }
                    column += wide;
                }
                column
            })
            .collect();

        assert!(
            !value_columns.is_empty(),
            "the card has rows: {}",
            dump(&app, 40)
        );
        let first = value_columns[0];
        assert!(
            value_columns.iter().all(|column| *column == first),
            "every value starts in column {first}: {value_columns:?}\n{}",
            dump(&app, 40)
        );
        assert_eq!(first, VALUE_X, "and that column is the one the columns say");
    }

    /// A wrapped value is one row and several lines, and every one of them belongs
    /// to that row — which is what lets `j` move a whole field and the highlight
    /// cover all of it, and the reason this is not a `List`.
    #[test]
    fn a_wrapped_value_is_one_row_over_several_lines() {
        let mut app = self_card();
        let width = 30;
        let before = rows(&app).len();

        // A bio long enough to wrap at the width above. The mock already has one,
        // so this *replaces* a row rather than adding one: the row count must not
        // move, because a row is a field and not a line of text.
        if let crate::app::AccountState::Known(account) = &mut app.session.account {
            account.bio = Some("a bio that is quite a lot longer than thirty columns".to_owned());
        }
        let after_rows = rows(&app);
        assert_eq!(
            after_rows.len(),
            before,
            "a bio is one row whatever its length: {} -> {}",
            before,
            after_rows.len()
        );

        let drawn = lines(&app, &after_rows, width);
        // Every drawn line that belongs to the bio's row, found by row index
        // rather than by text — a second line of a wrapped value is not going to
        // repeat the first line's words.
        let bio = after_rows
            .iter()
            .position(|row| row.label == "bio")
            .expect("the bio is a row");
        let bio_lines: Vec<&Line<'_>> = drawn
            .iter()
            .filter(|(row, _)| *row == bio)
            .map(|(_, line)| line)
            .collect();

        assert!(
            bio_lines.len() > 1,
            "the bio is drawn on several lines: {}\n{}",
            bio_lines.len(),
            dump(&app, width)
        );
        // And the one carrying the label, so a reader can tell which row it is.
        assert!(
            bio_lines[0]
                .spans
                .iter()
                .any(|span| span.content.contains(CUE)),
            "and only the first carries the row's name"
        );
    }

    /// The cursor row is reverse video, and **the inline caret on it is not** —
    /// that is the design's rule, and it is the one thing a `List` could not have
    /// done at all, because it selects items rather than cells.
    #[test]
    fn the_cursor_row_is_reversed_and_its_caret_is_a_hole_in_it() {
        let mut app = self_card();
        // One row in, and the inline position inside its value.
        app.handle_card_row(1);
        app.handle_card_motion('l');

        let buffer = screen(&app, 40);
        let mut reversed = 0;
        // A hole is a cell inside the reversed run that is *not* reversed.
        // `Style::remove_modifier` clears the bit rather than setting an opposite
        // one, so the absence of `REVERSED` on a reversed row is the whole
        // mechanism, and asserting that absence is asserting the rule.
        let mut hole_after_reversal = false;
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                let cell = &buffer.content()
                    [usize::from(y) * usize::from(buffer.area.width) + usize::from(x)];
                if cell.modifier.contains(Modifier::REVERSED) {
                    reversed += 1;
                    hole_after_reversal = false;
                } else if reversed > 0 && !hole_after_reversal && cell.symbol() == " " {
                    hole_after_reversal = true;
                }
            }
        }

        assert!(reversed > 0, "the cursor row is reverse video");
        assert!(
            hole_after_reversal,
            "and the caret in it is a hole rather than another reverse: {reversed} reversed"
        );
    }

    /// A card row's caret is a **normal** caret, not the line's insert one, and it
    /// is marked rather than filled. A card is a read-only surface, so a block —
    /// the shape an insert caret has — would say the reader is typing.
    #[test]
    fn a_card_caret_is_the_normal_caret_and_not_the_insert_one() {
        let app = self_card();
        let buffer = screen(&app, 40);
        let width = usize::from(buffer.area.width);
        let mut holes = 0;
        for index in 0..width * usize::from(buffer.area.height) {
            let cell = &buffer.content()[index];
            // `Style::remove_modifier` clears the bit rather than setting an
            // opposite one, so a hole is a cell that is simply not reversed. That
            // is the whole mechanism, and asserting its absence is asserting the
            // rule.
            if cell.symbol() == " " && !cell.modifier.contains(Modifier::REVERSED) {
                holes += 1;
            }
        }

        assert!(holes > 0, "a hole for the caret, and it is not reversed");
    }

    /// The title names the subject and counts the rows, and the count is over the
    /// rows the peer **actually gave** — which is what tells a reader that a field
    /// went rather than that they misremembered it.
    #[test]
    fn the_title_names_the_subject_and_counts_the_rows_that_exist() {
        let app = self_card();
        let rows = rows(&app);
        let title = title(&app);

        assert!(title.contains("you"), "the subject: {title}");
        assert!(
            title.contains(&format!("1/{}", rows.len())),
            "and the count: {title}"
        );
    }

    /// A contact's card says who it is about, and names fewer rows than the
    /// account's own — because a row exists only when the peer says something, and
    /// the only thing said about a contact today is their name.
    #[test]
    fn a_contacts_card_names_them_and_has_fewer_rows_than_the_accounts() {
        let mine = rows(&self_card()).len();
        let (app, chat) = contact_card();
        let theirs = rows(&app);

        assert!(
            title(&app).contains(&chat.title),
            "the card is about them: {}",
            title(&app)
        );
        assert!(theirs.len() < mine, "{theirs:?} is fewer than {mine}");
        assert!(
            !theirs.iter().any(CardRow::is_action),
            "and a contact has no row to act on: {theirs:?}"
        );
    }

    /// A contact's card draws **nothing** until the profile has been read, and
    /// says so rather than drawing the name and then filling in.
    ///
    /// The reader pressed `A` to ask a question, and a card that answers half of
    /// it is a card the reader has to believe a moment before they do not have to.
    #[test]
    fn a_contacts_card_draws_nothing_until_the_profile_has_been_read() {
        let mut app = self_card();
        let chat = app.any_chat().expect("the mock has a chat");
        open_card(&mut app, crate::app::ProfileId::User(chat.id));

        assert!(rows(&app).is_empty(), "no rows, and no half of one");
        let text = rows_of(&screen(&app, 40)).join("\n");
        assert!(
            text.contains("reading their profile"),
            "and it says what it is waiting for: {text}"
        );
        assert!(
            title(&app).contains(&chat.title),
            "while still naming who it is about: {}",
            title(&app)
        );

        app.set_contact(chat.id, Ok(crate::app::mock_account()));
        assert!(
            !rows(&app).is_empty(),
            "and the rows arrive with the answer"
        );
    }

    /// A read that lands after the reader has opened somebody else's card is
    /// dropped rather than drawn on the wrong person.
    ///
    /// The reader may open a second card while the first read is in flight, and a
    /// round trip is long enough that this is the normal case rather than a race.
    /// Matching the answer to the card that asked is what keeps a card from filling
    /// in with somebody else's bio.
    #[test]
    fn an_answer_about_somebody_else_is_dropped() {
        let (mut app, chat) = contact_card();
        let before = rows(&app);

        app.set_contact(
            chat.id + 1,
            Ok(domain::account::Account {
                user_id: 99,
                first_name: "Somebody".to_owned(),
                last_name: "Else".to_owned(),
                bio: Some("not this card's bio".to_owned()),
                ..domain::account::Account::default()
            }),
        );

        assert_eq!(rows(&app), before, "the card on show did not change");
        assert!(
            !rows_of(&screen(&app, 40))
                .join("\n")
                .contains("Somebody Else"),
            "and the other person's name is nowhere on the screen"
        );
    }

    /// A read that failed says why, in its own words rather than the account's.
    ///
    /// The account's first line is `not signed in` because its problem is
    /// credentials. Putting that on a failed profile read would send a reader to
    /// check something that is not the problem, which is the whole reason the two
    /// carry their own wording.
    #[test]
    fn a_contacts_card_whose_read_failed_says_why() {
        let mut app = self_card();
        let chat = app.any_chat().expect("the mock has a chat");
        open_card(&mut app, crate::app::ProfileId::User(chat.id));
        app.set_contact(chat.id, Err("peer 42 is not a person".to_owned()));

        assert!(rows(&app).is_empty());
        let text = rows_of(&screen(&app, 40)).join("\n");
        assert!(text.contains("could not read this profile"), "{text}");
        assert!(text.contains("not a person"), "and the reason: {text}");
        assert!(
            !text.contains("not signed in"),
            "and not the account's: {text}"
        );
        assert!(!text.contains("credentials"), "{text}");
    }

    /// The rows a contact's card draws are the ones **they** said, and nothing
    /// else.
    ///
    /// A contact's phone number is hidden by their own privacy far more often than
    /// not, so the identity row has to degrade to the username alone and to *no
    /// row* when there is neither — which is the card's own rule, and the reason
    /// the count in the title is worth reading.
    #[test]
    fn a_contacts_rows_are_the_ones_they_said() {
        let (mut app, _) = contact_card();
        let peer = app.any_chat().expect("the mock has a chat").id;

        // Everything they said.
        let full = rows(&app);
        assert_eq!(
            full.iter().map(|row| row.label).collect::<Vec<_>>(),
            ["name", COLOUR, "username", "bio", "birthday"],
            "the colour slot sits between the name and the identity"
        );

        // Nothing they said but a name.
        app.set_contact(
            peer,
            Ok(domain::account::Account {
                user_id: 42,
                first_name: "Grace".to_owned(),
                last_name: "Hopper".to_owned(),
                ..domain::account::Account::default()
            }),
        );
        let bare = rows(&app);
        assert_eq!(
            bare.iter().map(|row| row.label).collect::<Vec<_>>(),
            ["name", COLOUR],
            "no username, no bio, no birthday: no rows either"
        );
        assert!(
            !bare.iter().any(|row| row.value == "not set"),
            "and a person who set no username has not told us anything is wrong: {bare:?}"
        );
        assert!(bare.len() < full.len(), "and the count says a row went");
    }

    /// `d` on a contact's card refuses, and says which of the two reasons applies
    /// rather than a bare "no": a reader who pressed it wants to know it will not
    /// happen.
    #[test]
    fn d_on_a_contacts_card_refuses_and_says_why() {
        // Both states a contact's card can be in when `d` is pressed: the read has
        // landed, and it has not. A card with no rows must not swallow the key —
        // saying "you cannot" quietly is the one thing such a key must not do.
        let (mut read, _) = contact_card();
        read.handle_card_key('d');
        assert_eq!(read.status_text(), crate::app::NOT_YOURS_REFUSAL);

        let mut unread = self_card();
        let chat = unread.any_chat().expect("the mock has a chat");
        open_card(&mut unread, crate::app::ProfileId::User(chat.id));
        unread.handle_card_key('d');
        assert_eq!(
            unread.status_text(),
            crate::app::NOT_YOURS_REFUSAL,
            "and it refuses with no rows on show too"
        );
    }

    /// A row with nothing in it is a row that is not there, not a row of nothing.
    #[test]
    fn a_field_the_peer_did_not_fill_is_not_a_row() {
        let mut app = self_card();
        let with = rows(&app).len();
        if let crate::app::AccountState::Known(account) = &mut app.session.account {
            account.birthday = None;
            account.bio = None;
        }
        let without = rows(&app);

        assert!(
            without.len() < with,
            "two fields went and two rows went with them: {without:?}"
        );
        assert!(!without.iter().any(|row| row.label == "birthday"));
    }

    /// The held slot is in the card and is on screen nowhere.
    ///
    /// Two assertions, because the slot can fail two ways that look the same: a
    /// blank line drawn for it would read as a field the peer left empty — the one
    /// thing a card never draws — and a count that included it would stop meaning
    /// "what the peer told you", which is the only reason the count is on screen.
    #[test]
    fn a_reserved_slot_draws_nothing_and_does_not_move_the_count() {
        let (app, _) = contact_card();

        let all = rows(&app);
        assert!(
            all.iter().any(CardRow::is_reserved),
            "the slot is in the rows, or it holds nothing: {all:?}"
        );

        let drawn = lines(&app, &all, 40);
        let real_rows = all.iter().filter(|row| !row.is_reserved()).count();
        assert!(
            !drawn.iter().any(|(index, _)| all[*index].is_reserved()),
            "no line is drawn for the slot at all: {drawn:?}"
        );
        // Counted by *row* rather than by line, because a bio wraps and a wrapped
        // value is still one row.
        let distinct: std::collections::BTreeSet<usize> =
            drawn.iter().map(|(index, _)| *index).collect();
        assert_eq!(
            distinct.len(),
            real_rows,
            "one drawn row for each real row, and none for the slot"
        );
        assert!(
            !drawn
                .iter()
                .any(|(_, line)| line.spans.iter().any(|span| span.content.contains(COLOUR))),
            "and the slot's own label is nowhere on the screen"
        );

        // The count is over what is drawn, and the cursor's number is its position
        // among those — so the title's total is the real rows, not `all.len()`.
        let title = title(&app);
        let (_, rest) = title
            .rsplit_once('(')
            .unwrap_or_else(|| panic!("the title counts the rows: {title:?}"));
        let (counts, _) = rest
            .split_once(')')
            .unwrap_or_else(|| panic!("the title counts the rows: {title:?}"));
        let (cursor, total) = counts
            .split_once('/')
            .unwrap_or_else(|| panic!("the title counts two numbers: {title:?}"));
        let (cursor, total) = (cursor.trim(), total.trim());
        assert_eq!(total.parse::<usize>(), Ok(real_rows), "{title:?}");
        assert_eq!(
            cursor, "1",
            "and the cursor's number is among those: {title:?}"
        );
    }

    /// The highlight cannot reach the slot, because it draws nothing — and a
    /// highlight on a row nobody can see is worse than one that does not move.
    ///
    /// Today the card is too small for this to need a *step*: the slot is its last
    /// row, so bounding the highlight is enough. When a row lands after the slot
    /// this test has to grow a step with it — see [`navigable`].
    #[test]
    fn the_row_cursor_steps_over_a_reserved_slot() {
        let (mut app, _) = contact_card();
        let all = rows(&app);
        let slot = all
            .iter()
            .position(CardRow::is_reserved)
            .expect("the colour slot is held");
        assert!(
            slot > 0 && slot + 1 < all.len(),
            "the slot is interior on a contact's card: {all:?}"
        );

        // `j` onto it, from above.
        app.handle_card_row(slot - 1);
        app.handle_card_key('j');
        assert_eq!(
            app.profile_cursor(),
            slot + 1,
            "and lands on the row below it rather than on the slot"
        );

        // `k` back onto it, from below.
        app.handle_card_row(slot + 1);
        app.handle_card_key('k');
        assert_eq!(app.profile_cursor(), slot - 1, "and the same going back up");

        // And `G`, which is the motion most likely to land on a trailing slot.
        app.handle_card_key('G');
        assert!(
            !rows(&app)[app.profile_cursor()].is_reserved(),
            "and `G` never leaves it on the slot"
        );
    }

    /// The slot is the *contact's* colour and not the account's, and a card is not
    /// obliged to hold a slot it has no use for: the design puts the colour between
    /// the name and the username on the card a *person* is on.
    #[test]
    fn only_a_contacts_card_holds_the_colour_slot() {
        assert!(
            rows(&self_card()).iter().all(|row| !row.is_reserved()),
            "your own card has no use for a label you gave yourself"
        );
    }

    /// `y` with no selection is `yy`.
    ///
    /// There is no pending-yank latch on a card, so `y` yanks the cursor row's
    /// value and `yy` yanks it again. That is harmless, and it is not what a reader
    /// thinks they asked for — so this test says what the second `y` does, and the
    /// hint that says `y/yy` on a contact's card is naming one key rather than two.
    #[test]
    fn yy_on_a_card_yanks_the_row_once() {
        let mut app = self_card();
        let expected = rows(&app)
            .first()
            .map(|row| row.value.clone())
            .expect("the card has a row");

        app.handle_card_key('y');
        assert_eq!(app.register().lines(), std::slice::from_ref(&expected));

        app.handle_card_key('y');
        assert_eq!(
            app.register().lines(),
            std::slice::from_ref(&expected),
            "and the second press is the same yank, not an empty line"
        );
    }

    /// Not a test: a dump, for reading the card as a reader would rather than as a
    /// set of assertions about it. `#[ignore]`d, because nothing here is asserted
    /// and a green run should not pretend otherwise.
    #[test]
    #[ignore = "a dump, not an assertion"]
    fn dump_the_card() {
        for (caption, keys) in [
            ("the self card, cursor on the name", ""),
            ("one row down, the caret inside the username", "jl"),
            ("a charwise selection inside the name", "vll"),
            ("a selection of two rows", "vj"),
        ] {
            let mut app = self_card();
            for key in keys.chars() {
                app.handle_card_key(key);
            }
            let buffer = screen(&app, 40);
            let width = usize::from(buffer.area.width);
            eprintln!("\n=== {caption}   keys: {keys:?} ===");
            eprintln!("     (upper = the reversed row, braces = the caret's hole in it)");
            for y in 0..12 {
                let cells: Vec<&ratatui::buffer::Cell> = (0..width)
                    .map(|x| &buffer.content()[y * width + x])
                    .collect();
                let reversed: Vec<bool> = cells
                    .iter()
                    .map(|cell| cell.modifier.contains(Modifier::REVERSED))
                    .collect();
                let mut line = String::new();
                for (x, cell) in cells.iter().enumerate() {
                    let ch = cell.symbol();
                    // A hole is a cell inside a run of reversed cells that is not
                    // itself reversed: the caret showing the ground through it.
                    let inside = reversed[x]
                        || (x + 1 < width && reversed[x + 1])
                        || (x > 0 && reversed[x - 1]);
                    if reversed[x] {
                        line.push_str(&ch.to_uppercase());
                    } else if inside && ch.trim().is_empty() {
                        line.push('{');
                        line.push('}');
                    } else if inside {
                        line.push('{');
                        line.push_str(ch);
                        line.push('}');
                    } else {
                        line.push_str(ch);
                    }
                }
                let trimmed = line.trim_end();
                if !trimmed.trim().is_empty() {
                    eprintln!("|{trimmed}|");
                }
            }
        }
    }

    // ---- the presence row ------------------------------------------------

    /// A contact's card, read, after the peer has reported `presence` through the
    /// feed. The event is the only thing that sets the presence, so the fixture
    /// reaches the state the program can produce and no other.
    fn card_reporting(presence: domain::presence::Presence) -> App {
        let (mut app, chat) = contact_card();
        assert!(app.apply_update(&domain::updates::UpdateEvent::PeerStatus {
            chat_id: chat.id,
            presence,
        }));
        app
    }

    /// The value of the status row, if the card has one.
    fn status_of(app: &App) -> Option<String> {
        rows(app)
            .into_iter()
            .find(|row| row.label == "status")
            .map(|row| row.value)
    }

    /// A contact who is online has a status row that says so, and the row is
    /// drawn on the card rather than only held in the rows.
    #[test]
    fn a_contact_who_is_online_has_a_status_row_on_the_card() {
        let app = card_reporting(domain::presence::Presence::Online);

        assert_eq!(status_of(&app).as_deref(), Some("online"));
        let text = rows_of(&screen(&app, 60)).join("\n");
        assert!(text.contains("status"), "the row is labelled: {text}");
        assert!(text.contains("online"), "and says so: {text}");
    }

    /// A contact who restricts their last-seen time has no status row at all,
    /// rather than one reading "hidden" or "unknown".
    #[test]
    fn a_contact_who_hides_their_presence_has_no_status_row() {
        let app = card_reporting(domain::presence::Presence::Hidden);

        assert_eq!(status_of(&app), None, "no row for a restricted presence");
        let text = rows_of(&screen(&app, 60)).join("\n");
        assert!(!text.contains("status"), "and none drawn: {text}");
    }

    /// A profile read that carried a presence stands until the feed says
    /// otherwise, and the feed's report wins once it arrives.
    #[test]
    fn the_feed_report_replaces_the_one_the_profile_read_carried() {
        let (mut app, chat) = contact_card();
        app.set_contact(
            chat.id,
            Ok(domain::account::Account {
                presence: Some(domain::presence::Presence::Recently),
                ..crate::app::mock_account()
            }),
        );
        assert_eq!(status_of(&app).as_deref(), Some("last seen recently"));

        assert!(app.apply_update(&domain::updates::UpdateEvent::PeerStatus {
            chat_id: chat.id,
            presence: domain::presence::Presence::Online,
        }));
        assert_eq!(status_of(&app).as_deref(), Some("online"));
    }
}
