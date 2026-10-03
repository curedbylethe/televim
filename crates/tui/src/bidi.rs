//! Which way a row reads, and the order its pieces are drawn in.
//!
//! A terminal draws the cells it is handed, left to right, in the order it is
//! handed them. It does not know that `שלום` is one word read from the right, so
//! nothing here can rely on it: a row that reads right-to-left has to arrive at
//! the terminal as an ordered list of pieces, already permuted.
//!
//! Two questions get asked, and they are asked of different things. **Which
//! direction** is a property of the message as a whole, and is answered by the
//! first strong character anywhere in it — [`base_direction`]. **In what order
//! the pieces go** is a property of one wrapped row, and is answered by the
//! Unicode Bidirectional Algorithm over that row alone, at the base direction the
//! message was given ([`visual_row`]).
//!
//! A row is not a reordered `String`. It stays `&text[range]` — [`crate::wrap`]
//! needs that, and so does every row range in the panel — and what comes back is
//! a list of *logical* byte ranges in the order they are drawn. Each range is a
//! whole number of grapheme clusters: a row never tears a ZWJ family, because
//! [`crate::grapheme`] is what says where a cluster ends. Every index the
//! algorithm hands over is a byte index into a multi-byte character, so it is
//! snapped before it is used.
//!
//! Not here: shaping (a terminal that will not shape draws shaped text from a
//! font that will, and this has no opinion), mirroring (rule L4, which no
//! terminal applies either), and line breaking in visual order (rows are broken
//! logically, in [`crate::wrap`], which is the order the text is stored in).

use std::ops::Range;

pub use unicode_bidi::Direction;
use unicode_bidi::{Level, ParagraphBidiInfo, get_base_direction_full};

use crate::grapheme::clusters;
use crate::wrap::columns;

/// One piece of a row, as it is drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    /// Byte range of the piece in the logical string. Whole grapheme clusters,
    /// ascending, and disjoint from every other chunk's — so
    /// `&text[chunk.logical]` is always text, never a byte sequence that only
    /// looks like it.
    pub logical: Range<usize>,
    /// Terminal columns `&text[chunk.logical]` occupies, by [`columns`].
    pub cells: usize,
}

/// Which direction the message reads.
///
/// The first strongly-directional character in the *whole* text, which is not the
/// same as the first one in its first paragraph: a message that opens with
/// `---` and then says `שלום` is right-to-left, and the neutrals at the top
/// cannot outvote it. So this asks across paragraph separators and keeps looking
/// — the direction of an all-neutral message is
/// [`Direction::Mixed`], and the caller decides what that means.
#[must_use]
pub fn base_direction(text: &str) -> Direction {
    get_base_direction_full(text)
}

