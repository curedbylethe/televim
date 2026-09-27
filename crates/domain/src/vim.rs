//! Pure Vim motion calculator.
//!
//! No UI, no async — just cursor arithmetic. This is the piece the TUI and
//! (later) integration tests exercise exhaustively.
//!
//! Two kinds of motion live here, and they are kept apart on purpose. [`VimState`]
//! moves a cursor *between* the items of a list — messages, or search matches —
//! and knows nothing about what an item is. [`char_motion`] moves a position
//! *within* one item's text, and knows nothing about the list. The selection
//! needs both at once, and a type that could do both would be doing two things.
//!

/// A resolved motion. `Char`-level handling is done in [`VimState::handle_char`]
/// so multi-key sequences like `gg` work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Down,
    Up,
    First,
    Last,
    NextMatch,
    PrevMatch,
}

/// A motion *within* one message's text, rather than between messages.
///
/// Every one of these is bounded by the text it moves through: `w` at the end of
/// the last word stops there rather than crossing into the next message. That is
/// not Vim's rule — in Vim `w` at the end of a buffer wraps to the top — and it is
/// the rule on purpose. Crossing is what `j` is for, and a motion that silently
/// changes *what it selects* is the worst failure mode a selection can have.
///
/// A direction is a `bool` rather than a sign so that every step clamps in
/// integers rather than in a signed type wide enough to hold both ends of a
/// message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharMotion {
    /// `h` or `l`. One character, clamped at both ends of the text.
    Step {
        /// Which way.
        forward: bool,
    },

    /// `w` or `b`. The start of the next or the previous word.
    WordStart {
        /// Which way.
        forward: bool,
    },

    /// `e`. The end of the word the position is in, or of the next one.
    WordEnd,

    /// `0` or `$`. The start or the end of the text.
    Bound {
        /// The end rather than the start.
        end: bool,
    },

    /// `f`, `t`, `F` or `T`: the next or the previous occurrence of a character,
    /// standing on it or stopping one short.
    Find {
        /// The character to look for.
        target: char,

        /// Which way to look.
        forward: bool,

        /// Whether to land on the character itself rather than one short of it.
        onto: bool,
    },
}

/// Where `motion` takes a position in `text`, given where it is now.
///
/// A position past the end of the text clamps to its last character, and an empty
/// text has exactly one position: zero. A [`CharMotion::Find`] that finds nothing
/// leaves the position where it was — that is what Vim does, and moving somewhere
/// else would be a motion the reader did not ask for.
///
/// Character positions in, character positions out, and the conversion to byte
/// offsets happens here and nowhere else. A keystroke must not build a vector of
/// a four-thousand-character message's characters to find out where the next one
/// is.
#[must_use]
pub fn char_motion(text: &str, at: usize, motion: CharMotion) -> usize {
    // Clamped here rather than in `offset_of` because the clamped value is also
    // what a `Find` that finds nothing returns: a caller that asked for a position
    // past the end of the text gets the end of the text, not its length.
    let at = at.min(text.chars().count().saturating_sub(1));
    let here = offset_of(text, at);
    let end = text.len();

    match motion {
        CharMotion::Step { forward } => {
            if forward {
                position_of(text, forward_from(text, here).min(last_of(text)))
            } else {
                position_of(text, back_from(text, here))
            }
        }

        CharMotion::Bound { end: at_end } => {
            if at_end {
                position_of(text, back_from(text, end))
            } else {
                0
            }
        }

        CharMotion::WordStart { forward } => position_of(
            text,
            if forward {
                next_word(text, here)
            } else {
                previous_word(text, here)
            },
        ),

        CharMotion::WordEnd => position_of(text, word_end(text, here)),

        CharMotion::Find {
            target,
            forward,
            onto,
        } => {
            let found = if forward {
                text[here..]
                    .char_indices()
                    .skip(1)
                    .find(|(_, character)| *character == target)
                    .map(|(offset, _)| here + offset)
            } else {
                text[..here]
                    .char_indices()
                    .rev()
                    .find(|(_, character)| *character == target)
                    .map(|(offset, _)| offset)
            };

            let Some(found) = found else {
                return at;
            };

            let stopped = match (forward, onto) {
                (true, false) => back_from(text, found).max(here),
                (false, false) => forward_from(text, found).min(last_of(text)),
                _ => found,
            };
            position_of(text, stopped)
        }
    }
}

