//! One owner for the panel's geometry.
//!
//! How tall a message is, which rows the panel shows, and how wide the
//! decorations on a message's first and last rows are. The panel asks for the
//! rows and draws them; it measures nothing itself, because a second
//! measurement is a second thing to disagree with the first — and the panel's
//! slice and the scrollbar beside it are exactly the two things that must not.
//!
//! The invariants, which the tests here and in [`crate::app`] assert:
//!
//! 1. every [`RowSpan::len`] is at least one, so a message is never zero rows
//!    tall and a row's end is a row's start;
//! 2. the spans are in window order and do not overlap, so [`RowSpan::first`]
//!    increases strictly;
//! 3. a slice is the panel's height exactly, or the whole of what is left of
//!    the window, and never something between the two;
//! 4. the layout is a pure function of the window's messages, the open draft and
//!    the panel's width — not of the cursor, not of the mode, not of when it was
//!    asked;
//! 5. the width is the one left after the scrollbar's column is given up, so a
//!    message is never written under the bar;
//! 6. a row that is not a message — [`RowKind::Other`], a day separator or the
//!    unread marker that will follow it — is counted as a row and is never a
//!    place the cursor stands, so every position the cursor or a motion is
//!    given is a message index and a message index is what comes back. The one
//!    exception to the count is [`RowKind::Draft`], which is drawn after the
//!    last message and is in no total, slice or paging input, so a draft being
//!    typed moves nothing. Its rows are taken from the panel's budget below the
//!    messages ([`Reserved::draft`]), so the messages never fill them.

use std::borrow::Cow;
use std::ops::Range;

use domain::history::ConversationWindow;
use domain::message::{MediaKind, Message, MessageStatus};
use unicode_width::UnicodeWidthChar;

use crate::app::App;
use crate::date;
use crate::sticker::{STICKER_FIT_ROWS, StickerCache};
use crate::wrap::{columns, wrap_decorated};

/// The columns a message's sender is named in: `[you] ` or `[them] `.
///
/// Named on the first row of a message and on no other, so this is a constant
/// of the panel rather than something a message decides. A message that continues
/// its group is given the same seven columns of blank, so every message's text
/// begins in the same column whichever group it is in.
///
/// Seven, because the bracket, the name and the space that follows it are all
/// drawn. It was six, which is one narrower than what `message_row` puts on the
/// row: the first row was then laid out a column wider than the panel and its
/// last character was clipped by the terminal rather than wrapped to the next row.
/// `a_whole_message_is_as_wide_as_its_own_decorations` is the arithmetic that
/// pins it.
pub(crate) const WHO_WIDTH: usize = 7;

/// The columns a draft row's sender is named in: `[you|draft] `.
///
/// Wider than [`WHO_WIDTH`] because a draft says it is a draft. The layout takes
/// it off the first row's width the same way a message's decorations are taken
/// off, so the draft's body is cut to fit beside its name.
pub(crate) const DRAFT_WHO_WIDTH: usize = 12;

/// How far apart two messages of one group can be, in seconds.
///
/// Five minutes: long enough that a burst of quick replies reads as one turn of
/// the conversation, short enough that a pause opens a new one.
const GROUP_MIN: i64 = 300;

/// How much of a failed send's reason is quoted at the end of its last row.
///
/// A reason is longer than a row has room for and the whole of it is on the
/// status line while the cursor is on the message; this is only enough to say
/// that there is one.
const FAILED_REASON_WIDTH: usize = 24;

/// The month names, January first.
///
/// The same table [`crate::date`] holds, reached for only by the fallback in
/// [`separator_label`] — and `card.rs` keeps a third of them. They are names
/// rather than arithmetic, and the arithmetic that turns a timestamp into one of
/// these lives in one place.
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// What one entry in the layout is.
///
/// The discriminant that lets a row which is not a message sit between two that
/// are. A separator has to be in the layout — it occupies the screen and the
/// scrollbar counts what is on the screen — but it names no message, so it must
/// not be reachable by anything that means "the message here".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    /// A message in the window, at this position in it.
    ///
    /// Carries the window position rather than leaving it to a field of its own,
    /// because a row that is not a message has none to leave it in — and because
    /// this position is what the cursor counts in and what a move by rows has to
    /// answer with.
    Message { index: usize },

    /// A row that is not a message: `mi: -1`, the way a `Loading…` row is.
    ///
    /// Carries what the row says, because the panel draws the row the layout
    /// gives it and the layout is the one answer this panel measures from: a
    /// second derivation of "which day is this row?" would be a second thing to
    /// disagree with the first.
    ///
    /// Exactly one row, so it can never be half-drawn by a slice that starts
    /// inside it, and never the cursor's row, because a cursor names a message.
    Other { label: String },

    /// The open conversation's draft, drawn as the row after the last message.
    ///
    /// Carries nothing: the words are the input line's, and the span's `text`
    /// is the range of them that the row covers. It takes as many rows as the
    /// draft wraps to, and at most one such span exists, last in the layout.
    /// Unlike [`RowKind::Other`] it is not one row: a draft wraps like a
    /// message does. Unlike a message it is in no count — [`total_rows`] stops
    /// at the last message, so the slice, the fetch margins and the paging
    /// clamps never see it. The panel reserves room for it below the messages
    /// ([`Reserved::draft`]), capped so that at least one message row is left.
    Draft,
}

impl RowKind {
    /// Whether this entry is a message a cursor can stand on.
    #[must_use]
    pub fn is_message(&self) -> bool {
        matches!(self, Self::Message { .. })
    }

    /// Where the message this entry is sits in the window, if it is one.
    #[must_use]
    pub fn index(&self) -> Option<usize> {
        match self {
            Self::Message { index } => Some(*index),
            Self::Other { .. } | Self::Draft => None,
        }
    }

    /// What a row that is not a message says, if it says anything.
    #[must_use]
    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Message { .. } | Self::Draft => None,
            Self::Other { label } => Some(label),
        }
    }
}

/// Where one entry's rows are in the panel.
///
/// Named by [`RowSpan::message_id`] because an index means a different message
/// once a page lands — which is why a position the reader has to come back to
/// is carried as an identifier, and why a layout worked out before a page
/// arrives is thrown away rather than kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowSpan {
    /// A message, or a row that names none.
    pub kind: RowKind,

    /// The message, named rather than indexed, when this entry is one.
    ///
    /// `None` for a row that is not a message, and no identifier is invented
    /// for it: an invented one would be indistinguishable from a real message's,
    /// which is the whole of what `mi: -1` is for.
    pub message_id: Option<i64>,

    /// The first row it occupies, counted from the top of the laid-out window.
    pub first: usize,

    /// How many rows it occupies. Always at least one.
    pub len: usize,

    /// The whole of the message's text, in the units
    /// [`crate::wrap`] works in: byte offsets at character boundaries. The rows
    /// the text is cut into are worked out again from the width, so what is
    /// kept here is the range a selection or a yank would name. For a
    /// [`RowKind::Draft`], the range of the input line's text. Empty for a row
    /// that is not a message.
    pub text: Range<usize>,
}

/// The rows the panel draws, and where the cursor's row lands among them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slice {
    /// The first message drawn, as its position in the window.
    ///
    /// A window position rather than a place in the layout, which is what makes
    /// it usable at all once a row that is not a message is in the layout: the
    /// two only ever coincided while every row was a message.
    pub start: usize,

    /// The rows of it that are above the top of the panel, which is what a
    /// slice starting inside a taller message skips.
    pub skip: usize,

    /// The first row drawn, counted from the top of the whole layout. What the
    /// scrollbar shows, because a bar that counted messages would stop
    /// describing the rows beside it.
    pub start_row: usize,

    /// The rows the panel has room for.
    pub budget: usize,

    /// The rows the whole window occupies, laid out. A draft's rows are not in it.
    pub total: usize,

    /// The message rows drawn: the panel's height, or what is left of the
    /// window when it is shorter than the panel. A message that does not fit in
    /// what is left is not drawn at all rather than half-drawn.
    pub rows: usize,

    /// The cursor's row among the rows drawn. The rows a fetch is announced on
    /// are not counted, and neither is a row that names no message, because none
    /// of them is a place the cursor can stand.
    pub selection: usize,
}

/// The rows the panel spends on what is not a message: what it is fetching, and
/// the open draft, which is drawn below the messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Reserved {
    /// A page is on its way from in front of the window.
    pub older: bool,

    /// A jump the reader asked for is on its way.
    pub jumping: bool,

    /// A page is on its way from behind the window.
    pub newer: bool,

    /// The rows the open draft is drawn in, after the messages. Zero unless the
    /// reader is following the newest message, since the draft is not drawn
    /// while scrolled up.
    ///
    /// Taken from the panel before the messages are given their budget, because a
    /// slice in follow mode fills every row it is given and the draft has to be
    /// drawn in one of them. Counted by no total: it only decides how many rows
    /// the messages may fill.
    pub draft: usize,
}

impl Reserved {
    /// The rows above the messages.
    #[must_use]
    pub fn above(self) -> usize {
        usize::from(self.older) + usize::from(self.jumping)
    }

