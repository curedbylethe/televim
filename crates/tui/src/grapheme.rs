//! Where a grapheme cluster begins and ends.
//!
//! Three questions get asked of the same text, and they do not share an answer.
//! A **delete** removes the whole cluster — a backspace over a family removes
//! the family. A **row** is never cut inside a cluster — a row that starts on a
//! lone `👨` draws a stranger. The **caret** steps one code point, which is
//! what `h` and `l` do and what a caret that jumped an eleven-byte family in
//! one press would stop doing. This module answers the first two. The caret
//! stays a code point on purpose, in [`crate::line`], and a caret may sit
//! inside a cluster: nothing is deleted across it until a key asks to delete.
//!
//! A cluster is an extended grapheme cluster. That is the unit a terminal draws
//! as one glyph: a ZWJ family, an emoji plus its skin tone, a pair of regional
//! indicators, a character plus VS16. A newline is its own cluster, which is
//! what keeps a delete from walking across a line break to eat the line on the
//! other side.
//!
//! Both functions are total. Any index in `0..=len` returns an index in
//! `0..=len`, and an index that is already a cluster boundary — including
//! either end of the text — is returned as itself. An index *inside* a cluster
//! widens to that cluster's edge. Widening a boundary would take the next
//! cluster too, and an ASCII delete would remove the character after the one
//! the key asked for.

use unicode_segmentation::UnicodeSegmentation;

/// The clusters of `text`, as byte offset and text.
///
/// Extended clusters, so a family is one item. The offset is into `text`.
pub(crate) fn clusters(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.grapheme_indices(true)
}

/// The start of the cluster `at` falls in.
///
/// An index on a boundary is that boundary. `text.len()` and anything past it
/// is `text.len()`.
#[must_use]
pub fn cluster_start(text: &str, at: usize) -> usize {
    edges(text, at).0
}

/// The end of the cluster `at` falls in.
///
/// An index on a boundary is that boundary, not the end of the cluster that
/// starts there: the end of a delete is exclusive, and it already names the
/// edge it should stop on.
#[must_use]
pub fn cluster_end(text: &str, at: usize) -> usize {
    edges(text, at).1
}

/// The cluster edges `at` widens to, or `(at, at)` when `at` is already on one.
///
/// One walk from the start. An index has to be attributed to a cluster, and a
/// walk is how. The text is capped at a message, and neither a delete nor a
/// row break is a hot path.
fn edges(text: &str, at: usize) -> (usize, usize) {
    let at = at.min(text.len());
    if at == 0 || at == text.len() {
        return (at, at);
    }

    for (start, cluster) in clusters(text) {
        let end = start + cluster.len();
        if at <= start {
            return (at, at);
        }
        if at < end {
            return (start, end);
        }
    }

    (text.len(), text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAMILY: &str = "👨‍👩‍👧";

    #[test]
    fn an_index_at_either_end_is_itself() {
        assert_eq!(cluster_start(FAMILY, 0), 0);
        assert_eq!(cluster_end(FAMILY, 0), 0);
        assert_eq!(cluster_start(FAMILY, FAMILY.len()), FAMILY.len());
        assert_eq!(cluster_end(FAMILY, FAMILY.len()), FAMILY.len());
        assert_eq!(cluster_start("", 0), 0);
        assert_eq!(cluster_end("ab", 100), 2);
    }

    #[test]
    fn an_index_inside_a_cluster_widens_to_its_edges_and_no_further() {
        let text = format!("a{FAMILY}b");
        let inside = 1 + FAMILY.len() / 2;

        assert_eq!(cluster_start(&text, inside), 1);
        assert_eq!(cluster_end(&text, inside), 1 + FAMILY.len());
        assert_eq!(cluster_start(&text, 1), 1, "the cluster's own start");
        assert_eq!(
            cluster_end(&text, 1),
            1,
            "a boundary does not take the cluster that starts there"
        );
    }

    #[test]
    fn a_newline_is_its_own_cluster() {
        let text = format!("{FAMILY}\n👍🏽");
        let newline = FAMILY.len();

        assert_eq!(cluster_start(&text, newline), newline);
        assert_eq!(cluster_end(&text, newline + 1), newline + 1);
        assert_eq!(cluster_end(&text, newline), newline);
    }

    #[test]
    fn a_byte_inside_a_character_belongs_to_that_character_cluster() {
        let text = "é";

        assert_eq!(cluster_start(text, 1), 0);
        assert_eq!(cluster_end(text, 1), text.len());
    }

    #[test]
    fn an_ascii_character_is_a_cluster() {
        assert_eq!(cluster_start("abc", 1), 1);
        assert_eq!(cluster_end("abc", 2), 2);
    }
}
