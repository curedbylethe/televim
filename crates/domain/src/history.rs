//! The messages of the conversation on show.
//!
//! [`ChatList`](crate::updates::ChatList) holds what the client has seen across
//! every conversation, in one flat, newest-first window, so that an edit or a
//! deletion can be matched against a message wherever it lives. That shape is
//! right for applying updates and wrong for reading a conversation: it is
//! interleaved, and it runs backwards.
//!
//! This module is the view over the same messages for the one conversation the
//! user has open: oldest first — the order a conversation is read in — and
//! bounded, so that the open chat scrolls without the client holding its whole
//! history. Only one conversation is open at a time, so the ceiling here is a
//! ceiling on what scrolling costs.
//!
//! # Why a second window
//!
//! The two overlap by design, and deduplicating by message identifier is what
//! makes the overlap harmless: an arrival lands in both, and the second copy is
//! recognised as one this window already holds. Splitting them instead — one
//! list per conversation, grown without bound — is exactly what the memory
//! budget cannot absorb.
//!
//! # Anchoring
//!
//! A window moves under the reader: loading an older page puts messages in
//! front of the one on screen, and a bounded window eventually drops the one
//! behind. Something therefore has to say where the reader was, and it is said by
//! **message identifier**, never by line offset — how many lines a message wraps
//! onto is a question about the width of a terminal, which nothing here knows or
//! should, and an index means a different message on either side of a page.
//!
//! The identifier itself is not stored here. [`ConversationWindow::position_of`]
//! is how one is looked up again, and the reader's place is held by whatever is
//! already moving it — the cursor, on the side that draws. This module's part is
//! the window it is looked up in and the one fact that says whether an arrival
//! may move the reader at all.

use std::borrow::Cow;
use std::collections::{HashSet, VecDeque};

use crate::message::{Message, MessageStatus};
use crate::updates::UpdateEvent;

/// How many messages the conversation on show holds before the far end is
/// dropped.
///
/// Smaller than [`MESSAGE_WINDOW`](crate::updates::MESSAGE_WINDOW), because the
/// two answer different questions:
/// that one is how much the client remembers so it can apply an edit or a
/// deletion, and this one is how much of one conversation is worth holding to
/// scroll through. Two hundred is far more than a screenful, so the reader
/// cannot scroll out of what the client holds — and it is the number the memory
/// budget pays for, not a preference.
pub const CONVERSATION_WINDOW: usize = 200;

/// How many failed sends a conversation explains at once.
///
/// Each entry is the reason one pending message failed, and the bound is what
/// keeps a run of failures from growing without limit. Reaching it is a
/// deliberate act: the interface permits one send in flight at a time, so eight
/// undismissed failures means eight refusals in a row were left on screen. The
/// oldest is dropped to make room — and its message is dismissed with it, so a
/// `Failed` message is never left without the reason that explains it.
pub const FAILED_REASONS: usize = 8;

/// The messages of one conversation, oldest first.
///
/// Every mutation keeps two invariants: the deque is ordered oldest first, and it
/// never holds more than [`CONVERSATION_WINDOW`]. Both are load-bearing rather
/// than tidy — the first is what lets a window be rendered without sorting, and
/// the second is the ceiling on memory.
#[derive(Debug, Clone)]
pub struct ConversationWindow {
    /// The conversation these messages belong to.
    ///
    /// A message names its own conversation, so this is the filter rather than a
    /// second record of the same fact: a page arriving for another conversation
    /// is not spliced into the one on show.
    pub chat_id: i64,

    /// The messages, oldest first.
    messages: VecDeque<Message>,

    /// Whether the conversation has nothing in front of what is loaded.
    ///
    /// Mirrors what the fetch settled rather than being worked out here, because
    /// "there is no more" is something only Telegram can say. The layer that
    /// fetches is not visible to the one that renders, so the answer travels
    /// with the window.
    pub exhausted_older: bool,

    /// Whether the conversation has nothing behind what is loaded.
    pub exhausted_newer: bool,
}

impl ConversationWindow {
    /// An empty window for one conversation.
    #[must_use]
    pub fn new(chat_id: i64) -> Self {
        Self {
            chat_id,
            messages: VecDeque::new(),
            exhausted_older: false,
            exhausted_newer: false,
        }
    }

