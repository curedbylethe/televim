//! A run of text the reader can put a cursor in, painted.
//!
//! Three surfaces draw one: a message in the conversation, a row of a profile
//! card, and the line being composed. All three need the same three things in the
//! same order — the text, a search match patched on, a selection split out — and
//! the order is the rule rather than an accident, because a match and a selection
//! each set a foreground and the one applied last wins. Two of the three also need
//! a caret, and the two want it in different places, which is the one difference
//! worth naming here rather than at each call site.
//!
//! # What this does not know
//!
//! - **What the text is.** A row is a byte range into a string its caller owns
//!   ([`TextRow::text`] and [`TextRow::range`]), not a string of its own, so a
//!   selection across a wrapped value is three slices rather than a text-layout
//!   problem. [`crate::rows`] owns the geometry that cuts those ranges; this
//!   module never wraps anything.
//! - **What is around it.** A message's `[you]` prefix, a card's label gutter and
//!   the line's `: ` prompt are all drawn by the caller and spliced around the
//!   spans returned here. Folding them in is what turns one row into two rows
//!   wearing a trenchcoat, and they genuinely differ: a conversation's sender is
//!   a decoration that participates in wrapping, a card's label is a fixed gutter.
//! - **Whether the surface is a list, a card or a bar.** It takes an [`Ink`] and
//!   nothing else, and an [`Ink`] is built by one of three constructors.
//! - **What order the row is drawn in.** [`spans`] draws the row as it is
//!   stored, which is every surface but one; [`spans_permuted`] draws it in the
//!   order [`crate::bidi::visual_row`] names, which is a right-to-left row the
//!   reader has asked this program to permute. The marks are clipped to each
//!   piece in either case, so a selection follows its own characters across the
//!   reorder rather than its columns.
//!
//! # The caret, and why it is not a fifth style
//!
//! [`crate::theme`] patches four styles onto a cell, in a fixed order. A caret is
//! not a fifth one. On a row the surface has reversed, the caret comes out as the
//! ground colour — and it gets there by *not* being patched with
//! [`crate::theme::Theme::selection`] at all, so one cell of the row keeps its own
//! ink and reads as a hole in the reverse video. That is why [`TextRow::reversed`]
//! is a flag about the row and not a style: `patch` can only add, and here the
//! caret's whole job is to take something away.

use std::ops::Range;

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::bidi::Chunk;
use crate::rows;
use crate::theme::Theme;

/// The inks one surface paints a text row with.
///
/// Six styles, and the three constructors are the only place they are chosen, so
/// "what colour is a selection inside a message" has one answer rather than one
/// per call site.
#[derive(Debug, Clone, Copy)]
pub struct Ink {
    /// The text itself.
    pub plain: Style,

    /// A search hit within it, which is [`Theme::match_hit`].
    pub matched: Style,

    /// A selection within it.
    pub selected: Style,

    /// A caret, on a row this surface has *not* reversed.
    pub caret: Style,

    /// A dot in for a space, while a draft is being composed. `None` everywhere
    /// else, because a space that paints nothing is what a reader expects of
    /// prose and a dotted sentence reads as something else.
    pub dot: Option<Style>,
}

impl Ink {
    /// A read-only surface: a message, or a row of a card.
    ///
    /// A selection is [`Theme::selection_bg`] in both, because a selection is a
    /// selection whether it covers a message or a value — that is the whole claim
    /// the card is built on.
    #[must_use]
    pub fn readonly(theme: &Theme) -> Self {
        Self {
            plain: theme.text,
            matched: theme.match_hit,
            selected: theme.selection_bg,
            caret: theme.caret_normal,
            dot: None,
        }
    }

    /// A message whose body is a stand-in for an attachment rather than prose.
    ///
    /// Everything a read-only row does, except the text itself reads dimmer:
    /// the reader is being told what was sent, not what was written, so the
    /// placeholder does not carry the weight of the peer's own words. A
    /// selection, a match and a caret are unchanged — the row is still body
    /// text and is still selectable, yankable and searchable.
    #[must_use]
    pub fn placeholder(theme: &Theme) -> Self {
        Self {
            plain: theme.text_dim,
            ..Self::readonly(theme)
        }
    }

