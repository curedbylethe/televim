//! Per-peer parked drafts and read receipts.

use std::cell::RefCell;
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
    /// Not written to disk, for the same reason as [`DraftStore::read_receipts`]: a
    /// launch starts empty, and an account change clears it ([`App::set_chats`])
    /// so no words cross an account boundary.
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
    /// Not written to disk: a launch starts with nothing recorded, so a receipt
    /// the reader has not been shown is never drawn from a previous session's
    /// memory of it. That is the "never claim more than was received" rule at the
    /// storage layer.
    pub(crate) read_receipts: RefCell<HashMap<i64, i64>>,
}

impl DraftStore {
    /// No parked drafts, and nothing recorded as read.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            drafts: HashMap::new(),
            read_receipts: RefCell::new(HashMap::new()),
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
        let moved = recorded.get(&chat_id).is_none_or(|read| max_id > *read);
        if moved {
            recorded.insert(chat_id, max_id);
        }

        moved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, PromptKind};
    use crate::state::coordinate::{park_draft, resume_draft};
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
}