    /// How many messages the window holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Whether the window holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// The messages, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &Message> {
        self.messages.iter()
    }

    /// The message at `index`, counting from the oldest.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&Message> {
        self.messages.get(index)
    }

    /// Where a message sits in the window, if it is still in it.
    ///
    /// How the reader's position survives a window that moves: the identifier is
    /// remembered, and looked up again once the window has changed.
    #[must_use]
    pub fn position_of(&self, message_id: i64) -> Option<usize> {
        self.messages
            .iter()
            .position(|message| message.id == message_id)
    }

    /// Identifier of the oldest message in the window the server has numbered.
    ///
    /// Identifiers at or below zero are local placeholders for a send that has
    /// not been acknowledged — see [`ConversationView::queue_send`] — so they
    /// are skipped. What a bound is for is asking Telegram for what lies beyond
    /// it, and Telegram has no message numbered zero.
    #[must_use]
    pub fn oldest_id(&self) -> Option<i64> {
        self.messages
            .iter()
            .find(|message| is_numbered(message.id))
            .map(|message| message.id)
    }

    /// Identifier of the newest message in the window the server has numbered.
    ///
    /// Skips placeholders exactly as [`ConversationWindow::oldest_id`] does. A
    /// pending send sorts after every real identifier — it is negative — and
    /// would otherwise become "the newest message", poisoning everything that
    /// counts on from there.
    #[must_use]
    pub fn newest_id(&self) -> Option<i64> {
        self.messages
            .iter()
            .rev()
            .find(|message| is_numbered(message.id))
            .map(|message| message.id)
    }

    /// Replaces everything the window holds with `messages`.
    ///
    /// This is what opening a conversation, and jumping to a message inside one,
    /// do: the window is not extended but restarted, so both directions open
    /// again — a window that jumped is surrounded by the unknown. The caller is
    /// expected to say so on the flags.
    ///
    /// The newest [`CONVERSATION_WINDOW`] are kept: a page is never wider than
    /// what Telegram returns, so in practice nothing is dropped, but a caller
    /// that hands over a whole conversation should not be able to blow the
    /// budget by doing so.
    pub fn replace(&mut self, messages: impl IntoIterator<Item = Message>) {
        let mut page: Vec<Message> = messages
            .into_iter()
            .filter(|message| message.chat_id == self.chat_id)
            .collect();

        page.sort_by_key(|message| message.id);

        let dropped = page.len().saturating_sub(CONVERSATION_WINDOW);
        self.messages = page.into_iter().skip(dropped).collect();
    }

    /// Puts older messages in front of what the window holds.
    ///
    /// The page goes in the way it reads — oldest first — so that whatever order
    /// it arrives in, the window's own order survives it.
    ///
    /// Reports whether anything was added. A message the window already holds is
    /// skipped rather than duplicated: paging backwards can overlap by one, and
    /// a message rendered twice is worse than the round trip that caused it.
    ///
    /// What falls off the far end is the **newest**, because the window has
    /// moved towards the past.
    pub fn push_front(&mut self, older: impl IntoIterator<Item = Message>) -> bool {
        let mut page = self.accept(older);
        if page.is_empty() {
            return false;
        }

        page.sort_by_key(|message| message.id);

        // Pushed in reverse, because each one goes in front of the last: the
        // oldest of the page has to end up furthest forward.
        for message in page.into_iter().rev() {
            self.messages.push_front(message);
        }

        // Trimming from the back keeps the front, which is where the older
        // messages just went.
        self.messages.truncate(CONVERSATION_WINDOW);

        true
    }

    /// Puts newer messages behind what the window holds.
    ///
    /// Reports whether anything was added, on the same deduplication as
    /// [`ConversationWindow::push_front`]. What falls off the far end is the
    /// **oldest**, because the window has moved towards the present — which is
    /// what an arrival landing in a conversation the reader has scrolled back in
    /// does.
    pub fn push_back(&mut self, newer: impl IntoIterator<Item = Message>) -> bool {
        let mut page = self.accept(newer);
        if page.is_empty() {
            return false;
        }

        page.sort_by_key(|message| message.id);
        self.messages.extend(page);

        let dropped = self.messages.len().saturating_sub(CONVERSATION_WINDOW);
        for _ in 0..dropped {
            self.messages.pop_front();
        }

        true
    }

    /// The messages of a page that belong in this window and are not already in
    /// it.
    ///
    /// Deduplication is what makes the overlap between this window and the flat
    /// one harmless: an arrival is delivered to both, and the second delivery
    /// has to be recognised rather than rendered twice.
    fn accept(&self, page: impl IntoIterator<Item = Message>) -> Vec<Message> {
        let mut held: HashSet<i64> = self.messages.iter().map(|message| message.id).collect();

        page.into_iter()
            .filter(|message| message.chat_id == self.chat_id && held.insert(message.id))
            .collect()
    }

    /// Applies an event from the feed, reporting whether anything changed.
    ///
    /// The per-conversation counterpart to [`ChatList::apply_update`], and the
    /// same contract: `false` means nothing observable moved, so a caller does
    /// not owe a redraw.
    ///
    /// [`ChatList::apply_update`]: crate::updates::ChatList::apply_update
    #[must_use]
    pub fn apply_event(&mut self, event: &UpdateEvent) -> bool {
        match event {
            UpdateEvent::NewMessage(message) => self.push_back(std::iter::once(message.clone())),

            UpdateEvent::MessageEdited {
                chat_id,
                message_id,
                new_text,
            } => self.apply_edit(*chat_id, *message_id, new_text.as_ref()),

            // The event names no conversation, and neither does the window need
            // it to: everything the window holds belongs to one, so an
            // identifier is enough to find the message if it is here at all.
            UpdateEvent::MessagesDeleted { message_ids } => self.apply_deletion(message_ids),

            // The window holds messages and nothing else. Whether they have been
            // read is the conversation's fact, kept beside the window rather than
            // in it — which is what lets it outlive the page on show — and a peer
            // composing one is not a message. Nothing here expires a typing flag
            // either: the update that says the peer stopped is what clears it.
            UpdateEvent::ReadReceipt { .. } | UpdateEvent::PeerTyping { .. } => false,
        }
    }

    /// Replaces the text of a message in the window.
    fn apply_edit(&mut self, chat_id: i64, message_id: i64, new_text: &str) -> bool {
        if chat_id != self.chat_id {
            return false;
        }

        let Some(index) = self
            .messages
            .iter()
            .position(|message| message.id == message_id)
        else {
            return false;
        };

        if self.messages[index].text == new_text {
            return false;
        }

        self.messages[index].text = Cow::Owned(new_text.to_owned());

        true
    }

    /// Removes deleted messages from the window.
    fn apply_deletion(&mut self, message_ids: &[i64]) -> bool {
        if message_ids.is_empty() {
            return false;
        }

        let deleted: HashSet<i64> = message_ids.iter().copied().collect();
        let before = self.messages.len();

        self.messages
            .retain(|message| !deleted.contains(&message.id));

        self.messages.len() != before
    }

    /// Removes one message by identifier, reporting whether it was there.
    ///
    /// Used for the local placeholder a send is rendered as: confirming it or
    /// dismissing it takes the placeholder out, and the message it became joins
    /// on its own terms.
    fn remove(&mut self, message_id: i64) -> bool {
        let before = self.messages.len();

        self.messages.retain(|message| message.id != message_id);

        self.messages.len() != before
    }

    /// Marks one message with a new status, reporting whether it was there.
    fn set_status(&mut self, message_id: i64, status: MessageStatus) -> bool {
        let Some(message) = self
            .messages
            .iter_mut()
            .find(|message| message.id == message_id)
        else {
            return false;
        };

        message.status = status;
        true
    }
}

/// Whether an identifier names a message the server has numbered.
///
/// Telegram numbers real messages from one, so an identifier at or below zero is
/// a local placeholder rather than a message anyone else can be told about. The
/// distinction is load-bearing for [`ConversationWindow::oldest_id`] and
/// [`ConversationWindow::newest_id`]: a placeholder that sorts after every real
/// identifier must not be mistaken for the newest one.
fn is_numbered(id: i64) -> bool {
    id > 0
}