    /// The line being composed.
    ///
    /// Dimmed when the reader is not typing in it, because a draft they are not
    /// composing is read rather than edited and should not look like something the
    /// program just printed. A selection is [`Theme::mode_visual`] rather than a
    /// background, because inside the line a selection means the *line* is in
    /// Visual mode and says so with the mode's own colour.
    ///
    /// `focused` and `normal` are two flags rather than one because a caret needs
    /// both and they answer different questions: `focused` says whether there is a
    /// caret at all, and `normal` says which of the two shapes it is. A reader in
    /// the line's own Normal mode is still typing into the line, so gating the
    /// caret on the mode alone would lose it, and picking its shape from the focus
    /// alone would give the line's Normal mode an insert block.
    #[must_use]
    pub fn draft(theme: &Theme, focused: bool, normal: bool) -> Self {
        Self {
            plain: if focused { theme.text } else { theme.text_dim },
            matched: theme.match_hit,
            selected: theme.mode_visual,
            caret: if normal {
                theme.caret_normal
            } else {
                theme.caret_insert
            },
            dot: focused.then_some(theme.text_dim),
        }
    }
}

/// One row of text: a byte range into a string, and whatever stands on it.
///
/// Built by the caller and handed to [`spans`], which is the only thing here that
/// paints. The fields are public because a struct literal says at the call site
/// exactly which of them apply — which is the whole point, since a card row sets
/// a caret and a message row does not.
#[derive(Debug, Clone)]
pub struct TextRow<'a> {
    /// The text this is a row of.
    ///
    /// A row is a range into this rather than a string of its own, so a selection
    /// is arithmetic on two ranges ([`rows::clip`]) and never a re-layout.
    pub text: &'a str,

    /// The slice of [`TextRow::text`] this row shows.
    ///
    /// In **logical** order, whatever order the row is drawn in: a row is a range
    /// into the string its caller owns, and a reorder is something the painter
    /// does to a row rather than something the row becomes.
    pub range: Range<usize>,

    /// The row is part of a search hit, so its text takes [`Ink::matched`].
    pub matched: bool,

    /// The byte range of a selection within [`TextRow::text`], when one covers
    /// part of this row. A range that misses the row is not this caller's problem
    /// to detect: [`rows::clip`] returns an empty range and the row is simply not
    /// marked.
    pub selected: Option<Range<usize>>,

    /// The **byte** offset of a caret within [`TextRow::text`], when one falls on
    /// this row.
    ///
    /// A byte offset like every other field here, because a caret is clipped and
    /// wrapped by the same arithmetic as everything else and a row that mixed the
    /// two would be wrong in a way nothing would catch. A motion that produces a
    /// *character* position — [`domain::vim::char_motion`] does — is converted at
    /// the call site with [`rows::byte_span`], which is the one converter in the
    /// program; that is the whole reason it lives there.
    pub caret: Option<usize>,

    /// Whether the surface has reversed this row.
    ///
    /// Set when this row is the one the cursor is on, and it changes the caret
    /// alone: see the module docs.
    pub reversed: bool,

    /// Paint one `•` in place of every character, and only that.
    ///
    /// **A password is concealed in the paint, not in the buffer.** The text is
    /// the reader's and the caret has to keep moving over it, so nothing here is
    /// rewritten: every offset, wrap and selection is the same arithmetic it
    /// always was, and only the glyph that lands in the cell changes. A row that
    /// swapped its text for bullets would have a caret with nothing to stand on
    /// and a selection over characters the reader cannot see.
    pub concealed: bool,

    /// The inks to paint with.
    ///
    /// By value rather than by reference: it is five `Style`s and the borrow would
    /// tie a row to the frame's `Ink` for no gain.
    pub ink: Ink,
}

/// The spans for one row of text, in the order the theme composes them.
///
/// Borrowed from [`TextRow::text`], so a caller that needs `'static` (the
/// conversation's list, whose rows outlive the frame's own borrows) still has to
/// copy — but the bar, which is drawn from the app for the length of the frame,
/// does not, and neither does a card.
/// What divides a row, at one offset from its left edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    /// A selection begins here, so what follows wears its ink.
    Selected,

    /// A selection ended here, so what follows is the row's own again.
    Plain,

    /// A caret begins here: the character at this offset is marked.
    CaretOn,

    /// The caret's character is over and the row is its own ink again.
    CaretOff,
}

#[must_use]
pub fn spans<'a>(row: &TextRow<'a>) -> Vec<Span<'a>> {
    paint(row, std::slice::from_ref(&row.range))
}

