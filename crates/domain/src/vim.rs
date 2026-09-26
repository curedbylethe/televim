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

/// Cursor + search state for a scrollable buffer of `total` items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VimState {
    cursor: usize,
    total: usize,
    matches: Vec<usize>,
    match_index: Option<usize>,
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
            matches: Vec::new(),
            match_index: None,
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

    /// Replace the search-match list. Does not move the cursor.
    pub fn set_matches(&mut self, matches: Vec<usize>) {
        self.matches = matches;
        self.match_index = None;
    }

    /// Feed a single character as if typed in Normal mode.
    pub fn handle_char(&mut self, c: char) {
        if self.pending_g {
            self.pending_g = false;
            if c == 'g' {
                self.apply_motion(Motion::First);
            }
            return;
        }
        match c {
            'j' => self.apply_motion(Motion::Down),
            'k' => self.apply_motion(Motion::Up),
            'g' => self.pending_g = true,
            'G' => self.apply_motion(Motion::Last),
            'n' => self.apply_motion(Motion::NextMatch),
            'N' => self.apply_motion(Motion::PrevMatch),
            _ => {}
        }
    }

    pub fn apply_motion(&mut self, motion: Motion) {
        match motion {
            Motion::Down => self.move_down(),
            Motion::Up => self.move_up(),
            Motion::First => self.move_first(),
            Motion::Last => self.move_last(),
            Motion::NextMatch => self.next_match(),
            Motion::PrevMatch => self.prev_match(),
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

    fn next_match(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        let next = match self.match_index {
            Some(i) => (i + 1) % self.matches.len(),
            None => 0,
        };
        self.match_index = Some(next);
        self.cursor = self.matches[next];
    }

    fn prev_match(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        let prev = match self.match_index {
            Some(0) | None => self.matches.len() - 1,
            Some(i) => i - 1,
        };
        self.match_index = Some(prev);
        self.cursor = self.matches[prev];
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

    #[test]
    fn n_wraps_around_matches() {
        let mut v = VimState::new(10);
        v.set_matches(vec![2, 5, 8]);
        v.handle_char('n');
        assert_eq!(v.cursor(), 2);
        v.handle_char('n');
        assert_eq!(v.cursor(), 5);
        v.handle_char('n');
        assert_eq!(v.cursor(), 8);
        v.handle_char('n');
        assert_eq!(v.cursor(), 2);
    }

    #[test]
    fn big_n_wraps_backwards() {
        let mut v = VimState::new(10);
        v.set_matches(vec![2, 5, 8]);
        v.handle_char('N');
        assert_eq!(v.cursor(), 8);
        v.handle_char('N');
        assert_eq!(v.cursor(), 5);
    }

    #[test]
    fn n_with_no_matches_is_noop() {
        let mut v = VimState::new(10);
        v.handle_char('n');
        assert_eq!(v.cursor(), 0);
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
}