    /// The rows below the messages: the draft's, then a page on its way from
    /// behind the window.
    #[must_use]
    pub fn below(self) -> usize {
        usize::from(self.newer) + self.draft
    }
}

/// What a group has earned about its own delivery.
///
/// Said once, on the row that ends the group, and derived rather than stored: the
/// window holds a read *watermark* ([`ConversationView::read_watermark`]) and this
/// is what it means for the message the group ends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Receipt {
    /// Nothing to say. An incoming group has no receipt of its own, a group with
    /// a send on its way or one that failed says that instead, and a group the
    /// peer has said nothing at all about is not a claim that it is unread.
    None,

    /// The server has the group's newest message and the peer has not said it was
    /// read.
    Delivered,

    /// The peer has read the group's newest message.
    Read,
}

/// Where a message stands in its group.
///
/// The sender is named once per group and the time shown once per group, so every
/// message has to know whether it is the one that opens its group, the one that
/// closes it, or neither. What is in between is the ordinary case: it names
/// nobody and shows no time. The receipt is the group's rather than the message's
/// and is drawn only where the group ends, so it is the same for all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grouped {
    /// This message begins the group: it names the sender and quotes its target.
    pub first: bool,

    /// This message ends the group: it carries the group's time and its receipt.
    pub last: bool,

    /// What the group has earned about its own delivery, and what is drawn on
    /// this row.
    ///
    /// `Receipt::None` on every message but the one that ends the group, because
    /// that is the only row a receipt is drawn on — the same once-per-group rule
    /// the sender's name and the time follow.
    pub receipt: Receipt,
}

impl Grouped {
    /// A message that is a group of its own: the oldest on show, or one standing
    /// apart from both its neighbours.
    #[must_use]
    pub fn alone() -> Self {
        Self {
            first: true,
            last: true,
            receipt: Receipt::None,
        }
    }
}

/// Whether `current` continues the group `previous` belongs to.
///
/// The four rules, and all four together:
///
/// - **same side.** Direction is the sender in a private conversation: there is
///   no other. A change of direction is a change of who is talking.
/// - **the same day**, because a group is one turn of one conversation and a day
///   is where a reader expects one to start and end.
/// - **within [`GROUP_MIN`] of the message before it**, not of the group's first,
///   so a long run of quick messages is one group however long it is.
/// - **not a reply.** A reply is a message about another message and reads as
///   its own turn whatever else is around it.
///
/// A message with no time never groups: `timestamp <= 0` is a send that has not
/// been acknowledged yet, and it has no day and no gap to measure, so it stands
/// alone rather than being folded into a neighbour it cannot be measured
/// against. A timed message after one is in the same position — the previous
/// message is the one it would be measured against — and starts its own group.
#[must_use]
pub fn continues(previous: &Message, current: &Message) -> bool {
    previous.is_outgoing == current.is_outgoing
        && current.reply_to.is_none()
        && date::has_time(previous.timestamp)
        && date::has_time(current.timestamp)
        && date::day_key(previous.timestamp) == date::day_key(current.timestamp)
        && current.timestamp.saturating_sub(previous.timestamp) <= GROUP_MIN
}

/// Where the message at `index` stands in its group.
///
/// Two neighbours, so it is the group's edges that are worked out rather than the
/// whole run of messages: a group of fifty is still one comparison either side
/// of the message being drawn.
///
/// A pure function of the window and the conversation's read watermark, like the
/// layout that carries these rows: not of the cursor, not of the mode, not of
/// when it was asked. The receipt is worked out for the message that ends the
/// group only, because that is the only row it is drawn on.
#[must_use]
pub fn group_of(app: &App, index: usize) -> Grouped {
    let window = &app.conversation.conversation.window;
    let Some(current) = window.get(index) else {
        return Grouped::alone();
    };
    let last = window
        .get(index + 1)
        .is_none_or(|next| !continues(current, next));

    Grouped {
        first: index == 0
            || window
                .get(index - 1)
                .is_none_or(|previous| !continues(previous, current)),
        last,
        receipt: if last {
            receipt_of(app, index)
        } else {
            Receipt::None
        },
    }
}

/// What the group ending at `index` has earned about its own delivery.
///
/// Derived, never stored, and read off the message the group ends on — because a
/// group shows its newest message's state and not each message's (AC-12). Four
/// answers, in the order they rule each other out:
///
/// - an **incoming** group has no receipt at all: the design excludes incoming
///   read state, and there is nothing here that would be honest to show;
/// - a group with a send **on its way or failed** shows that instead, and never a
///   receipt — a message that did not reach the server has earned nothing
///   (AC-14), and it says so for the whole group rather than beside itself;
/// - a group the peer has **said nothing about** shows nothing either: silence
///   is not a claim that a message is unread, and the receipt is best-effort by
///   nature;
/// - what is left is the server's acknowledgement of a message the peer has read,
///   which is arithmetic against the watermark the peer reported.
fn receipt_of(app: &App, index: usize) -> Receipt {
    let window = &app.conversation.conversation.window;
    let Some(newest) = window.get(index) else {
        return Receipt::None;
    };

    if !newest.is_outgoing
        || !matches!(newest.status, MessageStatus::Sent)
        || group_is_waiting(window, index)
    {
        return Receipt::None;
    }

    match app.conversation.conversation.read_watermark() {
        None => Receipt::None,
        Some(read) if newest.id <= read => Receipt::Read,
        Some(_) => Receipt::Delivered,
    }
}

/// Whether any message of the group ending at `index` is still on its way or has
/// failed.
///
/// The group's own run, walked backwards from the message it ends on, so a group
/// of fifty costs the same as a group of one and a window of two hundred costs one
/// pass rather than one per message.
fn group_is_waiting(window: &ConversationWindow, index: usize) -> bool {
    let mut at = index;

    loop {
        let Some(current) = window.get(at) else {
            return false;
        };
        if matches!(
            current.status,
            MessageStatus::Sending | MessageStatus::Failed
        ) {
            return true;
        }
        let Some(previous_at) = at.checked_sub(1) else {
            return false;
        };
        let Some(previous) = window.get(previous_at) else {
            return false;
        };
        if !continues(previous, current) {
            return false;
        }
        at = previous_at;
    }
}

/// The day a message falls on, or nothing at all for one that has no time.
///
/// [`crate::date::day_key`] under a name that says what it is for here: the
/// layout walks the window with this to notice where the day changed.
#[must_use]
pub fn day_of(timestamp: i64) -> Option<i64> {
    date::day_key(timestamp)
}

/// Whether a separator is due in front of a message of this day.
///
/// `after` is the day of the last message before it that *had* a time — a send
/// still on its way has no day of its own and must not open one, so it is skipped
/// rather than counted (T3). `None` before the first timed message, which is what
/// anchors the oldest loaded day.
///
/// One row, sitting exactly between the last message of the day before and the
/// first message of this one: a day boundary is a group break too, so this row and
/// that break are the same seam rather than two rows for it.
#[must_use]
pub fn opens_day(timestamp: i64, after: Option<i64>) -> bool {
    day_of(timestamp).is_some_and(|today| Some(today) != after)
}

/// What a separator says.
///
/// Relative where the reader's clock is known and absolute where it is not:
/// `now` is the unix second the host recorded ([`crate::app::App::record_now`]),
/// and nothing in this crate reads a clock. Without one, `Today` is a claim about
/// a clock this workspace does not have, so the label says the date instead —
/// which is a fact about the message rather than about when it is being read.
#[must_use]
pub fn separator_label(timestamp: i64, now: i64) -> Cow<'static, str> {
    if let Some(relative) = date::day_label(timestamp, now) {
        return relative;
    }

    let date = date::civil_from_timestamp(timestamp);
    let month = MONTHS
        .get(usize::try_from(date.month).expect("month of a civil date is 1-12") - 1)
        .expect("month names cover 1-12");

    Cow::Owned(format!("{month} {}, {}", date.day, date.year))
}

/// The rows one message occupies, as character ranges into its text.
///
/// The height of a message is this, and only this: the panel draws the rows and
/// [`crate::app::App::row_layout`] counts them, so a message is as tall here as
/// it is on the screen — which is why the caller passes the [`Grouped`] the rows
/// are drawn with, rather than this working it out and the panel working it out
/// again.
#[must_use]
pub fn message_rows(
    app: &App,
    message: &Message,
    grouped: Grouped,
    width: u16,
) -> Vec<Range<usize>> {
    // A decoded sticker has no text rows: the token the body names is what a
    // miss draws instead, and the picture's rows are counted below.
    if sticker_block_rows(message, &app.conversation.stickers) > 0 {
        return Vec::new();
    }

    let (prefix, suffix) = decoration_columns(app, message, grouped, width);
    wrap_decorated(message.display_body(), prefix, suffix, width)
}