/// The spans for one row drawn in the order [`Chunk`]s name, rather than the
/// order the row is stored in.
///
/// Same three steps as [`spans`] — the text, a match on it, a selection split out
/// of it — and the only difference is that a **piece** of the row is a logical
/// byte range of its own rather than the whole row at once, so a permuted row
/// draws its pieces in the order it is given and each piece's selection is
/// clipped to that piece.
///
/// Which is the whole reason this is a second entry point rather than a field on
/// [`TextRow`]: the order a row is drawn in is a property of the message it came
/// from, not of the row, and a row that did not say so has exactly one answer.
///
/// A row whose chunks do not cover it draws nothing rather than drawing it twice:
/// the pieces are the row's own claim about its text, and a row that makes a
/// false one has nothing honest to fall back on.
#[must_use]
pub fn spans_permuted<'a>(row: &TextRow<'a>, chunks: &[Chunk]) -> Vec<Span<'a>> {
    let pieces: Vec<Range<usize>> = chunks.iter().map(|chunk| chunk.logical.clone()).collect();

    if is_the_whole_row(row.text, &pieces, &row.range) {
        paint(row, &pieces)
    } else {
        Vec::new()
    }
}

/// Whether `pieces` cover `row` exactly: every byte of the row once, no byte of it
/// twice, and all of them inside the text.
///
/// The check is on bytes and not on columns, because the bytes are what the
/// slices are taken with. A permutation that lost or duplicated a piece is not a
/// row drawn in a different order — it is a row drawn wrong, and the only honest
/// answer to that is no spans at all.
fn is_the_whole_row(text: &str, pieces: &[Range<usize>], row: &Range<usize>) -> bool {
    let mut covered = pieces.to_vec();
    covered.sort_by_key(|piece| (piece.start, piece.end));
    covered.dedup();

    // Folded rather than compared as a whole, so the first gap is the one that
    // fails: a piece out of order, a byte painted twice, a byte missing.
    covered.iter().try_fold(row.start, |at, piece| {
        text.get(piece.clone())?;
        (piece.start == at).then_some(piece.end)
    }) == Some(row.end)
}

/// The one paint, over however many pieces the row is drawn in.
///
/// A row is a list of logical byte ranges, in the order they reach the terminal.
/// With one piece that is the row as stored, which is every surface but the
/// conversation's; with several it is a right-to-left row the reader has asked
/// this program to permute. Everything else — the match first, the selection on
/// top of it, the caret as an overlay — is the same for both, and is per piece:
/// a selection scattered across four pieces has four clipped slices of itself,
/// not one.
fn paint<'a>(row: &TextRow<'a>, pieces: &[Range<usize>]) -> Vec<Span<'a>> {
    // A row with no text has no cell to put a caret in, and a caret needs one:
    // it is a cell. A value the peer did not send is a row that is not here at
    // all, not a row drawn empty.
    if row.text.get(row.range.clone()).is_none_or(str::is_empty) {
        return Vec::new();
    }

    let base = if row.matched {
        row.ink.plain.patch(row.ink.matched)
    } else {
        row.ink.plain
    };

    // A permuted row's pieces are sub-ranges of the row, not of the whole text, so
    // "the last row owns the offset one past its own end" is a question about the
    // row: with one piece the answer must stay exactly what [`caret_lands_here`]
    // says of the text, and with several there is no whole row left to ask of.
    // It is `row.range.end` and not the row's *length*, because `caret_lands_here`
    // compares a piece's `end` against it, and a piece's end is an offset into the
    // text rather than a distance from where the row began. A length would only
    // equal an offset for a row that starts at zero, so on any other row the
    // caret at the row's end — the one a reader is most often looking at — would
    // belong to no piece and be painted nowhere.
    let len = if pieces.len() == 1 {
        row.text.len()
    } else {
        row.range.end
    };

    let mut out: Vec<Span<'a>> = Vec::new();
    for piece in pieces {
        let Some(text) = row.text.get(piece.clone()) else {
            continue;
        };
        // Each piece is a row of its own for the length of this: it is painted in
        // one go and the marks that fall in it are the ones that divide it.
        paint_piece(row, &mut out, piece, text, base, len);
    }

    out
}

