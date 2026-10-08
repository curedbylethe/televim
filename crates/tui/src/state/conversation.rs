//! The open conversation's view, its editing and selection registers, and its search surfaces.

use std::ops::Range;

use domain::history::ConversationView;
use domain::message::{Message, MessageStatus};
use domain::search::SearchState;
use domain::selection::{Mark, Selection};
use domain::user::{UserCandidate, UserSearchState};
use domain::vim::{CharMotion, Motion, VimState, char_motion};

use crate::app::{ConfirmKind, Deletion, Register};
use crate::jumplist::Jumplist;
use crate::sticker::StickerCache;

/// What forwarding a selection would ask the server for.
///
/// `ids` is every numbered message the selection covers, oldest first, and
/// `skipped` is how many placeholders were left out of it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Forwarding {
    pub(crate) ids: Vec<i64>,
    pub(crate) skipped: usize,
}

/// A forward waiting for a destination, and the chat it is to land in.
///
/// The messages and their source are captured when the picker opens, so that
/// nothing the reader does to the selection afterwards changes what `Enter`
/// forwards. `selected` is the chat the picker's cursor is on, as an index into
/// the chat list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForwardPick {
    pub(crate) chat_id: i64,
    pub(crate) message_ids: Vec<i64>,
    pub(crate) skipped: usize,
    pub(crate) selected: usize,
}

pub struct ConversationState {
    /// The conversation on show, and where the reader is in it.
    ///
    /// The panel renders a slice of this, and the cursor below is the reader's
    /// place within it. Nothing here holds a whole conversation: the window is
    /// the ceiling on what the open chat costs.
    pub conversation: ConversationView,

    pub vim: VimState,

    /// Set by `/` search: the query text, the matches, and where the walk is.
    ///
    /// One value rather than a list beside a query: the label, the highlight and
    /// `n`/`N` all read the same state, and keeping them apart would let the
    /// three disagree about which list is on screen.
    pub(crate) search: SearchState,

    /// Set by a new-conversation search: the query, the people found, and where
    /// the reader is among them.
    ///
    /// Its own state rather than reusing [`App::search`], which is scoped to the
    /// open conversation: this one is about the chat list, and the two can be
    /// live at once without either overwriting the other.
    pub(crate) user_search: UserSearchState,

    /// What the reader last yanked.
    ///
    /// A yank is about *this* conversation and does not follow the reader into
    /// another one: carrying it across would be a feature nobody asked for and
    /// would need its own answer about whether it survives the change. So
    /// [`App::select_chat_none`] forgets it along with everything else.
    pub(crate) register: Register,

    /// What the reader has selected over the messages, if anything.
    ///
    /// `None` outside a selection — and a selection with no mode of its own: the
    /// conversation's [`Mode`] says whether a key is being applied to it, and a
    /// `dd` puts one here for as long as the prompt is up without ever asking
    /// for Visual.
    ///
    /// Both of its ends name a message by identifier, which is what lets it
    /// survive a page landing: see [`App::after_window_change`].
    pub(crate) selection: Option<Selection>,

    /// The forward being directed at a chat, if one is.
    ///
    /// Only meaningful while a selection is up: see [`Self::picking`]. Several
    /// places clear the selection directly, and a picker left behind by one of
    /// them is inert rather than a second thing to clear.
    pub(crate) forward: Option<ForwardPick>,

    /// The message the next composed message answers, if it is a reply.
    pub reply_to: Option<i64>,

    /// The message the buffer is editing, if it is an edit.
    pub editing: Option<i64>,

    /// The placeholder of the send in flight, if one is.
    ///
    /// An `Option` rather than a flag so that a result is matched to the send it
    /// answers: releasing the gate for a send that is no longer in flight is a
    /// no-op, and a duplicate result cannot release a later send's gate.
    pub sending: Option<i64>,

    /// The deletion waiting to be confirmed, if one is.
    pub confirm: Option<ConfirmKind>,

    /// Where the reader was before each jump they have taken.
    ///
    /// What `Ctrl-o` and `Ctrl-i` walk. It is per conversation and keyed by
    /// message identifier rather than by row, because a jump replaces the window
    /// and a row means a different message on either side of that.
    pub(crate) jumplist: Jumplist,

