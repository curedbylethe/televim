//! Pure Vim motion calculator.
//!
//! No UI, no async — just cursor arithmetic. This is the piece the TUI and
//! (later) integration tests exercise exhaustively.

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
}
