//! Per-peer parked drafts and read receipts.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use crate::app::PromptKind;
use crate::line::LineEditor;

/// Per-peer parked drafts and read receipts.
pub struct DraftStore {
    /// The drafts of the conversations the reader is not in.
    ///
    /// The open conversation's draft is [`InputState`](super::input::InputState)'s
    /// `line`; this holds the rest,
    /// parked under their peer id, so a reader who looks away and comes back
    /// finds the sentence they had started. The same seam as
    /// [`DraftStore::read_receipts`]: the view is replaced on every switch, and what
    /// belongs to the conversation rather than to the page on show is kept here.
    ///
    /// Only a plain message draft is stored — [`App::park_draft`] forgets the
    /// reply or edit subject on the way in — and a peer's entry is dropped when
    /// its draft is empty, so the map holds only peers with words in them.
    ///
    /// Persisted by `app` beside the configuration (`televim.drafts.json`):
    /// the loop loads the file at launch and re-saves the snapshot whenever
    /// it changes, so words survive a restart. An account change still clears
    /// it ([`App::set_chats`]), and a file tagged for another account is
    /// discarded at launch, so no words cross an account boundary.
    pub(crate) drafts: HashMap<i64, LineEditor>,

    /// How far each conversation this client has been told about has been read.
    ///
    /// One number per conversation, kept here rather than on
    /// [`ConversationView`] because the view is replaced on every chat switch:
    /// a reader who looks away and comes back must find the reading of that
    /// conversation as it was, not as a fresh view believes it. The view holds
    /// the one on show; this holds the rest.
    ///
    /// Monotone per conversation, because the wire says so: `max_id` is a
    /// watermark, and a read that has already been shown cannot be taken back by
    /// a later, lower one. Grows with the conversations the client is told about,
    /// which is no more than the chat list already holds.
    ///
    /// Persisted by `app` beside the parked drafts, as the newest
    /// [`DraftStore::recent_read_marks`] peers. Restored only through
    /// [`DraftStore::restore_read_marks`], which never moves a mark backwards, so
    /// a stored figure can only add a reading the reader was shown last session.
    /// A sign-out forgets them ([`DraftStore::clear_read_marks`]).
    pub(crate) read_receipts: RefCell<HashMap<i64, ReadMark>>,

    /// The stamp the next recorded mark takes. Orders the marks by when they
    /// last moved, so the persisted window keeps the most recent peers.
    read_clock: Cell<u64>,
}

/// One conversation's recorded read position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReadMark {
    /// The highest message id the peer has read.
    pub(crate) max_id: i64,
    /// When the mark last moved, against [`DraftStore::read_clock`].
    stamp: u64,
}