/// One piece of a row: its text, and whatever stands on it.
///
/// A selection is clipped to this piece and a caret is owned by the one piece it
/// falls in, because a permuted row scatters both across pieces — a selection of
/// four characters of a right-to-left word is four pieces and one of them is
/// marked, not four marked cells in one span.
fn paint_piece<'a>(
    row: &TextRow<'a>,
    out: &mut Vec<Span<'a>>,
    piece: &Range<usize>,
    text: &'a str,
    base: Style,
    len: usize,
) {
    // A selection is split out of the row rather than styled where it is built,
    // because styling a slice of a row means splitting the row, and a row is only
    // splittable while its text is still one span.
    let (from, to) = row
        .selected
        .as_ref()
        .map_or((0, 0), |range| rows::clip(range, piece));
    let is_selected = from < to;

    let caret = row
        .caret
        .filter(|at| caret_lands_here(piece, *at, len))
        .map(|at| at - piece.start);

    // What divides the piece, in the order it divides it. The selection goes on
    // first so that a caret sharing an offset with its edge is marked inside the
    // selection rather than beside it.
    let mut marks: Vec<(usize, Mark)> = Vec::with_capacity(4);
    if is_selected {
        marks.push((from, Mark::Selected));
        marks.push((to, Mark::Plain));
    }
    if let Some(at) = caret {
        // A caret is a **one-character style range**, from the character at its
        // offset to the one after it. It is not a mark in the text and it is not a
        // cell of its own: a cell that pushed the text along would move everything
        // after it, and a card's values would jump a column every time the cursor
        // landed on one of them. An overlay is also what the design's own stylesheet
        // does — a `::after` pseudo-element drawn on the cell rather than a
        // character in the stream — and it is the only reading under which a card's
        // columns stay put.
        //
        // One past the end of the value there is no character to mark, so the range
        // is empty and the cell is emitted on its own below. That is the only case
        // where a caret costs a column, and it is at the end of a value, where a
        // column of movement is invisible.
        let end = text[at..]
            .chars()
            .next()
            .map_or(at, |character| at + character.len_utf8());
        marks.push((at, Mark::CaretOn));
        marks.push((end, Mark::CaretOff));
    }
    marks.sort_by_key(|(at, _)| *at);

    let mut at = 0;
    let mut style = base;
    let mut on_caret = false;

    for (offset, mark) in marks {
        if at < offset {
            push(
                out,
                &text[at..offset],
                effective(row, style, on_caret),
                row.ink.dot,
                row.concealed,
            );
            at = offset;
        }
        match mark {
            Mark::Selected => style = base.patch(row.ink.selected),
            Mark::Plain => style = base,
            Mark::CaretOn => on_caret = true,
            Mark::CaretOff => on_caret = false,
        }
    }
    if at < text.len() {
        push(
            out,
            &text[at..],
            effective(row, style, on_caret),
            row.ink.dot,
            row.concealed,
        );
    }

    // A caret past the last character has nothing to mark, so it is the one cell
    // the row grows: a space **in the caret's own ink**, which is the whole reason
    // it is a cell at all. A reader looking at the end of a draft is looking at
    // this cell, and in the row's ink it would be nothing to see.
    if caret.is_some_and(|at| at >= text.len()) {
        out.push(Span::styled(" ", caret_style(row, style)));
    }
}

/// The style a span is painted in: the caret's where the caret is, and its own
/// everywhere else.
fn effective(row: &TextRow<'_>, style: Style, on_caret: bool) -> Style {
    if on_caret {
        caret_style(row, style)
    } else {
        style
    }
}

/// Whether a caret at byte offset `at` falls on the row `range` names.
///
/// A row owns the caret when the offset is inside it, and the last row of the
/// text owns the offset one past its own end as well: a caret in a value is most
/// often at the end of that value, because that is where a reader types.
///
/// Everywhere else the seam belongs to the row above, not this one: two rows of a
/// wrapped value meet at that byte and exactly one of them may paint it.
fn caret_lands_here(range: &Range<usize>, at: usize, len: usize) -> bool {
    range.start <= at && (at < range.end || (at == range.end && range.end == len))
}

/// The style a caret takes on this row.
///
/// On a row the surface has not reversed, that is the caret's own style and
/// nothing more. On a reversed row it is the row's ink with the reversal *left
/// off*: [`Style::remove_modifier`] is what turns one cell of a `REVERSED` row
/// back into the ground, and it is the only call in the program that does so.
fn caret_style(row: &TextRow<'_>, base: Style) -> Style {
    if row.reversed {
        base.remove_modifier(Modifier::REVERSED)
    } else {
        row.ink.caret
    }
}