// ---- the character motions, in byte offsets ---------------------------

/// The byte offset of character `at`, or the end of the text if there is none.
fn offset_of(text: &str, at: usize) -> usize {
    text.char_indices()
        .nth(at)
        .map_or(text.len(), |(offset, _)| offset)
}

/// The character position of the byte offset `offset`.
fn position_of(text: &str, offset: usize) -> usize {
    text[..offset].chars().count()
}

/// The byte offset of the text's last character, or zero for an empty text.
fn last_of(text: &str) -> usize {
    back_from(text, text.len())
}

/// The byte offset of the next character after `at`, or `at` at the end.
fn forward_from(text: &str, at: usize) -> usize {
    text[at..]
        .chars()
        .next()
        .map_or(at, |character| at + character.len_utf8())
}

/// The byte offset of the character before `at`, or `at` at the start.
fn back_from(text: &str, at: usize) -> usize {
    text[..at]
        .chars()
        .next_back()
        .map_or(at, |character| at - character.len_utf8())
}

/// Whether the character at `offset` is the first of a word.
///
/// A word is a run of non-whitespace, which is what a reader means by a word in a
/// message: punctuation inside a word is part of it, and a run of punctuation is
/// a word of its own.
fn starts_word(text: &str, offset: usize) -> bool {
    !text[offset..].starts_with(char::is_whitespace)
        && (offset == 0 || text[..offset].ends_with(char::is_whitespace))
}

/// The start of the next word after `at`, or `at` when there is none.
fn next_word(text: &str, at: usize) -> usize {
    let mut from = forward_from(text, at);
    while from < text.len() {
        if starts_word(text, from) {
            return from;
        }
        from = forward_from(text, from);
    }

    at
}

/// The start of the word before `at`, or `at` when there is none.
fn previous_word(text: &str, at: usize) -> usize {
    let mut from = at;
    while from > 0 {
        from = back_from(text, from);
        if starts_word(text, from) {
            return from;
        }
    }

    at
}

/// The last character of the run of non-whitespace starting at or after `from`.
fn run_end(text: &str, from: usize) -> usize {
    let mut end = from;
    while end < text.len() {
        end = forward_from(text, end);
        if end >= text.len() || text[end..].starts_with(char::is_whitespace) {
            return back_from(text, end);
        }
    }

    back_from(text, text.len())
}

/// The end of the word `at` is in, or of the next one.
///
/// `e` has to be able to get *past* a one-character word, which is why a position
/// that is already a word's end moves on rather than standing still — Vim's rule,
/// and the reason `ee` is how you cross a short word. Standing on whitespace is
/// the same case: `run_end` walks from there into the word after it, so `e` from
/// a space lands on that word's end.
fn word_end(text: &str, at: usize) -> usize {
    let here = run_end(text, at);
    if here > at {
        return here;
    }

    let next = next_word(text, at);
    if next == at {
        back_from(text, text.len())
    } else {
        run_end(text, next)
    }
}

/// Cursor state for a scrollable buffer of `total` items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VimState {
    cursor: usize,
    total: usize,
    pending_g: bool,
}
impl Default for VimState {
    fn default() -> Self {
        Self::new(0)
    }
}

impl VimState {
    #[must_use]
    pub fn new(total: usize) -> Self {
        Self {
            cursor: 0,
            total,
            pending_g: false,
        }
    }

    #[must_use]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    #[must_use]
    pub fn total(&self) -> usize {
        self.total
    }

    /// Replace the buffer length and re-clamp the cursor.
    pub fn set_total(&mut self, total: usize) {
        self.total = total;
        self.clamp();
    }

    /// Move the cursor to `cursor`, clamped to the buffer.
    ///
    /// Every other movement here is relative, because a keystroke is. This one
    /// is for a position that has to be *restored* rather than stepped to: a
    /// window that moved underneath the reader keeps its place by message, and
    /// looking that message up again yields an index.
    pub fn set_cursor(&mut self, cursor: usize) {
        self.cursor = cursor;
        self.clamp();
    }

