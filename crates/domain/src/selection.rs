//! A selection over the messages of one conversation.
//!
//! Two marks and the arithmetic between them, and nothing else: no [`Message`],
//! no window, no `ratatui`. The panel intersects a mark's range with each row it
//! draws, the operations ask what the two marks name, and neither of them has to
//! know how the other works.
//!
//! # The rule every operation follows
//!
//! > A selection inside one message is a *text* selection. Anything else is a
//! > *set of messages*.
//!
//! One sentence, and it answers every question the three operations would
//! otherwise have to resolve one at a time: [`Selection::text_range`] is `Some`
//! for the first case and `None` for the second, and nothing downstream has to
//! work out which case it is in.
//!
//! # A text range is half-open, and the two ends can be in either order
//!
//! [`Selection::text_range`] runs from the lower character position to the higher
//! one, whichever end is the anchor, so nothing that reads a selection has to
//! think about the direction the reader dragged it in. The range is half-open at
//! the top, which is the convention every other range in this crate uses and the
//! one that composes with a row's own range without an off-by-one at every
//! intersection.
//!
//! The consequence is that a charwise selection which has not been moved yet
//! spans no characters: `v` on its own is a position rather than a span. That is
//! visible in exactly one place — a yank of it yields nothing, and says so —
//! because every other operation either reads whole messages or moves the focus
//! first.
//!
//! # The distance between two marks is bounded by the window
//!
//! [`Selection::message_ids`] walks from the older mark to the newer one, so its
//! length is the number of messages between them. Both ends are messages the
//! client has loaded, so that number is at most the window's cap; nothing
//! constructs a selection from identifiers a conversation's worth of messages
//! apart.

use std::ops::Range;

/// One end of a selection: a message, and a place inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    /// The message, named rather than indexed.
    ///
    /// An index would name a different message the moment a page landed, which
    /// is the whole reason this is an identifier — and the reason a window
    /// moving under a selection cannot silently change what it covers.
    pub message_id: i64,

    /// Where inside the message, counted in characters from its start.
    ///
    /// `None` is the whole message: what a linewise selection is, and also what a
    /// cursor is. One type for both is what makes `v` a single keystroke rather
    /// than a mode change followed by a motion.
    ///
    /// Characters and not bytes, so a motion that steps by one steps by one
    /// thing the reader can see. Converting to the units a request needs is the
    /// caller's problem, at the boundary where the request is built.
    pub char: Option<usize>,
}

impl Mark {
    /// The whole of a message.
    #[must_use]
    pub const fn whole(message_id: i64) -> Self {
        Self {
            message_id,
            char: None,
        }
    }

    /// A place inside a message.
    #[must_use]
    pub const fn text(message_id: i64, char: usize) -> Self {
        Self {
            message_id,
            char: Some(char),
        }
    }
}

/// A selection between two marks.
///
/// The fields are public because the operations that build one — the keys — are
/// in another crate from the ones that read it, and because the discipline this
/// type needs is a rule about how it is *used* rather than a rule about how it
/// may be built: two marks, and whatever the reader did between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// Where the selection started. The end that stays put.
    pub anchor: Mark,

    /// Where the reader has moved to since. The end that moves.
    pub focus: Mark,
}

impl Selection {
    /// A selection that has not been moved yet: both ends at the same place.
    ///
    /// `char: None` is a linewise selection of one whole message, which is what
    /// `V` leaves behind; `char: Some(at)` is the position `v` starts from.
    #[must_use]
    pub const fn at(message_id: i64, char: Option<usize>) -> Self {
        let mark = Mark { message_id, char };
        Self {
            anchor: mark,
            focus: mark,
        }
    }

    /// Exchanges the two ends, so the moving end becomes the fixed one.
    ///
    /// What `o` is for: a selection dragged "backwards" is the same selection,
    /// and swapping is how the reader says "this end is the anchor now" without
    /// having to put it back where it was.
    pub fn swap(&mut self) {
        std::mem::swap(&mut self.anchor, &mut self.focus);
    }

    /// Whether `message_id` is covered, in whole or in part.
    ///
    /// True for every message between the two ends, not only the two named:
    /// a selection from one message to another covers the ones in between, and
    /// an operation that asked only about the ends would silently skip them.
    #[must_use]
    pub fn touches(&self, message_id: i64) -> bool {
        let (older, newer) = self.span();
        (older..=newer).contains(&message_id)
    }

    /// Every message the selection covers, oldest first.
    #[must_use]
    pub fn message_ids(&self) -> Vec<i64> {
        let (older, newer) = self.span();
        (older..=newer).collect()
    }

    /// The characters selected, when the selection is inside one message.
    ///
    /// `None` whenever it is not: two messages have no single range between
    /// them, and there is no such thing as a selection that quotes half of each.
    /// The normalisation is here rather than at the call site so that nothing
    /// reading a selection has to know which end the reader started from.
    #[must_use]
    pub fn text_range(&self) -> Option<(i64, Range<usize>)> {
        if self.anchor.message_id != self.focus.message_id {
            return None;
        }

        let (older, newer) = (self.anchor.char?, self.focus.char?);
        Some((self.anchor.message_id, older.min(newer)..older.max(newer)))
    }

    /// How much is selected, in whatever the selection is of.
    ///
    /// Characters for a text selection and messages for a set of them, because
    /// the unit is what the reader is counting: "3 selected" next to a set of
    /// three messages is three messages, and next to three characters it is
    /// three characters. A single answer cannot carry both, and picking the
    /// wrong unit is a number the reader cannot act on.
    #[must_use]
    pub fn len(&self) -> usize {
        if let Some((_, range)) = self.text_range() {
            return range.len();
        }

        let (older, newer) = self.span();
        usize::try_from(newer - older + 1).unwrap_or(usize::MAX)
    }