    /// The open conversation's decoded stickers, keyed by message identifier.
    ///
    /// Per conversation like the register: identifiers repeat across chats, so
    /// an entry belongs to the chat on show and [`Self::open_conversation`]
    /// forgets them all on switch.
    pub stickers: StickerCache,

    /// Whether the window on show came from the local cache rather than from the
    /// wire.
    ///
    /// A flag beside the view rather than a property of the messages, because
    /// what it records is where the window as a whole came from: a cached message
    /// and the one the server sends for it are the same message. It is set by
    /// [`Self::seed`] alone, and anything that replaces the window with a page
    /// the server sent clears it — the newest page above all, which is the
    /// revalidation it waits for.
    ///
    /// It never stands for an answer. Nothing that decides what to fetch reads
    /// it, so a cached window is asked about exactly as an empty one is, and a
    /// revalidation that fails is reported as any other failed page is.
    pub(crate) cached: bool,
}

impl ConversationState {
    /// An empty conversation: nothing open, nothing selected, nothing in flight.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            conversation: ConversationView::new(0),
            vim: VimState::new(0),
            search: SearchState::default(),
            user_search: UserSearchState::default(),
            register: Register::default(),
            selection: None,
            forward: None,
            reply_to: None,
            editing: None,
            sending: None,
            confirm: None,
            jumplist: Jumplist::default(),
            stickers: StickerCache::default(),
            cached: false,
        }
    }

    /// Opens the conversation `chat_id` names, forgetting the old window.
    ///
    /// The window is replaced rather than extended: it holds one conversation,
    /// and the one before it is gone.
    pub(crate) fn open_conversation(&mut self, chat_id: i64) {
        self.conversation = ConversationView::new(chat_id);
        self.cached = false;
        // The pictures belong to the chat on show, as the marks do: a reader
        // who returns finds them fetched again rather than mislabelled.
        self.stickers.clear();
    }

    /// Replaces the selection outright.
    ///
    /// The writer half of the selection reader, and the whole of what a motion
    /// needs: a motion moves one end of a selection and changes nothing else
    /// about it. It does not check that the marks are loaded, because a motion
    /// works from the window and the text in front of it and cannot name
    /// anything else.
    pub(crate) fn set_selection(&mut self, selection: Selection) {
        self.selection = Some(selection);
    }

    /// Drops the selection, whatever it was for.
    pub(crate) fn clear_selection(&mut self) {
        self.selection = None;
        self.forward = None;
    }

    /// The forward picker, while one is up over a selection.
    #[must_use]
    pub(crate) fn picking(&self) -> Option<&ForwardPick> {
        self.selection.as_ref().and(self.forward.as_ref())
    }

    /// Opens the picker over the selection, with what is to be forwarded
    /// captured now.
    pub(crate) fn open_forward(&mut self, forwarding: Forwarding) {
        self.forward = Some(ForwardPick {
            chat_id: self.conversation.window.chat_id,
            message_ids: forwarding.ids,
            skipped: forwarding.skipped,
            selected: 0,
        });
    }

    /// Puts the picker away and leaves the selection as it was.
    pub(crate) fn dismiss_forward(&mut self) {
        self.forward = None;
    }

    /// Starts a selection at `message_id`, and reports whether it could be.
    ///
    /// A mark can only be placed on a message the window holds, so a selection
    /// over one that is not loaded is refused rather than recorded: `d` on it
    /// would name an identifier nothing can act on, and a mark the panel cannot
    /// draw is a mark the reader cannot see.
    ///
    /// The mode is not touched. A selection is not a mode — `dd` leaves one here
    /// while the reader is in Normal, answering a question, and the conversation's
    /// mode is about what a key means rather than about what is selected.
    #[must_use]
    pub(crate) fn select(&mut self, message_id: i64, char: Option<usize>) -> bool {
        if self.conversation.window.position_of(message_id).is_none() {
            return false;
        }

        self.set_selection(Selection::at(message_id, char));
        true
    }

    /// Holds what was last yanked for the caller to write.
    pub(crate) fn set_register(&mut self, register: Register) {
        self.register = register;
    }

    /// Raises or lowers the deletion waiting to be confirmed.
    pub(crate) fn set_confirm(&mut self, confirm: Option<ConfirmKind>) {
        self.confirm = confirm;
    }

    /// Names the message the next composed message answers.
    pub(crate) fn set_reply_to(&mut self, reply_to: Option<i64>) {
        self.reply_to = reply_to;
    }

    /// Names the message the buffer is editing.
    pub(crate) fn set_editing(&mut self, editing: Option<i64>) {
        self.editing = editing;
    }

    /// The message the cursor is on, if the window holds anything.
    pub(crate) fn cursor_message(&self) -> Option<&Message> {
        self.conversation.window.get(self.vim.cursor())
    }

    /// Identifier of the message the cursor is on, if the window holds anything.
    pub(crate) fn cursor_message_id(&self) -> Option<i64> {
        self.cursor_message().map(|message| message.id)
    }

    /// Whether `page` holds anything for the conversation on show.
    ///
    /// A page is fetched for one conversation and the reader can open another
    /// while one is in flight, so a page that arrives late is recognised here
    /// rather than allowed to replace what is on screen.
    pub(crate) fn page_belongs_to_open_chat(&self, page: &[Message]) -> bool {
        let chat_id = self.conversation.window.chat_id;
        page.iter().any(|message| message.chat_id == chat_id)
    }

    /// Replaces the window with the newest page of the open conversation.
    ///
    /// The reader is put at the newest message: opening a conversation and
    /// loading the page that ends one both mean "show me the end".
    ///
    /// A selection does not survive, because the messages it named are not
    /// necessarily the ones on show now. There is nothing to restore it *to*:
    /// unlike a page that extends the window, a page that replaces it has shifted
    /// every message in it.
    ///
    /// Reports whether anything was shown.
    pub(crate) fn apply_latest(&mut self, page: Vec<Message>) -> bool {
        if !self.page_belongs_to_open_chat(&page) {
            return false;
        }

        self.conversation.window.replace(page);
        // The server's newest page is the answer a cached window was waiting
        // for, and it has replaced every row the cache put there.
        self.cached = false;
        self.selection = None;
        self.vim.set_total(self.conversation.window.len());
        self.conversation.follow();
        self.vim.apply_motion(Motion::Last);

        true
    }

    /// Fills a conversation that has just been opened with what the cache
    /// holds for it, oldest first.
    ///
    /// Accepted only for the conversation on show, and only while its window is
    /// empty: a window with anything in it already has something better than the
    /// cache — the server's newest page, or an arrival the feed delivered — and a
    /// cached page laid over either would show the reader older facts than the
    /// ones they had.
    ///
    /// Laid out exactly as the newest page is, through [`Self::apply_latest`]:
    /// the cache holds the end of the conversation, so the reader is put at its
    /// newest message and the view follows. The page the window is waiting for is
    /// not touched — it is still wanted, still in flight if it was, and it
    /// replaces these rows when it lands.
    ///
    /// Reports whether anything was shown.
    pub(crate) fn seed(&mut self, chat_id: i64, messages: Vec<Message>) -> bool {
        if !self.has_conversation()
            || self.conversation.window.chat_id != chat_id
            || !self.conversation.window.is_empty()
        {
            return false;
        }

        let seeded = self.apply_latest(messages);
        self.cached = seeded;
        seeded
    }

    /// Puts a page in front of what the window holds.
    ///
    /// Reports whether anything was added, and leaves the reader on the message
    /// they were reading: the window moved under them, not the other way round.
    pub(crate) fn apply_older(&mut self, page: Vec<Message>) -> bool {
        if !self.page_belongs_to_open_chat(&page) {
            return false;
        }

        let anchor = self.cursor_message_id();
        if !self.conversation.window.push_front(page) {
            return false;
        }

        self.after_window_change(anchor);

        true
    }

    /// Puts messages behind what the window holds: a fetched page, or one the
    /// reader has just typed.
    ///
    /// Reports whether anything was added.
    pub(crate) fn apply_newer(&mut self, page: Vec<Message>) -> bool {
        if !self.page_belongs_to_open_chat(&page) {
            return false;
        }

        let anchor = self.cursor_message_id();
        if !self.conversation.window.push_back(page) {
            return false;
        }

        self.after_window_change(anchor);

        true
    }

    /// Puts the reader back where they were, now that the window has moved.
    ///
    /// There are now **two** anchors to restore rather than one, and both are
    /// restored the same way: by message identifier, because an index means a
    /// different message on either side of a page. Restoring only the cursor is
    /// the failure this arrangement exists to prevent — an older page landing
    /// under a live selection shifts every index, so a selection left in index
    /// terms would silently come to cover different messages and the next `d`
    /// would delete something the reader did not select.
    pub(crate) fn after_window_change(&mut self, anchor: Option<i64>) {
        self.vim.set_total(self.conversation.window.len());

        let cursor = self.vim.cursor();
        let restored = anchor
            .and_then(|id| self.conversation.window.position_of(id))
            .unwrap_or(cursor);
        self.vim.set_cursor(restored);

        self.retain_selection();

        if self.conversation.auto_follow() {
            // A view pinned to the end stays pinned: what arrived is what the
            // reader asked to see.
            self.vim.apply_motion(Motion::Last);
        } else {
            self.settle_follow();
        }
    }

    /// Drops the selection unless both of its ends still name a message the
    /// window holds.
    ///
    /// Whole or not at all. A mark whose message has been paged out or pushed past
    /// the window's cap cannot be put back anywhere, and narrowing the selection
    /// to the end that survived would be worse than losing it: `d` on half a
    /// selection is a one-message deletion the reader never asked for, and it
    /// would be asked for by the same key they used last time.
    pub(crate) fn retain_selection(&mut self) {
        let Some(selection) = self.selection.take() else {
            return;
        };

        let window = &self.conversation.window;
        let held = window.position_of(selection.anchor.message_id).is_some()
            && window.position_of(selection.focus.message_id).is_some();

        if held {
            self.selection = Some(selection);
        }
    }

    /// Keeps the follow state in step with where the cursor ended up.
    ///
    /// The two are one fact seen twice: a view pinned to the newest message is
    /// one whose cursor is on it. Anywhere else means the reader has moved away,
    /// and an arrival no longer has the right to move them.
    pub(crate) fn settle_follow(&mut self) {
        let last = self.conversation.window.len().saturating_sub(1);

        if self.conversation.window.is_empty() || self.vim.cursor() >= last {
            self.conversation.follow();
        } else {
            self.conversation.unfollow();
        }
    }

    /// Records that a send for `temp_id` is in flight.
    ///
    /// The identifier rather than a flag, so releasing the gate can be matched
    /// to the send it answers.
    pub(crate) fn begin_send(&mut self, temp_id: i64) {
        self.sending = Some(temp_id);
    }

    /// Releases the in-flight gate, if it is still held for `temp_id`.
    ///
    /// A no-op for a send that has already been released, so a duplicate result
    /// cannot clear the gate of a later one.
    pub(crate) fn end_send(&mut self, temp_id: i64) {
        if self.sending == Some(temp_id) {
            self.sending = None;
        }
    }

    /// Replaces a send's placeholder with the message the server accepted.
    ///
    /// Reports whether anything changed, so the caller knows whether a redraw is
    /// owed.
    pub(crate) fn confirm_sent(&mut self, temp_id: i64, real: Message) -> bool {
        let anchor = self.cursor_message_id();
        let changed = self.conversation.confirm_sent(temp_id, real);
        if changed {
            self.after_window_change(anchor);
        }
        changed
    }

    /// Marks a send as failed, keeping the message and recording why.
    ///
    /// Reports whether the placeholder was there to mark.
    pub(crate) fn fail_send(&mut self, temp_id: i64, reason: String) -> bool {
        self.conversation.fail_send(temp_id, reason)
    }

    /// Removes a failed message and the reason recorded for it.
    ///
    /// Reports whether either was there.
    pub(crate) fn dismiss_failed(&mut self, temp_id: i64) -> bool {
        let anchor = self.cursor_message_id();
        let changed = self.conversation.dismiss_failed(temp_id);
        if changed {
            self.after_window_change(anchor);
        }
        changed
    }

    /// Dismisses the failed message under the cursor, if that is what it is.
    pub(crate) fn dismiss_failed_at_cursor(&mut self) {
        let Some((id, status)) = self
            .cursor_message()
            .map(|message| (message.id, message.status))
        else {
            return;
        };

        if !matches!(status, MessageStatus::Failed) {
            return;
        }

        let anchor = self.cursor_message_id();
        if self.conversation.dismiss_failed(id) {
            self.after_window_change(anchor);
        }
    }

    /// Applies a character motion to the focus's position within its message.
    ///
    /// Nothing happens without a character position to move: a linewise selection
    /// is of a whole message and there is no place inside it to move to, which is
    /// also what Vim does. The cursor does not follow — it stands on the anchor's
    /// message until the focus moves to another one, so that a charwise selection
    /// does not drag the viewport along with every character.
    pub(crate) fn move_focus(&mut self, motion: CharMotion) {
        let Some(selection) = &mut self.selection else {
            return;
        };
        let Some(at) = selection.focus.char else {
            return;
        };
        let id = selection.focus.message_id;

        let Some(message) = self.conversation.window.iter().find(|m| m.id == id) else {
            return;
        };

        selection.focus.char = Some(char_motion(message.display_body(), at, motion));
    }

    /// Moves the focus to the next or the previous message, and the cursor with it.
    ///
    /// The cursor follows because this is the only motion that leaves the message:
    /// a reader stepping through messages with `j` is reading, not selecting
    /// characters, and a cursor left behind would be off the selection entirely.
    ///
    /// A character position carries over where it still fits, so a selection that
    /// has already been moved within a message keeps its relative place in the
    /// next one. It stops mattering as soon as the two ends are in different
    /// messages — which is exactly what they now are.
    pub(crate) fn move_focus_to_message(&mut self, forward: bool) {
        let Some(focus) = self.selection.map(|selection| selection.focus) else {
            return;
        };
        let Some(index) = self.conversation.window.position_of(focus.message_id) else {
            return;
        };
        let next = if forward {
            index + 1
        } else {
            index.saturating_sub(1)
        };
        let Some(message) = self.conversation.window.get(next) else {
            return;
        };

        let id = message.id;
        let last = message.display_body().chars().count().saturating_sub(1);
        let char = focus.char.map(|at| at.min(last));

        if let Some(selection) = &mut self.selection {
            selection.focus = Mark {
                message_id: id,
                char,
            };
        }
        self.vim.set_cursor(next);
        self.settle_follow();
    }

    /// The query a search should run, resolving an empty one to the last search.
    ///
    /// `None` when there is nothing to repeat, which is the one case `/` cannot
    /// answer.
    pub(crate) fn search_to_run(&self, query: &str) -> Option<String> {
        let query = query.trim();
        if !query.is_empty() {
            return Some(query.to_owned());
        }

        self.search.query().map(str::to_owned)
    }

    /// Lands the reader on the match the walk has just moved to.
    ///
    /// The local pass's matches are all in the window, so this is synchronous.
    pub(crate) fn land_on_match(&mut self) {
        if let Some(id) = self.search.next()
            && let Some(position) = self.conversation.window.position_of(id)
        {
            self.vim.set_cursor(position);
        }

        self.settle_follow();
    }

    /// Replaces the local matches with the server's answer, if it is still
    /// wanted.
    ///
    /// Returns whether the answer landed. It is refused for a conversation that
    /// is no longer open and for a query the reader has replaced — the same
    /// discipline a send's result gets, applied to the other half of the
    /// answer's identity.
    pub(crate) fn apply_searched(
        &mut self,
        chat_id: i64,
        query: &str,
        ids: Vec<i64>,
        total: usize,
    ) -> bool {
        if self.conversation.window.chat_id != chat_id {
            return false;
        }

        let cursor_id = self.cursor_message_id();
        if !self.search.adopt_server(query, ids, total, cursor_id) {
            return false;
        }

        // The cursor was on a local match the server may not have confirmed.
        // Landing it on the nearest surviving match keeps its sense of place;
        // the next `n` then moves forward from there rather than restarting.
        if let Some(target) = self.search.landing(cursor_id)
            && let Some(position) = self.conversation.window.position_of(target)
        {
            self.vim.set_cursor(position);
        }

        self.settle_follow();
        true
    }

    /// Records that the server pass for `query` failed, keeping the local list.
    pub(crate) fn search_failed(&mut self, query: &str, reason: String) {
        if self.search.query() != Some(query) {
            return;
        }

        self.search.fail(reason);
    }

    /// Fills the new-conversation list with the answer to `query`, if it is still
    /// wanted.
    ///
    /// The stale-query refusal is the state's own, so a result for a query the
    /// reader has replaced is dropped without the caller having to check.
    /// Returns whether the answer landed.
    pub(crate) fn apply_users(&mut self, query: &str, candidates: Vec<UserCandidate>) -> bool {
        self.user_search.adopt(query, candidates)
    }

    /// Records that the lookup for `query` failed.
    ///
    /// Refused for a query the reader has replaced, for the same reason a result
    /// is: a late failure must not close a list that belongs to a newer search.
    /// Returns whether it landed.
    pub(crate) fn fail_users(&mut self, query: &str, reason: String) -> bool {
        if self.user_search.query() != Some(query) {
            return false;
        }

        self.user_search.fail(reason);
        true
    }

    /// Moves the user-search highlight one candidate, wrapping.
    pub(crate) fn move_user_selection(&mut self, forward: bool) {
        self.user_search
            .move_selection(if forward { 1 } else { -1 });
    }

    /// Puts the new-conversation overlay away and forgets the search.
    ///
    /// Forgetting rather than hiding: the query and its results belong to one
    /// question, and a list left behind would reappear under the next search
    /// before that search had asked anything.
    pub(crate) fn dismiss_user_search(&mut self) {
        self.user_search.clear();
    }

    /// Whether a conversation is open to put messages in.
    ///
    /// Telegram numbers peers from one, so a zero here is the absence of a
    /// conversation rather than a conversation with an odd identifier.
    #[must_use]
    pub(crate) fn has_conversation(&self) -> bool {
        self.conversation.window.chat_id != 0
    }

    /// Where a jump to `target` lands when the window does not hold it.
    ///
    /// The nearest message at or past the target, or the last one there is:
    /// a page centred on the target starts from the closest message to it.
    pub(crate) fn landing_index(&self, target: i64) -> usize {
        let window = &self.conversation.window;

        window
            .position_of(target)
            .or_else(|| window.iter().position(|message| message.id >= target))
            .unwrap_or_else(|| window.len().saturating_sub(1))
    }

    /// The window positions the selection covers, oldest first.
    ///
    /// **Positions**, and not the span between the two identifiers, because the
    /// numbers do not say what covers what: a placeholder for a send in flight is
    /// numbered below zero and sits at the *end* of the window, where the
    /// conversation has reached. A selection reaching one spans a different set of
    /// messages by identifier than by position, and acting on the wrong one is a
    /// deletion of messages the reader did not select.
    ///
    /// Empty when there is no selection, and when one of its ends is not in the
    /// window — which [`Self::retain_selection`] makes unreachable and which is
    /// answered as "nothing" rather than as a panic.
    ///
    /// The one answer, for the panel to mark with, the operations to act on, and
    /// the count to come from. Two answers would be two things to disagree.
    #[must_use]
    pub(crate) fn covered(&self, selection: Option<&Selection>) -> Range<usize> {
        let Some(selection) = selection else {
            return 0..0;
        };
        let window = &self.conversation.window;

        match (
            window.position_of(selection.anchor.message_id),
            window.position_of(selection.focus.message_id),
        ) {
            (Some(anchor), Some(focus)) => anchor.min(focus)..anchor.max(focus) + 1,
            _ => 0..0,
        }
    }

    /// The lines a selection yanks: one for a text selection, one per message for
    /// anything else.
    ///
    /// All of them possibly empty — a collapsed charwise selection yields one
    /// empty string, and a set of messages that happen to be blank yields several
    /// — which the caller notices. A selection naming a message the window no
    /// longer holds yields nothing, which [`Self::retain_selection`] makes
    /// unreachable and which is answered with an empty yank rather than a panic.
    pub(crate) fn yanked(&self, selection: &Selection) -> Vec<String> {
        if let Some((id, range)) = selection.text_range() {
            let Some(message) = self.conversation.window.iter().find(|m| m.id == id) else {
                return Vec::new();
            };

            let body = message.display_body();
            return vec![body[crate::rows::byte_span(body, range)].to_owned()];
        }

        let covered = self.covered(Some(selection));
        self.conversation
            .window
            .iter()
            .skip(covered.start)
            .take(covered.len())
            .map(|message| message.display_body().to_owned())
            .collect()
    }

    /// What deleting `selection` would ask the server for, or `None` when every
    /// message in it is a placeholder.
    ///
    /// A placeholder is a local stand-in for a send the server has not
    /// acknowledged, so it has no identifier the server knows: naming one would
    /// have the whole request refused and take the real messages down with it.
    /// They are left out of `ids` and counted, and a selection of nothing but
    /// placeholders has nothing left to ask for.
    pub(crate) fn deletion(&self, selection: &Selection) -> Option<Deletion> {
        let mut deletion = Deletion::default();
        let covered = self.covered(Some(selection));

        for message in self
            .conversation
            .window
            .iter()
            .skip(covered.start)
            .take(covered.len())
        {
            if message.id <= 0 {
                deletion.skipped += 1;
                continue;
            }

            deletion.ids.push(message.id);
            deletion.outgoing += usize::from(message.is_outgoing);
        }

        (!deletion.ids.is_empty()).then_some(deletion)
    }

    /// What forwarding `selection` would ask the server for, or `None` when every
    /// message in it is a placeholder.
    ///
    /// The rules of [`Self::deletion`], for the same reason: a placeholder has no
    /// identifier the server knows, so it is left out of `ids` and counted.
    /// Forwarding is message-granular, so a charwise selection forwards the whole
    /// message it sits in.
    pub(crate) fn forwardable(&self, selection: &Selection) -> Option<Forwarding> {
        let mut forwarding = Forwarding::default();
        let covered = self.covered(Some(selection));

        for message in self
            .conversation
            .window
            .iter()
            .skip(covered.start)
            .take(covered.len())
        {
            if message.id <= 0 {
                forwarding.skipped += 1;
                continue;
            }

            forwarding.ids.push(message.id);
        }

        (!forwarding.ids.is_empty()).then_some(forwarding)
    }

    /// The refusal for a forward of nothing but placeholders.
    ///
    /// The same split as [`Self::refuse_placeholders`] — a message still on its
    /// way is not one that failed — with the words that are true of forwarding:
    /// `D` is about dismissing and says nothing about forwarding.
    pub(crate) fn refuse_forward(&self, selection: &Selection) -> &'static str {
        let covered = self.covered(Some(selection));
        let mut count = 0;
        let mut in_flight = false;

        for message in self
            .conversation
            .window
            .iter()
            .skip(covered.start)
            .take(covered.len())
        {
            count += 1;
            in_flight |= !matches!(message.status, MessageStatus::Failed);
        }

        match (count == 1, in_flight) {
            (true, true) => "that message is still on its way, so it cannot be forwarded",
            (true, false) => "that message never left, so it cannot be forwarded",
            (false, true) => "those messages are still on their way, so they cannot be forwarded",
            (false, false) => "those messages never left, so they cannot be forwarded",
        }
    }

    /// The refusal for a selection of nothing but placeholders.
    ///
    /// The two sentences that already existed, kept: a failed message has a `D` to
    /// offer and one still on its way does not, and pointing at `D` for a message
    /// that has not left would be wrong. A selection of several gets the same
    /// distinction in the only words that are true of all of them — `D` dismisses
    /// one message at a time, and there is no bulk dismiss.
    pub(crate) fn refuse_placeholders(&self, selection: &Selection) -> &'static str {
        let covered = self.covered(Some(selection));
        let mut count = 0;
        let mut in_flight = false;

        for message in self
            .conversation
            .window
            .iter()
            .skip(covered.start)
            .take(covered.len())
        {
            count += 1;
            in_flight |= !matches!(message.status, MessageStatus::Failed);
        }

        let one = count == 1;

        match (one, in_flight) {
            (true, true) => "that message is still on its way",
            (true, false) => "that message never left — D dismisses it",
            (false, true) => "those messages are still on their way",
            (false, false) => "those messages never left — D dismisses one at a time",
        }
    }
}