    /// Feed a single character as if typed in Normal mode.
    ///
    /// Returns the motion the character applied, if it applied one: a character
    /// that opens a sequence, and one this state has no meaning for, both move
    /// nothing at all.
    ///
    /// The caller is told which, because some of these are more than motions to a
    /// screen. `gg` is where a reader's unread messages start rather than the top
    /// of what happens to be loaded, and `G` is the end of the conversation —
    /// neither of which this state can know about, and both of which it would
    /// otherwise hide behind an unchanged cursor. `n` and `N` are the same case
    /// for a different reason: a match is a place in a conversation, which this
    /// state cannot see, so it reports the motion and leaves the walking to the
    /// caller.
    pub fn handle_char(&mut self, c: char) -> Option<Motion> {
        if self.pending_g {
            self.pending_g = false;
            if c == 'g' {
                self.apply_motion(Motion::First);
                return Some(Motion::First);
            }
            return None;
        }

        let motion = match c {
            'j' => Motion::Down,
            'k' => Motion::Up,
            'g' => {
                self.pending_g = true;
                return None;
            }
            'G' => Motion::Last,
            'n' => Motion::NextMatch,
            'N' => Motion::PrevMatch,
            _ => return None,
        };

        self.apply_motion(motion);
        Some(motion)
    }

    pub fn apply_motion(&mut self, motion: Motion) {
        match motion {
            Motion::Down => self.move_down(),
            Motion::Up => self.move_up(),
            Motion::First => self.move_first(),
            Motion::Last => self.move_last(),
            // The two match motions are reported, not answered: what the next
            // match *is* is a list this state does not hold. The caller walks
            // its own list and moves the cursor to the message it names.
            Motion::NextMatch | Motion::PrevMatch => {}
        }
    }

    // ---- internals -----------------------------------------------------

    fn clamp(&mut self) {
        if self.total == 0 {
            self.cursor = 0;
        } else if self.cursor >= self.total {
            self.cursor = self.total - 1;
        }
    }

    fn move_down(&mut self) {
        if self.total > 0 && self.cursor + 1 < self.total {
            self.cursor += 1;
        }
    }

    fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn move_first(&mut self) {
        self.cursor = 0;
    }