/// Pushes `text` as spans, standing a dot in for every space in it.
///
/// A space occupies a cell and paints nothing, and the caret is a bar on a blank
/// cell, so a key that typed a space changed nothing a reader could see. The dot
/// is one column wide where the space was, which is what keeps the caret and the
/// wrap in step: the mark is the cell the space already had, not a column taken
/// from somewhere else.
///
/// `concealed` replaces every character with one `•` instead, which is the same
/// idea on a row that must not be read: one bullet per character, so the bar's
/// geometry is unchanged and only the glyph differs. It cannot borrow the text —
/// the glyph is not in it — so that row is owned, which is the one place in this
/// module that copies.
fn push<'a>(
    out: &mut Vec<Span<'a>>,
    text: &'a str,
    style: Style,
    dot: Option<Style>,
    concealed: bool,
) {
    // An empty span is a span the renderer has to skip and a reader cannot see,
    // and an empty row is what a range outside the text slices to.
    if text.is_empty() {
        return;
    }

    if concealed {
        out.push(Span::styled("•".repeat(text.chars().count()), style));
        return;
    }

    let Some(mark) = dot else {
        out.push(Span::styled(text, style));
        return;
    };

    let mut start = 0;
    for (at, character) in text.char_indices() {
        if character == ' ' {
            if start < at {
                out.push(Span::styled(&text[start..at], style));
            }
            out.push(Span::styled("·", mark));
            start = at + 1;
        }
    }
    if start < text.len() {
        out.push(Span::styled(&text[start..], style));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bidi;
    use ratatui::style::Modifier;

    fn theme() -> Theme {
        Theme::default()
    }

    fn ink() -> Ink {
        Ink::readonly(&theme())
    }

    /// The row's own text, with the styles stripped, so an assertion is about
    /// the text and not about the palette.
    fn plain_text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// The span a caret is on, found by **ink** rather than by content.
    ///
    /// A caret is an overlay: it is the character at its own offset, re-styled, and
    /// the text does not move. So it cannot be found by looking for a space — the
    /// character under it is still the reader's text. A caret one past the end of a
    /// value has no character to mark and takes a cell of its own, and it carries
    /// the same ink, so one lookup finds both cases.
    fn caret_index(out: &[Span<'_>], caret: Style) -> Option<usize> {
        out.iter().position(|span| span.style == caret)
    }

    #[test]
    fn a_row_with_nothing_on_it_is_one_span_in_the_text_ink() {
        let text = "Ada Lovelace";
        let row = TextRow {
            text,
            range: 0..text.len(),
            matched: false,
            selected: None,
            caret: None,
            concealed: false,
            reversed: false,
            ink: ink(),
        };

        let out = spans(&row);
        assert_eq!(plain_text(&out), "Ada Lovelace");
        assert_eq!(out.len(), 1, "nothing to split: {out:?}");
        assert_eq!(out[0].style, theme().text);
    }

    #[test]
    fn a_match_takes_the_match_ink_and_nothing_else() {
        let text = "Ada Lovelace";
        let row = TextRow {
            text,
            range: 0..text.len(),
            matched: true,
            selected: None,
            caret: None,
            concealed: false,
            reversed: false,
            ink: ink(),
        };

        let out = spans(&row);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].style, theme().text.patch(theme().match_hit));
    }

    /// The order the theme's module doc states: the match first, so the
    /// selection's own ink wins on a cell that is both.
    #[test]
    fn a_selection_inside_a_match_keeps_the_selection_ink_and_the_bold() {
        let text = "Ada Lovelace";
        let row = TextRow {
            text,
            range: 0..text.len(),
            matched: true,
            // The space at index 3, so the row is split three ways and the
            // selected cell is a cell that is also a match.
            selected: Some(3..4),
            caret: None,
            concealed: false,
            reversed: false,
            ink: ink(),
        };

        let out = spans(&row);
        assert_eq!(plain_text(&out), "Ada Lovelace");
        assert_eq!(
            out.len(),
            3,
            "the row is split into head, selection, tail: {out:?}"
        );
        assert_eq!(out[0].content, "Ada");
        assert_eq!(out[1].content, " ");
        assert_eq!(
            out[1].style,
            theme()
                .text
                .patch(theme().match_hit)
                .patch(theme().selection_bg)
        );
    }

    /// A range that misses the row entirely is not this module's problem to
    /// report: the row is simply not marked.
    #[test]
    fn a_selection_that_misses_the_row_marks_nothing() {
        let text = "Ada";
        let row = TextRow {
            text,
            range: 0..text.len(),
            matched: false,
            selected: Some(40..50),
            caret: None,
            concealed: false,
            reversed: false,
            ink: ink(),
        };

        let out = spans(&row);
        assert_eq!(out.len(), 1, "an empty clip is not a split: {out:?}");
        assert_eq!(plain_text(&out), "Ada");
    }

    /// The rule: on a reversed row the caret is a hole, so it is the row's ink
    /// with the reversal taken off. `remove_modifier` is the whole mechanism, and
    /// it emits "ensure not reversed" rather than "reversed is gone" — which is
    /// what punches the hole and is a no-op anywhere else.
    #[test]
    fn a_caret_on_a_reversed_row_is_the_row_without_its_reversal() {
        let theme = theme();
        let text = "Ada";
        let row = TextRow {
            text,
            range: 0..text.len(),
            matched: false,
            selected: None,
            caret: Some(1),
            concealed: false,
            reversed: true,
            // A reversed row is a row whose ink *is* reversed: the surface patched
            // the row, so the row's own text carries it.
            ink: Ink {
                plain: theme.selection,
                ..Ink::readonly(&theme)
            },
        };

        let out = spans(&row);
        // An overlay, so the text is the text: a card's values must not move a
        // column because the cursor landed on one of them.
        assert_eq!(plain_text(&out), "Ada");
        assert!(
            out.first()
                .is_some_and(|s| s.style.add_modifier.contains(Modifier::REVERSED)),
            "and the rest of the row still is"
        );

        // The hole is the character at the caret's offset, still the reader's and
        // no longer reversed.
        let caret = &out[1];
        assert_eq!(
            caret.content, "d",
            "the character under the caret is still there"
        );
        assert!(
            !caret.style.add_modifier.contains(Modifier::REVERSED),
            "a hole in the reverse video, not another reverse: {:?}",
            caret.style.add_modifier
        );
        // `Color::Reset`, not `text`: the ground a hole shows through to is the
        // terminal's own, which is why the hole needs no contrast floor against
        // it and a second ink would have needed one against `selection`.
        assert_eq!(caret.style.fg, None, "the terminal's own ground");
    }

    /// A caret is the row's ink with the reversal off, so the reversal the
    /// surface applies has to be in `base` for that to be true.
    #[test]
    fn a_caret_on_a_reversed_row_is_legible_whatever_the_row_is_painted_in() {
        let text = "Ada";
        let reversed = Style::default().add_modifier(Modifier::REVERSED);
        let row = TextRow {
            text,
            range: 0..text.len(),
            matched: true,
            selected: None,
            caret: Some(0),
            concealed: false,
            reversed: true,
            ink: Ink {
                plain: reversed,
                ..ink()
            },
        };

        let out = spans(&row);
        // The hole is the first character with the reversal off, and the match
        // underneath it must not put the reversal back.
        let caret = out
            .iter()
            .find(|span| !span.style.add_modifier.contains(Modifier::REVERSED))
            .expect("a hole in the row");
        assert_eq!(caret.content, "A", "and it is the character at the caret");
        assert!(
            !caret.style.add_modifier.contains(Modifier::REVERSED),
            "a match underneath must not put the reversal back: {:?}",
            caret.style.add_modifier
        );
        assert!(
            caret.style.add_modifier.contains(Modifier::BOLD),
            "while the match's own bold survives: {:?}",
            caret.style.add_modifier
        );
    }

    /// The two carets differ by a modifier, because a terminal has no border to
    /// draw a hollow with and a bar that overhangs a row is not a cell.
    #[test]
    fn the_two_carets_differ_by_a_modifier_and_not_by_a_colour() {
        let theme = theme();
        assert_eq!(theme.caret_insert.fg, theme.text.fg);
        assert_eq!(theme.caret_normal.fg, theme.text.fg);
        assert!(theme.caret_insert.add_modifier.contains(Modifier::REVERSED));
        assert!(
            theme
                .caret_normal
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
    }

    /// A caret one past the end of a row belongs to the next row: two rows of a
    /// wrapped value meet at that byte and exactly one of them owns it.
    #[test]
    fn a_caret_on_a_rows_seam_is_painted_by_exactly_one_row() {
        let text = "Ada\nGrace";
        let mut painted = 0;
        for range in [0..4, 4..9] {
            let row = TextRow {
                text,
                range: range.clone(),
                matched: false,
                selected: None,
                caret: Some(3),
                concealed: false,
                reversed: false,
                ink: ink(),
            };
            if caret_index(&spans(&row), theme().caret_normal).is_some() {
                painted += 1;
                assert_eq!(range, 0..4, "the first row owns the seam");
            }
        }
        assert_eq!(painted, 1, "two rows must not both paint the caret");
    }

    /// A caret one past the end of a value is the caret a reader is most often
    /// looking at, because that is where they type, so the last row owns it.
    #[test]
    fn a_caret_past_the_last_character_lands_at_the_end() {
        let text = "Ada";
        let row = TextRow {
            text,
            range: 0..text.len(),
            matched: false,
            selected: None,
            caret: Some(text.len()),
            concealed: false,
            reversed: false,
            ink: ink(),
        };

        let out = spans(&row);
        assert_eq!(out.last().map(|s| s.content.as_ref()), Some(" "));
    }

    /// A caret is a byte offset, and a motion that counts characters has to be
    /// converted before it gets here. The two only diverge once a multi-byte
    /// character has gone by, so the cake comes first: character 2 is byte 5,
    /// because the cake is four bytes and the space after it is the fifth.
    #[test]
    fn a_caret_is_a_byte_offset_and_lands_where_those_bytes_are() {
        let text = "🎂Ada";
        assert_eq!(text.find('A'), Some(4), "four bytes in, not one");
        let row = TextRow {
            text,
            range: 0..text.len(),
            matched: false,
            selected: None,
            caret: Some(4),
            concealed: false,
            reversed: false,
            ink: ink(),
        };

        let out = spans(&row);
        assert_eq!(
            plain_text(&out),
            "🎂Ada",
            "an overlay: the text has not moved"
        );
        assert_eq!(
            caret_index(&out, theme().caret_normal),
            Some(1),
            "and it is the A — the character at byte four, not the byte before it: {out:?}"
        );
    }

    /// The conversion the field's doc names, so the call site's one line is
    /// pinned by something.
    #[test]
    fn a_character_position_from_a_motion_becomes_a_byte_offset_by_one_converter() {
        let text = "🎂Ada";
        let at = 1; // what `char_motion` would return: the A, counting characters
        let offset = rows::byte_span(text, at..at + 1).start;

        let row = TextRow {
            text,
            range: 0..text.len(),
            matched: false,
            selected: None,
            caret: Some(offset),
            concealed: false,
            reversed: false,
            ink: ink(),
        };

        assert_eq!(offset, 4, "one character in is four bytes in");
        assert_eq!(caret_index(&spans(&row), theme().caret_normal), Some(1));
    }

    /// The dot stands in for a space so the caret has a cell to be seen in, and
    /// it is one column wide where the space was, so the wrap does not move.
    #[test]
    fn a_composed_draft_stands_a_dot_in_for_every_space() {
        let theme = theme();
        let text = "two words";
        let row = TextRow {
            text,
            range: 0..text.len(),
            matched: false,
            selected: None,
            caret: None,
            concealed: false,
            reversed: false,
            ink: Ink::draft(&theme, true, false),
        };

        let out = spans(&row);
        assert_eq!(plain_text(&out), "two·words");
        assert_eq!(out[1].content, "·");
        assert_eq!(out[1].style, theme.text_dim);
    }

    /// A draft nobody is typing in is read as prose, and a sentence with its
    /// spaces dotted reads as something else.
    #[test]
    fn a_draft_nobody_is_typing_in_is_dim_and_undotted() {
        let theme = theme();
        let text = "two words";
        let row = TextRow {
            text,
            range: 0..text.len(),
            matched: false,
            selected: None,
            caret: None,
            concealed: false,
            reversed: false,
            ink: Ink::draft(&theme, false, false),
        };

        let out = spans(&row);
        assert_eq!(plain_text(&out), "two words");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].style, theme.text_dim);
    }

    /// A selection inside the line means the *line* is in Visual mode, so it says
    /// so with the mode's own colour rather than with a background.
    #[test]
    fn a_selection_inside_the_line_is_the_visual_modes_ink() {
        let theme = theme();
        let text = "two words";
        let row = TextRow {
            text,
            range: 0..text.len(),
            matched: false,
            selected: Some(0..3),
            caret: None,
            concealed: false,
            reversed: false,
            ink: Ink::draft(&theme, true, false),
        };

        let out = spans(&row);
        let marked = out
            .iter()
            .find(|s| s.content == "two")
            .expect("the selection");
        assert_eq!(marked.style, theme.text.patch(theme.mode_visual));
    }

    #[test]
    fn a_read_only_surface_and_a_draft_never_agree_on_a_selection() {
        let theme = theme();
        assert_ne!(
            Ink::readonly(&theme).selected,
            Ink::draft(&theme, true, false).selected
        );
    }

    /// A row whose range is not inside its text is a caller's bug, but a panic
    /// here would take the frame with it, and `panic = "abort"` is set.
    #[test]
    fn a_range_outside_the_text_paints_nothing_rather_than_panicking() {
        let row = TextRow {
            text: "Ada",
            range: 0..99,
            matched: false,
            selected: None,
            caret: Some(0),
            concealed: false,
            reversed: false,
            ink: ink(),
        };

        let out = spans(&row);
        assert!(out.is_empty(), "an empty slice has no spans: {out:?}");
    }

    #[test]
    fn a_caret_is_only_ever_the_text_colour_the_theme_gave_it() {
        let theme = theme();
        let text = "Ada";
        for caret in [theme.caret_insert, theme.caret_normal] {
            let row = TextRow {
                text,
                range: 0..text.len(),
                matched: false,
                selected: None,
                caret: Some(1),
                concealed: false,
                reversed: false,
                ink: Ink { caret, ..ink() },
            };
            let out = spans(&row);
            let index = caret_index(&out, caret).expect("the caret is there");
            assert_eq!(
                out[index].style.fg, theme.text.fg,
                "a caret is never painted in a colour of its own"
            );
        }
    }

    /// A permuted row, with a caret on it for the first time.
    ///
    /// The caret is a byte offset into the **logical** row, so the piece that
    /// carries it is the one whose logical range holds that offset — the order
    /// the pieces are drawn in has nothing to say about it. At the row's end
    /// there is no character left to mark, so the piece that *ends* there owns
    /// it, on the same terms a whole row owns the byte past it. That is the
    /// case the ownership bound is measured against the row's end for: a caret
    /// painted by no piece is a caret nobody can see.
    ///
    /// The row is the text's second row rather than its first, so `range.start`
    /// is not zero: a piece's `end` is an offset into the text, and the row's
    /// **end** is an offset too where its length is not one.
    #[test]
    fn a_caret_on_a_permuted_row_is_owned_by_the_piece_whose_logical_range_holds_it() {
        let theme = theme();
        let text = "abc שלום עולם";
        // The pieces of the row, in the text's own coordinates.
        let pieces: Vec<Chunk> = bidi::visual_row(&text[4..], bidi::base_direction(text))
            .into_iter()
            .map(|chunk| Chunk {
                logical: (chunk.logical.start + 4)..(chunk.logical.end + 4),
                cells: chunk.cells,
            })
            .collect();
        assert!(
            pieces.len() > 1,
            "a right-to-left row is drawn in pieces: {pieces:?}"
        );

        // (caret, what it marks): past the end of the row there is no character,
        // so it takes the cell of its own; byte 13 is the seam between the space
        // and the second word, and the piece that *starts* there is the owner —
        // a seam belongs to the piece it opens, never to both.
        for (caret, marks) in [(text.len(), " "), (13, &text[13..15])] {
            let row = TextRow {
                text,
                range: 4..text.len(),
                matched: false,
                selected: None,
                caret: Some(caret),
                concealed: false,
                reversed: false,
                ink: ink(),
            };

            let out = spans_permuted(&row, &pieces);
            let owners: Vec<&Span<'_>> = out
                .iter()
                .filter(|span| span.style == theme.caret_normal)
                .collect();
            assert_eq!(
                owners.len(),
                1,
                "exactly one piece owns the caret at byte {caret}: {out:?}"
            );
            assert_eq!(
                owners[0].content, marks,
                "and it is the piece holding byte {caret}: {out:?}"
            );
        }
    }
}
