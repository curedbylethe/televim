//! Where the reader has been, so a jump can be taken back.
//!
//! Vim's jumplist is two stacks and nothing more: a jump records where the
//! reader was, `Ctrl-o` walks one stack and `Ctrl-i` the other, and the place a
//! walk came from is marked on the other stack so it can be walked back to.
//! That is the whole idea, and it is here rather than in the screen because
//! nothing about it is a screen: a mark is a *message id*, not a row, which is
//! what lets it outlive the window it was taken in.
//!
//! Per conversation, because one reader's jump in one chat says nothing about
//! where they were in another, and a mark carried across would put `Ctrl-o` in
//! a conversation the reader is not in.

use std::collections::HashMap;
use std::collections::VecDeque;

/// How many marks one conversation keeps unless the configuration names
/// another.
///
/// Bounded because a reader who keeps jumping would otherwise keep every
/// message they have ever been at, in a program whose whole budget is a few
/// megabytes. Vim's own `'jumplist'` depth is 50; 100 is the same answer with
/// room to spare, and a reader who has forgotten the last hundred places they
/// were was never going to walk back that far. [`Jumplist::new`] takes the
/// depth, and the configuration's `jumplist_depth` is what the application
/// hands it; this constant is the default the two agree on.
pub const DEFAULT_DEPTH: usize = 100;

/// The two stacks of marks for one conversation.
///
/// Both are `VecDeque` because both are walked from the top, and the back stack
/// is also trimmed from the bottom — which is the only place a `Vec` would want
/// an index shift on every record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Marks {
    /// Where the reader has been, newest last.
    back: VecDeque<i64>,
    /// Where a walk carried them away from, newest last.
    forward: VecDeque<i64>,
}

/// Drops the oldest marks of `stack` until it holds no more than `depth`.
///
/// Both stacks are capped after every push, so no walk and no record can leave
/// either one longer than the depth, whatever order the reader presses the keys.
fn trim(stack: &mut VecDeque<i64>, depth: usize) {
    while stack.len() > depth {
        stack.pop_front();
    }
}

/// Where the reader has been, per conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jumplist {
    conversations: HashMap<i64, Marks>,
    /// How many marks each conversation keeps, on either stack.
    depth: usize,
}

impl Default for Jumplist {
    fn default() -> Self {
        Self::new(DEFAULT_DEPTH)
    }
}

impl Jumplist {
    /// An empty jumplist whose conversations each keep at most `depth` marks.
    ///
    /// The depth is fixed at construction: it is read once from the
    /// configuration and handed here, like the other settings resolved before
    /// the first frame.
    #[must_use]
    pub fn new(depth: usize) -> Self {
        Self {
            conversations: HashMap::new(),
            depth,
        }
    }

    /// Remembers that the reader was at `id` in `peer_id`'s conversation.
    ///
    /// The forward marks go, because a new jump makes every place they had been
    /// heading somewhere irrelevant — which is Vim's rule and the one that keeps
    /// this a stack rather than a history.
    ///
    /// The same id twice in a row is one place, not two: a reader pressing `gd`
    /// twice on the same message has not gone anywhere, and recording it twice
    /// would make `Ctrl-o` a key that appears to do nothing once.
    pub fn record(&mut self, peer_id: i64, id: i64) {
        let depth = self.depth;
        let marks = self.conversations.entry(peer_id).or_default();
        if marks.back.back() == Some(&id) {
            return;
        }

        marks.back.push_back(id);
        marks.forward.clear();
        trim(&mut marks.back, depth);
    }

    /// The message before the one at `from`, or `None` when there is none.
    ///
    /// `from` is where the reader is now, and it is pushed onto the forward
    /// stack rather than dropped: that is what makes `Ctrl-i` able to bring
    /// them back again, and it is why the first `Ctrl-o` marks the place the
    /// reader was standing at the end of the list rather than discarding it.
    pub fn back(&mut self, peer_id: i64, from: i64) -> Option<i64> {
        let depth = self.depth;
        let marks = self.conversations.entry(peer_id).or_default();
        let target = marks.back.pop_back()?;

        marks.forward.push_back(from);
        trim(&mut marks.forward, depth);
        Some(target)
    }