/// The conversation on show, and whether the reader is following it.
///
/// One fact sits beside the window, and it is about position rather than
/// rendering: whether the view is pinned to the newest message, which is where a
/// conversation opens and where `G` leaves it — and it is the reason an arrival
/// can move the reader at all.
///
/// Where in the window the reader is, is not kept here. It is the cursor's
/// business, and [`ConversationWindow::position_of`] is how a cursor that has
/// moved with the window is put back: the two halves are joined by a message
/// identifier, which survives a page in a way an index does not.
#[derive(Debug, Clone)]
pub struct ConversationView {
    /// The messages of the open conversation.
    pub window: ConversationWindow,

    /// Whether the view is pinned to the newest message.
    auto_follow: bool,

    /// Why each failed send failed, keyed by the placeholder's identifier.
    ///
    /// The reason is not on [`Message`], because it is not a fact about the
    /// message — it is the outcome of one attempt to send it. Bounded by
    /// [`FAILED_REASONS`], and an evicted reason takes its message with it.
    failures: Vec<(i64, String)>,

    /// The highest outgoing message identifier the peer has read.
    ///
    /// A watermark rather than a state per message, because that is how the fact
    /// arrives: one number saying everything up to here has been read. Storing it
    /// once makes "has this been read" arithmetic over the identifier a message
    /// already has, rather than a mutation of every message below it — and it is
    /// what a group's state is derived from when the panel draws it.
    ///
    /// `None` is its own answer rather than "nothing has been read": it means
    /// nothing has *said*, which the panel shows as no receipt at all rather than
    /// as a claim that a message is unread.
    read_watermark: Option<i64>,

    /// The next placeholder identifier to hand out.
    ///
    /// Starts at `-1` and counts down. Negative, always, so a placeholder can
    /// never collide with a real identifier: Telegram numbers messages with
    /// positive `i32` values.
    next_temp_id: i64,
}

impl ConversationView {
    /// A view of one conversation, pinned to the bottom with nothing in it.
    ///
    /// A conversation opens where a reader expects to find it — at the newest
    /// message — so following is the state it starts in rather than one it has
    /// to be put into.
    #[must_use]
    pub fn new(chat_id: i64) -> Self {
        Self {
            window: ConversationWindow::new(chat_id),
            auto_follow: true,
            failures: Vec::new(),
            read_watermark: None,
            next_temp_id: -1,
        }
    }

    /// Whether the view is pinned to the newest message.
    #[must_use]
    pub fn auto_follow(&self) -> bool {
        self.auto_follow
    }

    /// Pins the view to the newest message.
    ///
    /// Reached by `G`, and by scrolling down to the last message: from there a
    /// reader expects an arrival to be shown, which is the whole of what
    /// following means.
    pub fn follow(&mut self) {
        self.auto_follow = true;
    }

    /// Unpins the view, so an arrival no longer moves the reader.
    ///
    /// What scrolling up does. The reader's place in the window is not touched
    /// here: they are where they were, and an arrival is simply no longer
    /// entitled to move them off it.
    pub fn unfollow(&mut self) {
        self.auto_follow = false;
    }

    /// Applies an event from the feed, reporting whether anything changed.
    ///
    /// Everything but a read acknowledgement delegates to the window, which is
    /// where the messages live; the acknowledgement is folded here instead,
    /// because the watermark is the view's state and outlives the page on show —
    /// a chat switch replaces the window, and how far that conversation has been
    /// read does not change because the reader looked away.
    #[must_use]
    pub fn apply_event(&mut self, event: &UpdateEvent) -> bool {
        match event {
            UpdateEvent::ReadReceipt { chat_id, max_id } => self.apply_read(*chat_id, *max_id),
            _ => self.window.apply_event(event),
        }
    }

    /// Advances the read watermark, and only for the conversation on show.
    ///
    /// A receipt about another conversation is not this view's business: the chat
    /// that owns it will be reopened later and asked again, and a watermark held
    /// here would mark messages read that nobody read.
    fn apply_read(&mut self, chat_id: i64, max_id: i64) -> bool {
        chat_id == self.window.chat_id && self.set_read_watermark(max_id)
    }

    /// Records the reader's own message before the server has acknowledged it.
    ///
    /// Returns the placeholder's identifier, which is what the caller matches the
    /// attempt's outcome against — [`ConversationView::confirm_sent`] or
    /// [`ConversationView::fail_send`]. It is always negative, counting down from
    /// `-1`, and Telegram numbers real messages with positive `i32` values, so a
    /// placeholder can never collide with one.
    ///
    /// The text is cloned once, into the message the reader will see. That is the
    /// whole of the copy this path makes, and it runs once per submit: a shared
    /// string would cost more to keep in step than it saves.
    ///
    /// The timestamp is zero, deliberately. [`ChatList`](crate::updates::ChatList)
    /// only lets a message become the conversation's preview when it is at least
    /// as recent as the one on show, so a placeholder stamped with the current
    /// time could suppress the real message's preview when it arrives. `domain`
    /// has no clock, and this is not the place to grow one.
    pub fn queue_send(&mut self, text: &str, reply_to: Option<i64>) -> i64 {
        let id = self.next_temp_id;
        self.next_temp_id -= 1;

        let message = Message {
            id,
            chat_id: self.window.chat_id,
            text: Cow::Owned(text.to_owned()),
            timestamp: 0,
            status: MessageStatus::Sending,
            is_outgoing: true,
            reply_to,
            media: None,
        };
        self.window.push_back(std::iter::once(message));

        id
    }

    /// Replaces a send's placeholder with the message the server accepted.
    ///
    /// Reports whether anything changed. The real message may already be in the
    /// window — the arrival and the send's result race, and the arrival can win —
    /// in which case it is recognised by identifier and not added twice, and this
    /// is only the placeholder leaving.
    pub fn confirm_sent(&mut self, temp_id: i64, real: Message) -> bool {
        let removed = self.window.remove(temp_id);
        let added = self.window.push_back(std::iter::once(real));
        self.forget_failure(temp_id);

        removed || added
    }

    /// Marks the send whose placeholder is `temp_id` as failed, and records why.
    ///
    /// Reports whether the placeholder was there to mark. Only acts on a
    /// negative identifier: the only messages [`ConversationView::queue_send`]
    /// creates are placeholders, so a `Failed` message is always one, and a
    /// message the server has numbered is refused rather than marked.
    pub fn fail_send(&mut self, temp_id: i64, reason: String) -> bool {
        if is_numbered(temp_id) || !self.window.set_status(temp_id, MessageStatus::Failed) {
            return false;
        }

        // The reason is replaced rather than appended to: a second outcome for
        // one attempt is the same attempt's answer, not another failure.
        self.forget_failure(temp_id);
        self.failures.push((temp_id, reason));
        self.trim_failures();

        true
    }

