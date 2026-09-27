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
//! # Which messages a selection covers is not this type's to answer
//!
//! It is tempting to read the two identifiers as a span and say that everything
//! between them is covered. That is wrong whenever a placeholder is involved, and
//! a placeholder is a local stand-in for a send the server has not acknowledged:
//! it is numbered below zero and sits at the *end* of the window, where the
//! conversation has reached. A selection reaching one therefore spans a different
//! set of messages by identifier than by position — and acting on the wrong one is
//! a deletion of messages the reader did not select.
//!
//! So this type answers the two questions that are purely about a position — the
//! character range, and which end is which — and the caller answers "which
//! messages" from the window's own order, which is the only place that order is
//! written down.

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

    /// Whether the selection spans no characters.
    ///
    /// Only reachable for a charwise selection that has not been moved, which
    /// covers no characters. A selection spanning messages always covers at least
    /// one, and whether it does is a question about the window rather than about
    /// the marks.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text_range().is_some_and(|(_, range)| range.is_empty())
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
        assert_eq!(
            within(2, 2).text_range(),
            Some((7, 2..2)),
            "and a position is a span of none"
        );
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
    }

    /// A mark with no character position is about a whole message, whichever
    /// message it is of.
    #[test]
    fn a_mark_with_no_position_is_never_a_text_range() {
        let mixed = Selection {
            anchor: Mark::whole(7),
            focus: Mark::text(7, 4),
        };
        assert_eq!(mixed.text_range(), None);

        assert_eq!(Selection::at(7, None).text_range(), None);
    }

    #[test]
    fn a_selection_that_has_not_moved_spans_no_characters() {
        let selection = Selection::at(7, Some(4));

        assert!(selection.is_empty(), "a position is not a span");
        assert_eq!(
            selection.text_range(),
            Some((7, 4..4)),
            "and the range says so too"
        );
    }

    #[test]
    fn a_selection_of_a_message_is_never_empty() {
        // The marks alone cannot say how many messages it spans, so they cannot
        // say it is empty either. That question belongs to the window.
        assert!(!Selection::at(7, None).is_empty());
        assert!(!within(2, 2 + 3).is_empty());
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

    #[test]
    fn a_mark_is_built_whole_or_at_a_character() {
        assert_eq!(
            Mark::whole(3),
            Mark {
                message_id: 3,
                char: None
            }
        );
        assert_eq!(
            Mark::text(3, 7),
            Mark {
                message_id: 3,
                char: Some(7)
            }
        );
        assert_eq!(
            Selection::at(3, None),
            Selection {
                anchor: Mark::whole(3),
                focus: Mark::whole(3)
            }
        );
    }
}