    /// The message after the one at `from`, or `None` when they are as far
    /// forward as they can go.
    ///
    /// The mirror of [`Jumplist::back`], and it moves a mark rather than
    /// copying one: a reader who walks back and forward twice has been in the
    /// same two places, not four.
    pub fn forward(&mut self, peer_id: i64, from: i64) -> Option<i64> {
        let depth = self.depth;
        let marks = self.conversations.entry(peer_id).or_default();
        let target = marks.forward.pop_back()?;

        marks.back.push_back(from);
        trim(&mut marks.back, depth);
        Some(target)
    }

    /// How many marks `peer_id`'s conversation keeps behind the reader.
    ///
    /// Read by the tests, and the only way to see what a walk left behind.
    #[must_use]
    pub fn depth(&self, peer_id: i64) -> usize {
        self.conversations
            .get(&peer_id)
            .map_or(0, |marks| marks.back.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHAT: i64 = 7;

    /// A place recorded and then walked back from `from`.
    fn walk_back(id: i64, from: i64) -> Option<i64> {
        let mut list = Jumplist::default();
        list.record(CHAT, id);
        list.back(CHAT, from)
    }

    #[test]
    fn a_mark_is_where_the_reader_was() {
        assert_eq!(walk_back(19, 24), Some(19));
    }

    /// The place a walk came from is kept, so `Ctrl-i` can answer it — and the
    /// end of the list is not an exception: the first `Ctrl-o` is where the
    /// reader's own position is learned.
    #[test]
    fn the_first_walk_back_marks_where_the_reader_was() {
        let mut list = Jumplist::default();
        list.record(CHAT, 19);

        assert_eq!(list.back(CHAT, 24), Some(19));
        assert_eq!(list.forward(CHAT, 19), Some(24), "`Ctrl-i` returns them");
    }

    /// A jump makes every place they had been heading irrelevant, so the forward
    /// marks go with it.
    #[test]
    fn a_new_mark_clears_the_forward_one() {
        let mut list = Jumplist::default();
        list.record(CHAT, 19);
        list.back(CHAT, 24);

        list.record(CHAT, 31);

        assert_eq!(list.forward(CHAT, 19), None, "the old way forward is gone");
        assert_eq!(list.back(CHAT, 31), Some(31), "the new mark is behind them");
        assert_eq!(list.depth(CHAT), 0, "and 19 is not");
    }

    /// Back and forward move a mark between the stacks rather than copying it,
    /// so walking out and back leaves the list where it started.
    #[test]
    fn a_walk_moves_the_mark_it_steps_over() {
        let mut list = Jumplist::default();
        list.record(CHAT, 10);
        list.record(CHAT, 19);

        assert_eq!(list.back(CHAT, 24), Some(19));
        assert_eq!(list.back(CHAT, 19), Some(10));
        assert_eq!(list.forward(CHAT, 10), Some(19));
        assert_eq!(list.forward(CHAT, 19), Some(24));
        assert_eq!(
            list.depth(CHAT),
            2,
            "the two places behind them are the two they were at"
        );
        assert_eq!(
            list.back(CHAT, 24),
            Some(19),
            "and walking back goes on the way it came"
        );
    }

    #[test]
    fn the_same_place_twice_is_one_place() {
        let mut list = Jumplist::default();
        list.record(CHAT, 19);
        list.record(CHAT, 19);

        assert_eq!(list.depth(CHAT), 1);
    }

    /// Bounded, or a reader who keeps jumping keeps every message they have
    /// ever been at.
    #[test]
    fn the_oldest_mark_goes_past_the_depth() {
        let newest =
            i64::try_from(DEFAULT_DEPTH).expect("a depth of marks fits in a message id") + 5;
        let mut marks = Jumplist::default();
        for id in 1..=newest {
            marks.record(CHAT, id);
        }

        assert_eq!(marks.depth(CHAT), DEFAULT_DEPTH);
        // The stack is walked from the newest end, so that is where the walk
        // starts: what it ends on is the oldest mark that survived.
        let mut walked = marks.back(CHAT, 0);
        let mut visited: Vec<i64> = Vec::new();
        while let Some(id) = walked {
            visited.push(id);
            walked = marks.back(CHAT, id);
        }

        assert_eq!(visited.len(), DEFAULT_DEPTH, "every mark is still walkable");
        assert_eq!(visited.first(), Some(&newest), "the newest survived");
        assert_eq!(visited.last(), Some(&6), "and the first five did not");
    }

    /// The cap is the default's, and the default is a hundred.
    #[test]
    fn the_default_depth_is_a_hundred() {
        assert_eq!(DEFAULT_DEPTH, 100);

        let mut marks = Jumplist::default();
        let past = i64::try_from(DEFAULT_DEPTH).expect("a hundred fits in a message id") + 1;
        for id in 1..=past {
            marks.record(CHAT, id);
        }

        assert_eq!(marks.depth(CHAT), 100, "the default caps at a hundred");
    }

    /// The depth is a parameter, not a constant: a conversation keeps whatever
    /// the configuration named, and nothing more.
    #[test]
    fn a_configured_depth_is_honoured() {
        let mut marks = Jumplist::new(3);
        for id in 1..=10 {
            marks.record(CHAT, id);
        }

        assert_eq!(marks.depth(CHAT), 3);
        // The newest three survived; the seventh did not.
        assert_eq!(marks.back(CHAT, 0), Some(10));
        assert_eq!(marks.back(CHAT, 10), Some(9));
        assert_eq!(marks.back(CHAT, 9), Some(8));
        assert_eq!(marks.back(CHAT, 8), None, "and the seventh is gone");
    }

    /// Both stacks are capped: a reader who walks the whole back stack out
    /// fills the forward one, and it holds no more than the depth either.
    #[test]
    fn the_forward_stack_is_capped_like_the_back_one() {
        let depth = 3;
        let last = i64::try_from(depth).expect("a small depth fits in a message id");
        let mut marks = Jumplist::new(depth);
        for id in 1..=last {
            marks.record(CHAT, id);
        }

        // Walk the whole back stack out, which moves every mark onto the
        // forward stack.
        let mut walked = marks.back(CHAT, 100);
        let mut backed = 0;
        while let Some(id) = walked {
            backed += 1;
            walked = marks.back(CHAT, id);
        }

        // Then walk it all back, which refills the back stack.
        let mut walked = marks.forward(CHAT, 0);
        let mut forwarded = 0;
        while let Some(id) = walked {
            forwarded += 1;
            walked = marks.forward(CHAT, id);
        }

        assert_eq!(backed, depth, "every back mark is walkable");
        assert_eq!(forwarded, depth, "and no forward one past the depth");
        assert_eq!(marks.depth(CHAT), depth);
    }

    #[test]
    fn nothing_to_walk_to_is_nothing() {
        let mut list = Jumplist::default();

        assert_eq!(list.back(CHAT, 24), None, "no marks at all");
        assert_eq!(list.forward(CHAT, 24), None);
    }

    #[test]
    fn the_end_of_the_list_is_not_a_place_to_go() {
        let mut list = Jumplist::default();
        list.record(CHAT, 19);
        list.record(CHAT, 24);

        assert_eq!(list.forward(CHAT, 24), None, "nothing was ever walked past");
        assert_eq!(
            list.depth(CHAT),
            2,
            "and asking does not mark the end of the list"
        );
    }

    /// One reader's jumps in one conversation are not another's: a mark is
    /// looked up by the conversation it belongs to, so switching chats cannot
    /// carry one across.
    #[test]
    fn separate_conversations_keep_separate_marks() {
        let mut list = Jumplist::default();
        list.record(CHAT, 19);
        list.record(CHAT + 1, 88);

        assert_eq!(list.back(CHAT + 1, 90), Some(88));
        assert_eq!(list.depth(CHAT), 1, "the other chat's mark is not here");
        assert_eq!(list.back(CHAT, 24), Some(19));
        assert_eq!(list.depth(CHAT), 0, "and this chat's is gone for good");
    }

    #[test]
    fn a_conversation_with_no_marks_has_no_depth() {
        assert_eq!(Jumplist::default().depth(CHAT), 0);
    }
}