/// How many rows a decoded sticker message paints its picture on: none without
/// bytes, the fit box's rows with them.
///
/// AFTER [`message_rows`]: [`RowSpan::len`] is the two added together (see
/// [`App::row_layout`]), while [`RowSpan::text`] stays message-level — a block
/// has no text range. The panel draws the block between the text rows and the
/// trailing note.
#[must_use]
pub fn sticker_block_rows(message: &Message, cache: &StickerCache) -> usize {
    match message.media {
        Some(MediaKind::Sticker) if message.text.is_empty() && cache.get(message.id).is_some() => {
            STICKER_FIT_ROWS as usize
        }
        _ => 0,
    }
}

/// The rows of the window the layout holds, which is every row but a draft's.
///
/// A draft is drawn after the last message and is not part of what the reader
/// is scrolling through, so the count stops at the last entry that is not one.
#[must_use]
pub fn total_rows(layout: &[RowSpan]) -> usize {
    layout
        .iter()
        .rev()
        .find(|span| span.kind != RowKind::Draft)
        .map_or(0, |span| span.first + span.len)
}

/// The rows a draft wraps to at `width`, as ranges into the draft's text.
///
/// The same wrap a message's body gets, with the draft's name in front of the
/// first row. Non-empty text is at least one row, so a draft that is there is
/// never zero rows tall.
#[must_use]
pub fn draft_rows(text: &str, width: u16) -> Vec<Range<usize>> {
    wrap_decorated(text, DRAFT_WHO_WIDTH, 0, width)
}

/// The message that owns `row`.
///
/// A move by rows lands in the middle of a message as often as not, and the
/// answer is the message rather than the row: a cursor stands on messages, and
/// a page down in a terminal puts the reader at the top of what it moved to.
///
/// A row that names no message is answered with the message below it rather than
/// with a position in the layout, which is what keeps "the message at this row"
/// an answer about a message. `None` only when the layout holds no message at or
/// after `row`.
#[must_use]
pub fn message_at_row(layout: &[RowSpan], row: usize) -> Option<usize> {
    layout
        .iter()
        .filter(|span| span.kind.is_message())
        .find(|span| span.first + span.len > row)
        .and_then(|span| span.kind.index())
}

/// The message a move that stops at `row` lands on.
///
/// `down` is the way the move was travelling, and it matters because a row that
/// names no message is not where a move comes to rest: a page down across a
/// separator lands on the message below it, and a page up lands on the message
/// above. Answering every such row downwards instead is what would leave
/// `Ctrl+u` standing still while the reader believes it moved.
///
/// Moving up past the first row of the layout lands on the first message there
/// is, which is what a reader paging to the top of a conversation expects and is
/// the other half of the same rule: no row that names no message is ever where a
/// move stops.
#[must_use]
pub fn message_at_row_moving(layout: &[RowSpan], row: usize, down: bool) -> Option<usize> {
    if down {
        return message_at_row(layout, row);
    }

    layout
        .iter()
        .rev()
        .find(|span| span.kind.is_message() && span.first <= row)
        .or_else(|| layout.iter().find(|span| span.kind.is_message()))
        .and_then(|span| span.kind.index())
}

/// The first row the message at window position `index` occupies.
///
/// The cursor counts messages and a layout entry is not a message, so every
/// cursor-to-row conversion goes through here rather than through indexing the
/// layout with the cursor — which is only the same thing while every entry is a
/// message.
#[must_use]
pub fn first_row_of_message(layout: &[RowSpan], index: usize) -> Option<usize> {
    layout
        .iter()
        .find(|span| span.kind.index() == Some(index))
        .map(|span| span.first)
}

/// The last message in the layout, as its position in the window.
///
/// What a slice starting past every message falls back on, so that
/// [`Slice::start`] is a window position whatever the slice began on.
fn last_message(layout: &[RowSpan]) -> Option<usize> {
    layout.iter().rev().find_map(|span| span.kind.index())
}

/// Which rows fill a panel of `budget` rows, for a cursor on `cursor`'s message.
///
/// `cursor` is a message index, and stays one whatever the layout holds: it is
/// the cursor that is over messages, never the layout that is over messages.
///
/// A cursor that is not pinned is centred by rows, which is the only measure
/// the screen has; a pinned one shows the end of the window. Either way the
/// slice starts at a row rather than at a message, so it fills the panel
/// exactly and the scrollbar beside it has something true to show.
#[must_use]
pub fn slice(layout: &[RowSpan], cursor: usize, budget: usize, follow: bool) -> Slice {
    let budget = budget.max(1);
    let total = total_rows(layout);
    let cursor_row = first_row_of_message(layout, cursor).unwrap_or(0);
    let target = if follow {
        total.saturating_sub(budget)
    } else {
        cursor_row.saturating_sub(budget / 2)
    };
    // Pulled back inside the window so the slice is as tall as the panel and
    // never starts past the end.
    let start_row = target.min(total.saturating_sub(budget));
    // A slice may begin on a row that names no message — a separator the
    // scrolled-up-to row is — and then the first message drawn is the one
    // below it, or the last there is if the layout ends on one of those rows.
    let start = message_at_row(layout, start_row)
        .or_else(|| last_message(layout))
        .unwrap_or(0);
    let skip =
        first_row_of_message(layout, start).map_or(0, |first| start_row.saturating_sub(first));

    Slice {
        start,
        skip,
        start_row,
        budget,
        total,
        rows: budget.min(total - start_row),
        selection: cursor_row.saturating_sub(start_row),
    }
}

/// The columns the decorations on a message's own rows take: what is drawn in
/// front of its first row, and behind its last.
///
/// Both are the panel's business rather than the layout's, and both are
/// subtracted from the width before the rows are cut rather than clipped after,
/// because a terminal clips without saying so.
fn decoration_columns(
    app: &App,
    message: &Message,
    grouped: Grouped,
    width: u16,
) -> (usize, usize) {
    let prefix = WHO_WIDTH
        + message
            .reply_to
            .map_or(0, |reply_to| columns(&reply_prefix(app, reply_to, width)));
    let suffix = trailing_note(app, message, grouped).map_or(0, |text| columns(&text));

    (prefix, suffix)
}

/// What stands behind a message's last row: what its own send is doing, what its
/// group has earned, and the time the group ended.
///
/// Three facts about three different subjects, drawn in that order — the message's
/// own state first, then the group's, then when it ended — and each one only
/// where it belongs:
///
/// - a status belongs to the **message** that carries it, on that message's own
///   last row, which is where Stage 03 put it and where it stays;
/// - the receipt belongs to the **group** and is drawn once, on the row that ends
///   it, never beside each message that shares it;
/// - the time belongs to the group too, and to the same row.
///
/// A send on its way or one that failed therefore never also shows a receipt: the
/// two are in the same string and only one of them is ever there (AC-14).
///
/// `None` when there is nothing to say, which is the ordinary case: an incoming
/// message from the middle of a group.
pub(crate) fn trailing_note(app: &App, message: &Message, grouped: Grouped) -> Option<String> {
    // The two-space gap is added here rather than inside any of the three, so a
    // message that says all of them reads as one list rather than as three
    // conventions.
    let mut note = String::new();

    if let Some(status) = status_suffix(app, message) {
        note.push_str("  ");
        note.push_str(&status);
    }
    if grouped.last {
        if let Some(receipt) = receipt_word(grouped.receipt) {
            note.push_str("  ");
            note.push_str(receipt);
        }
        if let Some(time) = date::clock(message.timestamp) {
            note.push_str("  ");
            note.push_str(&time);
        }
    }

    (!note.is_empty()).then_some(note)
}

/// The bracketed word a receipt is drawn as.
fn receipt_word(receipt: Receipt) -> Option<&'static str> {
    match receipt {
        Receipt::None => None,
        Receipt::Delivered => Some("[delivered]"),
        Receipt::Read => Some("[read]"),
    }
}

/// The target a reply quotes, as the prefix in front of the body it answers.
///
/// A share of what the first row has left once the sender is named, so the body
/// still has room: a prefix that filled the row would push the thing it is a
/// prefix to off it. A target the window does not hold says so rather than
/// leaving the reply unanchored, and one that is still on its way says that
/// instead of quoting a message that has not arrived.
pub(crate) fn reply_prefix(app: &App, reply_to: i64, width: u16) -> String {
    let text = match app.conversation.conversation.message(reply_to) {
        Some(message) if matches!(message.status, MessageStatus::Sending) => {
            "[sending…]".to_owned()
        }
        // The body, not the caption: a reply to a photo quotes "[image]", which
        // is what the reader is answering, rather than quoting nothing.
        Some(message) => message.display_body().to_owned(),
        None => "[message not loaded]".to_owned(),
    };

    // `> ` in front and ` ‖ ` behind, the brackets of the quoted text apart.
    let room = usize::from(width).saturating_sub(WHO_WIDTH + 5) / 2;
    format!("> {} ‖ ", truncate(&text, room.max(8)))
}

