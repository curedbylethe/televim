//! Soft wrapping: how much of a message fits on one row.
//!
//! The panel draws a message on as many rows as its text needs at the width it
//! was given, and a row is the unit every other piece of the geometry is
//! measured in. This is where that number comes from: a pure function of text
//! and width, so a row's height can be worked out without drawing anything, and
//! without anyone measuring the same thing twice.

use std::ops::Range;

use unicode_width::UnicodeWidthStr;

use crate::grapheme::clusters;

/// How many terminal columns `text` occupies.
///
/// The one place a cell count is computed. A column is a cell, not a
/// character: an emoji is two, a combining mark is none, and a CJK ideograph is
/// two. `unicode-width` is the table, and it answers the multi-part emoji
/// sequences as one cell count rather than as the sum of their parts — a
/// fully-qualified ZWJ family, a modifier sequence (`👍🏽`) and a VS16 sequence
/// (`❤️`) are each two columns wide, which is what the terminal draws. A row
/// asks it of one cluster at a time, and a caret column is the same function
/// over the slice in front of the caret.
#[must_use]
pub fn columns(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Splits `text` into the rows it occupies at `width` columns.
///
/// Byte offsets, always at a character boundary, so that a row is `&text[range]`
/// and nothing else: a boundary a range cannot be indexed with is not one. A
/// row is never empty, so a message is never zero rows tall even when its text
/// is.
///
/// Whitespace is where a row is broken, when there is whitespace to break it.
/// A reader scanning a conversation wants its words whole, and a word is cut
/// only where no space falls — that is, where the word is wider than the panel.
/// Whitespace at the end of a row does not count toward its width, because a
/// terminal does not draw it and a break decided with it counted would wrap
/// early for nothing. A newline in the text starts a row, which is what makes
/// this the same function for a message that carries one.
///
/// A column is a cell, counted by [`columns`] over one grapheme cluster at a
/// time — so a row of emoji in a ten-column panel holds five of them, and a
/// ZWJ family is the two cells the terminal draws. A cluster that does not fit
/// the room left ends the row before it, which can leave the row short of
/// `width`. The one row wider than the panel is a cluster wider than the row
/// it starts: an empty row still takes it, because dropping it and looping on
/// it are both worse.
#[must_use]
pub fn wrap(text: &str, width: u16) -> Vec<Range<usize>> {
    rows(text, width, 0, 0, false)
}

/// Splits `text` into the rows it occupies at `width`, leaving the whitespace a
/// row ends with *on* that row.
///
/// The same rows as [`wrap`], and the same count: the space run a row is broken
/// at is drawn on the row before the break rather than given to neither, and
/// trailing whitespace reaches the end of its line instead of stopping short of
/// it. Nothing is wider for it — a run of spaces is only ever recorded while the
/// row still has room after it — so a caret in one of those spaces has a cell of
/// its own to stand in.
///
/// This is for the input bar, where a space the reader typed has to be a cell
/// they can see: dropped, the bar is exactly as blank after the key as it was
/// before it. The conversation wraps with [`wrap`], because a message is read as
/// prose and its trailing spaces are not what the reader is looking at.
#[must_use]
pub fn wrap_keeping_whitespace(text: &str, width: u16) -> Vec<Range<usize>> {
    rows(text, width, 0, 0, true)
}

/// Splits `text` into rows, where the first shares `prefix` columns with a
/// decoration in front of it and every row gives `suffix` columns to one behind
/// it.
///
/// The decorations belong to a message rather than to a row of it: who it is
/// from, the message it quotes, whether it is still on its way. There is one
/// set of them per message, so they go on the first row — the row the reader
/// reaches the message by — and the last row is what gives up the width a
/// trailing one needs.
///
/// Both are subtracted from the width here rather than clipped by the terminal
/// afterwards, because a terminal clips silently: what was pushed off the row
/// would be text the reader can scroll to and never see.
pub fn wrap_decorated(text: &str, prefix: usize, suffix: usize, width: u16) -> Vec<Range<usize>> {
    rows(text, width, prefix, suffix, false)
}

/// Lays `text` out, one line at a time.
///
/// Each line always contributes at least one row, so the text's own newlines
/// are what separate the rows rather than an accident of the widths.
fn rows(text: &str, width: u16, prefix: usize, suffix: usize, keep: bool) -> Vec<Range<usize>> {
    // A panel with no width is not a panel, but it must not be a loop either:
    // every character still gets a row of its own, and no row is nothing.
    let width = usize::from(width).max(1);
    let mut laid_out: Vec<Range<usize>> = Vec::new();
    let mut offset = 0;

    for line in text.split('\n') {
        fill(&mut laid_out, line, offset, width, prefix, suffix, keep);
        offset += line.len() + 1;
    }

    laid_out
}

/// Lays one line out, appending its rows to `laid_out`.
///
/// One row at a time, because where a row ends is only known once the row
/// before it has been looked at: a row is cut at the last space that fits, and
/// that space is behind the point the scan has reached.
fn fill(
    laid_out: &mut Vec<Range<usize>>,
    line: &str,
    offset: usize,
    width: usize,
    prefix: usize,
    suffix: usize,
    keep: bool,
) {
    let mut from = 0;

    loop {
        if !keep {
            from = past_spaces(line, from);
        }
        let limit = row_width(laid_out.len(), width, prefix, suffix);
        let (end, next) = one_row(line, from, limit, keep);
        laid_out.push(offset + from..offset + end);

        if next <= from || next >= line.len() {
            break;
        }
        from = next;
    }
}

/// The row beginning at `from`: where it ends, and where the next one begins.
///
/// The row ends at the last run of whitespace that fits, so that no word is
/// left behind; a row with no such space in it is cut where the edge falls,
/// which is the only thing to be done with a word wider than the panel. A
/// cluster that does not fit the room left ends the row before it, so a row
/// can finish short of `limit`. The one row wider than `limit` is a cluster
/// wider than the row it starts: an empty row still takes it, because the
/// alternative is to loop. A space is only remembered while the row still has
/// room after it.
///
/// With `keep`, the run of spaces is the row's end rather than the gap between
/// it and the next one — see [`wrap_keeping_whitespace`].
fn one_row(line: &str, from: usize, limit: usize, keep: bool) -> (usize, usize) {
    let mut at_space: Option<(usize, usize)> = None;
    let mut used = 0;

    // One cluster at a time, from `from` rather than from the start of the
    // line: a frame lays every visible message out, and walking each row from
    // the beginning would be quadratic. `columns` of the cluster is what the
    // terminal draws, so a family is two cells and the row ends before a
    // cluster that does not fit rather than inside it.
    for (offset, cluster) in clusters(&line[from..]) {
        let at = from + offset;
        let width = columns(cluster);

        // Whether this cluster *fits*, rather than whether the row has a
        // column left: on ASCII the two are the same, and on a two-cell
        // cluster they are not. An empty row is always given this one, which
        // is what stops a panel narrower than a cluster from looping.
        if used > 0 && used + width > limit {
            let (start, next) = at_space.unwrap_or((at, at));
            return if keep { (next, next) } else { (start, next) };
        }
        used += width;

        if cluster.chars().all(char::is_whitespace) {
            // The first space of a run is where the row can be broken. The rest
            // of the run only has to be remembered, so that the row after it
            // begins on a word rather than on a space.
            let end = at + cluster.len();
            at_space = Some(match at_space {
                Some((start, next)) if next == at => (start, end),
                _ => (at, end),
            });
        }
    }

    // The line ran out inside the row, so it is the line's end the row reaches
    // — except where the last space run runs to that end, which is trailing
    // whitespace: the terminal does not draw it, and neither does the slice the
    // panel takes. Keeping it gives those spaces a cell each, which is the whole
    // of what the input bar wants and the whole of what it costs to have.
    let end = match at_space {
        Some((start, next)) if next >= line.len() && !keep => start,
        _ => line.len(),
    };

    (end, line.len())
}

/// How far the whitespace at the start of a row runs.
///
/// That run is the one the row before it was broken at, so it belongs to
/// neither row: kept, it would begin a row with a blank column and leave the
/// two rows disagreeing about where a word starts. [`wrap_keeping_whitespace`]
/// keeps it on the row before the break instead, so the row after the break
/// still begins on a word and nothing calls for this.
fn past_spaces(line: &str, from: usize) -> usize {
    let rest = &line[from..];
    from + rest.len() - rest.trim_start_matches(char::is_whitespace).len()
}

/// The columns one row has for text, given what is drawn on it.
fn row_width(row: usize, width: usize, prefix: usize, suffix: usize) -> usize {
    let reserved = if row == 0 { prefix } else { suffix };
    width.saturating_sub(reserved).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The character ranges a text is laid out into.
    fn bounds(text: &str, width: u16) -> Vec<Range<usize>> {
        wrap(text, width)
    }

    /// The rows a text is laid out into, as the text of each.
    ///
    /// Indexed rather than counted, so that a range which was not on a character
    /// boundary would fail to compile.
    fn rows_of(text: &str, width: u16) -> Vec<&str> {
        bounds(text, width)
            .into_iter()
            .map(|range| &text[range])
            .collect()
    }

    /// The rows a text is laid out into, as the text of each, with the
    /// whitespace kept on the row it ends with.
    fn kept_rows_of(text: &str, width: u16) -> Vec<&str> {
        wrap_keeping_whitespace(text, width)
            .into_iter()
            .map(|range| &text[range])
            .collect()
    }

    // ---- the whitespace an input bar has to draw -------------------------

    /// The space the reader typed at the end of a draft is a cell of the row, or
    /// the bar is exactly as blank after the key as it was before it.
    #[test]
    fn trailing_whitespace_reaches_the_end_of_its_row_when_it_is_kept() {
        assert_eq!(kept_rows_of("hi  ", 10), vec!["hi  "]);
    }

    /// The case with nothing else on the bar to go on: a draft of nothing but
    /// spaces has no visible character in it at all.
    #[test]
    fn a_line_of_nothing_but_spaces_is_a_row_of_them() {
        assert_eq!(kept_rows_of("   ", 10), vec!["   "]);
        assert_eq!(kept_rows_of("a\n   \nb", 10), vec!["a", "   ", "b"]);
    }

    /// The run a row is broken at belongs to the row before the break rather
    /// than to neither of them — and the row after it still begins on a word,
    /// which is the reason the run is not simply left at the start of a row.
    #[test]
    fn the_run_a_row_is_broken_at_stays_on_that_row() {
        assert_eq!(kept_rows_of("alpha   beta", 8), vec!["alpha   ", "beta"]);
        assert_eq!(rows_of("alpha   beta", 8), vec!["alpha", "beta"]);
    }

    /// Nothing is wider for keeping the whitespace, because a run of spaces is
    /// only ever recorded while the row still has room after it. A row that did
    /// overflow would push the caret off the end of the bar.
    #[test]
    fn keeping_the_whitespace_makes_no_row_wider() {
        let texts = [
            "the quick brown fox jumps over the lazy dog",
            "a  b   c    d",
            "   leading spaces and trailing ones   ",
            "one\ntwo   \n   three",
        ];

        for text in texts {
            for width in [1, 3, 8, 12, 80] {
                for row in kept_rows_of(text, width) {
                    assert!(
                        columns(row) <= usize::from(width),
                        "{row:?} is wider than {width} in {text:?}"
                    );
                }
            }
        }
    }

    /// The same rows and the same number of them, which is what lets the bar
    /// measure a draft's height with one function and lay it out with the other.
    #[test]
    fn keeping_the_whitespace_adds_no_row() {
        let text = "alpha  beta   gamma\n  delta  ";

        assert_eq!(
            kept_rows_of(text, 12).len(),
            rows_of(text, 12).len(),
            "{:?}",
            kept_rows_of(text, 12)
        );
    }

    // ---- the rules -------------------------------------------------------

    #[test]
    fn an_empty_text_is_still_a_row() {
        assert_eq!(bounds("", 10), vec![0..0], "a message is never zero rows");
    }

    #[test]
    fn a_row_is_never_wider_than_the_width() {
        for text in ["the quick brown fox jumps over the lazy dog"] {
            for width in [1, 3, 8, 12, 80] {
                for row in rows_of(text, width) {
                    assert!(
                        columns(row) <= usize::from(width),
                        "{row:?} is {} columns wide in {text:?} at {width}",
                        columns(row)
                    );
                }
            }
        }
    }

    /// The same invariant with characters that are not one cell, which is the
    /// case a terminal clips silently: a row laid out one column too wide loses
    /// its last character to the edge, and the row after it is pushed out of
    /// sight. A panel *narrower* than a two-cell character is the documented
    /// exception — that character still gets a row, because dropping it and
    /// looping on it are both worse.
    #[test]
    fn a_row_of_wide_characters_is_never_wider_than_the_width() {
        for text in [
            "😀😀😀😀😀😀 mixed with 漢字 and é",
            "👨‍👩‍👧 family and 👍🏽 and ❤️ in one row",
        ] {
            for width in [3, 8, 12, 80] {
                for row in rows_of(text, width) {
                    assert!(
                        columns(row) <= usize::from(width),
                        "{row:?} is {} columns wide in {text:?} at {width}",
                        columns(row)
                    );
                }
            }
        }
    }

    #[test]
    fn a_row_is_never_empty() {
        assert_eq!(rows_of("hello", 40), vec!["hello"]);

        // Every character gets a row of its own, so none of them is nothing.
        assert_eq!(rows_of("hello", 1), vec!["h", "e", "l", "l", "o"]);
    }

    #[test]
    fn a_word_is_kept_whole_where_there_is_a_space_to_break_it() {
        assert_eq!(rows_of("alpha beta gamma", 11), vec!["alpha beta", "gamma"]);
    }

    #[test]
    fn a_word_wider_than_the_panel_is_cut_where_the_edge_falls() {
        assert_eq!(
            rows_of("antidisestablishmentarianism", 6),
            vec!["antidi", "sestab", "lishme", "ntaria", "nism"]
        );
    }

    #[test]
    fn trailing_whitespace_does_not_count_toward_a_row() {
        // Five columns of text and four of spaces still fit a row of six: the
        // terminal draws neither the spaces nor a break decided with them.
        assert_eq!(rows_of("hello    world", 9), vec!["hello", "world"]);
    }

    #[test]
    fn a_newline_starts_a_row() {
        assert_eq!(rows_of("one\ntwo", 40), vec!["one", "two"]);
        assert_eq!(
            rows_of("one\ntwo", 40),
            vec!["one", "two"],
            "a newline is a row whatever the width"
        );
    }

    #[test]
    fn a_character_wider_than_the_panel_still_occupies_its_own_row() {
        // A panel one column wide cannot hold a two-column character, and the
        // alternative is dropping it or looping on it forever.
        assert_eq!(rows_of("ab", 1), vec!["a", "b"]);
    }

    #[test]
    fn a_panel_with_no_width_lays_out_one_character_per_row() {
        assert_eq!(rows_of("hi", 0), vec!["h", "i"]);
    }

    // ---- the rules together ----------------------------------------------

    #[test]
    fn a_message_of_only_whitespace_is_one_row() {
        assert_eq!(rows_of("   \t  ", 10), vec![""]);
    }

    #[test]
    fn lines_are_wrapped_independently() {
        // The second line starts at a row of its own rather than finishing the
        // first one's.
        assert_eq!(rows_of("alpha beta\nc", 11), vec!["alpha beta", "c"]);
    }

    #[test]
    fn a_trailing_newline_is_an_empty_row_of_its_own() {
        assert_eq!(rows_of("one\n", 40), vec!["one", ""]);
    }

    /// The terminal gives an emoji two cells, so a row of a ten-column bar
    /// holds five of them rather than ten — and the break falls where the
    /// terminal's would, which is the only way a row's range and the drawing of
    /// it agree.
    #[test]
    fn a_wide_character_is_two_columns() {
        let text = "😀".repeat(12);

        assert_eq!(
            rows_of(&text, 10),
            vec!["😀😀😀😀😀", "😀😀😀😀😀", "😀😀"],
            "five to a row, and the twelfth is the start of a third"
        );
    }

    #[test]
    fn an_emoji_at_the_break_point_is_not_split() {
        // Counted as the two cells the terminal draws it in, so a break never
        // lands inside it — a range that did would not be a range that can be
        // indexed — and never makes the row wider than the panel either. Here
        // that means the emoji begins a row of its own rather than finishing the
        // one before it.
        assert_eq!(rows_of("ab😀cd", 3), vec!["ab", "😀c", "d"]);
    }

    /// A row's ends are cluster boundaries. A cluster that does not fit is left
    /// for the next row, and the one row allowed to be wider than the panel is
    /// a single cluster that was wider than the row it started.
    #[test]
    fn a_row_is_never_cut_inside_a_cluster() {
        let texts = [
            "a👨‍👩‍👧b👍🏽c🇬🇧d❤️e",
            "👨‍👩‍👧 family and 👍🏽 and ❤️ in one row",
            "the quick brown fox",
            "a\n👨‍👩‍👧\nb",
        ];
        for text in texts {
            for width in [1, 2, 3, 4, 8] {
                for keep in [false, true] {
                    let ranges = if keep {
                        wrap_keeping_whitespace(text, width)
                    } else {
                        wrap(text, width)
                    };
                    let mut prev = 0;
                    for range in ranges {
                        assert!(
                            range.start >= prev && range.end >= range.start,
                            "{range:?} overlaps the row before it in {text:?} at {width}"
                        );
                        prev = range.end;
                        for at in [range.start, range.end] {
                            assert_eq!(
                                crate::grapheme::cluster_start(text, at),
                                at,
                                "a row of {text:?} at {width} starts inside a cluster"
                            );
                            assert_eq!(
                                crate::grapheme::cluster_end(text, at),
                                at,
                                "a row of {text:?} at {width} ends inside a cluster"
                            );
                        }
                        let row = &text[range];
                        let single = clusters(row).count() == 1;
                        assert!(
                            columns(row) <= usize::from(width) || single,
                            "{row:?} is {} columns, and more than one cluster, in {text:?} at {width}",
                            columns(row)
                        );
                    }
                }
            }
        }
    }

    /// `a` is one column and the family is two, so a two-column row ends after
    /// the `a` with a column spare rather than cutting the family to fill it.
    #[test]
    fn a_row_may_end_short_of_its_limit() {
        let family = "👨‍👩‍👧";
        let text = format!("a{family}");

        assert_eq!(rows_of(&text, 2), vec!["a", family]);
        assert_eq!(kept_rows_of(&text, 2), vec!["a", family]);
        assert!(columns("a") < 2, "the first row stops short of the limit");
        assert_eq!(columns(family), 2);
    }

    // ---- decorations -----------------------------------------------------

    #[test]
    fn the_first_row_gives_up_the_columns_its_prefix_takes() {
        let decorated = wrap_decorated("alpha beta gamma", 6, 0, 11);

        assert_eq!(decorated, vec![0..5, 6..16], "the prefix eats five");
    }

    /// The first row has no prefix to give up, so a suffix is paid for on the
    /// rows that follow — which is where it is drawn.
    #[test]
    fn every_row_but_the_first_gives_up_the_columns_its_suffix_takes() {
        let decorated = wrap_decorated("abcdefghijkl", 0, 3, 7);

        assert_eq!(decorated, vec![0..7, 7..11, 11..12]);
    }

    #[test]
    fn a_prefix_wider_than_the_panel_leaves_the_text_a_column() {
        // Nothing to give: every character still gets a row, rather than the
        // layout looping or dropping the text.
        let decorated = wrap_decorated("ab", 20, 0, 8);

        assert_eq!(decorated, vec![0..1, 1..2]);
    }

    #[test]
    fn the_rows_of_a_text_tile_the_lines_it_is_made_of() {
        let text = "alpha beta\n\ngamma";
        let rows = rows_of(text, 8);

        assert_eq!(rows, vec!["alpha", "beta", "", "gamma"]);
    }
}