    /// Whether nothing at all is selected.
    ///
    /// Only reachable for a charwise selection that has not been moved, which
    /// covers no characters; a selection spanning messages always covers at
    /// least one.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The older and newer of the two messages, in that order.
    ///
    /// Messages in a conversation are numbered in the order they were sent, so
    /// the older is the smaller identifier — which is what makes "oldest first"
    /// arithmetic on identifiers rather than a lookup.
    fn span(&self) -> (i64, i64) {
        let (left, right) = (self.anchor.message_id, self.focus.message_id);
        (left.min(right), left.max(right))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A selection from one character to another of the same message.
    fn within(from: usize, to: usize) -> Selection {
        Selection {
            anchor: Mark::text(7, from),
            focus: Mark::text(7, to),
        }
    }

    // ---- text_range ------------------------------------------------------

    #[test]
    fn a_text_range_runs_from_the_lower_position_to_the_higher_one() {
        assert_eq!(within(0, 3).text_range(), Some((7, 0..3)));
        assert_eq!(
            within(3, 0).text_range(),
            Some((7, 0..3)),
            "which end the reader started from is not the caller's problem"
        );
        assert_eq!(within(2, 2).text_range(), Some((7, 2..2)), "and a position is a span of none");
    }

    /// There is no range that names a piece of one message and a piece of
    /// another, so there is nothing to return.
    #[test]
    fn there_is_no_text_range_across_two_messages() {
        let selection = Selection {
            anchor: Mark::text(7, 0),
            focus: Mark::text(8, 4),
        };

        assert_eq!(selection.text_range(), None);
        assert_eq!(selection.message_ids(), vec![7, 8]);
    }

    /// A selection with no character position is about whole messages, whichever
    /// message they are of.
    #[test]
    fn a_mark_with_no_position_is_never_a_text_range() {
        let mixed = Selection {
            anchor: Mark::whole(7),
            focus: Mark::text(7, 4),
        };
        assert_eq!(mixed.text_range(), None);

        let whole = Selection::at(7, None);
        assert_eq!(whole.text_range(), None);
        assert_eq!(whole.message_ids(), vec![7]);
    }

    #[test]
    fn a_selection_that_has_not_moved_spans_no_characters() {
        let selection = Selection::at(7, Some(4));

        assert_eq!(selection.text_range(), Some((7, 4..4)));
        assert!(selection.is_empty(), "a position is not a span");
        assert!(
            selection.touches(7),
            "and it is still a position inside a message the operations can act on"
        );
    }

    // ---- the messages a selection covers --------------------------------

    #[test]
    fn a_selection_covers_every_message_between_its_ends_oldest_first() {
        let forward = Selection {
            anchor: Mark::whole(4),
            focus: Mark::whole(9),
        };
        let backward = Selection {
            anchor: Mark::whole(9),
            focus: Mark::whole(4),
        };

        let expected: Vec<i64> = (4..=9).collect();
        assert_eq!(forward.message_ids(), expected);
        assert_eq!(
            backward.message_ids(),
            expected,
            "the reader dragged it the other way and it is the same selection"
        );
    }

    /// A selection whose ends are the same message is one message, not two of
    /// them and not none.
    #[test]
    fn a_selection_within_one_message_covers_exactly_that_message() {
        let selection = within(2, 9);

        assert_eq!(selection.message_ids(), vec![7]);
        assert_eq!(selection.len(), 7, "seven characters of it");
    }

    #[test]
    fn touching_is_the_whole_span_and_not_only_the_ends() {
        let selection = Selection {
            anchor: Mark::whole(4),
            focus: Mark::whole(6),
        };

        for id in 3..=7 {
            assert_eq!(
                selection.touches(id),
                (4..=6).contains(&id),
                "{id} is {} the selection",
                if (4..=6).contains(&id) { "in" } else { "outside" }
            );
        }
    }

    /// A placeholder is a negative identifier and sorts before every real
    /// message, so a selection can span one without anything going wrong.
    #[test]
    fn a_selection_can_span_a_placeholder() {
        let selection = Selection {
            anchor: Mark::whole(-1),
            focus: Mark::whole(2),
        };

        assert_eq!(selection.message_ids(), vec![-1, 0, 1, 2]);
        assert!(selection.touches(-1));
    }

    // ---- the moving end -------------------------------------------------

    #[test]
    fn swapping_the_ends_leaves_the_selection_the_same() {
        let mut selection = within(0, 3);
        let before = selection.text_range();

        selection.swap();

        assert_ne!(selection.anchor, selection.focus);
        assert_eq!(selection.text_range(), before, "only the direction changed");
    }

    #[test]
    fn swapping_twice_is_the_selection_it_started_as() {
        let selection = Selection {
            anchor: Mark::whole(4),
            focus: Mark::text(9, 2),
        };
        let mut swapped = selection;
        swapped.swap();
        swapped.swap();

        assert_eq!(swapped, selection);
    }

    // ---- the unit a count is in ----------------------------------------

    #[test]
    fn a_count_is_in_whichever_unit_the_selection_is_of() {
        assert_eq!(within(0, 3).len(), 3, "three characters");
        assert_eq!(
            Selection {
                anchor: Mark::whole(4),
                focus: Mark::whole(6)
            }
            .len(),
            3,
            "three messages"
        );
    }

    #[test]
    fn a_selection_of_messages_is_never_empty() {
        for (older, newer) in [(4_i64, 4_i64), (4, 6), (-1, 3)] {
            let selection = Selection {
                anchor: Mark::whole(older),
                focus: Mark::whole(newer),
            };
            assert!(!selection.is_empty(), "{older}..={newer}");
        }
    }
}