/// What a send is doing, for the end of its message's last row.
///
/// A fact about the whole message rather than about a row of it, so it is drawn
/// once, on the row with room for it. The gap in front of it belongs to
/// [`trailing_note`], which is what puts it there.
pub(crate) fn status_suffix(app: &App, message: &Message) -> Option<String> {
    match message.status {
        MessageStatus::Sending => Some("[sending…]".to_owned()),
        MessageStatus::Failed => {
            let reason = app
                .conversation
                .conversation
                .failure(message.id)
                .unwrap_or("failed");
            Some(format!(
                "[failed: {}]",
                truncate(reason, FAILED_REASON_WIDTH)
            ))
        }
        MessageStatus::Sent | MessageStatus::Received => None,
    }
}

/// Truncates `text` to `budget` columns, marking the cut.
///
/// Columns rather than characters, because a decoration is subtracted from a
/// row's width before the row is cut: a prefix that is fewer characters than the
/// budget but wider than it would push the body off the row, and one that is
/// more characters but narrower is harmless. Walked a character at a time, so a
/// cut is never inside one — including an emoji that straddles the budget, which
/// is left out whole rather than half drawn.
pub(crate) fn truncate(text: &str, budget: usize) -> String {
    if columns(text) <= budget {
        return text.to_owned();
    }

    let room = budget.saturating_sub(1);
    let mut used = 0;
    let mut shortened = String::new();

    for character in text.chars() {
        let width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + width > room {
            break;
        }
        used += width;
        shortened.push(character);
    }

    shortened.push('…');
    shortened
}

/// Where `chars` of `text` fall, in the units a row's range is in.
///
/// The one place two units meet. A mark's position is counted in characters,
/// because a motion that steps by one has to step by one thing the reader can
/// see; a row's range is counted in bytes, because that is what indexes a string.
/// Everything downstream of here is arithmetic on byte offsets.
///
/// A position past the end of the text clamps to the end of it rather than
/// panicking or wrapping, which is what a motion that ran off the end of a
/// message should do.
pub(crate) fn byte_span(text: &str, chars: Range<usize>) -> Range<usize> {
    let mut span = text.len()..text.len();

    for (index, offset) in text
        .char_indices()
        .map(|(offset, _)| offset)
        .chain(std::iter::once(text.len()))
        .enumerate()
    {
        if index == chars.start {
            span.start = offset;
        }
        if index == chars.end {
            span.end = offset;
            break;
        }
    }

    span
}

