//! Soft wrapping: how much of a message fits on one row.
//!
//! The panel draws a message on as many rows as its text needs at the width it
//! was given, and a row is the unit every other piece of the geometry is
//! measured in. This is where that number comes from: a pure function of text
//! and width, so a row's height can be worked out without drawing anything, and
//! without anyone measuring the same thing twice.

use std::ops::Range;

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
/// A column is a character. A cell is what a character occupies for most of
/// what a conversation holds; what a double-width character or a combining
/// mark costs is a fact about the terminal's font rather than about the text,
/// and is not answered here.
#[must_use]
pub fn wrap(text: &str, width: u16) -> Vec<Range<usize>> {
    rows(text, width, 0, 0)
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
    rows(text, width, prefix, suffix)
}

/// Lays `text` out, one line at a time.
///
/// Each line always contributes at least one row, so the text's own newlines
/// are what separate the rows rather than an accident of the widths.
fn rows(text: &str, width: u16, prefix: usize, suffix: usize) -> Vec<Range<usize>> {
    // A panel with no width is not a panel, but it must not be a loop either:
    // every character still gets a row of its own, and no row is nothing.
    let width = usize::from(width).max(1);
    let mut laid_out: Vec<Range<usize>> = Vec::new();
    let mut offset = 0;

    for line in text.split('\n') {
        fill(&mut laid_out, line, offset, width, prefix, suffix);
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
) {
    let mut from = 0;

    loop {
        from = past_spaces(line, from);
        let limit = row_width(laid_out.len(), width, prefix, suffix);
        let (end, next) = one_row(line, from, limit);
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
/// which is the only thing to be done with a word wider than the panel. Neither
/// is wider than `limit`: the row stops at the first character that does not
/// fit, and a space is only remembered while the row still has room after it.
fn one_row(line: &str, from: usize, limit: usize) -> (usize, usize) {
    let mut at_space: Option<(usize, usize)> = None;

    for (used, (offset, character)) in line[from..].char_indices().enumerate() {
        let at = from + offset;
        if used >= limit {
            return at_space.unwrap_or((at, at));
        }

        if character.is_whitespace() {
            // The first space of a run is where the row can be broken. The rest
            // of the run only has to be remembered, so that the row after it
            // begins on a word rather than on a space.
            at_space = Some(match at_space {
                Some((start, next)) if next == at => (start, at + character.len_utf8()),
                _ => (at, at + character.len_utf8()),
            });
        }
    }

    // The line ran out inside the row, so it is the line's end the row reaches
    // — except where the last space run runs to that end, which is trailing
    // whitespace: the terminal does not draw it, and neither does the slice the
    // panel takes.
    let end = match at_space {
        Some((start, next)) if next >= line.len() => start,
        _ => line.len(),
    };

    (end, line.len())
}

/// How far the whitespace at the start of a row runs.
///
/// That run is the one the row before it was broken at, so it belongs to
/// neither row: kept, it would begin a row with a blank column and leave the
/// two rows disagreeing about where a word starts.
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

    // ---- the rules -------------------------------------------------------

    #[test]
    fn an_empty_text_is_still_a_row() {
        assert_eq!(bounds("", 10), vec![0..0], "a message is never zero rows");
    }

    #[test]
    fn a_row_is_never_wider_than_the_width() {
        let text = "the quick brown fox jumps over the lazy dog";

        for row in rows_of(text, 12) {
            assert!(row.chars().count() <= 12, "{row:?} is wider than the panel");
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

    #[test]
    fn an_emoji_at_the_break_point_is_not_split() {
        // Counted as the one character it is, so a break never lands inside it
        // — and a range that did would not be a range that can be indexed.
        assert_eq!(rows_of("ab😀cd", 3), vec!["ab😀", "cd"]);
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