    /// Takes a failed message and its reason out of the conversation.
    ///
    /// Reports whether either was there. The two leave together: a reason without
    /// its message explains nothing, and a `Failed` message whose reason is gone
    /// is a dead end with nothing left to clear it.
    pub fn dismiss_failed(&mut self, temp_id: i64) -> bool {
        let removed_message = self.window.remove(temp_id);
        let before = self.failures.len();
        self.forget_failure(temp_id);

        removed_message || self.failures.len() != before
    }

    /// The message with this identifier, if the window holds it.
    ///
    /// Any message, not only a pending one: the reply excerpt looks up the
    /// message a reply names, which is usually an ordinary one the server
    /// numbered long ago.
    #[must_use]
    pub fn message(&self, message_id: i64) -> Option<&Message> {
        self.window.iter().find(|message| message.id == message_id)
    }

    /// Why the message with this identifier failed, if it did.
    #[must_use]
    pub fn failure(&self, message_id: i64) -> Option<&str> {
        self.failures
            .iter()
            .find(|(id, _)| *id == message_id)
            .map(|(_, reason)| reason.as_str())
    }

    /// The highest outgoing message identifier the peer has read, if anything has
    /// said.
    #[must_use]
    pub fn read_watermark(&self) -> Option<i64> {
        self.read_watermark
    }

    /// Records that the peer has read every outgoing message up to `max_id`.
    ///
    /// Reports whether the watermark moved. Monotone — `max(existing, max_id)` —
    /// because the acknowledgements arrive best-effort and can repeat or arrive
    /// out of order, and a watermark that went backwards would take back a read
    /// the reader has already been shown.
    ///
    /// An identifier at or below zero is refused: the only such identifiers are
    /// the placeholders of sends the server has not acknowledged, and no peer can
    /// have read one of those.
    ///
    /// This is where the feed's read state enters the client. Nothing here reads
    /// a clock, fetches anything, or knows what a protocol update is; the caller
    /// has already decided what the number means.
    pub fn set_read_watermark(&mut self, max_id: i64) -> bool {
        if max_id <= 0 {
            return false;
        }

        let moved = self.read_watermark.is_none_or(|read| max_id > read);
        if moved {
            self.read_watermark = Some(max_id);
        }

        moved
    }

    /// Forgets any reason recorded for a message.
    fn forget_failure(&mut self, message_id: i64) {
        self.failures.retain(|(id, _)| *id != message_id);
    }

    /// Drops the oldest failures until the bound holds, dismissing their
    /// messages with them.
    fn trim_failures(&mut self) {
        while self.failures.len() > FAILED_REASONS {
            let (id, _) = self.failures.remove(0);
            self.window.remove(id);
        }
    }
}