/// The pieces of one wrapped row, in the order the terminal is handed them.
///
/// `text` is a single row — `&message_text[range]`, no newline — and `base` is
/// [`base_direction`] of the message it came from, because a row of an
/// all-neutral message carries no evidence of its own.
///
/// Left-to-right text comes back as one chunk covering the whole row: nothing
/// moved, so nothing is said twice. Right-to-left text comes back reversed, one
/// chunk per cluster, because a chunk is a logical slice drawn *as written* and
/// a Hebrew word is not written in the order it is read. Embedded
/// left-to-right runs — digits, a bracketed word in an otherwise right-to-left
/// row — stay whole, in the order they are written.
///
/// The chunks partition the row: no byte is dropped, none is drawn twice, and
/// their widths sum to [`columns`]`(text)`, whatever the permutation did.
// `Direction` is neither `Copy` nor `Clone`, so by value is what the caller has
// in hand: `base_direction` handed it to them, and a borrow would only make them
// keep the local alive.
#[allow(clippy::needless_pass_by_value)]
#[must_use]
pub fn visual_row(text: &str, base: Direction) -> Vec<Chunk> {
    if text.is_empty() {
        return Vec::new();
    }

    // Rule P2/P3: the base direction is the paragraph's level, and text with no
    // strong character in it is left-to-right.
    let base_level = match base {
        Direction::Ltr => Some(Level::ltr()),
        Direction::Rtl => Some(Level::rtl()),
        Direction::Mixed => None,
    };

    // One cluster, one level. The algorithm works per byte, and a multi-byte
    // character is one character: every byte of it carries the same level, so
    // the level at a cluster's start is the level of the cluster.
    let spans: Vec<Range<usize>> = clusters(text)
        .map(|(start, cluster)| start..start + cluster.len())
        .collect();
    // Rule L1 runs first: trailing whitespace takes the paragraph's level, which
    // is what puts the space after a right-to-left word where a reader looks for
    // it rather than at the other end of the row.
    let byte_levels = ParagraphBidiInfo::new(text, base_level).reordered_levels(0..text.len());
    let levels: Vec<Level> = spans.iter().map(|span| byte_levels[span.start]).collect();

    // Rule L2: each visual position, as the index of the cluster drawn there.
    let visual = ParagraphBidiInfo::reorder_visual(&levels);

    // A chunk may only grow while the visual order walks the logical string
    // forwards — a reversed run has no logical slice to hand the painter, so it
    // is a chunk per cluster rather than a chunk drawn backwards.
    let mut logical: Vec<Range<usize>> = Vec::new();
    for index in visual {
        let span = &spans[index];
        match logical.last_mut() {
            Some(last) if last.end == span.start => last.end = span.end,
            _ => logical.push(span.clone()),
        }
    }

    logical
        .into_iter()
        .map(|range| Chunk {
            cells: columns(&text[range.clone()]),
            logical: range,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chunks as `(start, end, cells)`, which pins the order *and* the
    /// logical range of each one.
    fn shape(text: &str, base: Direction) -> Vec<(usize, usize, usize)> {
        visual_row(text, base)
            .into_iter()
            .map(|c| (c.logical.start, c.logical.end, c.cells))
            .collect()
    }

    fn shape_of(text: &str) -> Vec<(usize, usize, usize)> {
        shape(text, base_direction(text))
    }

    #[test]
    fn a_pure_hebrew_word_is_reversed_one_cluster_at_a_time() {
        let text = "שלום";
        assert_eq!(base_direction(text), Direction::Rtl);

        assert_eq!(
            shape_of(text),
            vec![(6, 8, 1), (4, 6, 1), (2, 4, 1), (0, 2, 1)],
            "rightmost first, and no chunk claims to be written the other way"
        );
    }

    #[test]
    fn an_arabic_word_is_reversed_the_same_way() {
        let text = "مرحبا";
        assert_eq!(base_direction(text), Direction::Rtl);

        let chunks = shape_of(text);
        let drawn: Vec<&str> = visual_row(text, Direction::Rtl)
            .iter()
            .map(|c| &text[c.logical.clone()])
            .collect();

        assert_eq!(chunks.len(), 5, "five clusters, none merged");
        assert_eq!(
            drawn,
            vec!["ا", "ب", "ح", "ر", "م"],
            "drawn rightmost first"
        );
    }

    #[test]
    fn leading_and_trailing_neutrals_travel_with_the_direction() {
        let text = " שלום ";
        assert_eq!(base_direction(text), Direction::Rtl);

        assert_eq!(
            shape_of(text),
            vec![
                (9, 10, 1),
                (7, 9, 1),
                (5, 7, 1),
                (3, 5, 1),
                (1, 3, 1),
                (0, 1, 1),
            ],
            "rule L1 leaves the trailing space at the paragraph's level, so it
             draws rightmost, where the sentence it ends is written"
        );
    }

    #[test]
    fn an_all_neutral_row_reads_left_to_right() {
        let text = "--- 123 ---";
        assert_eq!(base_direction(text), Direction::Mixed);

        assert_eq!(
            shape_of(text),
            vec![(0, text.len(), text.len())],
            "no strong character, so rule P3 leaves it alone"
        );
    }

    #[test]
    fn a_digits_run_stays_in_the_order_it_is_written() {
        let text = "שלום 123";
        assert_eq!(base_direction(text), Direction::Rtl);

        assert_eq!(
            shape_of(text),
            vec![
                (9, 12, 3),
                (8, 9, 1),
                (6, 8, 1),
                (4, 6, 1),
                (2, 4, 1),
                (0, 2, 1),
            ],
            "the number is one left-to-right run of three cells, drawn as written"
        );
    }

    #[test]
    fn an_embedded_bracket_pair_is_split_by_what_it_holds() {
        let text = "(שלום)";
        assert_eq!(base_direction(text), Direction::Rtl);

        assert_eq!(
            shape_of(text),
            vec![
                (9, 10, 1),
                (7, 9, 1),
                (5, 7, 1),
                (3, 5, 1),
                (1, 3, 1),
                (0, 1, 1),
            ],
            "the closing bracket draws rightmost, the word inside it next, and the
             opening bracket last"
        );
    }

    #[test]
    fn left_to_right_text_is_one_chunk_covering_the_row() {
        let text = "hello 123 (world)";
        assert_eq!(base_direction(text), Direction::Ltr);

        assert_eq!(shape_of(text), vec![(0, text.len(), columns(text))]);
    }

    #[test]
    fn an_empty_row_is_no_chunks() {
        assert_eq!(visual_row("", Direction::Ltr), Vec::new());
        assert_eq!(visual_row("", Direction::Rtl), Vec::new());
    }

    #[test]
    fn a_zwj_family_is_never_torn_apart() {
        let family = "👨‍👩‍👧";
        let text = format!("a{family}ש");

        for chunk in visual_row(&text, Direction::Rtl) {
            assert!(
                text.is_char_boundary(chunk.logical.start)
                    && text.is_char_boundary(chunk.logical.end),
                "a chunk boundary inside a character"
            );
        }

        let drawn: Vec<&str> = visual_row(&text, Direction::Rtl)
            .iter()
            .map(|c| &text[c.logical.clone()])
            .collect();
        assert_eq!(drawn, vec!["ש", family, "a"], "the family is one piece");
    }

    /// The permutation is a bijection over the row's clusters, and it does not
    /// change how wide the row is.
    #[test]
    fn the_permutation_is_a_bijection_and_keeps_the_width() {
        const ROWS: &[&str] = &[
            "",
            "abc",
            "שלום",
            "שלום 123",
            "(שלום)",
            " hello (world) 123 ",
            "שלום 123 مرحبا",
            "אבג (גדה) 456",
            "a👨‍👩‍👧ש 1",
            "--- 123 ---",
        ];

        for row in ROWS {
            for base in [Direction::Ltr, Direction::Rtl, Direction::Mixed] {
                // `Direction` is neither `Copy` nor `Clone`, so the row is asked
                // about before the message is named.
                let at = format!("{base:?}");
                let chunks = visual_row(row, base);

                // Expanded to one entry per cluster, so a chunk that swallowed
                // three of them still counts as three.
                let mut drawn: Vec<(usize, usize)> = chunks
                    .iter()
                    .flat_map(|chunk| {
                        clusters(&row[chunk.logical.clone()])
                            .map(move |(start, cluster)| {
                                let from = chunk.logical.start + start;
                                (from, from + cluster.len())
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect();
                let order = drawn.clone();
                drawn.sort_unstable();
                drawn.dedup();

                assert_eq!(
                    drawn.len(),
                    order.len(),
                    "a cluster is drawn twice in {row:?} at {at}"
                );

                let mut expected: Vec<(usize, usize)> = clusters(row)
                    .map(|(start, cluster)| (start, start + cluster.len()))
                    .collect();
                expected.sort_unstable();
                assert_eq!(
                    drawn, expected,
                    "the chunks do not partition the clusters of {row:?} at {at}"
                );

                assert_eq!(
                    chunks.iter().map(|c| c.cells).sum::<usize>(),
                    columns(row),
                    "the permutation changed the width of {row:?} at {at}"
                );

                // Every chunk is indexable, so a painter can slice with it.
                for chunk in &chunks {
                    assert!(
                        row.is_char_boundary(chunk.logical.start)
                            && row.is_char_boundary(chunk.logical.end),
                        "{:?} is not indexable in {row:?}",
                        chunk.logical
                    );
                }
            }
        }
    }
}
