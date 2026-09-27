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
//! 4. the layout is a pure function of the window's messages and the panel's
//!    width — not of the cursor, not of the mode, not of when it was asked;
//! 5. the width is the one left after the scrollbar's column is given up, so a
//!    message is never written under the bar.

use std::ops::Range;

use domain::message::{Message, MessageStatus};

use crate::app::App;
use crate::wrap::wrap_decorated;

/// The columns a message's sender is named in: `[you] ` or `[them] `.
///
/// Named on the first row of a message and on no other, so this is a constant
/// of the panel rather than something a message decides.
///
/// Seven, because the bracket, the name and the space that follows it are all
/// drawn. It was six, which is one narrower than what `message_row` puts on the
/// row: the first row was then laid out a column wider than the panel and its
/// last character was clipped by the terminal rather than wrapped to the next row.
/// `a_whole_message_is_as_wide_as_its_own_decorations` is the arithmetic that
/// pins it.
const WHO_WIDTH: usize = 7;

/// How much of a failed send's reason is quoted at the end of its last row.
///
/// A reason is longer than a row has room for and the whole of it is on the
/// status line while the cursor is on the message; this is only enough to say
/// that there is one.
const FAILED_REASON_WIDTH: usize = 24;

/// Where one message's rows are in the panel.
///
/// Named by [`RowSpan::message_id`] because an index means a different message
/// once a page lands — which is why a position the reader has to come back to
/// is carried as an identifier, and why a layout worked out before a page
/// arrives is thrown away rather than kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowSpan {
    /// The message, named rather than indexed.
    pub message_id: i64,

    /// Its index in the window, for looking the message up.
    pub index: usize,

    /// The first row it occupies, counted from the top of the laid-out window.
    pub first: usize,

    /// How many rows it occupies. Always at least one.
    pub len: usize,

    /// The whole of the message's text, in the units
    /// [`crate::wrap`] works in: byte offsets at character boundaries. The rows
    /// the text is cut into are worked out again from the width, so what is
    /// kept here is the range a selection or a yank would name.
    pub text: Range<usize>,
}

/// The rows the panel draws, and where the cursor's row lands among them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slice {
    /// The first message drawn.
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

    /// The rows the whole window occupies, laid out.
    pub total: usize,

    /// The message rows drawn: the panel's height, or what is left of the
    /// window when it is shorter than the panel. A message that does not fit in
    /// what is left is not drawn at all rather than half-drawn.
    pub rows: usize,

    /// The cursor's row among the rows drawn. The rows a fetch is announced on
    /// are not counted, because they are not places the cursor can stand.
    pub selection: usize,
}

/// The rows the panel spends on what it is fetching, rather than on messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Reserved {
    /// A page is on its way from in front of the window.
    pub older: bool,

    /// A jump the reader asked for is on its way.
    pub jumping: bool,

    /// A page is on its way from behind the window.
    pub newer: bool,
}

impl Reserved {
    /// The rows above the messages.
    #[must_use]
    pub fn above(self) -> usize {
        usize::from(self.older) + usize::from(self.jumping)
    }

    /// The rows below the messages.
    #[must_use]
    pub fn below(self) -> usize {
        usize::from(self.newer)
    }
}

/// The rows one message occupies, as character ranges into its text.
///
/// The height of a message is this, and only this: the panel draws the rows and
/// [`crate::app::App::row_layout`] counts them, so a message is as tall here as
/// it is on the screen.
#[must_use]
pub fn message_rows(app: &App, message: &Message, width: u16) -> Vec<Range<usize>> {
    let (prefix, suffix) = decoration_columns(app, message, width);
    wrap_decorated(&message.text, prefix, suffix, width)
}

/// How many rows a layout occupies.
#[must_use]
pub fn total_rows(layout: &[RowSpan]) -> usize {
    layout.last().map_or(0, |span| span.first + span.len)
}

/// The message that owns `row`.
///
/// A move by rows lands in the middle of a message as often as not, and the
/// answer is the message rather than the row: a cursor stands on messages, and
/// a page down in a terminal puts the reader at the top of what it moved to.
#[must_use]
pub fn message_at_row(layout: &[RowSpan], row: usize) -> Option<usize> {
    layout.iter().position(|span| span.first + span.len > row)
}