    fn move_last(&mut self) {
        if self.total > 0 {
            self.cursor = self.total - 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_buffer_never_moves() {
        let mut v = VimState::new(0);
        v.handle_char('j');
        v.handle_char('G');
        v.handle_char('g');
        v.handle_char('g');
        assert_eq!(v.cursor(), 0);
        assert_eq!(v.total(), 0);
    }

    #[test]
    fn j_k_clamp_at_bounds() {
        let mut v = VimState::new(3);
        v.handle_char('k');
        assert_eq!(v.cursor(), 0);
        v.handle_char('j');
        v.handle_char('j');
        v.handle_char('j');
        assert_eq!(v.cursor(), 2);
    }

    #[test]
    fn gg_jumps_to_top_from_bottom() {
        let mut v = VimState::new(10);
        v.handle_char('G');
        assert_eq!(v.cursor(), 9);
        v.handle_char('g');
        v.handle_char('g');
        assert_eq!(v.cursor(), 0);
    }

    #[test]
    fn g_then_non_g_is_noop() {
        let mut v = VimState::new(10);
        v.handle_char('G');
        v.handle_char('g');
        v.handle_char('x');
        assert_eq!(v.cursor(), 9);
    }

    #[test]
    fn big_g_jumps_to_last() {
        let mut v = VimState::new(5);
        v.handle_char('G');
        assert_eq!(v.cursor(), 4);
    }

    /// The two match motions are the one thing this state reports but cannot
    /// answer: a match is a place in a conversation, and the list of them lives
    /// with the search. The cursor must not move on its own for either.
    #[test]
    fn the_match_motions_are_reported_but_not_answered() {
        let mut v = VimState::new(10);
        let before = v.cursor();

        assert_eq!(v.handle_char('n'), Some(Motion::NextMatch));
        assert_eq!(v.cursor(), before, "the matches are the caller's to walk");
        assert_eq!(v.handle_char('N'), Some(Motion::PrevMatch));
        assert_eq!(v.cursor(), before);
    }

    #[test]
    fn shrinking_buffer_reclamps() {
        let mut v = VimState::new(10);
        v.handle_char('G');
        assert_eq!(v.cursor(), 9);
        v.set_total(3);
        assert_eq!(v.cursor(), 2);
    }

    #[test]
    fn shrinking_to_zero_resets() {
        let mut v = VimState::new(5);
        v.handle_char('j');
        v.handle_char('j');
        v.set_total(0);
        assert_eq!(v.cursor(), 0);
    }

    #[test]
    fn set_cursor_clamps_to_the_buffer() {
        let mut v = VimState::new(3);
        v.set_cursor(2);
        assert_eq!(v.cursor(), 2);

        v.set_cursor(99);
        assert_eq!(v.cursor(), 2, "the cursor cannot leave the buffer");
    }

    #[test]
    fn set_cursor_on_an_empty_buffer_stays_at_zero() {
        let mut v = VimState::new(0);
        v.set_cursor(5);
        assert_eq!(v.cursor(), 0);
    }

    /// A cursor that did not move says nothing about why, so the motion is what
    /// the caller is told: `gg` from the top and a lone `g` at the top leave the
    /// cursor in the same place and mean different things.
    #[test]
    fn the_motion_a_character_applied_is_reported() {
        let mut v = VimState::new(10);

        assert_eq!(v.handle_char('g'), None, "the first `g` moves nothing yet");
        assert_eq!(v.handle_char('g'), Some(Motion::First));

        assert_eq!(v.handle_char('g'), None, "and the sequence starts over");
        assert_eq!(
            v.handle_char('x'),
            None,
            "a character that ends the sequence moves nothing"
        );

        assert_eq!(v.handle_char('G'), Some(Motion::Last));
        assert_eq!(v.handle_char('j'), Some(Motion::Down));
        assert_eq!(v.handle_char('n'), Some(Motion::NextMatch));
        assert_eq!(v.handle_char('q'), None, "and a character with no meaning");
    }

    // ---- motions within a message's text ---------------------------------

    /// Where a motion lands in `text`, from position zero.
    fn from_start(text: &str, motion: CharMotion) -> usize {
        char_motion(text, 0, motion)
    }

    /// Where a motion lands in `text`, from position `at`.
    fn at(text: &str, at: usize, motion: CharMotion) -> usize {
        char_motion(text, at, motion)
    }

    const STEP: fn(bool) -> CharMotion = |forward| CharMotion::Step { forward };
    const WORD: fn(bool) -> CharMotion = |forward| CharMotion::WordStart { forward };
    const BOUND: fn(bool) -> CharMotion = |end| CharMotion::Bound { end };

    #[test]
    fn a_step_moves_one_character_and_clamps_at_both_ends() {
        let text = "abcdef";

        assert_eq!(at(text, 2, STEP(true)), 3);
        assert_eq!(at(text, 2, STEP(false)), 1);

        assert_eq!(at(text, 0, STEP(false)), 0, "`h` at the start does nothing");
        assert_eq!(
            at(text, 5, STEP(true)),
            5,
            "and `l` at the end does nothing"
        );
    }

    /// A step is one *character*, not one byte: a `é` is one press and one cell,
    /// and a selection that stopped in the middle of one would be over nothing.
    #[test]
    fn a_step_moves_one_character_rather_than_one_byte() {
        let text = "aé😀b";

        assert_eq!(at(text, 0, STEP(true)), 1);
        assert_eq!(at(text, 1, STEP(true)), 2, "over a two-byte character");
        assert_eq!(at(text, 2, STEP(true)), 3, "and a four-byte one");
        assert_eq!(at(text, 3, STEP(true)), 3, "and it stops at the last one");
        assert_eq!(at(text, 3, STEP(false)), 2);
        assert_eq!(at(text, 1, STEP(false)), 0);
    }

    #[test]
    fn zero_and_the_end_are_the_ends_of_the_text() {
        let text = "hello world";

        assert_eq!(from_start(text, BOUND(false)), 0);
        assert_eq!(
            at(text, 5, BOUND(false)),
            0,
            "`0` goes to the start from anywhere"
        );
        assert_eq!(at(text, 3, BOUND(true)), 10, "`$` to the last character");
        assert_eq!(from_start(text, BOUND(true)), 10);
    }

    /// A word is a run of non-whitespace, which is what a reader means by a word
    /// in a message: `don't` is one word and `...` is another.
    #[test]
    fn a_word_starts_after_whitespace_and_at_the_beginning() {
        let text = "hi there  you";

        assert_eq!(at(text, 0, WORD(true)), 3, "`w` over the first word");
        assert_eq!(at(text, 3, WORD(true)), 10, "and over the run of spaces");
        assert_eq!(at(text, 1, WORD(true)), 3, "from inside a word as well");
        assert_eq!(
            at(text, 0, WORD(false)),
            0,
            "there is nothing before the first"
        );
    }

    #[test]
    fn b_goes_back_to_the_previous_word_start() {
        let text = "alpha beta gamma";

        assert_eq!(at(text, 10, WORD(false)), 6, "`b` over `gamma`");
        assert_eq!(at(text, 6, WORD(false)), 0, "and over `beta`");
        assert_eq!(at(text, 1, WORD(false)), 0, "and over `alpha`");
        assert_eq!(
            at(text, 0, WORD(false)),
            0,
            "and there is nothing before it"
        );
    }

    /// The exception that proves the rule: a motion stops at the end of the text
    /// rather than crossing into the next message, which is what `j` is for.
    #[test]
    fn a_word_motion_stops_at_the_end_of_the_text_instead_of_crossing() {
        for text in ["alpha beta", "alpha   ", "alpha", ""] {
            let last = text.chars().count().saturating_sub(1);

            assert_eq!(at(text, last, WORD(true)), last, "{text:?} forwards");
            assert_eq!(at(text, 0, WORD(false)), 0, "{text:?} backwards");
        }
    }

    #[test]
    fn e_lands_on_the_end_of_the_word_it_is_in() {
        let text = "alpha beta";

        assert_eq!(at(text, 0, CharMotion::WordEnd), 4);
        assert_eq!(at(text, 2, CharMotion::WordEnd), 4, "from inside it");
        assert_eq!(
            at(text, 3, CharMotion::WordEnd),
            4,
            "and from just before its end"
        );
    }

    /// `e` has to get past a one-character word, which is the only way to cross
    /// one in Vim — a position that is already a word's end stands still, and the
    /// space is what carries `e` over it.
    #[test]
    fn e_moves_past_a_word_it_is_already_at_the_end_of() {
        let text = "a bc";

        assert_eq!(
            at(text, 0, CharMotion::WordEnd),
            3,
            "`e` on a one-character word goes to the end of the *next* one"
        );
        assert_eq!(
            at(text, 1, CharMotion::WordEnd),
            3,
            "and from the space as well"
        );
        assert_eq!(
            at(text, 3, CharMotion::WordEnd),
            3,
            "and at the end of the last word there is nothing to move to"
        );
    }

    /// Repeated `e` walks the words rather than sticking at the end of the first,
    /// which is what "already at this word's end" has to mean for it to be a
    /// motion at all.
    #[test]
    fn repeated_e_walks_the_ends_of_the_words() {
        let text = "one two three";

        let mut at_position = 0;
        for expected in [2, 6, 12] {
            at_position = at(text, at_position, CharMotion::WordEnd);
            assert_eq!(at_position, expected, "in {text:?}");
        }
    }

    #[test]
    fn e_from_whitespace_lands_on_the_end_of_the_word_after_it() {
        let text = "alpha   beta";

        assert_eq!(at(text, 5, CharMotion::WordEnd), 11);
        assert_eq!(at(text, 7, CharMotion::WordEnd), 11);
    }

    #[test]
    fn f_and_t_find_the_next_occurrence_and_its_predecessor() {
        let text = "a.b.c";
        let find = |target, onto| CharMotion::Find {
            target,
            forward: true,
            onto,
        };

        assert_eq!(at(text, 0, find('.', true)), 1, "`f.`");
        assert_eq!(
            at(text, 0, find('.', false)),
            0,
            "`t.` stops short, which is here"
        );
        assert_eq!(at(text, 2, find('.', true)), 3);
        assert_eq!(at(text, 2, find('.', false)), 2);
    }

    #[test]
    fn f_and_t_find_the_previous_occurrence_and_its_successor() {
        let text = "a.b.c";
        let find = |target, onto| CharMotion::Find {
            target,
            forward: false,
            onto,
        };

        assert_eq!(at(text, 4, find('.', true)), 3, "`F.`");
        assert_eq!(
            at(text, 4, find('.', false)),
            4,
            "`T.` stops short, which is here"
        );
        assert_eq!(at(text, 2, find('.', true)), 1);
        assert_eq!(at(text, 2, find('.', false)), 2);
    }

    /// A `t` at the very start of the text has nowhere to stop short, so it stands
    /// still rather than walking backwards off the end.
    #[test]
    fn a_stop_short_that_would_leave_the_text_stands_still() {
        let text = "a.b";
        let stop_short = |from: usize, forward: bool| {
            char_motion(
                text,
                from,
                CharMotion::Find {
                    target: '.',
                    forward,
                    onto: false,
                },
            )
        };

        assert_eq!(stop_short(0, true), 0, "`t.` at the very first character");
        assert_eq!(stop_short(2, false), 2, "`T.` with nothing after it");
    }

    /// A character that is not there is a keypress that goes nowhere. Standing
    /// still is Vim's answer, and moving somewhere else would be a motion the
    /// reader did not ask for.
    #[test]
    fn a_find_that_finds_nothing_leaves_the_position_alone() {
        let text = "abc";
        for forward in [true, false] {
            assert_eq!(
                at(
                    text,
                    1,
                    CharMotion::Find {
                        target: 'z',
                        forward,
                        onto: true
                    }
                ),
                1,
                "forward {forward}"
            );
        }
    }

    /// A position a caller could not have meant — past the end, or in an empty
    /// text — clamps rather than panicking. The release profile aborts on a
    /// panic, and a keypress must not be able to take the process down.
    #[test]
    fn a_position_past_the_end_of_the_text_clamps_to_it() {
        let text = "héllo";
        let last = 4;

        for at in [5, 99, usize::MAX] {
            for motion in [
                STEP(true),
                STEP(false),
                WORD(true),
                WORD(false),
                CharMotion::WordEnd,
            ] {
                let landed = char_motion(text, at, motion);
                assert!(
                    landed <= last,
                    "{motion:?} from {at} landed on {landed}, past the last character"
                );
            }
        }

        for motion in [
            STEP(true),
            STEP(false),
            WORD(true),
            WORD(false),
            CharMotion::WordEnd,
            BOUND(true),
            BOUND(false),
        ] {
            assert_eq!(char_motion("", 42, motion), 0, "{motion:?} in nothing");
        }
    }

    /// One answer for the same question, whatever the text: every position this
    /// function returns is a position the text can actually be indexed at.
    #[test]
    fn every_landing_is_a_position_the_text_has() {
        let text = "  two words, then  a third… and 😀 more  ";

        for at in 0..text.chars().count() {
            for motion in [
                STEP(true),
                STEP(false),
                WORD(true),
                WORD(false),
                CharMotion::WordEnd,
                BOUND(true),
                BOUND(false),
                CharMotion::Find {
                    target: 'o',
                    forward: true,
                    onto: false,
                },
                CharMotion::Find {
                    target: 'o',
                    forward: false,
                    onto: true,
                },
            ] {
                let landed = char_motion(text, at, motion);
                assert!(
                    landed < text.chars().count(),
                    "{motion:?} from {at} landed on {landed}, past the end of {text:?}"
                );
                assert!(
                    text.is_char_boundary(byte_of(text, landed)),
                    "{motion:?} from {at} landed inside a character"
                );
            }
        }
    }

    /// The byte offset of character `at`.
    fn byte_of(text: &str, at: usize) -> usize {
        text.char_indices()
            .nth(at)
            .map_or(text.len(), |(offset, _)| offset)
    }
}