/// The part of `row` that `selected` covers, as offsets into `row`.
///
/// Both are ranges into the same string, so the answer is arithmetic. That is the
/// whole reason the panel wraps by byte range: a selected substring is then three
/// slices rather than a text-layout problem.
///
/// A selection that misses the row entirely comes back empty — `start` past
/// `end` — which the caller reads as "this row is not covered" rather than as a
/// range to slice with.
pub(crate) fn clip(selected: &Range<usize>, row: &Range<usize>) -> (usize, usize) {
    let start = selected.start.max(row.start) - row.start;

    (start, selected.end.min(row.end).saturating_sub(row.start))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rows messages with these texts occupy, in window order.
    fn layout_of(texts: &[&str], width: u16) -> Vec<RowSpan> {
        let app = App::mock();
        let mut spans = Vec::new();
        let mut first = 0;

        for (index, text) in texts.iter().enumerate() {
            let message = Message {
                id: i64::try_from(index).expect("a test's index is an identifier"),
                chat_id: 1,
                text: (*text).to_owned().into(),
                timestamp: 0,
                status: MessageStatus::Received,
                is_outgoing: false,
                reply_to: None,
                media: None,
            };
            let len = message_rows(&app, &message, Grouped::alone(), width).len();
            spans.push(RowSpan {
                kind: RowKind::Message { index },
                message_id: Some(message.id),
                first,
                len,
                text: 0..text.len(),
            });
            first += len;
        }

        spans
    }

    /// The same, with a row that names no message put in front of the message at
    /// `before`, and everything below it moved down to make room for it.
    ///
    /// The shape a day separator gives the layout, built here because nothing in
    /// the program emits one yet.
    fn layout_with_other_row(texts: &[&str], before: usize, width: u16) -> Vec<RowSpan> {
        let mut spans = layout_of(texts, width);
        let first = spans
            .get(before)
            .expect("the fixture has a message in front of the row")
            .first;

        spans.insert(
            before,
            RowSpan {
                kind: RowKind::Other {
                    label: "── test ──".to_owned(),
                },
                message_id: None,
                first,
                len: 1,
                text: 0..0,
            },
        );
        for span in &mut spans[before + 1..] {
            span.first += 1;
        }

        spans
    }

    fn lens(spans: &[RowSpan]) -> Vec<usize> {
        spans.iter().map(|span| span.len).collect()
    }

    fn firsts(spans: &[RowSpan]) -> Vec<usize> {
        spans.iter().map(|span| span.first).collect()
    }

    // ---- grouping --------------------------------------------------------

    /// A moment in the middle of a day, which is where a message has to be to be
    /// grouped at all: 2024-11-14 22:13:20 UTC.
    const AT: i64 = 1_730_000_000;

    fn at(id: i64, timestamp: i64, outgoing: bool) -> Message {
        Message {
            id,
            chat_id: 1,
            text: "text".into(),
            timestamp,
            status: MessageStatus::Received,
            is_outgoing: outgoing,
            reply_to: None,
            media: None,
        }
    }

    /// An application holding exactly these messages, oldest first.
    fn holding(messages: Vec<Message>) -> App {
        let mut app = App::mock();
        app.conversation.conversation.window.replace(messages);
        app
    }

    /// Where each message stands in its group, in window order.
    fn places(app: &App) -> Vec<Grouped> {
        (0..app.conversation.conversation.window.len())
            .map(|index| group_of(app, index))
            .collect()
    }

    const OPEN: Grouped = Grouped {
        first: true,
        last: false,
        receipt: Receipt::None,
    };
    const WITHIN: Grouped = Grouped {
        first: false,
        last: false,
        receipt: Receipt::None,
    };
    const CLOSES: Grouped = Grouped {
        first: false,
        last: true,
        receipt: Receipt::None,
    };
    const ALONE: Grouped = Grouped {
        first: true,
        last: true,
        receipt: Receipt::None,
    };

    /// Same side, same day, a minute apart: one group, named once and dated once.
    #[test]
    fn same_side_within_five_minutes_is_one_group() {
        let app = holding(vec![
            at(1, AT, false),
            at(2, AT + 60, false),
            at(3, AT + 120, false),
        ]);

        assert_eq!(places(&app), vec![OPEN, WITHIN, CLOSES]);
    }

    /// The gap is to the message before it, not to the group's first, so a long
    /// run of quick messages is one group however long the run is.
    #[test]
    fn the_gap_is_to_the_message_before_and_five_minutes_is_the_window() {
        let together = holding(vec![at(1, AT, false), at(2, AT + 299, false)]);
        assert_eq!(
            places(&together),
            vec![OPEN, CLOSES],
            "299 seconds is inside"
        );

        let apart = holding(vec![at(1, AT, false), at(2, AT + 301, false)]);
        assert_eq!(places(&apart), vec![ALONE, ALONE], "301 is outside");
    }

    #[test]
    fn a_change_of_direction_starts_a_group() {
        let app = holding(vec![
            at(1, AT, false),
            at(2, AT + 60, true),
            at(3, AT + 120, false),
        ]);

        assert_eq!(places(&app), vec![ALONE, ALONE, ALONE]);
    }

    // ---- day separators --------------------------------------------------

    /// The rows the separators of a window's layout sit on, in order.
    fn separators(app: &App) -> Vec<(usize, String)> {
        app.row_layout()
            .iter()
            .filter_map(|span| Some((span.first, span.kind.label()?.to_owned())))
            .collect()
    }

    /// One row before the first message of each day, sitting exactly between the
    /// last message of the day before and the first of this one — and one for the
    /// oldest loaded day, which has no day before it in the window to sit after.
    #[test]
    fn a_separator_precedes_the_first_message_of_each_day() {
        let app = holding(vec![
            at(1, AT, false),
            at(2, AT + 60, false),
            at(3, AT + 86_400, false),
            at(4, AT + 86_460, false),
        ]);
        let layout = app.row_layout();

        assert_eq!(
            separators(&app),
            vec![
                (0, separator_label(AT, 0).into_owned()),
                (3, separator_label(AT + 86_400, 0).into_owned()),
            ],
            "one separator per day, in order"
        );
        assert_eq!(firsts(&layout), vec![0, 1, 2, 3, 4, 5]);
        for (row, _) in separators(&app) {
            assert_eq!(layout[row].len, 1, "a separator is one row");
            assert_eq!(layout[row].message_id, None, "and names no message");
            assert_eq!(
                layout[row].first + 1,
                layout[row + 1].first,
                "row {row} sits immediately above a message"
            );
            let first_of_day = layout[row + 1].kind.index().expect("a message below it");

            if row == 0 {
                assert_eq!(
                    first_of_day, 0,
                    "and the oldest day is anchored above the first message"
                );
                continue;
            }

            let last_of_yesterday = layout[row - 1].kind.index().expect("a message above it");
            assert_ne!(
                day_of(stamped(&app, last_of_yesterday)),
                day_of(stamped(&app, first_of_day)),
                "the two messages either side of row {row} are of different days"
            );
            assert_eq!(
                layout[row - 1].first + layout[row - 1].len,
                row,
                "and it is the row immediately after the last message of the day before"
            );
        }
    }

    /// The timestamp of the message at window position `index`.
    fn stamped(app: &App, index: usize) -> i64 {
        app.conversation
            .conversation
            .window
            .get(index)
            .expect("the window holds the message")
            .timestamp
    }

    /// A send still on its way has no day, so it neither opens one nor hides the
    /// next message's: the day it is in is the one it sits between.
    #[test]
    fn a_message_with_no_time_does_not_open_a_day() {
        let mut pending = at(2, 0, false);
        pending.timestamp = 0;
        let app = holding(vec![at(1, AT, false), pending, at(3, AT + 86_400, false)]);

        assert_eq!(
            separators(&app).len(),
            2,
            "the two days, and not a third for the message with no time: {:?}",
            separators(&app)
        );
        assert_eq!(
            app.row_layout()
                .iter()
                .find(|span| span.kind.index() == Some(1))
                .map(|span| span.kind.is_message()),
            Some(true),
            "and the message with no time is still a message row"
        );
    }

    /// What a separator says depends on when it is being read, so the label is
    /// the one thing in the layout that is not a fact about the window alone.
    #[test]
    fn a_separator_label_is_relative_where_the_clock_is_known_and_absolute_where_it_is_not() {
        let now = AT;

        assert_eq!(separator_label(now, now), "Today");
        assert_eq!(separator_label(now - 86_400, now), "Yesterday");
        assert_eq!(
            separator_label(now, 0),
            "Oct 27, 2024",
            "and with no clock recorded it says the date rather than `Today`"
        );
    }

    /// The same window answers the same way twice: a separator is not cached, it is
    /// worked out, so re-rendering and reloading cannot disagree.
    #[test]
    fn the_same_window_lays_out_the_same_separators_twice_over() {
        let messages = vec![at(1, AT, false), at(2, AT + 86_400, false)];
        let reloaded = holding(messages.clone());

        assert_eq!(holding(messages).row_layout(), reloaded.row_layout());
    }

    /// Landing on the first message of a day — `gg`, a search, any move that puts
    /// the cursor there — shows the separator above it, because the slice is
    /// centred by rows and the separator is the row above the cursor's.
    #[test]
    fn the_separator_above_a_days_first_message_is_in_the_slice_it_lands_in() {
        let app = holding(vec![
            at(1, AT, false),
            at(2, AT + 86_400, false),
            at(3, AT + 172_800, false),
        ]);
        let layout = app.row_layout();

        for (row, _) in separators(&app) {
            let index = layout[row + 1]
                .kind
                .index()
                .expect("a day begins at a message");
            for budget in 2..12 {
                let view = slice(&layout, index, budget, false);
                let on_screen = view.start_row <= row && row < view.start_row + view.rows;

                assert!(
                    on_screen,
                    "the separator at row {row} is off a {budget}-row slice of message {index}"
                );
            }
        }
    }

    /// A group is one turn of one conversation, so it does not cross midnight.
    /// The break is on the day rather than on the gap, which is why 23:59 and
    /// 00:01 are two groups an hour apart.
    #[test]
    fn a_day_boundary_starts_a_group() {
        let midnight = date::day_key(AT + 86_400).expect("a day key");
        let last_of_the_day = midnight * 86_400 - 1;
        let app = holding(vec![
            at(1, last_of_the_day, false),
            at(2, last_of_the_day + 1, false),
        ]);

        assert_eq!(places(&app), vec![ALONE, ALONE]);
    }

    /// A reply is a message about another message and reads as its own turn; a
    /// message after it may still follow it in, which is the only way a reply's
    /// group is ever more than one message long.
    #[test]
    fn a_reply_starts_a_group_and_can_be_followed_into() {
        let mut reply = at(2, AT + 60, false);
        reply.reply_to = Some(1);
        let app = holding(vec![
            at(1, AT, false),
            reply.clone(),
            at(3, AT + 120, false),
        ]);

        assert_eq!(
            places(&app),
            vec![ALONE, OPEN, CLOSES],
            "the reply opens its group and the message after it follows in"
        );

        let reply_last = holding(vec![reply.clone(), at(3, AT + 120, false)]);
        assert_eq!(
            places(&reply_last),
            vec![OPEN, CLOSES],
            "and a reply at the head of the window is the group"
        );
    }

    /// A send that has not been acknowledged yet has no time, so it has no day
    /// and no gap to be measured against: it stands alone rather than being
    /// folded into a neighbour it cannot be compared with.
    #[test]
    fn a_message_with_no_time_is_a_group_of_its_own() {
        let app = holding(vec![
            at(1, AT, false),
            at(2, 0, false),
            at(3, AT + 60, false),
        ]);

        assert_eq!(
            places(&app),
            vec![ALONE, ALONE, ALONE],
            "the messages either side of one with no time are two groups of their own"
        );

        let oldest = holding(vec![at(1, 0, false), at(2, AT, false)]);
        assert_eq!(
            places(&oldest),
            vec![ALONE, ALONE],
            "and the one after it has nothing to be measured against"
        );
    }

    /// Every group's end is the next group's beginning, which is what makes the
    /// time appear exactly once however the messages fall.
    #[test]
    fn a_group_ends_where_the_next_one_begins() {
        let mut reply = at(4, AT + 120, true);
        reply.reply_to = Some(3);
        let app = holding(vec![
            at(1, AT, false),
            at(2, AT + 60, false),
            at(3, AT + 1_000, false),
            reply,
        ]);
        let places = places(&app);

        assert_eq!(
            places[0].last, places[1].first,
            "a group ends where the next one begins"
        );
        assert_eq!(places[2].last, places[3].first);
        assert!(places.last().is_some_and(|group| group.last));
    }

    /// Grouping is a function of the window, like the layout that carries these
    /// rows: not of the cursor, not of the mode, not of when it was asked.
    #[test]
    fn the_groups_do_not_depend_on_where_the_cursor_is() {
        let mut app = holding(vec![
            at(1, AT, false),
            at(2, AT + 60, false),
            at(3, AT + 3_600, false),
        ]);

        let before = places(&app);
        app.conversation.vim.set_cursor(0);
        app.conversation.vim.set_cursor(2);

        assert_eq!(
            before,
            places(&app),
            "the cursor is not an input to grouping"
        );
    }

    // ---- what a group has earned ----------------------------------------

    /// One of the reader's own messages as the server left it: numbered, and
    /// accepted, which is the only state a receipt can be about.
    fn mine(id: i64, seconds: i64) -> Message {
        Message {
            status: MessageStatus::Sent,
            is_outgoing: true,
            ..at(id, seconds, true)
        }
    }

    /// A two-message group of the reader's own, and a watermark of `read`.
    fn my_group(read: Option<i64>) -> App {
        let mut app = holding(vec![mine(1, AT), mine(2, AT + 60)]);
        if let Some(read) = read {
            app.conversation.conversation.set_read_watermark(read);
        }

        app
    }

    /// The receipt the group ending at its newest message draws.
    fn receipt_of_group(app: &App) -> Receipt {
        group_of(app, 1).receipt
    }

    /// A receipt is arithmetic against the peer's watermark and nothing else: a
    /// message the server numbered is delivered until the peer says it has read
    /// it, and read after that.
    #[test]
    fn an_acknowledged_outgoing_group_shows_delivered_until_the_watermark_covers_it() {
        assert_eq!(
            receipt_of_group(&my_group(None)),
            Receipt::None,
            "the peer has said nothing, which is not a claim that a message is unread"
        );
        assert_eq!(
            receipt_of_group(&my_group(Some(1))),
            Receipt::Delivered,
            "a read up to the first message leaves the second one delivered"
        );
        assert_eq!(
            receipt_of_group(&my_group(Some(2))),
            Receipt::Read,
            "and a read that covers it is a read"
        );
        assert_eq!(
            receipt_of_group(&my_group(Some(9))),
            Receipt::Read,
            "whatever else was read along with it"
        );
    }

    /// A group shows its newest message's state rather than each message's: two
    /// messages the peer has read up to the first of them is one group that has
    /// been read up to the first of them, and what it says is about the second.
    #[test]
    fn a_group_shows_its_newest_message_s_state_and_not_each_messages() {
        let app = my_group(Some(1));

        assert_eq!(
            group_of(&app, 0).receipt,
            Receipt::None,
            "and not on any other row"
        );
        assert_eq!(
            receipt_of_group(&app),
            Receipt::Delivered,
            "the group's state is its newest message's"
        );
    }

    /// Incoming messages have no receipt of their own. There is nothing here that
    /// would be honest to draw, and the design excludes it.
    #[test]
    fn an_incoming_group_is_never_given_a_receipt() {
        let mut app = holding(vec![at(1, AT, false), at(2, AT + 60, false)]);
        app.conversation.conversation.set_read_watermark(99);

        assert_eq!(receipt_of_group(&app), Receipt::None);
    }

    /// A send that is on its way or has failed says what it is doing instead of a
    /// receipt, and it says so for the whole group rather than beside itself.
    ///
    /// The fixture is a failed send *inside* a group, which today's placeholders
    /// cannot be — they carry no time, so they stand alone (`T3`) — and which is
    /// exactly why the rule has to be in the model rather than in a rendering that
    /// never meets the case.
    #[test]
    fn a_send_on_its_way_or_failed_takes_the_receipt_away_from_its_group() {
        let mut failed = mine(2, AT + 60);
        failed.id = -1;
        failed.status = MessageStatus::Failed;
        let mut app = holding(vec![mine(1, AT), failed]);
        app.conversation.conversation.set_read_watermark(99);

        assert_eq!(
            receipt_of_group(&app),
            Receipt::None,
            "a failure earns nothing"
        );

        let mut waiting = mine(2, AT + 60);
        waiting.id = -1;
        waiting.status = MessageStatus::Sending;
        let mut sending = holding(vec![mine(1, AT), waiting]);
        sending.conversation.conversation.set_read_watermark(99);

        assert_eq!(
            receipt_of_group(&sending),
            Receipt::None,
            "and neither does a send that has not been acknowledged"
        );
    }

    /// A failed send is its own group today, and it keeps its reason; dismissing it
    /// takes it out of the window and the group it was beside earns what it earned
    /// (AC-14).
    #[test]
    fn a_dismissed_failure_leaves_the_group_it_was_beside_as_it_was() {
        let mut app = my_group(Some(2));
        let failed = app.conversation.conversation.queue_send("on its way", None);
        assert!(
            app.conversation
                .conversation
                .fail_send(failed, "no route".to_owned())
        );

        let placeholder = app.conversation.conversation.window.len() - 1;
        assert_eq!(group_of(&app, placeholder).receipt, Receipt::None);
        assert!(
            trailing_note(
                &app,
                app.conversation
                    .conversation
                    .window
                    .get(placeholder)
                    .expect("it is there"),
                group_of(&app, placeholder)
            )
            .is_some_and(|note| note.contains("[failed: no route]")),
            "and it says why, with no receipt beside it"
        );

        assert!(app.conversation.conversation.dismiss_failed(failed));
        assert_eq!(
            receipt_of_group(&app),
            Receipt::Read,
            "and the group it was beside is as it was"
        );
    }

    /// The note is what the panel draws, so the derivation above is only worth
    /// anything if it reaches it: the word, then the time, both on the row that
    /// ends the group, and never on any other.
    #[test]
    fn the_receipt_is_drawn_once_on_the_row_that_ends_the_group() {
        let read = my_group(Some(2));
        let message = read
            .conversation
            .conversation
            .window
            .get(1)
            .expect("the window holds it")
            .clone();
        let time = date::clock(message.timestamp).expect("the fixture has a time");

        assert_eq!(
            trailing_note(&read, &message, group_of(&read, 1)).as_deref(),
            Some(format!("  [read]  {time}").as_str()),
            "the word then the time, on the last row of the group"
        );
        assert_eq!(
            trailing_note(
                &read,
                &message,
                Grouped {
                    first: false,
                    last: false,
                    receipt: Receipt::None
                }
            ),
            None,
            "and on no other row of it"
        );

        let unread = my_group(Some(1));
        assert!(
            trailing_note(&unread, &message, group_of(&unread, 1))
                .is_some_and(|note| note.starts_with("  [delivered]")),
            "delivered before read"
        );

        let silent = my_group(None);
        assert_eq!(
            trailing_note(&silent, &message, group_of(&silent, 1)).as_deref(),
            Some(format!("  {time}").as_str()),
            "and nothing at all where the peer has said nothing"
        );
    }

    /// A group of one is the ordinary case for a conversation that alternates,
    /// and it must not cost a message its own sender's name.
    #[test]
    fn a_message_alone_in_the_window_is_its_own_group() {
        let app = holding(vec![at(1, AT, false)]);

        assert_eq!(places(&app), vec![ALONE]);
        assert_eq!(
            group_of(&App::new(), 0),
            ALONE,
            "and an empty window has none"
        );
    }

    // ---- the layout ------------------------------------------------------

    #[test]
    fn an_empty_window_occupies_no_rows() {
        let app = App::new();

        assert_eq!(app.row_layout(), vec![]);
        assert_eq!(total_rows(&app.row_layout()), 0);
    }

    #[test]
    fn a_short_message_is_one_row() {
        assert_eq!(lens(&layout_of(&["hi"], 40)), vec![1]);
    }

    #[test]
    fn a_long_message_is_as_many_rows_as_its_text_needs() {
        // Two columns short of the panel's width, once `[them] ` is named.
        let long = "x".repeat(100);
        let spans = layout_of(&[&long], 40);

        assert_eq!(lens(&spans), vec![3], "34 + 40 + 26");
    }

    #[test]
    fn mixed_messages_keep_window_order_and_do_not_overlap() {
        let spans = layout_of(&["hi", &"y".repeat(200), "short", ""], 40);

        assert_eq!(lens(&spans), vec![1, 6, 1, 1]);
        assert_eq!(firsts(&spans), vec![0, 1, 7, 8]);
        assert!(
            spans.windows(2).all(|pair| pair[0].first < pair[1].first),
            "`first` increases strictly"
        );
    }

    #[test]
    fn every_message_is_at_least_one_row() {
        for text in ["", " ", "hi", &"z".repeat(500)] {
            let spans = layout_of(&[text], 20);
            assert!(spans.iter().all(|span| span.len >= 1), "{text:?}");
        }
    }

    /// A message that carries a thing and says nothing about it.
    fn carrying(media: domain::message::MediaKind) -> Message {
        Message {
            id: 1,
            chat_id: 1,
            text: String::new().into(),
            timestamp: 0,
            status: MessageStatus::Received,
            is_outgoing: false,
            reply_to: None,
            media: Some(media),
        }
    }

    #[test]
    fn a_message_that_only_carries_something_is_one_row_of_its_placeholder() {
        let app = App::mock();
        let message = carrying(domain::message::MediaKind::Gif);

        let rows = message_rows(&app, &message, Grouped::alone(), 40);

        assert_eq!(rows.len(), 1, "a placeholder is a line, not a paragraph");
        assert_eq!(
            rows[0].end,
            message.display_body().len(),
            "and the range the panel paints indexes the very string it wraps"
        );
        assert_eq!(&message.display_body()[rows[0].clone()], "[gif]");
    }

    /// The soft-wrap rule, stated as a range: whatever the label is, the body is
    /// one row and that row indexes the whole of `display_body()` and nothing else.
    #[test]
    fn every_placeholder_is_one_row_spanning_the_whole_body() {
        for media in [
            domain::message::MediaKind::Photo,
            domain::message::MediaKind::Video,
            domain::message::MediaKind::Gif,
            domain::message::MediaKind::Voice,
            domain::message::MediaKind::File,
        ] {
            let app = App::mock();
            let message = carrying(media);

            let rows = message_rows(&app, &message, Grouped::alone(), 40);

            assert_eq!(
                rows,
                vec![0..message.display_body().len()],
                "{media:?} is one row of the very string the panel wraps"
            );
        }
    }

    /// A sticker with no bytes is a placeholder like any other: one text row,
    /// no block rows.
    #[test]
    fn a_sticker_without_bytes_is_one_text_row_and_no_block() {
        let app = App::mock();
        let message = carrying(domain::message::MediaKind::Sticker);

        assert_eq!(
            message_rows(&app, &message, Grouped::alone(), 40),
            vec![0..message.display_body().len()]
        );
        assert_eq!(sticker_block_rows(&message, &app.conversation.stickers), 0);
    }

    /// A decoded sticker has no text rows at all: the token the body names is
    /// what a miss draws instead, and the picture takes the fit box's rows.
    #[test]
    fn a_decoded_sticker_is_eight_block_rows_and_no_text() {
        let mut app = App::mock();
        let message = carrying(domain::message::MediaKind::Sticker);
        app.conversation
            .stickers
            .insert_bytes(message.id, crate::sticker::STICKER_TEST_WEBP)
            .expect("the fixture decodes");

        assert!(message_rows(&app, &message, Grouped::alone(), 40).is_empty());
        assert_eq!(sticker_block_rows(&message, &app.conversation.stickers), 8);
    }

    /// A caption wins over the picture the way it wins over the token: bytes
    /// or not, a sticker that says something is text.
    #[test]
    fn a_captioned_sticker_has_text_rows_and_no_block() {
        let mut app = App::mock();
        let mut message = carrying(domain::message::MediaKind::Sticker);
        message.text = String::from("back at you").into();
        app.conversation
            .stickers
            .insert_bytes(message.id, crate::sticker::STICKER_TEST_WEBP)
            .expect("the fixture decodes");

        assert!(!message_rows(&app, &message, Grouped::alone(), 40).is_empty());
        assert_eq!(sticker_block_rows(&message, &app.conversation.stickers), 0);
    }

    /// The columns a note takes are subtracted before the body is cut, so a note
    /// cannot turn a one-line body into two.
    #[test]
    fn a_trailing_note_does_not_add_a_row_to_a_placeholder() {
        let app = App::mock();
        let mut message = carrying(domain::message::MediaKind::Photo);
        message.timestamp = 1_730_000_000;
        let grouped = Grouped::alone();
        let note = trailing_note(&app, &message, grouped).expect("the group's time is on show");

        let rows = message_rows(&app, &message, grouped, 40);

        assert_eq!(
            rows,
            vec![0..message.display_body().len()],
            "{note:?} took columns from the row rather than a row of its own"
        );
    }

    #[test]
    fn the_layout_is_the_same_answer_twice_over() {
        let app = App::mock();

        assert_eq!(
            app.row_layout(),
            app.row_layout(),
            "one owner, one answer: the same window and width lay out the same way"
        );
    }

    #[test]
    fn the_layout_does_not_depend_on_where_the_cursor_is() {
        let mut app = App::mock();

        let before = app.row_layout();
        app.conversation.vim.set_cursor(0);
        let after = app.row_layout();

        assert_eq!(before, after, "the cursor is not an input to the geometry");
    }

    // ---- the slice -------------------------------------------------------

    #[test]
    fn a_pinned_view_shows_the_end_of_the_window() {
        let layout = layout_of(&["a", "b", "c", "d", "e"], 40);

        let view = slice(&layout, 4, 3, true);

        assert_eq!((view.start, view.skip), (2, 0));
        assert_eq!(view.start_row, 2);
        assert_eq!((view.rows, view.total), (3, 5));
    }

    #[test]
    fn a_cursor_that_is_not_pinned_is_centred_by_rows() {
        let layout = layout_of(&["a", "b", "c", "d", "e"], 40);

        let view = slice(&layout, 2, 3, false);

        assert_eq!(view.start_row, 1, "half a panel either side");
        assert_eq!(view.selection, 1);
    }

    #[test]
    fn a_slice_that_starts_inside_a_tall_message_skips_its_first_rows() {
        let layout = layout_of(&["a", &"y".repeat(200), "b"], 40);
        let tall = 1;

        // The tall message begins at row 1 and runs for six rows, so a slice
        // that has to start at row 5 begins inside it.
        let view = slice(&layout, 2, 3, false);

        assert_eq!(view.start_row, 5);
        assert_eq!((view.start, view.skip), (tall, 4));
        assert_eq!(
            view.rows, 3,
            "the panel's height, filled from inside a message"
        );
    }

    #[test]
    fn a_slice_is_never_shorter_than_the_panel_can_show() {
        let layout = layout_of(&["a", "b", "c", "d", "e"], 40);

        for cursor in 0..layout.len() {
            for follow in [true, false] {
                let view = slice(&layout, cursor, 3, follow);
                assert_eq!(view.rows, 3, "cursor {cursor}, follow {follow}");
                assert!(
                    view.selection < view.rows,
                    "the cursor's row is on the screen: {} of {}",
                    view.selection,
                    view.rows
                );
            }
        }
    }

    #[test]
    fn a_panel_taller_than_the_window_shows_the_whole_of_it() {
        let layout = layout_of(&["a", "b", "c"], 40);

        let view = slice(&layout, 1, 20, true);

        assert_eq!((view.start, view.start_row, view.rows), (0, 0, 3));
    }

    #[test]
    fn a_panel_with_no_room_still_shows_a_row() {
        let layout = layout_of(&["a", "b"], 40);

        let view = slice(&layout, 1, 0, true);

        assert_eq!((view.start, view.budget, view.rows), (1, 1, 1));
    }

    #[test]
    fn a_row_is_owned_by_the_message_it_falls_in() {
        let layout = layout_of(&["a", &"y".repeat(200), "b"], 40);

        assert_eq!(message_at_row(&layout, 0), Some(0));
        assert_eq!(message_at_row(&layout, 3), Some(1), "inside the tall one");
        assert_eq!(message_at_row(&layout, 7), Some(2));
        assert_eq!(message_at_row(&layout, 99), None);
    }

    // ---- a row that names no message -------------------------------------

    /// A separator sits between two messages and takes a row of the screen, so
    /// it is in the layout and the scrollbar counts it — and it names no
    /// message, so nothing that means "the message here" ever answers with it.
    #[test]
    fn a_row_that_names_no_message_is_counted_and_is_never_answered_as_a_message() {
        let layout = layout_with_other_row(&["a", "b", "c"], 1, 40);
        let other = layout
            .iter()
            .find(|span| !span.kind.is_message())
            .expect("the fixture has one");

        assert_eq!(other.first, 1, "between the first and the second message");
        assert_eq!(
            other.message_id, None,
            "and no identifier is invented for it: a made-up one would be a message"
        );
        assert_eq!(
            total_rows(&layout),
            4,
            "it is a row of the window, which the bar beside it counts"
        );

        assert_eq!(message_at_row(&layout, 0), Some(0));
        assert_eq!(
            message_at_row(&layout, 1),
            Some(1),
            "the row names no message, so the answer is the one below it"
        );
        assert_eq!(message_at_row(&layout, 2), Some(1));
        assert_eq!(message_at_row(&layout, 99), None);
    }

    /// The invariant the whole arrangement exists for: a cursor index is a
    /// message, every one of them, and the row a slice selects is that message's
    /// own first row.
    #[test]
    fn every_message_index_names_a_message_and_never_a_row_that_names_none() {
        let layout = layout_with_other_row(&["a", "b", "c", "d"], 2, 40);

        for index in 0..4 {
            let first = first_row_of_message(&layout, index).expect("the message is laid out");

            assert_eq!(
                layout
                    .iter()
                    .find(|span| span.first == first)
                    .map(|span| span.kind.clone()),
                Some(RowKind::Message { index }),
                "row {first} is message {index} and nothing else"
            );

            for follow in [true, false] {
                let view = slice(&layout, index, 2, follow);

                if view.start_row > first {
                    // A pinned slice shows the end of the window, and a cursor
                    // above it is not on the screen: there is no row of it to
                    // land on.
                    continue;
                }

                assert_eq!(
                    view.start_row + view.selection,
                    first,
                    "the cursor's row is message {index}'s own, follow {follow}"
                );
            }
        }

        assert_eq!(
            first_row_of_message(&layout, 4),
            None,
            "and an index the window does not hold names nothing, rather than a row"
        );
    }

    /// A row that names no message is not where a move comes to rest. Answering
    /// every such row downwards is what would leave `Ctrl+u` standing still
    /// while the reader believes it moved.
    #[test]
    fn a_move_that_arrives_on_a_row_that_names_no_message_carries_on() {
        let layout = layout_with_other_row(&["a", "b", "c"], 1, 40);

        assert_eq!(
            message_at_row_moving(&layout, 1, true),
            Some(1),
            "a page down lands on the message below it"
        );
        assert_eq!(
            message_at_row_moving(&layout, 1, false),
            Some(0),
            "and a page up on the message above"
        );
    }

    /// A layout with a row that names no message keeps the shape the panel's
    /// arithmetic assumes: every entry at least one row, none of them
    /// overlapping, and the window's row count the sum of them.
    #[test]
    fn a_layout_holding_a_row_that_names_no_message_keeps_its_shape() {
        let layout = layout_with_other_row(&["a", &"y".repeat(200), "b"], 1, 40);

        assert_eq!(lens(&layout), vec![1, 1, 6, 1]);
        assert_eq!(firsts(&layout), vec![0, 1, 2, 8]);
        assert!(
            layout
                .windows(2)
                .all(|pair| pair[0].first + pair[0].len <= pair[1].first),
            "the spans do not overlap"
        );
        assert_eq!(
            total_rows(&layout),
            layout.iter().map(|span| span.len).sum::<usize>(),
            "and the window is as tall as its entries together"
        );
    }

    /// A slice is the panel's height whatever the layout holds, and it starts on
    /// a message — `start` is a window position, which is the only thing the
    /// panel could look up.
    #[test]
    fn a_slice_of_a_layout_holding_a_row_that_names_no_message_still_fills_the_panel() {
        let layout = layout_with_other_row(&["a", "b", "c", "d", "e"], 2, 40);

        for cursor in 0..5 {
            for follow in [true, false] {
                let view = slice(&layout, cursor, 3, follow);

                assert_eq!(view.rows, 3, "cursor {cursor}, follow {follow}");
                assert!(
                    view.selection < view.rows,
                    "the cursor's row is on the screen: {} of {}",
                    view.selection,
                    view.rows
                );
                assert_eq!(view.total, 6, "and the row that names no message is in it");
                assert!(view.start < 5, "the slice starts on a message");
            }
        }
    }

    // ---- decorations -----------------------------------------------------

    /// A reply quotes a share of the row its body shares, and a target the
    /// window does not hold says so rather than leaving the reply unanchored.
    #[test]
    fn a_reply_prefix_is_quoted_within_the_room_the_first_row_has() {
        let mut app = App::mock();
        let chat_id = app.conversation.conversation.window.chat_id;
        app.apply_latest(vec![Message {
            id: 90,
            chat_id,
            text: "w".repeat(200).into(),
            timestamp: 0,
            status: MessageStatus::Received,
            is_outgoing: false,
            reply_to: Some(1),
            media: None,
        }]);

        let quoted = reply_prefix(&app, 1, 39);
        assert!(
            columns(&quoted) < 39 / 2 + WHO_WIDTH + 5,
            "a prefix that filled the row would push the body off it: {quoted:?}"
        );
        assert_eq!(
            reply_prefix(&app, 999, 53),
            "> [message not loaded] ‖ ",
            "and one that is there to be said is said whole"
        );
    }

    #[test]
    fn a_send_on_its_way_or_failed_says_so_once() {
        let app = App::mock();
        let mut sending = app
            .conversation
            .conversation
            .window
            .get(0)
            .expect("the window holds it")
            .clone();
        sending.status = MessageStatus::Sending;
        assert_eq!(status_suffix(&app, &sending).as_deref(), Some("[sending…]"));

        let mut failed = sending.clone();
        failed.status = MessageStatus::Failed;
        assert!(
            status_suffix(&app, &failed).is_some(),
            "the reason the window holds is what is quoted"
        );

        let mut sent = sending;
        sent.status = MessageStatus::Sent;
        assert_eq!(status_suffix(&app, &sent), None);
    }

    /// What stands behind a message's last row: its own status, the time its
    /// group ended, and both together — a group never hides a message's status,
    /// which is a fact about the message rather than about the group.
    #[test]
    fn the_note_behind_a_message_is_its_status_and_the_time_of_its_group() {
        let app = App::mock();
        let sent = app
            .conversation
            .conversation
            .window
            .get(0)
            .expect("the window holds it")
            .clone();
        let time = date::clock(sent.timestamp).expect("the sample messages have a time");
        let alone = Grouped::alone();
        let within = Grouped {
            first: false,
            last: false,
            receipt: Receipt::None,
        };

        assert_eq!(
            trailing_note(&app, &sent, alone).as_deref(),
            Some(format!("  {time}").as_str()),
            "a message that closes its group says when"
        );
        assert_eq!(
            trailing_note(&app, &sent, within),
            None,
            "and one that only continues it says nothing behind it"
        );

        let mut sending = sent.clone();
        sending.status = MessageStatus::Sending;
        assert_eq!(
            trailing_note(&app, &sending, within).as_deref(),
            Some("  [sending…]"),
            "a send on its way says so mid-group, which is the case grouping must not swallow"
        );
        assert_eq!(
            trailing_note(&app, &sending, alone).as_deref(),
            Some(format!("  [sending…]  {time}").as_str()),
            "and on the row that ends its group it says both"
        );

        let mut silent = sent.clone();
        silent.timestamp = 0;
        assert_eq!(
            trailing_note(&app, &silent, alone),
            None,
            "a message with no time shows no time"
        );
    }

    /// A message is as wide as the panel gives it: the rows of its first row,
    /// plus the sender's name in front of it, are the panel's columns and not one
    /// more.
    ///
    /// A terminal clips without saying so, so a row laid out one column too wide
    /// loses its last character to the edge rather than wrapping. That is what
    /// [`WHO_WIDTH`] being one narrower than the drawn name used to cause.
    #[test]
    fn a_whole_message_is_as_wide_as_its_own_decorations() {
        let app = App::mock();
        let width = 40_u16;

        for text in ["hi", &"x".repeat(500)] {
            let message = Message {
                text: (*text).to_owned().into(),
                ..app
                    .conversation
                    .conversation
                    .window
                    .get(0)
                    .expect("the window holds a message")
                    .clone()
            };

            let grouped = group_of(&app, 0);
            let (prefix, suffix) = decoration_columns(&app, &message, grouped, width);
            let rows = message_rows(&app, &message, grouped, width);

            let drawn: usize = rows
                .iter()
                .enumerate()
                .map(|(index, range)| {
                    // What the panel puts on this row, decorations included.
                    prefix.min(usize::from(width)) * usize::from(index == 0)
                        + (range.end - range.start)
                        + suffix * usize::from(index + 1 == rows.len())
                })
                .max()
                .unwrap_or(0);

            assert!(
                drawn <= usize::from(width),
                "{rows:?} is drawn {drawn} columns into a panel of {width}"
            );
        }
    }

    #[test]
    fn a_truncation_is_marked_and_never_cuts_a_character_in_half() {
        assert_eq!(truncate("hello", 8), "hello");
        assert_eq!(truncate("hello", 4), "hel…");
        assert_eq!(truncate("héllo", 3), "hé…");
    }

    /// A budget is in columns, because a decoration is subtracted from a row's
    /// width before the row is cut. An emoji that straddles the budget is left
    /// out whole rather than half drawn — and the `…` is counted against the
    /// budget, so a truncated decoration is never a column too wide.
    #[test]
    fn truncate_cuts_on_a_column_budget_and_never_mid_character() {
        assert_eq!(
            truncate("😀😀😀", 8),
            "😀😀😀",
            "and it fits, so it is whole"
        );
        assert_eq!(truncate("😀😀😀", 5), "😀😀…", "four columns and the mark");
        assert_eq!(truncate("a😀b", 4), "a😀b", "four columns is a whole fit");
        assert_eq!(
            truncate("a😀b", 3),
            "a…",
            "one column short, and the emoji would straddle it, so it is not taken"
        );
        for budget in 1..8 {
            let cut = truncate("😀a😀b漢", budget);
            assert!(
                columns(&cut) <= budget,
                "{cut:?} is {} columns against a budget of {budget}",
                columns(&cut)
            );
        }
    }

    /// A quoted reply is measured in the units it is drawn in, so an emoji in
    /// the quoted text costs the body the two cells it is given.
    #[test]
    fn a_decoration_containing_an_emoji_is_measured_in_columns() {
        let mut app = App::mock();
        let chat_id = app.conversation.conversation.window.chat_id;
        app.apply_latest(vec![Message {
            id: 90,
            chat_id,
            text: "😀😀😀😀😀😀😀😀".repeat(4).into(),
            timestamp: 0,
            status: MessageStatus::Received,
            is_outgoing: false,
            reply_to: None,
            media: None,
        }]);
        let reply = Message {
            id: 91,
            chat_id,
            text: "sure".into(),
            timestamp: 0,
            status: MessageStatus::Received,
            is_outgoing: true,
            reply_to: Some(90),
            media: None,
        };

        let (prefix, suffix) = decoration_columns(&app, &reply, Grouped::alone(), 40);

        assert_eq!(suffix, 0, "a sent reply draws nothing behind it");
        assert_eq!(
            prefix,
            WHO_WIDTH + columns(&reply_prefix(&app, 90, 40)),
            "and the quote is counted in cells, not characters"
        );
    }

    // ---- the two range units ---------------------------------------------

    /// A mark's position counts characters and a row's range counts bytes, so
    /// this is where the two meet. The multi-byte cases are the whole reason: a
    /// character position is not a byte position, and a selection that silently
    /// cut a `é` in half would be a selection over nothing.
    #[test]
    fn a_character_span_is_where_those_characters_start_in_bytes() {
        assert_eq!(
            byte_span("hello", 0..3),
            0..3,
            "ascii is the same either way"
        );
        assert_eq!(
            byte_span("héllo", 0..2),
            0..3,
            "two characters, three bytes"
        );
        assert_eq!(
            byte_span("héllo", 1..3),
            1..4,
            "and a span that starts after it"
        );
        assert_eq!(
            byte_span("😀 ok", 0..1),
            0..4,
            "one character, four bytes: the case a byte position gets wrong"
        );
        assert_eq!(byte_span("😀 ok", 1..3), 4..6);
        assert_eq!(
            byte_span("héllo", 0..99),
            0..6,
            "and past the end is the end"
        );
        assert_eq!(byte_span("héllo", 99..99), 6..6);
        assert_eq!(byte_span("", 0..1), 0..0, "an empty message is not a panic");
    }

    #[test]
    fn a_selection_is_clipped_to_the_row_it_is_drawn_on() {
        // Row 0 of a three-row wrap of "hello", over the whole of it.
        assert_eq!(clip(&(1..4), &(0..2)), (1, 2), "the tail of the selection");
        assert_eq!(clip(&(1..4), &(2..4)), (0, 2), "the whole of the row");
        assert_eq!(clip(&(3..5), &(2..4)), (1, 2), "the head of the selection");
    }

    /// A selection that does not reach a row clips to nothing, which is how the
    /// caller tells "not covered" from "covered and empty": a row the selection
    /// misses entirely must not be sliced at its start.
    #[test]
    fn a_selection_that_misses_a_row_clips_to_nothing() {
        for (selected, row) in [((0..1), (5..9)), ((8..9), (0..2)), ((4..5), (0..4))] {
            let (from, to) = clip(&selected, &row);
            assert!(
                from >= to,
                "{selected:?} against {row:?} clipped to {from}..{to}, which is not empty"
            );
        }
    }
}