/// Which rows fill a panel of `budget` rows, for a cursor on `cursor`'s message.
///
/// A cursor that is not pinned is centred by rows, which is the only measure
/// the screen has; a pinned one shows the end of the window. Either way the
/// slice starts at a row rather than at a message, so it fills the panel
/// exactly and the scrollbar beside it has something true to show.
#[must_use]
pub fn slice(layout: &[RowSpan], cursor: usize, budget: usize, follow: bool) -> Slice {
    let budget = budget.max(1);
    let total = total_rows(layout);
    let cursor_row = layout.get(cursor).map_or(0, |span| span.first);
    let target = if follow {
        total.saturating_sub(budget)
    } else {
        cursor_row.saturating_sub(budget / 2)
    };
    // Pulled back inside the window so the slice is as tall as the panel and
    // never starts past the end.
    let start_row = target.min(total.saturating_sub(budget));
    let start = message_at_row(layout, start_row).unwrap_or(layout.len());
    let skip = layout.get(start).map_or(0, |span| start_row - span.first);

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
fn decoration_columns(app: &App, message: &Message, width: u16) -> (usize, usize) {
    let prefix = WHO_WIDTH
        + message.reply_to.map_or(0, |reply_to| {
            reply_prefix(app, reply_to, width).chars().count()
        });
    let suffix = status_suffix(app, message).map_or(0, |text| text.chars().count());

    (prefix, suffix)
}

/// The target a reply quotes, as the prefix in front of the body it answers.
///
/// A share of what the first row has left once the sender is named, so the body
/// still has room: a prefix that filled the row would push the thing it is a
/// prefix to off it. A target the window does not hold says so rather than
/// leaving the reply unanchored, and one that is still on its way says that
/// instead of quoting a message that has not arrived.
pub(crate) fn reply_prefix(app: &App, reply_to: i64, width: u16) -> String {
    let text = match app.conversation.message(reply_to) {
        Some(message) if matches!(message.status, MessageStatus::Sending) => {
            "[sending…]".to_owned()
        }
        Some(message) => message.text.to_string(),
        None => "[message not loaded]".to_owned(),
    };

    // `> ` in front and ` ‖ ` behind, the brackets of the quoted text apart.
    let room = usize::from(width).saturating_sub(WHO_WIDTH + 5) / 2;
    format!("> {} ‖ ", truncate(&text, room.max(8)))
}

/// What a send is doing, for the end of its message's last row.
///
/// A fact about the whole message rather than about a row of it, so it is drawn
/// once, on the row with room for it.
pub(crate) fn status_suffix(app: &App, message: &Message) -> Option<String> {
    match message.status {
        MessageStatus::Sending => Some("  [sending…]".to_owned()),
        MessageStatus::Failed => {
            let reason = app.conversation.failure(message.id).unwrap_or("failed");
            Some(format!(
                "  [failed: {}]",
                truncate(reason, FAILED_REASON_WIDTH)
            ))
        }
        MessageStatus::Sent | MessageStatus::Received => None,
    }
}

/// Truncates `text` to `budget` characters, marking the cut.
///
/// Characters rather than bytes, because a multibyte character cut in half is
/// not text at all.
pub(crate) fn truncate(text: &str, budget: usize) -> String {
    if text.chars().count() <= budget {
        return text.to_owned();
    }

    let mut shortened: String = text.chars().take(budget.saturating_sub(1)).collect();
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
            };
            let len = message_rows(&app, &message, width).len();
            spans.push(RowSpan {
                message_id: message.id,
                index,
                first,
                len,
                text: 0..text.len(),
            });
            first += len;
        }

        spans
    }

    fn lens(spans: &[RowSpan]) -> Vec<usize> {
        spans.iter().map(|span| span.len).collect()
    }

    fn firsts(spans: &[RowSpan]) -> Vec<usize> {
        spans.iter().map(|span| span.first).collect()
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
        app.vim.set_cursor(0);
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

    // ---- decorations -----------------------------------------------------

    /// A reply quotes a share of the row its body shares, and a target the
    /// window does not hold says so rather than leaving the reply unanchored.
    #[test]
    fn a_reply_prefix_is_quoted_within_the_room_the_first_row_has() {
        let mut app = App::mock();
        let chat_id = app.conversation.window.chat_id;
        app.apply_latest(vec![Message {
            id: 90,
            chat_id,
            text: "w".repeat(200).into(),
            timestamp: 0,
            status: MessageStatus::Received,
            is_outgoing: false,
            reply_to: Some(1),
        }]);

        let quoted = reply_prefix(&app, 1, 39);
        assert!(
            quoted.chars().count() < 39 / 2 + WHO_WIDTH + 5,
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
            .window
            .get(0)
            .expect("the window holds it")
            .clone();
        sending.status = MessageStatus::Sending;
        assert_eq!(
            status_suffix(&app, &sending).as_deref(),
            Some("  [sending…]")
        );

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
                    .window
                    .get(0)
                    .expect("the window holds a message")
                    .clone()
            };

            let (prefix, suffix) = decoration_columns(&app, &message, width);
            let rows = message_rows(&app, &message, width);

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