/// The message a conversation's unread messages start at, as far as its
/// numbering can say.
///
/// Telegram numbers messages within a conversation from one and reports how many
/// of them are unread, so the first unread is the newest one counted back by the
/// rest of them. That is arithmetic over two facts rather than a fact the client
/// holds: nothing in the wire format says *which* message a reader stopped at.
///
/// Deletions leave gaps in the numbering, so this can land in front of the true
/// first unread. That is what it is for. A message the client cannot name is a
/// message no page can be fetched around, whereas a page *around* an estimate
/// holds the real one whenever the estimate is close.
///
/// `None` when there is nothing unread, and when the conversation has no message
/// to count back from.
#[must_use]
pub fn unread_target(last_message_id: Option<i64>, unread_count: u32) -> Option<i64> {
    if unread_count == 0 {
        return None;
    }

    let last = last_message_id?;
    let unread = i64::from(unread_count);

    // Counted back from the newest rather than forward to it, and floored at
    // one: a message identifier below the first a conversation can have would
    // name something that does not exist. A conversation whose count is larger
    // than its numbering — every message deleted and the count left behind —
    // saturates here rather than running off the end of the type.
    Some(last.saturating_sub(unread.saturating_sub(1)).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::MessageStatus;

    /// A message with nothing but the fields under test filled in.
    fn message(chat_id: i64, id: i64, text: &'static str) -> Message {
        Message {
            id,
            chat_id,
            text: Cow::Borrowed(text),
            timestamp: 1_700_000_000 + id,
            status: MessageStatus::Received,
            is_outgoing: false,
            reply_to: None,
            media: None,
        }
    }

    /// Messages 1..=`to`, oldest first, in one conversation.
    fn messages(to: i64) -> Vec<Message> {
        (1..=to).map(|id| message(42, id, "text")).collect()
    }

    /// A page of messages with these identifiers, in one conversation.
    fn page(chat_id: i64, ids: &[i64]) -> Vec<Message> {
        ids.iter().map(|id| message(chat_id, *id, "text")).collect()
    }

    fn ids(window: &ConversationWindow) -> Vec<i64> {
        window.iter().map(|message| message.id).collect()
    }

    fn window_with(ids: &[i64]) -> ConversationWindow {
        let mut window = ConversationWindow::new(42);
        window.replace(page(42, ids));
        window
    }

    /// Applies a change the test expects to land.
    ///
    /// Asserting the report here rather than discarding it keeps the setup
    /// honest: a fixture that silently stopped taking effect would turn the
    /// assertions that follow into tests of nothing.
    fn applied(window: &mut ConversationWindow, event: &UpdateEvent) {
        assert!(
            window.apply_event(event),
            "the event was expected to change the window"
        );
    }

    fn arrival(chat_id: i64, id: i64, text: &'static str) -> UpdateEvent {
        UpdateEvent::NewMessage(message(chat_id, id, text))
    }

    // ---- the window ----------------------------------------------------

    #[test]
    fn a_window_opens_empty_and_in_one_conversation() {
        let window = ConversationWindow::new(42);

        assert_eq!(window.chat_id, 42);
        assert!(window.is_empty());
        assert_eq!(window.len(), 0);
        assert_eq!(window.oldest_id(), None);
        assert_eq!(window.newest_id(), None);
        assert!(
            !window.exhausted_older && !window.exhausted_newer,
            "nothing has ruled either direction out yet"
        );
    }

    #[test]
    fn a_replacement_comes_out_oldest_first() {
        let mut window = ConversationWindow::new(42);
        window.replace([
            message(42, 3, "c"),
            message(42, 1, "a"),
            message(42, 2, "b"),
        ]);

        assert_eq!(
            ids(&window),
            vec![1, 2, 3],
            "a page arrives in no useful order"
        );
        assert_eq!(window.oldest_id(), Some(1));
        assert_eq!(window.newest_id(), Some(3));
    }

    #[test]
    fn a_replacement_ignores_another_conversation() {
        let mut window = ConversationWindow::new(42);
        window.replace([message(42, 1, "mine"), message(99, 2, "not mine")]);

        assert_eq!(
            ids(&window),
            vec![1],
            "a window belongs to one conversation"
        );
    }

    /// The ceiling is a ceiling: a caller handing over a whole conversation must
    /// not be able to spend the memory budget in one call.
    #[test]
    fn a_replacement_keeps_only_the_newest_messages() {
        let mut window = ConversationWindow::new(42);
        let overflow = i64::try_from(CONVERSATION_WINDOW).expect("the cap fits an identifier") + 5;

        window.replace(messages(overflow));

        assert_eq!(window.len(), CONVERSATION_WINDOW);
        assert_eq!(
            window.newest_id(),
            Some(overflow),
            "the newest message is still on show"
        );
        assert_eq!(
            window.oldest_id(),
            Some(6),
            "the oldest are the ones that were dropped"
        );
    }

    #[test]
    fn an_older_page_goes_in_front() {
        let mut window = window_with(&[10, 11, 12]);

        assert!(window.push_front(page(42, &[8, 9])));

        assert_eq!(ids(&window), vec![8, 9, 10, 11, 12]);
        assert_eq!(window.oldest_id(), Some(8));
    }

    /// The eviction direction is the whole point: loading the past must not
    /// throw away the present the reader scrolled back from.
    #[test]
    fn an_older_page_evicts_the_newest() {
        let mut window = window_with(&[1, 2, 3]);
        let cap = i64::try_from(CONVERSATION_WINDOW).expect("the cap fits an identifier");

        // Fill to the brim, then push one more page in front of it.
        window.replace(messages(cap));
        assert!(window.push_front([message(42, 0, "older")]));

        assert_eq!(window.len(), CONVERSATION_WINDOW);
        assert_eq!(
            window.get(0).map(|message| message.id),
            Some(0),
            "the page that just arrived is kept"
        );
        assert_eq!(
            window.oldest_id(),
            Some(1),
            "a bound names only a message the server numbered, not a placeholder"
        );
        assert_eq!(
            window.newest_id(),
            Some(cap - 1),
            "the newest message is what scrolling towards the past gives up"
        );
    }

    #[test]
    fn a_newer_page_evicts_the_oldest() {
        let mut window = window_with(&[1, 2, 3]);
        let cap = i64::try_from(CONVERSATION_WINDOW).expect("the cap fits an identifier");

        window.replace(messages(cap));
        assert!(window.push_back([message(42, cap + 1, "newer")]));

        assert_eq!(window.len(), CONVERSATION_WINDOW);
        assert_eq!(window.oldest_id(), Some(2), "the oldest is what falls off");
        assert_eq!(window.newest_id(), Some(cap + 1));
    }

    /// The invariant that makes the ceiling real, rather than a number written
    /// down: however long the client runs, the window does not grow.
    #[test]
    fn the_window_never_outgrows_its_ceiling() {
        let mut window = ConversationWindow::new(42);
        let rounds = CONVERSATION_WINDOW * 3;

        for round in 0..rounds {
            let id = i64::try_from(round).expect("the round fits an identifier");
            window.push_back([message(42, id, "text")]);
            window.push_front([message(42, -id, "text")]);
        }

        assert!(window.len() <= CONVERSATION_WINDOW);
        assert_eq!(window.len(), CONVERSATION_WINDOW);
    }

    #[test]
    fn a_message_the_window_already_holds_is_not_duplicated() {
        let mut window = window_with(&[10, 11, 12]);

        assert!(
            !window.push_front([message(42, 11, "again")]),
            "paging can overlap by one; a message rendered twice is worse than that"
        );
        assert!(!window.push_back([message(42, 12, "again")]));
        assert_eq!(ids(&window), vec![10, 11, 12]);
    }

    #[test]
    fn a_page_for_another_conversation_is_not_spliced_in() {
        let mut window = window_with(&[10, 11, 12]);

        assert!(!window.push_front([message(99, 9, "stranger")]));
        assert!(!window.push_back([message(99, 13, "stranger")]));
        assert_eq!(ids(&window), vec![10, 11, 12]);
    }

    // ---- events --------------------------------------------------------

    #[test]
    fn an_arrival_lands_behind_the_rest() {
        let mut window = window_with(&[10, 11, 12]);

        applied(&mut window, &arrival(42, 13, "hello"));

        assert_eq!(ids(&window), vec![10, 11, 12, 13]);
        assert_eq!(
            window.get(3).map(|message| message.text.as_ref()),
            Some("hello")
        );
    }

    #[test]
    fn an_arrival_for_another_conversation_changes_nothing() {
        let mut window = window_with(&[10, 11, 12]);

        assert!(!window.apply_event(&arrival(99, 13, "hello")));
        assert_eq!(ids(&window), vec![10, 11, 12]);
    }

    /// The overlap between this window and the global one is harmless only
    /// because of this: the same arrival is delivered to both, and the second
    /// delivery has to be recognised.
    #[test]
    fn an_arrival_the_window_already_holds_is_not_a_change() {
        let mut window = window_with(&[10, 11, 12]);

        assert!(
            !window.apply_event(&arrival(42, 12, "again")),
            "a duplicate arrival owes no redraw"
        );
        assert_eq!(ids(&window), vec![10, 11, 12]);
    }

    #[test]
    fn an_edit_replaces_the_text_in_place() {
        let mut window = window_with(&[10, 11, 12]);

        applied(
            &mut window,
            &UpdateEvent::MessageEdited {
                chat_id: 42,
                message_id: 11,
                new_text: Cow::Borrowed("after"),
            },
        );

        assert_eq!(ids(&window), vec![10, 11, 12], "an edit moves nothing");
        assert_eq!(
            window.get(1).map(|message| message.text.as_ref()),
            Some("after")
        );
    }

    #[test]
    fn an_edit_that_changes_nothing_is_not_a_change() {
        let mut window = window_with(&[10, 11, 12]);
        // The fixture builds every message with the same text.
        let event = UpdateEvent::MessageEdited {
            chat_id: 42,
            message_id: 11,
            new_text: Cow::Borrowed("text"),
        };

        assert!(!window.apply_event(&event), "a redraw is not owed for it");
    }

    #[test]
    fn an_edit_for_another_conversation_or_message_changes_nothing() {
        let mut window = window_with(&[10, 11, 12]);

        let elsewhere = UpdateEvent::MessageEdited {
            chat_id: 99,
            message_id: 11,
            new_text: Cow::Borrowed("after"),
        };
        let unknown = UpdateEvent::MessageEdited {
            chat_id: 42,
            message_id: 999,
            new_text: Cow::Borrowed("after"),
        };

        assert!(!window.apply_event(&elsewhere));
        assert!(!window.apply_event(&unknown));
        assert_eq!(ids(&window), vec![10, 11, 12]);
    }

    #[test]
    fn a_deletion_removes_the_named_messages() {
        let mut window = window_with(&[10, 11, 12]);

        applied(
            &mut window,
            &UpdateEvent::MessagesDeleted {
                message_ids: vec![11],
            },
        );

        assert_eq!(ids(&window), vec![10, 12]);
    }

    #[test]
    fn a_deletion_of_messages_the_window_does_not_hold_changes_nothing() {
        let mut window = window_with(&[10, 11, 12]);

        assert!(!window.apply_event(&UpdateEvent::MessagesDeleted {
            message_ids: vec![99],
        }));
        assert!(!window.apply_event(&UpdateEvent::MessagesDeleted {
            message_ids: Vec::new(),
        }));
        assert_eq!(ids(&window), vec![10, 11, 12]);
    }

    // ---- where the reader is --------------------------------------------

    /// How a reader's position survives a window that moved: the identifier is
    /// looked up again once it has. The window is the half of that which lives
    /// here; the cursor holding the identifier is the half on the other side of
    /// the crate boundary.
    #[test]
    fn an_anchor_is_found_again_after_the_window_moves() {
        let mut window = window_with(&[10, 11, 12]);
        let anchor = window.get(1).map(|message| message.id);

        window.push_front([message(42, 8, "older"), message(42, 9, "older")]);

        assert_eq!(
            anchor.and_then(|id| window.position_of(id)),
            Some(3),
            "the message is still there; two arrived in front of it"
        );
    }

    /// The other half of the anchor rule: a position that fell out of the window
    /// cannot be restored, and saying so is better than silently landing
    /// somewhere else.
    #[test]
    fn an_anchor_that_was_evicted_is_simply_not_found() {
        let mut window = ConversationWindow::new(42);
        let cap = i64::try_from(CONVERSATION_WINDOW).expect("the cap fits an identifier");
        window.replace(messages(cap));
        let anchor = window.newest_id();

        window.push_front([message(42, 0, "older")]);

        assert_eq!(anchor, Some(cap));
        assert_eq!(
            anchor.and_then(|id| window.position_of(id)),
            None,
            "the window is a ceiling on memory; an evicted message has no position"
        );
    }

    // ---- following ------------------------------------------------------

    #[test]
    fn a_view_opens_pinned_to_the_bottom() {
        let view = ConversationView::new(42);

        assert!(
            view.auto_follow(),
            "a conversation opens where a reader expects to find it"
        );
        assert!(view.window.is_empty());
    }

    /// Following is the whole of the position this type holds, so the two
    /// transitions are the whole of its surface: a reader who moves away from
    /// the end is not moved back by an arrival, and one who returns is.
    #[test]
    fn moving_away_from_the_end_and_back_are_the_two_transitions() {
        let mut view = ConversationView::new(42);

        view.unfollow();
        assert!(
            !view.auto_follow(),
            "an arrival no longer has the right to move the reader"
        );

        view.follow();
        assert!(
            view.auto_follow(),
            "and returning to the newest message takes that right back"
        );
    }

    /// Unfollowing is a claim about arrivals, not a move: the reader is left
    /// exactly where they were, and the window is not touched.
    #[test]
    fn unfollowing_moves_nothing() {
        let mut view = ConversationView::new(42);
        view.window
            .replace([message(42, 10, "text"), message(42, 11, "text")]);

        view.unfollow();

        assert!(!view.auto_follow());
        assert_eq!(
            view.window
                .iter()
                .map(|message| message.id)
                .collect::<Vec<_>>(),
            vec![10, 11],
            "the reader stays where they were, with the window they were reading"
        );
    }

    #[test]
    fn a_view_applies_an_event_to_its_window() {
        let mut view = ConversationView::new(42);
        view.window.replace([message(42, 10, "text")]);

        assert!(view.apply_event(&arrival(42, 11, "hello")));

        assert_eq!(
            view.window
                .iter()
                .map(|message| message.id)
                .collect::<Vec<_>>(),
            vec![10, 11]
        );
    }

    // ---- where the unread messages start --------------------------------

    #[test]
    fn nothing_unread_is_nothing_to_count_back_from() {
        assert_eq!(unread_target(Some(112), 0), None);
        assert_eq!(
            unread_target(None, 0),
            None,
            "and a conversation with no message has nothing either way"
        );
    }

    /// A count without a message to count from is a conversation whose preview
    /// the client has not been given: there is nothing to subtract from.
    #[test]
    fn a_conversation_with_no_preview_has_no_first_unread() {
        assert_eq!(unread_target(None, 3), None);
    }

    #[test]
    fn the_first_unread_is_counted_back_from_the_newest() {
        assert_eq!(
            unread_target(Some(112), 3),
            Some(110),
            "the newest is 112, so three of them start at 110"
        );
        assert_eq!(
            unread_target(Some(112), 1),
            Some(112),
            "one unread message is the newest one"
        );
    }

    /// Identifiers start at one, so an estimate that would run off the front of
    /// the numbering names the first message rather than one that cannot exist.
    #[test]
    fn an_estimate_never_reaches_before_the_first_message() {
        assert_eq!(unread_target(Some(2), 5), Some(1));
        assert_eq!(
            unread_target(Some(3), u32::MAX),
            Some(1),
            "and a count larger than the conversation saturates rather than wrapping"
        );
        assert_eq!(
            unread_target(Some(i64::MIN), 2),
            Some(1),
            "as does a newest identifier with nothing to count back into"
        );
    }

    // ---- sending, and waiting for the server ---------------------------

    /// A conversation with one message the server has already numbered.
    fn view_with_a_message() -> ConversationView {
        let mut view = ConversationView::new(42);
        view.window.replace([message(42, 5, "hello")]);
        view
    }

    #[test]
    fn only_positive_identifiers_are_numbered_messages() {
        assert!(is_numbered(1));
        assert!(is_numbered(i64::MAX));
        assert!(
            !is_numbered(0) && !is_numbered(-1) && !is_numbered(i64::MIN),
            "a placeholder is never a message the server knows about"
        );
    }

    #[test]
    fn a_queued_send_is_a_negative_placeholder_at_the_bottom() {
        let mut view = view_with_a_message();

        let id = view.queue_send("on its way", None);

        assert_eq!(id, -1, "the first placeholder is minus one");
        let queued = view.message(id).expect("the placeholder is in the window");
        assert_eq!(queued.text, "on its way");
        assert!(queued.is_outgoing, "the reader wrote it");
        assert!(matches!(queued.status, MessageStatus::Sending));
        assert_eq!(
            queued.timestamp, 0,
            "a placeholder must not be stamped with a time, or it could suppress the real preview"
        );
        assert_eq!(queued.chat_id, 42);
        assert_eq!(queued.reply_to, None);
    }

    #[test]
    fn a_placeholder_carries_the_reply_it_was_composed_with() {
        let mut view = view_with_a_message();

        let id = view.queue_send("answering", Some(5));

        assert_eq!(
            view.message(id).and_then(|message| message.reply_to),
            Some(5)
        );
    }

    #[test]
    fn placeholders_count_down_and_are_never_reissued() {
        let mut view = ConversationView::new(42);

        let ids: Vec<i64> = (0..1_000).map(|_| view.queue_send("x", None)).collect();

        assert!(
            ids.iter().all(|id| *id < 0),
            "a placeholder is always negative, so it cannot collide with a real id"
        );
        assert!(
            ids.windows(2).all(|pair| pair[1] < pair[0]),
            "each is below the last, so a stale result cannot be taken for a fresh one"
        );
    }

    #[test]
    fn confirming_a_send_replaces_the_placeholder_with_the_real_message() {
        let mut view = view_with_a_message();
        let id = view.queue_send("hello", None);

        assert!(view.confirm_sent(id, message(42, 6, "hello")));

        assert!(view.message(id).is_none(), "the placeholder is gone");
        assert_eq!(
            view.window
                .iter()
                .map(|message| message.id)
                .collect::<Vec<_>>(),
            vec![5, 6]
        );
        assert_eq!(
            view.window.newest_id(),
            Some(6),
            "the real message is the newest"
        );
    }

    /// The race the two windows are meant to survive: the feed's arrival wins,
    /// so the real message is already in the window when the send's result
    /// arrives. Confirming must not add it twice.
    #[test]
    fn confirming_a_send_the_arrival_already_delivered_does_not_duplicate_it() {
        let mut view = view_with_a_message();
        let id = view.queue_send("hello", None);
        assert!(view.apply_event(&arrival(42, 6, "hello")));

        assert!(view.confirm_sent(id, message(42, 6, "hello")));

        assert_eq!(
            view.window.iter().filter(|message| message.id == 6).count(),
            1,
            "the real message appears once, whichever path delivered it"
        );
        assert!(view.message(id).is_none(), "and the placeholder has gone");
    }

    #[test]
    fn confirming_a_send_with_no_placeholder_still_accepts_the_real_message() {
        let mut view = ConversationView::new(42);

        assert!(view.confirm_sent(-1, message(42, 6, "hello")));

        assert_eq!(view.window.newest_id(), Some(6));
    }

    /// A conversation says nothing about the peer's reading until something says
    /// something, and `None` is that answer rather than "nothing has been read".
    #[test]
    fn a_view_starts_with_no_read_watermark() {
        assert_eq!(ConversationView::new(42).read_watermark(), None);
    }

    /// The watermark is one number for the whole conversation, and it only ever
    /// moves forward: an acknowledgement that repeats or arrives late must not
    /// take back a read the reader has already been shown.
    #[test]
    fn the_read_watermark_is_monotone() {
        let mut view = ConversationView::new(42);

        assert!(view.set_read_watermark(7));
        assert_eq!(view.read_watermark(), Some(7));
        assert!(view.set_read_watermark(9), "a higher read moves it");
        assert_eq!(view.read_watermark(), Some(9));
        assert!(
            !view.set_read_watermark(9),
            "the same read again changes nothing"
        );
        assert!(
            !view.set_read_watermark(8),
            "and a lower one does not take it back"
        );
        assert_eq!(view.read_watermark(), Some(9));
    }

    /// A send the server has not acknowledged has an identifier no peer can have
    /// read, so it is not a watermark.
    #[test]
    fn a_read_watermark_must_name_a_real_message() {
        let mut view = ConversationView::new(42);

        assert!(!view.set_read_watermark(0));
        assert!(!view.set_read_watermark(-1));
        assert_eq!(view.read_watermark(), None);
    }

    /// The watermark is a fact about the conversation rather than about the
    /// window, so replacing the window — as a page, a chat switch or a jump does —
    /// does not forget it.
    #[test]
    fn the_read_watermark_outlives_the_window_it_was_recorded_against() {
        let mut view = ConversationView::new(42);
        view.set_read_watermark(5);

        view.window.replace([message(42, 6, "hello")]);

        assert_eq!(view.read_watermark(), Some(5));
    }

    /// A read acknowledgement advances the watermark of the conversation it names,
    /// and of no other: a receipt about a chat the reader is not in is not this
    /// view's business.
    #[test]
    fn a_read_acknowledgement_advances_the_open_conversation_only() {
        let mut view = ConversationView::new(42);

        assert!(view.apply_event(&UpdateEvent::ReadReceipt {
            chat_id: 42,
            max_id: 5
        }));
        assert_eq!(view.read_watermark(), Some(5));

        assert!(
            !view.apply_event(&UpdateEvent::ReadReceipt {
                chat_id: 43,
                max_id: 9
            }),
            "a conversation that is not on show is not this view's"
        );
        assert_eq!(
            view.read_watermark(),
            Some(5),
            "and its watermark is unmoved"
        );
    }

    /// A watermark only moves forward, so a repeated or late acknowledgement
    /// reports no change and leaves the reader's view of what was read alone.
    #[test]
    fn a_read_acknowledgement_never_moves_the_watermark_backwards() {
        let mut view = ConversationView::new(42);
        assert!(view.apply_event(&UpdateEvent::ReadReceipt {
            chat_id: 42,
            max_id: 9,
        }));

        for older in [9, 5] {
            assert!(
                !view.apply_event(&UpdateEvent::ReadReceipt {
                    chat_id: 42,
                    max_id: older
                }),
                "{older} is not further than 9, and nothing observable moved"
            );
        }
        assert_eq!(view.read_watermark(), Some(9));
    }

    /// A read that names no message is not a read: the only identifiers below one
    /// are placeholders of sends the server has not acknowledged, and no peer can
    /// have read one of those.
    #[test]
    fn a_read_acknowledgement_of_nothing_real_is_benign() {
        let mut view = ConversationView::new(42);

        assert!(!view.apply_event(&UpdateEvent::ReadReceipt {
            chat_id: 42,
            max_id: 0
        }));
        assert_eq!(view.read_watermark(), None);

        assert!(view.apply_event(&UpdateEvent::ReadReceipt {
            chat_id: 42,
            max_id: 3
        }));
        assert_eq!(view.read_watermark(), Some(3));
    }

    /// A watermark older than the page on show is still the right answer for the
    /// messages that page holds: comparison, not rewriting, is what makes an older
    /// page cost nothing extra.
    #[test]
    fn a_read_watermark_marks_the_older_page_it_covers() {
        let mut view = ConversationView::new(42);
        view.window
            .replace((4..=6).map(|id| message(42, id, "text")));
        view.set_read_watermark(5);

        assert!(view.window.iter().any(|message| message.id == 5));
        assert_eq!(
            view.read_watermark(),
            Some(5),
            "which is what a reader is told: everything up to here was read"
        );
    }

    #[test]
    fn a_failed_send_keeps_its_message_and_records_the_reason() {
        let mut view = view_with_a_message();
        let id = view.queue_send("hello", None);

        assert!(view.fail_send(id, "flood wait, retry in 42s".to_owned()));

        assert_eq!(
            view.message(id).map(|message| message.status),
            Some(MessageStatus::Failed)
        );
        assert_eq!(view.failure(id), Some("flood wait, retry in 42s"));
        assert_eq!(
            view.window.len(),
            2,
            "a failed send is still the reader's message, not a thing to hide"
        );
    }

    #[test]
    fn failing_a_send_again_replaces_the_reason_rather_than_stacking_it() {
        let mut view = view_with_a_message();
        let id = view.queue_send("hello", None);

        assert!(view.fail_send(id, "first".to_owned()));
        assert!(view.fail_send(id, "second".to_owned()));

        assert_eq!(view.failure(id), Some("second"));
    }

    #[test]
    fn failing_a_message_that_was_never_queued_does_nothing() {
        let mut view = view_with_a_message();

        assert!(
            !view.fail_send(6, "nope".to_owned()),
            "there is no placeholder with that identifier"
        );
        assert!(
            !view.fail_send(5, "nope".to_owned()),
            "a message the server numbered is not a pending send"
        );

        assert_eq!(view.failure(5), None);
        assert_eq!(
            view.message(5).map(|message| message.status),
            Some(MessageStatus::Received),
            "and the real message keeps the status it arrived with"
        );
    }

    #[test]
    fn dismissing_a_failed_send_removes_the_message_and_the_reason_together() {
        let mut view = view_with_a_message();
        let id = view.queue_send("hello", None);
        view.fail_send(id, "no route".to_owned());

        assert!(view.dismiss_failed(id));

        assert!(view.message(id).is_none());
        assert_eq!(
            view.failure(id),
            None,
            "a reason with no message explains nothing"
        );
        assert!(
            !view.dismiss_failed(id),
            "dismissing the same failure again is a no-op"
        );
    }

    /// The bound is what keeps a reader who ignores failure after failure from
    /// growing the reasons without limit. Eviction is one operation over the
    /// reasons and the window: the oldest reason leaves with its message, so a
    /// surviving `Failed` message is never left unexplained.
    #[test]
    fn the_reasons_a_conversation_gives_are_bounded() {
        let mut view = ConversationView::new(42);

        let held: Vec<i64> = (0..=FAILED_REASONS)
            .map(|_| {
                let id = view.queue_send("x", None);
                view.fail_send(id, "reason".to_owned());
                id
            })
            .collect();

        assert_eq!(held.len(), FAILED_REASONS + 1, "one more than the bound");
        assert_eq!(view.failure(held[0]), None, "the oldest reason was evicted");
        assert!(
            view.message(held[0]).is_none(),
            "and its message was dismissed with it"
        );
        assert_eq!(view.window.len(), FAILED_REASONS, "the bound holds");
        assert_eq!(
            view.failure(held[FAILED_REASONS]),
            Some("reason"),
            "the newest failure is the one that stays"
        );
    }

    /// The invariant whose failure is invisible on screen: a placeholder sorts
    /// after every real identifier, so a bound that read it as the newest would
    /// have the client paging from a message Telegram has never heard of.
    #[test]
    fn a_pending_message_does_not_become_a_bound_to_page_from() {
        let mut view = ConversationView::new(42);
        view.window.replace(messages(5));

        let id = view.queue_send("on its way", None);

        assert_eq!(id, -1, "the placeholder sorts after every real identifier");
        assert_eq!(
            view.window
                .get(view.window.len() - 1)
                .map(|message| message.id),
            Some(-1),
            "and it really is the last row"
        );
        assert_eq!(
            view.window.newest_id(),
            Some(5),
            "but the newest numbered message is still the one the server sent"
        );
        assert_eq!(view.window.oldest_id(), Some(1));
    }

    /// A send the reader never confirms still cannot grow the window past its
    /// ceiling: the placeholder goes in the bottom and the oldest real message
    /// falls off, which is what a failing send costs.
    #[test]
    fn the_window_stays_capped_however_many_sends_are_left_unconfirmed() {
        let mut view = ConversationView::new(42);

        for _ in 0..500 {
            view.queue_send("x", None);
        }

        assert_eq!(view.window.len(), CONVERSATION_WINDOW);
    }
}
