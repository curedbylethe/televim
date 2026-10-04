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

/// How many marks one conversation keeps.
///
/// Bounded because a reader who keeps jumping would otherwise keep every
/// message they have ever been at, in a program whose whole budget is a few
/// megabytes. Vim's own `'jumplist'` depth is 50; 100 is the same answer with
/// room to spare, and a reader who has forgotten the last hundred places they
/// were was never going to walk back that far.
const DEPTH: usize = 100;

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

/// Where the reader has been, per conversation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Jumplist {
    conversations: HashMap<i64, Marks>,
}

impl Jumplist {
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
        let marks = self.conversations.entry(peer_id).or_default();
        if marks.back.back() == Some(&id) {
            return;
        }

        marks.back.push_back(id);
        marks.forward.clear();
        while marks.back.len() > DEPTH {
            marks.back.pop_front();
        }
    }

    /// The message before the one at `from`, or `None` when there is none.
    ///
    /// `from` is where the reader is now, and it is pushed onto the forward
    /// stack rather than dropped: that is what makes `Ctrl-i` able to bring
    /// them back again, and it is why the first `Ctrl-o` marks the place the
    /// reader was standing at the end of the list rather than discarding it.
    pub fn back(&mut self, peer_id: i64, from: i64) -> Option<i64> {
        let marks = self.conversations.entry(peer_id).or_default();
        let target = marks.back.pop_back()?;

        marks.forward.push_back(from);
        Some(target)
    }

    /// The message after the one at `from`, or `None` when they are as far
    /// forward as they can go.
    ///
    /// The mirror of [`Jumplist::back`], and it moves a mark rather than
    /// copying one: a reader who walks back and forward twice has been in the
    /// same two places, not four.
    pub fn forward(&mut self, peer_id: i64, from: i64) -> Option<i64> {
        let marks = self.conversations.entry(peer_id).or_default();
        let target = marks.forward.pop_back()?;

        marks.back.push_back(from);
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
        let newest = i64::try_from(DEPTH).expect("a depth of marks fits in a message id") + 5;
        let mut marks = Jumplist::default();
        for id in 1..=newest {
            marks.record(CHAT, id);
        }

        assert_eq!(marks.depth(CHAT), DEPTH);
        // The stack is walked from the newest end, so that is where the walk
        // starts: what it ends on is the oldest mark that survived.
        let mut walked = marks.back(CHAT, 0);
        let mut visited: Vec<i64> = Vec::new();
        while let Some(id) = walked {
            visited.push(id);
            walked = marks.back(CHAT, id);
        }

        assert_eq!(visited.len(), DEPTH, "every mark is still walkable");
        assert_eq!(visited.first(), Some(&newest), "the newest survived");
        assert_eq!(visited.last(), Some(&6), "and the first five did not");
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