impl DraftStore {
    /// No parked drafts, and nothing recorded as read.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            drafts: HashMap::new(),
            read_receipts: RefCell::new(HashMap::new()),
            read_clock: Cell::new(0),
        }
    }

    /// Puts a conversation's parked draft back on the line.
    ///
    /// The value is moved out of the map, so the map never holds the open
    /// conversation's draft — which is what keeps [`DraftStore::snapshot`]
    /// from reporting it twice.
    pub(crate) fn take_draft(&mut self, chat_id: i64) -> LineEditor {
        self.drafts.remove(&chat_id).unwrap_or_default()
    }

    /// Exports every draft as plain `(peer id, text)` pairs.
    ///
    /// The parked map plus the open conversation's live line, when `open`
    /// names a real conversation (`chat_id != 0`) holding a buffer draft
    /// ([`PromptKind::is_buffer`]) with words in it. Sorted by peer id, so
    /// two snapshots of the same drafts compare equal. Empty lines and
    /// non-buffer purposes contribute nothing, and the live line wins over a
    /// parked entry for the same peer, so no draft is reported twice.
    ///
    /// Plain text only: the cursor, the undo history and the yank buffer stay
    /// behind, and there is no filesystem here — the caller decides where the
    /// pairs go.
    pub fn snapshot(&self, open: Option<(i64, &LineEditor)>) -> Vec<(i64, String)> {
        let live = open.filter(|(chat_id, line)| {
            *chat_id != 0 && line.purpose().is_buffer() && !line.is_empty()
        });
        let mut out: Vec<(i64, String)> = self
            .drafts
            .iter()
            .filter(|(chat_id, line)| {
                !line.text().is_empty() && live.is_none_or(|(open_id, _)| **chat_id != open_id)
            })
            .map(|(chat_id, line)| (*chat_id, line.text().to_owned()))
            .collect();
        if let Some((chat_id, line)) = live {
            out.push((chat_id, line.text().to_owned()));
        }
        out.sort_by_key(|(chat_id, _)| *chat_id);
        out
    }

    /// Imports `snapshot` pairs back into the parked map.
    ///
    /// Each pair is rebuilt through [`LineEditor::open_with`] as a plain
    /// message — the same shape [`park_draft`](super::coordinate::park_draft)
    /// stores — so restored words come back the way a resumed draft does:
    /// text back in, editing state default. Empty strings are skipped, and
    /// nothing already present is overwritten: the store starts empty at
    /// launch, so the guard is against loading twice, not a merge policy.
    pub fn restore(&mut self, drafts: Vec<(i64, String)>) {
        for (chat_id, text) in drafts {
            if text.is_empty() || self.drafts.contains_key(&chat_id) {
                continue;
            }
            let mut line = LineEditor::new();
            line.open_with(PromptKind::Message, text);
            self.drafts.insert(chat_id, line);
        }
    }

    /// Records how far a conversation has been read, keeping the highest figure
    /// seen for it.
    ///
    /// The feed's acknowledgement can repeat or arrive late, so a lower one is
    /// dropped rather than applied: a read already shown cannot be taken back.
    /// Reports whether this figure moved the conversation's watermark.
    pub(crate) fn note_read(&self, chat_id: i64, max_id: i64) -> bool {
        if max_id <= 0 {
            return false;
        }

        let mut recorded = self.read_receipts.borrow_mut();
        let moved = recorded
            .get(&chat_id)
            .is_none_or(|mark| max_id > mark.max_id);
        if moved {
            let mark = ReadMark {
                max_id,
                stamp: self.next_stamp(),
            };
            recorded.insert(chat_id, mark);
        }

        moved
    }

    /// The `limit` most recently moved marks, as `(peer id, max id)` pairs
    /// sorted by peer id, so the persisted form is the same bytes for the same
    /// marks.
    #[must_use]
    pub fn recent_read_marks(&self, limit: usize) -> Vec<(i64, i64)> {
        let recorded = self.read_receipts.borrow();
        let mut newest: Vec<(i64, ReadMark)> = recorded.iter().map(|(id, m)| (*id, *m)).collect();
        newest.sort_by_key(|(_, mark)| std::cmp::Reverse(mark.stamp));
        newest.truncate(limit);

        let mut out: Vec<(i64, i64)> = newest
            .into_iter()
            .map(|(chat_id, mark)| (chat_id, mark.max_id))
            .collect();
        out.sort_by_key(|(chat_id, _)| *chat_id);
        out
    }

    /// Merges marks loaded from disk, keeping the higher figure for each peer.
    ///
    /// A stored mark can only add to what is recorded: one that is not above
    /// the figure already held is dropped, so a stale file never moves a
    /// watermark backwards. Non-positive figures are dropped, as [`note_read`]
    /// drops them.
    ///
    /// [`note_read`]: DraftStore::note_read
    pub fn restore_read_marks(&mut self, marks: Vec<(i64, i64)>) {
        for (chat_id, max_id) in marks {
            if max_id <= 0 {
                continue;
            }
            let held = self.read_receipts.get_mut().get(&chat_id).map(|m| m.max_id);
            if held.is_none_or(|held| max_id > held) {
                let mark = ReadMark {
                    max_id,
                    stamp: self.next_stamp(),
                };
                self.read_receipts.get_mut().insert(chat_id, mark);
            }
        }
    }

    /// Forgets every recorded read position: a sign-out, so the next account
    /// never inherits the last one's marks.
    pub fn clear_read_marks(&mut self) {
        self.read_receipts.get_mut().clear();
    }

    /// The next stamp, after the last one handed out.
    fn next_stamp(&self) -> u64 {
        let stamp = self.read_clock.get() + 1;
        self.read_clock.set(stamp);
        stamp
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, PromptKind};
    use crate::state::coordinate::{park_draft, resume_draft, submit_edit, submit_message};
    use crate::state::input::InputState;

    /// A line opened for `purpose` with `text` in it.
    fn line(purpose: PromptKind, text: &str) -> LineEditor {
        let mut line = LineEditor::new();
        line.open_with(purpose, text.to_owned());
        line
    }

    /// Parks `text` for `chat_id` the way [`park_draft`] leaves it: a plain
    /// message, editing state fresh.
    fn parked(store: &mut DraftStore, chat_id: i64, text: &str) {
        store
            .drafts
            .insert(chat_id, line(PromptKind::Message, text));
    }

    #[test]
    fn snapshot_reports_parked_entries_sorted() {
        let mut store = DraftStore::new();
        parked(&mut store, 30, "thirty");
        parked(&mut store, 7, "seven");
        parked(&mut store, 12, "twelve");

        assert_eq!(
            store.snapshot(None),
            vec![
                (7, "seven".to_owned()),
                (12, "twelve".to_owned()),
                (30, "thirty".to_owned()),
            ],
            "parked drafts export sorted by peer id, so snapshots compare"
        );
    }

    #[test]
    fn snapshot_includes_the_live_line_only_for_a_buffer_with_words() {
        let mut store = DraftStore::new();
        parked(&mut store, 3, "parked");

        let message = line(PromptKind::Message, "live");
        assert_eq!(
            store.snapshot(Some((9, &message))),
            vec![(3, "parked".to_owned()), (9, "live".to_owned())],
            "a message draft on the open chat joins the parked ones"
        );

        let reply = line(PromptKind::Reply, "answering");
        assert_eq!(
            store.snapshot(Some((9, &reply))),
            vec![(3, "parked".to_owned()), (9, "answering".to_owned())],
            "a reply is a buffer too, and exports with it"
        );

        let empty = line(PromptKind::Message, "");
        assert_eq!(
            store.snapshot(Some((9, &empty))),
            vec![(3, "parked".to_owned())],
            "an empty line contributes nothing"
        );

        let command = line(PromptKind::Command, ":quit");
        assert_eq!(
            store.snapshot(Some((9, &command))),
            vec![(3, "parked".to_owned())],
            "a prompt is a question mid-answer, not a draft"
        );

        let nowhere = line(PromptKind::Message, "live");
        assert_eq!(
            store.snapshot(Some((0, &nowhere))),
            vec![(3, "parked".to_owned())],
            "chat zero is no conversation, so its line belongs to none"
        );
    }

    #[test]
    fn snapshot_reports_the_live_line_once_when_it_is_also_parked() {
        let mut store = DraftStore::new();
        parked(&mut store, 9, "stale");
        let live = line(PromptKind::Message, "live");

        assert_eq!(
            store.snapshot(Some((9, &live))),
            vec![(9, "live".to_owned())],
            "the line on show wins over a parked entry for the same peer"
        );
    }

    #[test]
    fn restore_round_trips_text_and_skips_empties_without_overwriting() {
        let mut store = DraftStore::new();
        store.restore(vec![
            (2, "second".to_owned()),
            (1, String::new()),
            (1, "first".to_owned()),
        ]);

        assert_eq!(
            store.snapshot(None),
            vec![(1, "first".to_owned()), (2, "second".to_owned())],
            "empty strings are skipped and pairs land sorted"
        );

        store.restore(vec![(1, "overwrite".to_owned()), (3, "third".to_owned())]);
        assert_eq!(
            store.snapshot(None),
            vec![
                (1, "first".to_owned()),
                (2, "second".to_owned()),
                (3, "third".to_owned()),
            ],
            "loading twice keeps what was there: no overwrite, only additions"
        );

        let mut input = InputState::new();
        resume_draft(&mut store, &mut input, 2);
        assert_eq!(input.line.text(), "second", "text comes back on the line");
        assert_eq!(
            input.line.purpose(),
            PromptKind::Message,
            "as a plain message, the way a parked draft resumes"
        );
    }

    #[test]
    fn park_then_resume_then_snapshot_shows_the_draft_once() {
        let mut app = App::mock();
        let chat_id = app.conversation.conversation.window.chat_id;
        assert!(chat_id != 0, "the mock opens a conversation");

        app.input
            .set_line(line(PromptKind::Message, "mid-sentence"));
        park_draft(&mut app.conversation, &mut app.input, &mut app.drafts);
        assert!(
            app.drafts.drafts.contains_key(&chat_id),
            "leaving parks the draft under its peer"
        );

        resume_draft(&mut app.drafts, &mut app.input, chat_id);
        assert_eq!(app.input.line.text(), "mid-sentence");

        assert_eq!(
            app.drafts.snapshot(Some((chat_id, &app.input.line))),
            vec![(chat_id, "mid-sentence".to_owned())],
            "resume takes the entry out, so the live line reports it exactly once"
        );
    }

    #[test]
    fn submit_message_clears_the_open_chat_parked_entry() {
        let mut app = App::mock();
        let chat_id = app.conversation.conversation.window.chat_id;
        app.drafts.restore(vec![
            (chat_id, "parked".to_owned()),
            (4242, "someone else's".to_owned()),
        ]);
        app.input
            .set_line(line(PromptKind::Message, "sending this"));

        submit_message(
            &mut app.ui,
            &mut app.conversation,
            &mut app.input,
            &mut app.outbox,
            &mut app.drafts,
        );

        assert!(
            !app.drafts.drafts.contains_key(&chat_id),
            "a sent message leaves no draft behind, or `Enter` would send it twice"
        );
        assert!(
            app.drafts.drafts.contains_key(&4242),
            "but a send clears only the peer it went to"
        );
    }

    #[test]
    fn submit_edit_clears_the_open_chat_parked_entry() {
        let mut app = App::mock();
        let chat_id = app.conversation.conversation.window.chat_id;
        app.drafts.restore(vec![(chat_id, "parked".to_owned())]);
        app.conversation.editing = Some(77);
        app.input.set_line(line(PromptKind::Edit, "edited"));

        submit_edit(
            &mut app.ui,
            &mut app.conversation,
            &mut app.input,
            &mut app.outbox,
            &mut app.drafts,
        );

        assert!(
            !app.drafts.drafts.contains_key(&chat_id),
            "a queued edit leaves no draft behind either"
        );
    }

    #[test]
    fn a_loaded_mark_never_moves_a_watermark_backwards() {
        let mut store = DraftStore::new();
        assert!(store.note_read(7, 9));

        store.restore_read_marks(vec![(7, 4), (8, 3), (9, 0), (10, -1)]);
        assert_eq!(
            store.recent_read_marks(10),
            vec![(7, 9), (8, 3)],
            "a stale load keeps the figure held, and a non-positive one is no mark"
        );

        store.restore_read_marks(vec![(7, 12)]);
        assert_eq!(
            store.recent_read_marks(10),
            vec![(7, 12), (8, 3)],
            "a higher loaded figure does raise it"
        );
    }

    #[test]
    fn restored_marks_come_back_sorted_by_peer() {
        let mut store = DraftStore::new();
        store.restore_read_marks(vec![(4, 10), (2, 7)]);

        assert_eq!(store.recent_read_marks(10), vec![(2, 7), (4, 10)]);
    }

    #[test]
    fn the_persisted_window_keeps_the_most_recently_moved_peers() {
        let store = DraftStore::new();
        assert!(store.note_read(1, 5));
        assert!(store.note_read(2, 5));
        assert!(store.note_read(3, 5));
        assert!(
            store.note_read(1, 6),
            "peer 1 moves again, so it is the newest"
        );

        assert_eq!(
            store.recent_read_marks(2),
            vec![(1, 6), (3, 5)],
            "the two most recently moved, sorted by peer id"
        );
    }

    #[test]
    fn clearing_read_marks_forgets_them_all() {
        let mut store = DraftStore::new();
        assert!(store.note_read(7, 3));

        store.clear_read_marks();

        assert!(store.recent_read_marks(32).is_empty());
        assert!(store.note_read(7, 3), "and a cleared peer records afresh");
    }
}
