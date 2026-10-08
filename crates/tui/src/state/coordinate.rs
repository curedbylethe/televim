//! Cross-cutting coordination: the mutations no single state type owns.
//!
//! Every other module in [`crate::state`] holds one piece of
//! [`crate::app::App`] and the mutations that touch only that piece. The
//! methods that mutate two or more pieces at once cannot live on any one of
//! them, so they move here as free functions taking exactly the substructs
//! they mutate — never `&mut App`.
//!
//! A seam function takes `&mut` on exactly the substructs its body writes,
//! and `&` on the ones it only reads. Values the body needs from
//! [`crate::app::App`] itself — rendered card rows, the frame's layout, a
//! yanked line — are computed by the [`crate::app::App`] delegate first and
//! passed in by value, because [`crate::card::rows`] and the widgets render
//! from `&App` and cannot move here.
//!
//! [`crate::app::App`] keeps a thin delegate per function so dispatch shells,
//! readers, panels, and tests call the same names as before.

// Seam functions take one parameter per substruct they touch, which is always
// more than seven. That width is the design, not an accident, so the lint is
// off for this module rather than allowed thirty times over.
#![allow(clippy::too_many_arguments)]

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use domain::chat::Chat;
use domain::history::{CONVERSATION_WINDOW, ConversationView, ConversationWindow, unread_target};
use domain::message::Message;
use domain::search::word_prefix_match;
use domain::selection::Selection;
use domain::updates::UpdateEvent;
use domain::user::UserCandidate;
use domain::vim::{CharMotion, Motion, VimState};

use crate::app::{
    ACTION_QUEUE, ADD_ACCOUNT_REFUSAL, AccountState, Action, ChatChoice, ConfirmKind,
    ContactProfile, Find, Focus, JUMP_UNAVAILABLE, Jump, JumpKind, LoginField, Mode, NOT_A_REPLY,
    NOT_YOURS_REFUSAL, Pane, ProfileId, PromptKind, Register, SignIn, SignInFlow, TYPING_FOR,
};
use crate::card::CardRow;
use crate::jumplist::Jumplist;
use crate::line::LineVerdict;
use crate::rows::{self, RowSpan};
use crate::state::ui::IDLE_STATUS;

use super::chat_list::ChatListState;
use super::conversation::ConversationState;
use super::drafts::DraftStore;
use super::input::InputState;
use super::outbox::Outbox;
use super::pending::Pending;
use super::profile::ProfileCard;
use super::session::SessionState;
use super::ui::UiState;

/// Installs a freshly fetched chat list.
///
/// The window the messages were seen in goes with it: the list is replaced
/// wholesale, and a message kept from the one before would be matched
/// against conversations that are no longer on screen.
///
/// The selection is clamped rather than reset, because the reader's place
/// is a position in a list that may have become shorter. An empty list
/// leaves nothing selected, which is what closes the conversation: there is
/// no chat for the window to belong to.
pub(crate) fn set_chats(
    ui: &mut UiState,
    list: &mut ChatListState,
    outbox: &mut Outbox,
    pending: &mut Pending,
    conversation: &mut ConversationState,
    input: &mut InputState,
    drafts: &mut DraftStore,
    chats: Vec<Chat>,
) {
    list.install(chats);

    if list.list.chats.is_empty() {
        select_chat_none(
            &mut *ui,
            &mut *outbox,
            &mut *pending,
            &mut *conversation,
            &mut *input,
            &mut *drafts,
        );
        // The account's drafts go with its list: a peer id can be reused by
        // another account, and inheriting a stranger's words is worse than
        // losing one's own.
        drafts.drafts.clear();
    }
}

/// Installs a freshly fetched chat list while keeping the open conversation.
///
/// The other half of [`App::set_chats`], and the one a client that has been
/// brought back up needs: the list is replaced wholesale, but the reader's
/// place in it is not. The conversation, its window, the cursor in it, the
/// jumplist, the selection, the register and the draft are left exactly as
/// they were, because a re-fetch is not the reader changing conversations —
/// so none of [`select_chat_none`]'s resets run here.
///
/// The highlight is restored by the open conversation's own id, never by
/// index: the list comes back ordered by recency, and an index means a
/// different conversation on either side of the fetch. An id the new list
/// does not hold falls back to the top and reports `false`, so the caller
/// can say where the reader landed.
pub(crate) fn refresh_chats(
    list: &mut ChatListState,
    conversation: &mut ConversationState,
    chats: Vec<Chat>,
) -> bool {
    let open = conversation.conversation.window.chat_id;
    list.reinstall(chats, open)
}

/// Moves the highlight to `index` in the chat list, and asks for the
/// conversation it names to be taken to.
///
/// The highlight moves at once and the open is recorded rather than made,
/// because a reader who holds `j` would otherwise have every conversation
/// they passed fetched. What the reader sees follows their key; what the
/// network is asked for waits for them to stop.
///
/// An index outside the list moves nothing.
pub(crate) fn choose_chat(list: &mut ChatListState, pending: &mut Pending, index: usize) {
    if list.list.chats.get(index).is_none() {
        return;
    }

    list.select(index);
    pending.set_pending_chat(Some(ChatChoice {
        index,
        at: Instant::now(),
    }));
}

/// Opens the conversation at `index` in the chat list.
///
/// The window is replaced rather than extended: it holds one conversation,
/// and the one before it is gone. Whatever was in flight for the old one is
/// forgotten too — a page that arrives late belongs to a conversation that
/// is no longer open, and the window refuses it.
///
/// An index outside the list leaves the screen as it was.
pub(crate) fn select_chat(
    ui: &mut UiState,
    list: &mut ChatListState,
    outbox: &mut Outbox,
    pending: &mut Pending,
    conversation: &mut ConversationState,
    input: &mut InputState,
    drafts: &mut DraftStore,
    index: usize,
) {
    let Some(chat) = list.list.chats.get(index) else {
        return;
    };
    let chat_id = chat.id;

    list.select(index);
    pending.set_pending_chat(None);
    select_chat_none(
        &mut *ui,
        &mut *outbox,
        &mut *pending,
        &mut *conversation,
        &mut *input,
        &mut *drafts,
    );
    // The new view starts its placeholder ids at the bottom again, so an
    // identifier the old view handed out can be handed out once more. That is
    // safe only because a result for the old view cannot reach this one —
    // whatever else changes here, that has to stay true. The counter is not
    // carried across on purpose; `net`'s drop test is the executable form of
    // this sentence.
    conversation.open_conversation(chat_id);
    // The window is gone and with it the watermark the new view starts
    // without. How far this conversation has been read is not a fact about
    // the page on show, so it is put back from what the feed has said.
    restore_read_watermark(&mut *drafts, &mut *conversation, chat_id);
    // And with it the draft the reader left in this conversation, for the
    // same reason: the words are not a fact about the page on show.
    resume_draft(&mut *drafts, &mut *input, chat_id);
}

/// Opens the conversation whose chat id is `id`.
///
/// The same lookup `:chat` does: the position of the id in the list, and
/// nothing is opened when it is not there. Reports whether the id was
/// found, so a caller that must land somewhere can say where it landed
/// instead of staying silent the way `:chat` does.
pub(crate) fn select_chat_by_id(
    ui: &mut UiState,
    list: &mut ChatListState,
    outbox: &mut Outbox,
    pending: &mut Pending,
    conversation: &mut ConversationState,
    input: &mut InputState,
    drafts: &mut DraftStore,
    id: i64,
) -> bool {
    let Some(pos) = list.list.chats.iter().position(|c| c.id == id) else {
        return false;
    };
    select_chat(
        &mut *ui,
        &mut *list,
        &mut *outbox,
        &mut *pending,
        &mut *conversation,
        &mut *input,
        &mut *drafts,
        pos,
    );
    true
}

/// Puts the conversation's recorded read watermark on the view just opened.
///
/// Reports whether there was one to put back, which is what a reader switching
/// to a conversation nobody has read yet gets.
pub(crate) fn restore_read_watermark(
    drafts: &mut DraftStore,
    conversation: &mut ConversationState,
    chat_id: i64,
) -> bool {
    let recorded = drafts.read_receipts.borrow().get(&chat_id).copied();
    match recorded {
        Some(max_id) => conversation.conversation.set_read_watermark(max_id),
        None => false,
    }
}

/// Puts a conversation's parked draft back on the line.
///
/// The mirror of [`park_draft`]: the value is moved out of the map, so
/// the map never holds the open conversation's draft, and a peer with no
/// entry gets a fresh line.
pub(crate) fn resume_draft(drafts: &mut DraftStore, input: &mut InputState, chat_id: i64) {
    let draft = drafts.take_draft(chat_id);
    input.set_line(draft);
}

/// Parks the open conversation's draft under its peer id.
///
/// Called while [`ConversationState`] still names the peer the words belong
/// to, before [`select_chat_none`] zeroes it. The draft is moved out
/// rather than copied, and its purpose is forgotten on the way into the map,
/// so the stored copy is always a plain message: the reply or edit subject
/// names a message in the conversation being left. An empty buffer drops the
/// peer's entry rather than storing a blank, so the map holds only peers
/// with a live draft.
///
/// Only a buffer draft is parked. A sign-in field or a `:`/`/` prompt is a
/// question mid-answer, not words the reader is writing, and the caller's own
/// reset is what finishes with those.
pub(crate) fn park_draft(
    conversation: &mut ConversationState,
    input: &mut InputState,
    drafts: &mut DraftStore,
) {
    let chat_id = conversation.conversation.window.chat_id;
    if chat_id == 0 || !input.line.purpose().is_buffer() {
        return;
    }

    if input.line.is_empty() {
        drafts.drafts.remove(&chat_id);
        return;
    }

    let mut draft = std::mem::take(&mut input.line);
    draft.forget_purpose();
    drafts.drafts.insert(chat_id, draft);
}

/// Closes the conversation on show.
///
/// The outgoing conversation's draft is parked under its peer id first, so a
/// reader who switches chats mid-sentence finds the sentence again on their
/// return — [`App::select_chat`] restores it with [`resume_draft`]. The
/// other acts of this function are resets. What is *not* kept is the draft's
/// subject: the reply it answers and the message it edits are dropped here,
/// because they name something in the conversation that has just closed, and
/// a reply sent into a different chat to a message that is not in it is not a
/// reply at all. The words survive per conversation; what they were for does
/// not, and the draft becomes a message when it comes back.
pub(crate) fn select_chat_none(
    ui: &mut UiState,
    outbox: &mut Outbox,
    pending: &mut Pending,
    conversation: &mut ConversationState,
    input: &mut InputState,
    drafts: &mut DraftStore,
) {
    // Before the conversation below is zeroed: parking needs the peer id the
    // page on show still names.
    park_draft(&mut *conversation, &mut *input, &mut *drafts);
    conversation.conversation = ConversationView::new(0);
    // The note belongs to the chat being left: returning must not revive it,
    // so the deadline goes with the view rather than with the reader's memory.
    ui.set_typing(None);
    conversation.vim = VimState::new(0);
    outbox.fetching.clear();
    pending.set_jump(None);
    // The marks are per conversation, and this is the path every switch goes
    // through, so this is where they go too: a reader who has closed the
    // conversation has nowhere to walk back to.
    conversation.jumplist = Jumplist::default();
    conversation.search.clear();
    conversation.reply_to = None;
    conversation.editing = None;
    conversation.confirm = None;
    conversation.selection = None;
    conversation.register = Register::default();
    input.line.forget_purpose();
}

/// Adds an operation to the queue the caller drains.
///
/// The queue is bounded so that a burst cannot grow without limit. It is
/// drained every pass, so reaching the bound means [`ACTION_QUEUE`]
/// operations were queued between two ticks; the oldest is refused to make
/// room, and the refusal is said out loud rather than that operation
/// vanishing.
pub(crate) fn queue_action(outbox: &mut Outbox, ui: &mut UiState, action: Action) {
    if outbox.actions.len() >= ACTION_QUEUE {
        outbox.actions.pop_front();
        ui.flash("too many requests at once — the oldest was dropped");
    }
    outbox.actions.push_back(action);
}

/// Takes the reader back to where they were before the last jump.
///
/// `Ctrl-o`. A mark the window still holds is a cursor move; one it does not
/// is a jump of its own, on the same terms as any other, because the jump
/// that took the reader away replaced the window the mark was in.
pub(crate) fn jump_back(pending: &mut Pending, conversation: &mut ConversationState) {
    // Asked before the stack is walked: a walk moves a mark between the two
    // stacks, and a reader who presses `Ctrl-o` while a page is on its way
    // must not have moved one for a return that did not happen.
    if pending.pending_jump.is_some() || !conversation.has_conversation() {
        return;
    }

    let Some(from) = conversation.cursor_message_id() else {
        return;
    };
    let Some(target) = conversation
        .jumplist
        .back(conversation.conversation.window.chat_id, from)
    else {
        return;
    };

    go_to(&mut *pending, &mut *conversation, target, JumpKind::Back);
}

/// Takes the reader forward to the place a `Ctrl-o` walked them away from.
///
/// `Ctrl-i`, and nothing else: a terminal that cannot report it apart from
/// `Tab` sends `Tab` instead, and `Tab` is the pane switch here — so on such
/// a terminal only `Ctrl-o` works, which is what the design accepted.
pub(crate) fn jump_forward(pending: &mut Pending, conversation: &mut ConversationState) {
    if pending.pending_jump.is_some() || !conversation.has_conversation() {
        return;
    }

    let Some(from) = conversation.cursor_message_id() else {
        return;
    };
    let Some(target) = conversation
        .jumplist
        .forward(conversation.conversation.window.chat_id, from)
    else {
        return;
    };

    go_to(&mut *pending, &mut *conversation, target, JumpKind::Forward);
}

/// Goes to `id`, moving the cursor when the window holds it and asking for a
/// page centred on it when it does not.
///
/// One answer for a return in either direction, because the two differ only
/// in which stack was walked — and in what the status line says while the
/// page is on its way, which is `kind`'s whole job.
pub(crate) fn go_to(
    pending: &mut Pending,
    conversation: &mut ConversationState,
    id: i64,
    kind: JumpKind,
) {
    if pending.pending_jump.is_some() {
        return;
    }

    if let Some(index) = conversation.conversation.window.position_of(id) {
        conversation.vim.set_cursor(index);
        conversation.settle_follow();
        return;
    }

    pending.set_jump(Some(Jump {
        peer_id: conversation.conversation.window.chat_id,
        target_id: id,
        kind,
    }));
    conversation.settle_follow();
}

/// Walks the search's matches in the direction given.
///
/// A match that is loaded is a cursor move; one that is not is a [`Jump`],
/// which is the same path `gg` takes. Wrapping announces itself, because a
/// walk that looped silently reads as a stuck key.
pub(crate) fn walk_search(
    pending: &mut Pending,
    conversation: &mut ConversationState,
    ui: &mut UiState,
    forward: bool,
) {
    if !conversation.search.is_active() {
        ui.flash("no previous search");
        return;
    }
    if conversation.search.is_empty() {
        ui.flash("nothing matched");
        return;
    }

    conversation.search.clear_notice();
    let before = conversation.search.index();
    let Some(id) = (if forward {
        conversation.search.next()
    } else {
        conversation.search.prev()
    }) else {
        return;
    };

    if wrapped(
        before,
        conversation.search.index(),
        conversation.search.len(),
    ) {
        conversation.search.note_wrap(forward);
    }

    if let Some(position) = conversation.conversation.window.position_of(id) {
        conversation.vim.set_cursor(position);
    } else {
        pending.set_jump(Some(Jump {
            peer_id: conversation.conversation.window.chat_id,
            target_id: id,
            kind: JumpKind::Unread,
        }));
    }

    conversation.settle_follow();
}

/// Replaces the window with a page fetched around a message the reader asked
/// to be taken to, and puts them on it.
///
/// Reports whether the window took the page. A page nobody is waiting for any
/// more is refused — the reader has opened another conversation, or told the
/// client to take them to the end instead — and the jump is over either way,
/// so that a fetch which failed or came back empty cannot leave the key
/// wedged.
///
/// The cursor lands on the target, or on the first message after it when the
/// page does not hold it: the page is centred on the target, so that is the
/// nearest the fetch came to where the reader was going.
///
/// A selection does not survive, for the same reason it does not survive
/// [`App::apply_latest`]: the page replaced the window.
pub(crate) fn apply_jump(
    pending: &mut Pending,
    conversation: &mut ConversationState,
    ui: &mut UiState,
    page: &[Message],
    target_id: i64,
) -> bool {
    // Which jump this page answers, read before `clear_jump` ends the wait.
    // A jump the reader asked for by name says so when its message cannot be
    // found; `gg`'s first-unread jump keeps its old silence, because its key
    // means "take me to the unread" and not "take me to message 19", and its
    // behaviour does not change here.
    let kind = pending.pending_jump.map(|jump| jump.kind);
    if !pending.clear_jump(target_id) {
        return false;
    }

    if !conversation.page_belongs_to_open_chat(page) {
        // A page that came back with nothing in it is the fetch's answer:
        // the client does not hold the message the reader was taken to, so
        // there is nowhere for the window to go. Said as a refusal because
        // the jump is over and the key is free, and the status line is
        // where a refusal belongs.
        if page.is_empty() && kind.is_some_and(|kind| kind != JumpKind::Unread) {
            ui.flash(JUMP_UNAVAILABLE);
        }
        return false;
    }

    // Copied into the window rather than moved: a page that replaces a
    // window is the caller's to report to the cursor it keeps, and that
    // cursor is counted from the same messages.
    conversation
        .conversation
        .window
        .replace(page.iter().cloned());
    conversation.clear_selection();

    // A window that jumped is surrounded by the unknown on both sides,
    // whatever the one before it had run out of.
    conversation.conversation.window.exhausted_older = false;
    conversation.conversation.window.exhausted_newer = false;

    conversation
        .vim
        .set_total(conversation.conversation.window.len());

    let landing = conversation.landing_index(target_id);
    conversation.vim.set_cursor(landing);
    conversation.settle_follow();

    true
}

/// Applies an event from the feed to everything it touches.
///
/// One event, two places: the list keeps the preview and the unread count,
/// the open conversation keeps the messages. The same contract as the flat
/// window's — `false` means nothing observable moved, so the caller owes no
/// redraw. Deduplicating by identifier is what makes the overlap between
/// the two windows harmless.
///
/// A read acknowledgement is the one event that is not a window change, and
/// it is handled apart from the rest rather than by a flag on the path: see
/// the comment where it is matched.
///
/// The event is copied rather than shared because the list moves an arrival
/// into its own window, so it needs one of its own. One copy per event is
/// the price of a single event reaching both.
#[must_use]
pub(crate) fn apply_update(
    ui: &mut UiState,
    list: &mut ChatListState,
    conversation: &mut ConversationState,
    drafts: &mut DraftStore,
    event: &UpdateEvent,
) -> bool {
    // The peer's typing is the title's business and nothing else's: no
    // message arrived, changed or left, so there is no window to re-anchor
    // and the reader's place in it is untouched. It answers for the open
    // conversation only, because the note is drawn on that conversation's
    // title — an event about a chat the reader is not in would be zeroed on
    // their arrival anyway.
    if let UpdateEvent::PeerTyping { chat_id, typing } = event {
        return apply_typing(&mut *ui, &mut *conversation, *chat_id, *typing);
    }

    let listed = list.list.apply_update(event.clone());

    // The message is what the typing was for, so it ends it. Cleared here
    // rather than on the cancel action alone because the cancel is not sent
    // reliably: a peer who sends instead of cancelling would otherwise be
    // shown as typing with their own message on screen.
    if let UpdateEvent::NewMessage(message) = event
        && message.chat_id == conversation.conversation.window.chat_id
    {
        ui.set_typing(None);
    }

    // A read acknowledgement is not a window change. It moves the watermark
    // and nothing else — no message arrived, changed or left — so there is
    // nothing to re-anchor and a reader scrolled back up stays exactly where
    // they are (US-X4). The redraw is still owed: the receipt is on the screen
    // now. It is recorded for the conversation either way, so a chat the
    // reader is not in yet carries its reading when they open it.
    if let UpdateEvent::ReadReceipt { chat_id, max_id } = event {
        let noted = drafts.note_read(*chat_id, *max_id);
        let open = conversation.conversation.apply_event(event);

        return listed || noted || open;
    }

    let anchor = conversation.cursor_message_id();
    let windowed = conversation.conversation.apply_event(event);
    if windowed {
        conversation.after_window_change(anchor);
    }

    listed || windowed
}

/// Records or drops the peer's typing for the conversation on show.
///
/// Reports whether a redraw is owed. A conversation other than the open one
/// is dropped rather than remembered: the note is drawn on one title, and a
/// deadline kept for a chat the reader has left would be one nobody is shown
/// and the reader has not been told about.
pub(crate) fn apply_typing(
    ui: &mut UiState,
    conversation: &mut ConversationState,
    chat_id: i64,
    typing: bool,
) -> bool {
    if chat_id != conversation.conversation.window.chat_id {
        return false;
    }

    ui.set_typing(if typing {
        // Re-armed rather than set once, because a peer who keeps typing past
        // the deadline is still typing.
        Some((chat_id, Instant::now() + TYPING_FOR))
    } else {
        None
    });
    true
}

/// Puts the focus somewhere, leaving whatever the pane it came from was in.
///
/// Visual mode belongs to the conversation and names messages in it, so
/// leaving the conversation drops the selection and returns the mode to
/// Normal. A selection for a conversation nobody is looking at would leave
/// `d` holding something the reader cannot see.
pub(crate) fn set_focus(
    ui: &mut UiState,
    conversation: &mut ConversationState,
    input: &mut InputState,
    profile: &mut ProfileCard,
    focus: Focus,
) {
    if focus != Focus::Conversation {
        ui.set_mode(Mode::Normal);
        conversation.clear_selection();
    }
    // The single clear point for every way out of a pane, beside the one
    // below it for the line. A profile is not a stack: leaving it means the
    // conversation is on show again, and there is no previous pane to go
    // back to.
    close_profile(&mut *ui, &mut *profile);
    // The single clear point for every way out of the line: `Tab`,
    // `BackTab`, `Ctrl+w` and `Esc`-to-leave all pass through here, so the
    // completion does not need a case in each of them.
    if focus != Focus::Input {
        input.dismiss_completion();
    }
    ui.set_focus(focus);
}

/// Moves the focus one pane on, in the direction given, wrapping.
///
/// The order is the order the panes are drawn in, so `Tab` walks the screen
/// rather than an arbitrary list of them.
pub(crate) fn cycle_focus(
    ui: &mut UiState,
    conversation: &mut ConversationState,
    input: &mut InputState,
    profile: &mut ProfileCard,
    forward: bool,
) {
    const PANES: [Focus; 3] = [Focus::ChatList, Focus::Conversation, Focus::Input];
    let step = if forward { 1 } else { PANES.len() - 1 };

    let at = PANES.iter().position(|pane| *pane == ui.focus).unwrap_or(0);

    set_focus(
        &mut *ui,
        &mut *conversation,
        &mut *input,
        &mut *profile,
        PANES[(at + step) % PANES.len()],
    );
}

/// Leaves the input line for the conversation, keeping what was typed.
///
/// `Ctrl+w` is Vim's other idiom for this, and the one bound to the pane
/// walk, because `Esc` is no longer a single key: a reader stepping between
/// panes should not have to know how many `Esc` presses the line's current
/// mode takes, and this one leaves from any of them. It only ever looks
/// away — nothing typed is lost to it.
pub(crate) fn leave_line(
    ui: &mut UiState,
    conversation: &mut ConversationState,
    input: &mut InputState,
    profile: &mut ProfileCard,
) {
    if ui.focus == Focus::Input {
        set_focus(
            &mut *ui,
            &mut *conversation,
            &mut *input,
            &mut *profile,
            Focus::Conversation,
        );
    }
}

/// Opens the conversation with a person the search found, or focuses the one
/// already there.
///
/// **A person the chat list already holds is focused**, which is the local
/// half of "already-existing chats focus rather than duplicate": the match is
/// by identifier, because a private chat's identifier *is* the peer's, and
/// [`domain::updates::ChatList::ensure_private_chat`] returns that existing index untouched.
/// **Someone it does not hold is listed and opened**: the helper appends a
/// private chat in the candidate's name without disturbing the order, and
/// returns the new index.
///
/// From there the open mirrors every other one — [`App::select_chat`] sets
/// the view and the read watermark up, the focus moves to the conversation,
/// and the overlay is forgotten — so an opened chat behaves like any other.
/// No dialog reload is needed for a single person: the candidate already
/// carries the name to list, which is Q6's recommended insertion path.
pub(crate) fn open_user(
    ui: &mut UiState,
    list: &mut ChatListState,
    outbox: &mut Outbox,
    pending: &mut Pending,
    conversation: &mut ConversationState,
    input: &mut InputState,
    drafts: &mut DraftStore,
    profile: &mut ProfileCard,
    user: &UserCandidate,
) {
    let index = list
        .list
        .ensure_private_chat(user.user_id, user.display_name.clone());

    select_chat(
        &mut *ui,
        &mut *list,
        &mut *outbox,
        &mut *pending,
        &mut *conversation,
        &mut *input,
        &mut *drafts,
        index,
    );
    set_focus(
        &mut *ui,
        &mut *conversation,
        &mut *input,
        &mut *profile,
        Focus::Conversation,
    );
    conversation.user_search.clear();
}

/// Puts the conversation back in the right-hand pane.
pub(crate) fn close_profile(ui: &mut UiState, profile: &mut ProfileCard) {
    ui.set_pane(Pane::Conversation);
    profile.resize(0);
}

/// Leaves a card, for the way back rather than for `Esc`.
///
/// Drops the selection and the inline position as well as the pane, because
/// all three describe a card that is no longer on show: a position inside a
/// value means nothing in a conversation, and leaving it behind would be a
/// caret waiting to be drawn on the wrong surface.
pub(crate) fn close_card(ui: &mut UiState, profile: &mut ProfileCard) {
    profile.clear_transient();
    close_profile(&mut *ui, &mut *profile);
}

/// `Esc` on a card, as a ladder: a selection, then the card, then out.
///
/// Four presses from a selection on a second row, and no special case
/// anywhere in it — a key that means one thing in one state and another in the
/// next is a key a reader has to learn twice, and `Esc` is the one key every
/// reader already reaches for.
pub(crate) fn escape_card(ui: &mut UiState, profile: &mut ProfileCard) {
    if profile.profile_visual.take().is_some() {
        return;
    }
    close_card(&mut *ui, &mut *profile);
}

/// Handles `y` in Visual: what the selection covers goes into the register.
///
/// A text selection yanks exactly the characters selected and nothing else.
/// Anything else yanks one line per message, oldest first, so a yank of three
/// messages pastes back as three messages — which is what "yank these" means
/// in a conversation, where a message is the unit a reader thinks in.
///
/// Visual is left either way, worked or not. A `y` that found nothing has
/// still answered the key, and staying in Visual would hide the refusal: the
/// selection's own note outranks a transient status, so a `flash` written
/// while a selection is up is a message the reader never sees.
///
/// The register is the load-bearing half. The system clipboard is a
/// convenience that depends on the terminal, the terminal emulator and often
/// the user's settings — three things none of which can be tested here — and a
/// yank that only works in the second is a yank that appears broken.
pub(crate) fn yank(ui: &mut UiState, outbox: &mut Outbox, conversation: &mut ConversationState) {
    let Some(selection) = conversation.selection else {
        return;
    };

    let lines = conversation.yanked(&selection);
    conversation.clear_selection();
    ui.set_mode(Mode::Normal);

    if lines.iter().all(String::is_empty) {
        // A charwise selection that has not been moved is a position rather
        // than a span, and there is nothing in it to take. Said rather than
        // silently replacing whatever was in the register with nothing.
        ui.flash("nothing to yank — move the selection first");
        return;
    }

    conversation.set_register(Register::set(lines));
    outbox.store_clipboard(conversation.register.text());
}

/// Handles `r` in Visual, which is a refusal.
///
/// This is the whole implementation, and it is a refusal for one of two
/// reasons:
///
/// - A selection that is not inside one message has no quote to send. Telegram
///   quotes a fragment of *one* message, and there is no wire representation
///   for quoting five — so `V` and a range have nothing to answer.
/// - A quote of one message cannot be sent at all on the pinned `grammers`:
///   its `InputMessage` has no field for one, and it hard-codes
///   `quote_text`/`quote_offset` to `None` in the reply it builds. The
///   upstream gap is written up in
///   `~/.opencode/plan/pr-grammers-quote-support.md`, and it is a refusal
///   rather than a workaround because composing the quote as ordinary message
///   text produces something that *looks* like a quote and is not — the
///   difference is visible to the person receiving it.
///
/// Which is also why this is not "reply to the cursor's message instead": a key
/// that answered a different question than the one asked, while the screen said
/// `-- VISUAL --`, would be worse than a refusal.
///
/// Visual is left either way, for the reason [`App::yank`] gives: the selection's
/// own note outranks a transient status, so a refusal written while a selection
/// is up is a line the reader never sees.
pub(crate) fn reply_to_selection(ui: &mut UiState, conversation: &mut ConversationState) {
    let Some(selection) = conversation.selection else {
        return;
    };

    let refused = if selection.text_range().is_some() {
        "quoting a reply is not built yet"
    } else {
        "a reply can only quote words inside one message"
    };

    conversation.clear_selection();
    ui.set_mode(Mode::Normal);
    ui.flash(refused);
}

/// Asks whether the reader meant to quit.
///
/// `q` sits where a reader's hand already is and next to keys that type
/// nothing else, so an accidental one is a real event rather than a
/// hypothetical: a confirmation is the whole difference between losing the
/// window and having pressed a key.
///
/// `Ctrl-C` deliberately does not come through here. It is the way out when
/// the program is wedged, and a question in front of it would be a question
/// the reader cannot see, because a terminal that is not answering cannot
/// draw one either.
pub(crate) fn request_quit(ui: &mut UiState, conversation: &mut ConversationState) {
    ui.set_mode(Mode::Confirm);
    conversation.set_confirm(Some(ConfirmKind::Quit));
}

/// Opens the buffer for a new message, with no reply and no edit.
///
/// The draft is kept: a half-written message is not garbage, and the one
/// thing a reader who pressed `i` by reflex should never lose is the thing
/// they were writing.
pub(crate) fn start_compose(
    ui: &mut UiState,
    conversation: &mut ConversationState,
    input: &mut InputState,
) {
    ui.set_focus(Focus::Input);
    input.line.open(PromptKind::Message);
    conversation.set_reply_to(None);
    conversation.set_editing(None);
}

/// Opens the buffer to answer the message under the cursor.
///
/// Replying needs a message to answer; with the window empty there is none,
/// and the key does nothing rather than opening a reply to nowhere.
pub(crate) fn start_reply(
    ui: &mut UiState,
    conversation: &mut ConversationState,
    input: &mut InputState,
) {
    let Some(id) = conversation.cursor_message_id() else {
        return;
    };

    ui.set_focus(Focus::Input);
    input.line.open(PromptKind::Reply);
    conversation.set_reply_to(Some(id));
    conversation.set_editing(None);
}

/// Opens the buffer with the cursor's own message in it, for editing.
///
/// The one opener that replaces the text, and the only one: the buffer's
/// meaning changes from "a message" to "an edit of message 42", so there is
/// only one right thing for it to contain.
///
/// Both refusals are the same fact — there is nothing on the server to edit
/// yet — so they share one line. A key that does nothing and says nothing
/// reads as a hang, and this one is hit constantly now that `dd` accepts an
/// incoming message.
pub(crate) fn start_edit(
    ui: &mut UiState,
    conversation: &mut ConversationState,
    input: &mut InputState,
) {
    let Some(message) = conversation.cursor_message() else {
        return;
    };

    if message.id <= 0 {
        ui.flash("it hasn't been sent yet");
        return;
    }
    if !message.is_outgoing {
        ui.flash("you can only edit your own messages");
        return;
    }

    let text = message.text.to_string();
    let id = message.id;

    ui.set_focus(Focus::Input);
    input.line.open_with(PromptKind::Edit, text);
    conversation.set_editing(Some(id));
    conversation.set_reply_to(None);
}

/// Opens the line to find a person to talk to, with `query` already in it.
///
/// The one opener for this prompt, from `/` on the chat list and from
/// `:new`, so the two routes cannot drift apart. A draft is not carried in:
/// the previous query is not the reader's words to keep, and a fresh search
/// starts clean.
pub(crate) fn begin_new_chat(ui: &mut UiState, input: &mut InputState, query: &str) {
    ui.set_focus(Focus::Input);
    input.line.open_with(PromptKind::NewChat, query.to_owned());
}

/// Raises the confirmation for deleting every message the selection covers.
///
/// A selection inside one message deletes the whole of it: a partial message
/// is not something the protocol can do, and half a deletion is not something
/// the reader would recognise afterwards.
pub(crate) fn confirm_delete(ui: &mut UiState, conversation: &mut ConversationState) {
    let Some(selection) = conversation.selection else {
        return;
    };

    let Some(deletion) = conversation.deletion(&selection) else {
        conversation.clear_selection();
        ui.set_mode(Mode::Normal);
        ui.flash(conversation.refuse_placeholders(&selection));
        return;
    };

    ui.set_mode(Mode::Confirm);
    conversation.set_confirm(Some(ConfirmKind::DeleteMessages {
        ids: deletion.ids,
        outgoing: deletion.outgoing,
        skipped: deletion.skipped,
    }));
}

/// Handles a key while a confirmation is up.
///
/// However it ends, the selection goes with it: it was made for this
/// question, and a second `d` afterwards must ask about whatever is under the
/// cursor then rather than reusing a range the reader has already answered.
pub(crate) fn handle_confirm(
    ui: &mut UiState,
    conversation: &mut ConversationState,
    outbox: &mut Outbox,
    key: KeyEvent,
) {
    match key.code {
        KeyCode::Char('y') => {
            match &conversation.confirm {
                Some(ConfirmKind::Quit) => ui.quit(),
                Some(ConfirmKind::Logout) => queue_action(&mut *outbox, &mut *ui, Action::Logout),
                Some(ConfirmKind::DeleteMessages { ids, .. }) => {
                    let chat_id = conversation.conversation.window.chat_id;
                    queue_action(
                        &mut *outbox,
                        &mut *ui,
                        Action::Delete {
                            chat_id,
                            message_ids: ids.clone(),
                        },
                    );
                }
                None => {}
            }
            conversation.set_confirm(None);
            conversation.clear_selection();
            ui.set_mode(Mode::Normal);
        }
        KeyCode::Char('n') | KeyCode::Esc => {
            conversation.set_confirm(None);
            conversation.clear_selection();
            ui.set_mode(Mode::Normal);
        }
        _ => {}
    }
}

/// Handles a key while a selection is being made.
///
/// `Esc` is the way out of it: the selection goes and the mode returns to
/// Normal, rather than the selection being left behind for a `d` to find.
/// `o` and `O` exchange the two ends, so a selection dragged "backwards" can
/// be re-anchored without being put back where it was.
///
/// The character motions move the *focus* — the end that moves — rather than
/// the anchor, which is what makes a selection grow from one end. They clamp
/// to the message's own text and never cross into the next one: `j` and `k`
/// are for that, and a motion that silently changed what it selected would be
/// the worst thing a selection could do.
pub(crate) fn handle_visual(
    ui: &mut UiState,
    pending: &mut Pending,
    conversation: &mut ConversationState,
    outbox: &mut Outbox,
    key: KeyEvent,
) {
    // A `f` takes the very next keypress as the character to look for, whatever
    // it is: that is what `fw` means, and reading the `w` as a motion would be
    // a different key entirely. Anything else ends the sequence.
    if let Some(Find { forward, onto }) = pending.pending_find.take()
        && let KeyCode::Char(target) = key.code
    {
        conversation.move_focus(CharMotion::Find {
            target,
            forward,
            onto,
        });
        return;
    }
    pending.set_find(None);

    match key.code {
        KeyCode::Esc => {
            conversation.selection = None;
            ui.set_mode(Mode::Normal);
            ui.set_status(IDLE_STATUS.into());
        }

        // Re-anchoring on the cursor's message is `v` again, which is what it
        // is for: the reader is saying "start here instead".
        KeyCode::Char('v') => begin_selection(&mut *ui, &mut *conversation, Some(0)),
        KeyCode::Char('V') => begin_selection(&mut *ui, &mut *conversation, None),
        KeyCode::Char('o' | 'O') => {
            if let Some(selection) = &mut conversation.selection {
                selection.swap();
            }
        }

        KeyCode::Char('y') => yank(&mut *ui, &mut *outbox, &mut *conversation),
        KeyCode::Char('d') => request_delete(&mut *ui, &mut *conversation),
        KeyCode::Char('r') => reply_to_selection(&mut *ui, &mut *conversation),

        KeyCode::Char('j') => conversation.move_focus_to_message(true),
        KeyCode::Char('k') => conversation.move_focus_to_message(false),

        KeyCode::Char('h') => conversation.move_focus(CharMotion::Step { forward: false }),
        KeyCode::Char('l') => conversation.move_focus(CharMotion::Step { forward: true }),
        KeyCode::Char('w') => conversation.move_focus(CharMotion::WordStart { forward: true }),
        KeyCode::Char('b') => conversation.move_focus(CharMotion::WordStart { forward: false }),
        KeyCode::Char('e') => conversation.move_focus(CharMotion::WordEnd),
        KeyCode::Char('0') => conversation.move_focus(CharMotion::Bound { end: false }),
        KeyCode::Char('$') => conversation.move_focus(CharMotion::Bound { end: true }),

        KeyCode::Char('f') => {
            pending.set_find(Some(Find {
                forward: true,
                onto: true,
            }));
        }
        KeyCode::Char('t') => {
            pending.set_find(Some(Find {
                forward: true,
                onto: false,
            }));
        }
        KeyCode::Char('F') => {
            pending.set_find(Some(Find {
                forward: false,
                onto: true,
            }));
        }
        KeyCode::Char('T') => {
            pending.set_find(Some(Find {
                forward: false,
                onto: false,
            }));
        }

        _ => {}
    }
}

/// Handles `p` in Normal: opens the line with what was last yanked.
///
/// A yank with no paste is a one-way trip to the system clipboard, and the
/// system clipboard is not somewhere a message can be sent from. This is the
/// paste that puts the reader's own words back in front of them, at the
/// caret, to be edited and sent like anything else they typed.
///
/// A draft already in the bar is kept rather than replaced, and the register
/// goes in at the caret: `p` is a paste, so it behaves like one everywhere
/// else, and a reader who wants to throw their draft away has a key that
/// does that.
pub(crate) fn paste(
    ui: &mut UiState,
    conversation: &mut ConversationState,
    input: &mut InputState,
) {
    if conversation.register.is_empty() {
        ui.flash("nothing has been yanked");
        return;
    }

    start_compose(&mut *ui, &mut *conversation, &mut *input);
    input.line.insert(&conversation.register.text());
}

/// Queues the composed message as a send, and shows it immediately.
///
/// The placeholder is what the reader sees until the server answers, and its
/// identifier is what the answer is matched against. One send is in flight at
/// a time: Telegram throttles per conversation, and a second send would only
/// earn a `FLOOD_WAIT` — but a silent no-op reads as a hang, so the refusal
/// says so.
///
/// A queued send clears the peer's parked entry, if there is one: the words
/// are on their way, so there is no draft left to keep. Mirrors what
/// [`park_draft`] does for empty lines.
pub(crate) fn submit_message(
    ui: &mut UiState,
    conversation: &mut ConversationState,
    input: &mut InputState,
    outbox: &mut Outbox,
    drafts: &mut DraftStore,
) {
    if conversation.sending.is_some() {
        ui.flash("a message is already on its way");
        return;
    }
    if !conversation.has_conversation() || input.line.text().trim().is_empty() {
        return;
    }

    let anchor = conversation.cursor_message_id();
    let chat_id = conversation.conversation.window.chat_id;
    let text = input.line.take();
    let temp_id = conversation
        .conversation
        .queue_send(&text, conversation.reply_to);
    conversation.begin_send(temp_id);
    queue_action(
        &mut *outbox,
        &mut *ui,
        Action::Send {
            chat_id,
            temp_id,
            text,
            reply_to: conversation.reply_to,
        },
    );
    conversation.after_window_change(anchor);
    drafts.drafts.remove(&chat_id);
}

/// Queues the edit of the message the buffer was opened with.
///
/// Nothing is shown optimistically: an edit is reflected when the server's
/// `MessageEdited` arrives, which is the only path by which its new text
/// reaches the window.
///
/// Like [`submit_message`], a queued edit clears the peer's parked entry:
/// the words are on their way, so there is no draft left to keep.
pub(crate) fn submit_edit(
    ui: &mut UiState,
    conversation: &mut ConversationState,
    input: &mut InputState,
    outbox: &mut Outbox,
    drafts: &mut DraftStore,
) {
    let Some(message_id) = conversation.editing else {
        return;
    };
    if !conversation.has_conversation() || input.line.text().trim().is_empty() {
        return;
    }

    let chat_id = conversation.conversation.window.chat_id;
    let text = input.line.take();
    queue_action(
        &mut *outbox,
        &mut *ui,
        Action::Edit {
            chat_id,
            message_id,
            text,
        },
    );
    drafts.drafts.remove(&chat_id);
}

/// Starts a search for a person, purely locally.
///
/// The list is cleared and marked in flight, and the lookup is queued: `tui`
/// cannot reach the network, so the caller performs it and answers with
/// [`App::apply_users`] or [`App::fail_users`]. An empty query asks nothing,
/// and a key that did nothing silently reads as a hang, so it says so.
pub(crate) fn submit_new_chat(
    ui: &mut UiState,
    conversation: &mut ConversationState,
    outbox: &mut Outbox,
    query: &str,
) {
    if query.is_empty() {
        ui.flash("type a name or @username to search for");
        return;
    }

    conversation.user_search.begin(query);
    queue_action(
        &mut *outbox,
        &mut *ui,
        Action::ResolveUser {
            query: query.to_owned(),
        },
    );
}

/// Answers `/`: scans the window now, and asks the server if it can do
/// better.
///
/// The local pass is free and synchronous, so `n` works in the same frame.
/// It is provisional — it can only see what is loaded, and it approximates
/// what the server does — so the server's answer replaces it when it comes.
///
/// An empty query repeats the last search, as in Vim. With nothing to
/// repeat, the refusal is visible rather than the key doing nothing.
pub(crate) fn run_search(
    ui: &mut UiState,
    conversation: &mut ConversationState,
    outbox: &mut Outbox,
    query: &str,
) {
    let Some(query) = conversation.search_to_run(query) else {
        ui.flash("no previous search");
        return;
    };

    let chat_id = conversation.conversation.window.chat_id;
    let ids: Vec<i64> = conversation
        .conversation
        .window
        .iter()
        .filter(|message| word_prefix_match(message.display_body(), &query))
        .map(|message| message.id)
        .collect();

    conversation.search.begin_local(&query, ids);
    conversation.land_on_match();

    // A conversation the window holds in full cannot be searched better, so
    // the round trip would be pure latency. Otherwise the request is handed
    // to the caller, which can reach the network, and the answer arrives at
    // [`App::apply_searched`].
    if holds_everything(&conversation.conversation.window) {
        conversation.search.finish_local();
    } else {
        queue_action(&mut *outbox, &mut *ui, Action::Search { chat_id, query });
    }
}

/// The one way in, whatever the reader came from: the signed-out card's
/// `:signin`, and a launch with no session are the same flow, because they
/// are the same question. The card is closed rather than covered — a
/// conversation and a card have nothing to say while somebody is typing a
/// password, and a card the reader asked to leave should be left.
///
/// **A machine with no application credentials gets the sentence instead.**
/// `:signin` is answered wherever it is typed, including from the no-credentials
/// screen and from a card that is not about the account — and a form whose
/// answer could not be used is worse than the sentence that explains why.
pub(crate) fn begin_signin(
    ui: &mut UiState,
    session: &mut SessionState,
    conversation: &mut ConversationState,
    input: &mut InputState,
    profile: &mut ProfileCard,
) {
    if !session.credentials_configured {
        begin_no_credentials(
            &mut *ui,
            &mut *session,
            &mut *conversation,
            &mut *input,
            &mut *profile,
        );
        return;
    }

    ui.set_pane(Pane::Conversation);
    ui.set_mode(Mode::Normal);
    session.begin_signin(SignIn::Flow(SignInFlow::default()));
    open_signin_field(&mut *session, &mut *input, LoginField::Phone);
    set_focus(
        &mut *ui,
        &mut *conversation,
        &mut *input,
        &mut *profile,
        Focus::Input,
    );
}

/// Puts up the sentence a machine with no credentials gets.
///
/// A sentence and not a form, because there is nothing to type: the missing
/// `api_id` and `api_hash` are in a file, and a phone field here would ask
/// the reader for something the program still could not do with.
pub(crate) fn begin_no_credentials(
    ui: &mut UiState,
    session: &mut SessionState,
    conversation: &mut ConversationState,
    input: &mut InputState,
    profile: &mut ProfileCard,
) {
    ui.set_pane(Pane::Conversation);
    ui.set_mode(Mode::Normal);
    session.begin_signin(SignIn::NoCredentials);
    set_focus(
        &mut *ui,
        &mut *conversation,
        &mut *input,
        &mut *profile,
        Focus::Conversation,
    );
}

/// Records that the client is to be brought up again.
///
/// The `offline:` sentence is answered the moment it is read, so the status
/// line says the retry is under way rather than leaving a sentence about a
/// failed launch up beside one in flight — and what the network side writes
/// next (the retry sentence, or the next `offline:`) replaces it.
pub(crate) fn request_retry(ui: &mut UiState, session: &mut SessionState) {
    session.request_retry();
    // Written straight to `status` rather than through `flash`, because a
    // bring-up is not a thing that passes on its own: it ends in an event, and
    // that event brings its own sentence.
    ui.show_persistent("reconnecting");
}

/// The account is signed in: the surface has nothing left to say.
///
/// The flow is dropped rather than left at its last step, so the conversation
/// is the whole program again — which is what signing in is for.
pub(crate) fn login_complete(
    session: &mut SessionState,
    input: &mut InputState,
    ui: &mut UiState,
    conversation: &mut ConversationState,
    profile: &mut ProfileCard,
) {
    session.clear_signin();
    input.line.clear();
    set_focus(
        &mut *ui,
        &mut *conversation,
        &mut *input,
        &mut *profile,
        Focus::Conversation,
    );
    ui.clear_status();
}

/// Opens `field`, taking the configuration's value only when `prefill` says
/// this is a step opening rather than a step being refused.
pub(crate) fn open_signin_field_with(
    session: &mut SessionState,
    input: &mut InputState,
    field: LoginField,
    prefill: bool,
) {
    let prompt = match field {
        LoginField::Phone => PromptKind::Phone,
        LoginField::Code => PromptKind::Code,
        LoginField::Password => PromptKind::Password,
    };
    let text = match field {
        LoginField::Phone if prefill => session.phone.trim().to_owned(),
        LoginField::Code if prefill => session.code_prefill.clone(),
        LoginField::Password if prefill => session.password_prefill.clone(),
        _ => String::new(),
    };
    input.line.open_with(prompt, text);
}

/// Opens `field` in the line, pre-filled from the configuration.
///
/// The phone is the configuration's number because a phone number is not a
/// secret and is the one thing about a sign-in a reader does not have to
/// type. The code and the password are pre-filled only where the
/// configuration carries them, and only when the step *opens*: what arrives
/// here from a refusal is the same call, which is why a wrong code is not
/// restored — a line holding a wrong answer is a line holding what Telegram
/// already refused.
pub(crate) fn open_signin_field(
    session: &mut SessionState,
    input: &mut InputState,
    field: LoginField,
) {
    open_signin_field_with(&mut *session, &mut *input, field, true);
}

/// Opens `field` with nothing in it, whatever the configuration carries.
///
/// The refusal path, and the reason it is a separate call: a step the reader
/// has just been told was wrong opens on a blank line, because a pre-filled
/// one would put back the answer that was refused — and a code Telegram
/// expires is worse than one the reader has to look up again.
pub(crate) fn open_signin_field_blank(
    session: &mut SessionState,
    input: &mut InputState,
    field: LoginField,
) {
    open_signin_field_with(&mut *session, &mut *input, field, false);
}

/// Steps away from the flow, keeping the step.
///
/// `Tab` and `Ctrl+w` are the pane walk everywhere else and here they mean
/// this, which is why the sign-in is answered before the walk rather than
/// through it. The code does not survive the trip — see
/// [`SignInFlow::lost_code`] — and everything else does, because a phone
/// number is not a secret and a password stays in a line that paints itself
/// as bullets whether it has the focus or not.
pub(crate) fn signin_away(
    session: &mut SessionState,
    input: &mut InputState,
    ui: &mut UiState,
    conversation: &mut ConversationState,
    profile: &mut ProfileCard,
) {
    if session.signin_field() == Some(LoginField::Code) {
        input.line.clear();
        if let Some(flow) = session.signin.as_mut().and_then(SignIn::flow_mut) {
            flow.lost_code = true;
        }
    }
    set_focus(
        &mut *ui,
        &mut *conversation,
        &mut *input,
        &mut *profile,
        Focus::ChatList,
    );
    ui.flash("sign-in paused; Tab brings it back");
}

/// `Esc` at the code or the password: back to the phone.
///
/// The code Telegram sent is discarded rather than kept, so the sentence
/// says what that costs and offers the way to get another one. The flow
/// stays up: the reader asked to sign in, not to stop.
pub(crate) fn signin_cancel(
    session: &mut SessionState,
    input: &mut InputState,
    ui: &mut UiState,
    outbox: &mut Outbox,
    conversation: &mut ConversationState,
    profile: &mut ProfileCard,
) {
    queue_action(&mut *outbox, &mut *ui, Action::LoginCancelled);
    if let Some(flow) = session.signin.as_mut().and_then(SignIn::flow_mut) {
        flow.login = domain::session::LoginState::default();
    }
    open_signin_field(&mut *session, &mut *input, LoginField::Phone);
    set_focus(
        &mut *ui,
        &mut *conversation,
        &mut *input,
        &mut *profile,
        Focus::Input,
    );
    ui.flash("cancelling discards the code Telegram sent; ⏎ asks for a new one");
}

/// Comes back to the field the step is at.
///
/// The step survives the trip away and the code does not, so the return says
/// what was lost and what to do about it rather than opening an empty line
/// the reader has to work out the state of.
pub(crate) fn signin_back(
    session: &mut SessionState,
    input: &mut InputState,
    ui: &mut UiState,
    conversation: &mut ConversationState,
    profile: &mut ProfileCard,
) {
    let Some(field) = session.signin_field() else {
        return;
    };
    open_signin_field(&mut *session, &mut *input, field);
    set_focus(
        &mut *ui,
        &mut *conversation,
        &mut *input,
        &mut *profile,
        Focus::Input,
    );
    let lost = session
        .signin
        .as_mut()
        .and_then(SignIn::flow_mut)
        .is_some_and(|flow| std::mem::take(&mut flow.lost_code));
    if lost {
        ui.flash("the code did not survive; ⏎ asks for a new one");
    } else {
        ui.clear_status();
    }
}

/// `⏎` on a field: asks the caller to move the flow on.
///
/// The value leaves as an [`Action::Login`] rather than as a call, because a
/// sign-in request is the network's and `tui` may not name it.
///
/// **The in-flight guard is the whole reason this is a method rather than
/// three lines in the key handler.** A second `⏎` while `waiting` fires no
/// second request: Telegram counts a login attempt per request, and a reader
/// pressing `⏎` twice is asking whether the first went out, not for two
/// codes. The first one is answered with the sentence, and after that the
/// key is silence — see [`SignInFlow::still_said`].
pub(crate) fn submit_signin_field(
    session: &mut SessionState,
    input: &mut InputState,
    ui: &mut UiState,
    outbox: &mut Outbox,
    conversation: &mut ConversationState,
    profile: &mut ProfileCard,
) {
    let Some(field) = session.signin_field() else {
        return;
    };

    if let Some(flow) = session.signin.as_ref().and_then(SignIn::flow)
        && flow.waiting
    {
        let said = session
            .signin
            .as_mut()
            .and_then(SignIn::flow_mut)
            .is_some_and(|flow| !std::mem::take(&mut flow.still_said));
        if said {
            ui.flash("still checking — the answer is on its way");
        }
        return;
    }

    let value = input.line.text().trim().to_owned();

    // An empty field is refused here rather than sent. A blank phone number
    // is a request Telegram throttles, a blank code is a login attempt the
    // reader did not mean to spend, and a blank password says nothing at
    // all — none of which is worth a round trip to be told.
    if value.is_empty() {
        ui.flash(match field {
            LoginField::Phone => "there is no phone number to ask for a code with".to_owned(),
            LoginField::Code => "there is no login code to send".to_owned(),
            LoginField::Password => "there is no password to check".to_owned(),
        });
        return;
    }

    // **The client-less branch.** A request nobody will carry is not a
    // request on its way, so the flow is not told one is: that would put
    // "Checking…" on the panel for an answer that is never coming, and the
    // key after it would be swallowed by the guard as a second press. The
    // sentence is the whole answer, the draft stays in the line because the
    // client coming up is not the reader typing it again, and nothing is
    // queued — a queued login would fire on its own if a client appeared
    // later, which is a sign-in attempt nobody asked for.
    if !session.client_available {
        ui.flash("not connected yet — the client is not up");
        return;
    }

    queue_action(&mut *outbox, &mut *ui, Action::Login { field, value });
    if let Some(flow) = session.signin.as_mut().and_then(SignIn::flow_mut) {
        flow.waiting = true;
        flow.still_said = false;
    }
    // The line stays, dimmed, because the reader has to see what they sent
    // while it is in flight — and because emptying it would make the answer
    // arrive against nothing at all.
    set_focus(
        &mut *ui,
        &mut *conversation,
        &mut *input,
        &mut *profile,
        Focus::ChatList,
    );
}

/// A key while the flow is paused or waiting.
///
/// `Tab`, `BackTab` and `Ctrl-w` all come back, because the reader stepped
/// away with one of them and one route back is not one per way out. `Enter`
/// answers the stale session's offer, and nothing else answers anything: a
/// request is in flight, or the reader is somewhere else, and a key that
/// moved the flow on from here would be a key moving it on without a field.
pub(crate) fn handle_signin_away(
    session: &mut SessionState,
    input: &mut InputState,
    ui: &mut UiState,
    outbox: &mut Outbox,
    conversation: &mut ConversationState,
    profile: &mut ProfileCard,
    key: KeyEvent,
) {
    match key.code {
        KeyCode::Tab | KeyCode::BackTab => signin_back(
            &mut *session,
            &mut *input,
            &mut *ui,
            &mut *conversation,
            &mut *profile,
        ),
        _ if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('w') => {
            signin_back(
                &mut *session,
                &mut *input,
                &mut *ui,
                &mut *conversation,
                &mut *profile,
            );
        }
        KeyCode::Enter
            if session
                .signin
                .as_ref()
                .and_then(SignIn::flow)
                .is_some_and(|flow| flow.stale) =>
        {
            signin_back(
                &mut *session,
                &mut *input,
                &mut *ui,
                &mut *conversation,
                &mut *profile,
            );
            submit_signin_field(
                &mut *session,
                &mut *input,
                &mut *ui,
                &mut *outbox,
                &mut *conversation,
                &mut *profile,
            );
        }
        _ => {}
    }
}

/// Telegram answered: the flow is at `state` now.
///
/// The answer opens the next field, because a step the reader cannot type
/// into is a step they are waiting on, and it opens it with whatever the
/// configuration carries for it — this is the step change, which is the one
/// moment a pre-fill is right. `SESSION_PASSWORD_NEEDED` lands here rather
/// than in [`App::login_refused`] — it is not a refusal, it is the answer
/// that puts the password row up.
pub(crate) fn login_advanced(
    session: &mut SessionState,
    input: &mut InputState,
    ui: &mut UiState,
    conversation: &mut ConversationState,
    profile: &mut ProfileCard,
    state: domain::session::SessionState,
    hint: Option<String>,
) {
    let Some(flow) = session.signin.as_mut().and_then(SignIn::flow_mut) else {
        return;
    };
    flow.login.step = state;
    flow.login.refusal = None;
    flow.waiting = false;
    flow.still_said = false;
    // Kept only where it means something. An account with no two-step
    // password answers `None`, and so does every other step, so a hint read
    // on the password step cannot be drawn on the code step that follows it.
    flow.hint = match &flow.login.step {
        domain::session::SessionState::AwaitingPassword { .. } => hint,
        _ => None,
    };

    match session.signin_field() {
        Some(field) => {
            open_signin_field(&mut *session, &mut *input, field);
            set_focus(
                &mut *ui,
                &mut *conversation,
                &mut *input,
                &mut *profile,
                Focus::Input,
            );
        }
        None => login_complete(
            &mut *session,
            &mut *input,
            &mut *ui,
            &mut *conversation,
            &mut *profile,
        ),
    }
}

/// Telegram refused, in the reader's own words.
///
/// The sentence is carried rather than a code, because the words are what
/// the reader reads and the code is Telegram's — and `tui` may not name the
/// enum that holds it. `used` is the password count the same sentence is
/// counted from, so the row and the refusal cannot disagree about how many
/// attempts are left.
///
/// The field is re-opened **blank** rather than left as it was: a refusal
/// arrives after the reader has stopped looking at the line, and a line
/// holding what they typed is a line holding a wrong answer.
pub(crate) fn login_refused(
    session: &mut SessionState,
    input: &mut InputState,
    ui: &mut UiState,
    conversation: &mut ConversationState,
    profile: &mut ProfileCard,
    sentence: String,
    used: u8,
) {
    let Some(flow) = session.signin.as_mut().and_then(SignIn::flow_mut) else {
        return;
    };
    flow.login.refusal = Some(sentence);
    flow.used = used;
    flow.waiting = false;
    flow.still_said = false;

    if let Some(field) = session.signin_field() {
        open_signin_field_blank(&mut *session, &mut *input, field);
        set_focus(
            &mut *ui,
            &mut *conversation,
            &mut *input,
            &mut *profile,
            Focus::Input,
        );
    }
}

/// Takes the reader to where the conversation's unread messages start.
///
/// `gg` is Vim's top-of-buffer, and that is what it stays when there is
/// nothing unread to be taken to. When there is, the reader means the first
/// unread message, and this answers it from what is loaded wherever it can:
/// the cursor moves and nothing is returned.
///
/// What comes back is a jump the window cannot answer — the target is
/// somewhere the client has not fetched, and only the caller can go and get
/// it. Returning the intent rather than recording it keeps the two in step
/// at the one call site: the reader asked for *this*, and anything they had
/// asked for before is replaced by it, whether or not there is one.
///
/// The estimate is arithmetic on identifiers, and identifiers have gaps
/// wherever messages were deleted, so it can land in front of the true first
/// unread. A window that ends where the conversation does is the exception,
/// and it is the common case: the unread messages are then the newest ones
/// there are, so they are counted back from the end and land exactly.
#[must_use]
pub(crate) fn jump_to_unread(
    list: &mut ChatListState,
    conversation: &mut ConversationState,
    _ui: &mut UiState,
) -> Option<Jump> {
    if !conversation.has_conversation() {
        return None;
    }

    let target = first_unread(&mut *list, &mut *conversation)?;

    // Counted from the end when the window reaches the end of the
    // conversation: the unread messages are the newest ones there are, so
    // their number says exactly where they start however the messages are
    // numbered.
    if holds_newest_edge(&mut *list, &mut *conversation)
        && let Some(index) = landing_position(
            conversation.conversation.window.len(),
            unread(&mut *list, &mut *conversation),
        )
    {
        conversation.vim.set_cursor(index);
        return None;
    }

    // Or found by identifier, when the window holds the target but not the
    // end of the conversation.
    if let Some(index) = conversation.conversation.window.position_of(target) {
        conversation.vim.set_cursor(index);
        return None;
    }

    Some(Jump {
        peer_id: conversation.conversation.window.chat_id,
        target_id: target,
        kind: JumpKind::Unread,
    })
}

/// Takes the reader to the message the one under the cursor quotes.
///
/// `gd`, and the second producer of a jump: a reply's quote is a message in
/// the same conversation, which is loaded or is not. A target the window
/// holds is a cursor move and nothing else — no round trip for a message
/// already on screen. A target it does not hold is a jump, on the same terms
/// as `gg`'s.
///
/// A message that quotes nothing refuses, because there is nowhere to go:
/// the sentence names the key and what it does, which is more use to a
/// reader than silence.
///
/// Where the reader was is recorded before the cursor moves and before the
/// jump is armed, and in the in-window case too: that is the same place to
/// come back to as any other, and `Ctrl-o` after a jump that needed no fetch
/// is the case where the reader most expects to be able to undo it.
#[must_use]
pub(crate) fn jump_to_reply(
    _list: &mut ChatListState,
    conversation: &mut ConversationState,
    ui: &mut UiState,
) -> Option<Jump> {
    if !conversation.has_conversation() {
        return None;
    }

    let Some(target) = conversation
        .cursor_message()
        .and_then(|message| message.reply_to)
    else {
        ui.flash(NOT_A_REPLY);
        return None;
    };

    if let Some(origin) = conversation.cursor_message_id() {
        conversation
            .jumplist
            .record(conversation.conversation.window.chat_id, origin);
    }

    if let Some(index) = conversation.conversation.window.position_of(target) {
        conversation.vim.set_cursor(index);
        return None;
    }

    Some(Jump {
        peer_id: conversation.conversation.window.chat_id,
        target_id: target,
        kind: JumpKind::Reply,
    })
}

/// Where the open conversation's unread messages start, as far as its
/// numbering can say.
///
/// Counted from the message the conversation last showed, which the chat list
/// has held since it was fetched — no round trip. A conversation the list has
/// no preview for falls back on the newest message loaded, which is the same
/// message whenever anything has arrived while the conversation was open.
pub(crate) fn first_unread(
    list: &mut ChatListState,
    conversation: &mut ConversationState,
) -> Option<i64> {
    let chat = open_chat(&*list, &*conversation)?;
    let last_id = chat
        .last_message_id
        .or_else(|| conversation.conversation.window.newest_id());

    unread_target(last_id, chat.unread_count)
}

/// Whether the window ends where the conversation does.
///
/// Two ways to know, and the second is the one that covers a conversation
/// that was just opened — its newest page *is* the end, whatever a fetch
/// behind it has or has not said. An arrival updates the preview, so this
/// stays true as the conversation grows.
pub(crate) fn holds_newest_edge(
    list: &mut ChatListState,
    conversation: &mut ConversationState,
) -> bool {
    let window = &conversation.conversation.window;
    if window.is_empty() {
        return false;
    }
    if window.exhausted_newer {
        return true;
    }
    let newest = window.newest_id();

    open_chat(&*list, &*conversation)
        .and_then(|chat| chat.last_message_id)
        .is_some_and(|last| newest == Some(last))
}

/// How many messages the open conversation has unread.
pub(crate) fn unread(list: &mut ChatListState, conversation: &mut ConversationState) -> u32 {
    open_chat(&*list, &*conversation).map_or(0, |chat| chat.unread_count)
}

/// Re-derives the completion from the draft and the caret.
///
/// Called after every key the line answered, and never anywhere else. A
/// re-detection is a few microseconds, so there is nothing to be careful
/// about. The gate is [`PromptKind::is_buffer`] rather than
/// [`PromptKind::Message`]: a `:` command line and a `/` search line never
/// complete, but a reply and an edit do. It is also insert mode only,
/// because a shortcode is typed: the key that leaves insert for the line's
/// own normal mode is not editing the text, so it must not re-open what the
/// reader just put away.
///
/// The row the reader was on is carried across, clamped into the new list,
/// so typing one more character does not move them off a candidate that is
/// still there.
pub(crate) fn refresh_completion(ui: &mut UiState, input: &mut InputState) {
    if ui.focus != Focus::Input
        || !input.line.purpose().is_buffer()
        || input.line.status() != "INSERT"
    {
        input.dismiss_completion();
        return;
    }

    let selected = input.emoji.as_ref().map_or(0, |trigger| trigger.selected);
    input.redetect_completion(selected);
}

/// Asks to delete what the selection covers, or the message under the cursor
/// when there is none.
///
/// A `d` in Normal is `dd` in Vim: a selection of exactly the message under
/// the cursor, with no second press to distinguish. Two ways to say one thing
/// is exactly what the old latch existed to arbitrate, and a reader who
/// presses `dd` gets the same answer either way — which is why there is no
/// latch any more, and why the test that a motion between the two `d`s cleared
/// it is now a test that `j` and then `dd` deletes the message now under the
/// cursor.
///
/// Deletion is allowed on any real message, incoming included: Telegram
/// permits it, and a private chat does remove the other side's words.
pub(crate) fn request_delete(ui: &mut UiState, conversation: &mut ConversationState) {
    if conversation.selection.is_none() {
        // The mark goes on directly rather than through `App::select`: the
        // message came out of the window a line ago, so there is nothing to
        // check.
        let Some(id) = conversation.cursor_message_id() else {
            return;
        };
        conversation.set_selection(Selection::at(id, None));
    }

    confirm_delete(&mut *ui, &mut *conversation);
}

/// Starts a selection at the cursor's message, character-wise or whole.    ///
/// `Some(0)` is a charwise selection from the message's first character;
/// `None` is the whole message, which is what `V` selects. Both set the mode,
/// because this is the only way *into* Visual.
///
/// The mark goes on directly rather than through [`App::select`]: the message
/// came out of the window a line ago, so there is nothing to check.
pub(crate) fn begin_selection(
    ui: &mut UiState,
    conversation: &mut ConversationState,
    char: Option<usize>,
) {
    let Some(id) = conversation.cursor_message_id() else {
        return;
    };

    conversation.set_selection(Selection::at(id, char));
    ui.set_mode(Mode::Visual);
}
/// Where the unread messages start in a window that ends where the conversation
/// does.
///
/// Counted back from the end rather than looked up by identifier, which is what
/// makes the answer exact where the numbering has gaps: the unread messages are
/// the newest ones there are, so they are the last `unread` positions of the
/// window.
///
/// `None` when there is nothing unread, and when the unread messages reach past
/// the window — they start somewhere the client has not loaded, and counting
/// them from the end would land on a message that is not one of them.
pub(crate) fn landing_position(len: usize, unread: u32) -> Option<usize> {
    if unread == 0 {
        return None;
    }

    // A count that does not fit an index is far larger than any window, which
    // the comparison below settles without the conversion mattering.
    let unread = usize::try_from(unread).unwrap_or(usize::MAX);

    (unread <= len).then(|| len - unread)
}

/// Whether the window holds the whole conversation.
///
/// The broad reading — "both ends exhausted means the window holds the whole
/// conversation" — is false, because the window keeps its **newest**
/// [`CONVERSATION_WINDOW`] messages and drops the rest from the front: a
/// conversation whose ends have both been reached still holds no more than the
/// cap. Only both-ended **and** shorter than the cap means nothing was ever
/// dropped, which is precisely the conversation a full scan is cheapest in and a
/// round trip would only delay.
///
/// A conversation exactly at the cap is excluded conservatively: that is a
/// missed optimisation, not a wrong answer.
pub(crate) fn holds_everything(window: &ConversationWindow) -> bool {
    window.exhausted_older && window.exhausted_newer && window.len() < CONVERSATION_WINDOW
}

/// Whether a walk wrapped from one end of the match list to the other.
///
/// A single match is its own neighbour, so its "wrap" carries no information and
/// is not announced.
fn wrapped(before: Option<usize>, after: Option<usize>, len: usize) -> bool {
    if len <= 1 {
        return false;
    }

    (before == Some(len - 1) && after == Some(0)) || (before == Some(0) && after == Some(len - 1))
}

/// Puts the card about `subject` in the right-hand pane.
///
/// Returns true: a card always opens, and the highlight still needs sizing
/// over the rows the contact just stored. The [`crate::app::App`] delegate
/// sizes it afterwards, because [`crate::card::rows`] renders from `&App`.
pub(crate) fn open_card(
    profile: &mut ProfileCard,
    ui: &mut UiState,
    conversation: &mut ConversationState,
    outbox: &mut Outbox,
    subject: ProfileId,
) -> bool {
    // One read per card opened, asked for here rather than by the panel: the
    // panel draws what it has, and the reader asked a question by pressing `A`.
    let contact = match subject {
        ProfileId::User(peer_id) => {
            queue_action(&mut *outbox, &mut *ui, Action::FetchContact { peer_id });
            Some(ContactProfile {
                peer_id,
                state: AccountState::Unfetched,
            })
        }
        ProfileId::SelfAccount => None,
    };
    profile.open(subject, contact);
    ui.set_pane(Pane::Profile(subject));
    ui.set_focus(Focus::Conversation);
    ui.set_mode(Mode::Normal);
    conversation.clear_selection();
    true
}

/// Puts the card for whoever the chat list is highlighting, in the pane.
///
/// The highlight rather than the open conversation, so a reader can look
/// somebody up without opening them — which is the whole difference between a
/// chat list and a list of links, and the reason the chat list is one.
///
/// An empty list opens nothing: there is nobody to open, and a card about
/// nobody would fall back to the account's own, which is a card the reader did
/// not ask for.
pub(crate) fn open_contact(
    list: &ChatListState,
    profile: &mut ProfileCard,
    ui: &mut UiState,
    conversation: &mut ConversationState,
    outbox: &mut Outbox,
) -> bool {
    // From the chat list it is the highlight, and from the conversation it is
    // the open one: the two are the same value, because opening a conversation
    // is what moves the highlight to it.
    let Some(chat) = list.list.chats.get(list.selected_chat) else {
        return false;
    };
    open_card(profile, ui, conversation, outbox, ProfileId::User(chat.id))
}

/// Puts the profile in the right-hand pane.
///
/// Sizes the highlight from whatever rows exist, so an empty panel has
/// nothing to move a highlight over rather than a highlight on nothing.
pub(crate) fn open_profile(
    profile: &mut ProfileCard,
    ui: &mut UiState,
    conversation: &mut ConversationState,
    outbox: &mut Outbox,
) -> bool {
    open_card(profile, ui, conversation, outbox, ProfileId::SelfAccount)
}

/// Starts a card selection at the inline position, or extends the one there is.
pub(crate) fn start_card_visual(profile: &mut ProfileCard) {
    let row = profile.card_row_id();
    profile.start_visual(row);
}

/// One charwise motion within the cursor row's value over already-drawn rows.
pub(crate) fn card_motion_char(profile: &mut ProfileCard, rows: &[CardRow], c: char) {
    let Some(motion) = CharMotion::from_key(c) else {
        return;
    };
    let Some(value) = rows
        .get(profile.profile_vim.cursor())
        .map(|row| row.value.clone())
    else {
        return;
    };
    profile.move_char(motion, &value);
}

/// A motion between rows over already-drawn rows, which resets the inline position.
pub(crate) fn card_motion_row(profile: &mut ProfileCard, rows: &[CardRow], c: char) {
    // `handle_char` applies the motion *and* returns it, so calling
    // `apply_motion` on the result would move the row twice — which is a bug
    // that looks like a card with one more row than it has.
    profile.profile_vim.handle_char(c);
    off_reserved(profile, rows, matches!(c, 'j' | 'G'));
    profile.set_caret(0);
}

/// Steps the row cursor off a held slot in already-drawn rows.
pub(crate) fn off_reserved(profile: &mut ProfileCard, rows: &[CardRow], forward: bool) {
    let last = rows.len().saturating_sub(1);
    let start = profile.profile_vim.cursor().min(last);
    if !rows
        .get(start)
        .is_some_and(crate::card::CardRow::is_reserved)
    {
        return;
    }

    for ahead in [forward, !forward] {
        let mut cursor = start;
        for _ in 0..rows.len() {
            if !rows
                .get(cursor)
                .is_some_and(crate::card::CardRow::is_reserved)
            {
                profile.profile_vim.set_cursor(cursor);
                return;
            }
            cursor = if ahead {
                cursor + 1
            } else {
                cursor.saturating_sub(1)
            };
        }
    }
}

/// The conversation on show, as the chat list holds it.
///
/// Looked up by the window's own identifier rather than by the selected
/// index: the two agree, and the window is what every question here is about.
pub(crate) fn open_chat<'a>(
    list: &'a ChatListState,
    conversation: &ConversationState,
) -> Option<&'a Chat> {
    let chat_id = conversation.conversation.window.chat_id;
    list.list.chats.iter().find(|chat| chat.id == chat_id)
}

/// Handles the keys a completion takes while it is up.
///
/// Answers whether the key was consumed. Only four keys are: `Up`/`Down`
/// move the candidate, `Tab` and `Enter` accept, and `Esc` puts the
/// completion away. Everything else is passed through to the line.
pub(crate) fn handle_completion(input: &mut InputState, ui: &mut UiState, key: KeyEvent) -> bool {
    if input.emoji.is_none() || key.modifiers != KeyModifiers::NONE {
        return false;
    }

    match key.code {
        KeyCode::Up => input.move_completion(false),
        KeyCode::Down => input.move_completion(true),
        KeyCode::Tab | KeyCode::Enter => {
            if input.accept_completion() == LineVerdict::TooLong {
                ui.flash("message is too long");
            }
        }
        KeyCode::Esc => input.dismiss_completion(),
        _ => return false,
    }

    true
}

/// `y` on a card over already-yanked lines, which yanks one of two things.
///
/// A selection inside one row is the selected characters, and a selection
/// across rows is one line per row, oldest first. The register is also offered
/// to the system clipboard, best-effort.
pub(crate) fn yank_card(
    profile: &mut ProfileCard,
    ui: &mut UiState,
    outbox: &mut Outbox,
    conversation: &mut ConversationState,
    lines: Vec<String>,
) {
    profile.clear_visual();

    if lines.iter().all(String::is_empty) {
        ui.flash("nothing to yank — move the selection first");
        return;
    }

    conversation.set_register(Register::set(lines));
    outbox.store_clipboard(conversation.register.text());
}

/// Acts on the profile row under `cursor` in already-drawn rows.
pub(crate) fn activate_profile_row(
    ui: &mut UiState,
    conversation: &mut ConversationState,
    is_contact: bool,
    rows: &[CardRow],
    cursor: usize,
) {
    // A contact's card has no row that acts, and that is decided by *who it is
    // about* rather than by which row the cursor is on — so the refusal comes
    // before the row is looked for. A card whose profile has not been read has
    // no rows at all, and a key that means "you cannot" has to say so there
    // too: swallowing it is the one thing such a key must not do.
    if is_contact {
        ui.flash(NOT_YOURS_REFUSAL);
        return;
    }

    // The row is whatever the drawn rows say the panel is drawing, rather than
    // a second enumeration of it. A key that acted on its own list could act
    // on a row the panel is not showing, and that failure is silent: no
    // message, no wrong frame, just a key that did the wrong thing.
    let Some(label) = rows
        .get(cursor)
        .filter(|row| row.is_action())
        .map(|row| row.label)
    else {
        return;
    };
    match label {
        // A deliberate refusal rather than a failure. The bracketed
        // `[failed: …]` form is for something that tried and did not come
        // back, and nothing was sent.
        crate::card::ADD_ACCOUNT => ui.flash(ADD_ACCOUNT_REFUSAL),
        // Confirm first and refuse inside the confirmation. A panel that
        // flashes instead of confirming has taught the reader the wrong
        // thing about a key that will eventually discard the one secret this
        // program holds.
        crate::card::LOGOUT => {
            ui.set_mode(Mode::Confirm);
            conversation.set_confirm(Some(ConfirmKind::Logout));
        }
        _ => {}
    }
}

/// Moves the cursor a screenful over a precomputed layout.
///
/// A screenful is rows, and a page lands on a message: moving down can
/// arrive in the middle of one, and the message that owns the row the
/// reader asked for is what the cursor stands on — its first row, as a
/// terminal page puts the reader at the top of what it moved to.
///
/// Landing on the newest message re-engages following and moving away from
/// it disengages, on the same rule as `j` and `k`, so a page and a line
/// cannot disagree about whether the view is pinned.
pub(crate) fn page(
    ui: &UiState,
    conversation: &mut ConversationState,
    layout: &[RowSpan],
    down: bool,
) {
    let step = ui.metrics.rows.get().max(1);
    let total = rows::total_rows(layout);
    let here = rows::first_row_of_message(layout, conversation.vim.cursor()).unwrap_or(0);

    let target = if down {
        here.saturating_add(step).min(total.saturating_sub(1))
    } else {
        here.saturating_sub(step)
    };

    // A row that names no message — a day separator — is not somewhere the
    // cursor stops, so the page carries on past it in the direction it was
    // going rather than landing on it.
    if let Some(cursor) = rows::message_at_row_moving(layout, target, down) {
        conversation.vim.set_cursor(cursor);
    }
    conversation.settle_follow();
}

/// Handles `Enter` in the input line.
///
/// Which prompt it is decides what is handed over; the buffer and the reply
/// context are cleared either way, because the work leaves here rather than
/// happening here. The focus goes back to the conversation for the same
/// reason: the line has given up what it was for.
///
/// The text is taken *after* the check that a send is allowed, so a send
/// refused here leaves the words in the bar rather than taking them out of
/// it — the reader would otherwise lose a message they had already written
/// to a line that was already refusing.
///
/// Returns true when a card was opened and its highlight still needs sizing.
///
/// The one command that leaves the line as something else is `:signin`,
/// which opens a field rather than finishing on the conversation: that one
/// returns before the reset, because the reset is what would empty it.
pub(crate) fn submit(
    ui: &mut UiState,
    conversation: &mut ConversationState,
    input: &mut InputState,
    outbox: &mut Outbox,
    pending: &mut Pending,
    session: &mut SessionState,
    list: &mut ChatListState,
    drafts: &mut DraftStore,
    profile: &mut ProfileCard,
) -> bool {
    let opened = match input.line.purpose() {
        PromptKind::Message | PromptKind::Reply => {
            submit_message(
                &mut *ui,
                &mut *conversation,
                &mut *input,
                &mut *outbox,
                &mut *drafts,
            );
            false
        }
        PromptKind::Edit => {
            submit_edit(
                &mut *ui,
                &mut *conversation,
                &mut *input,
                &mut *outbox,
                &mut *drafts,
            );
            false
        }
        PromptKind::Command => {
            let cmd = input.line.take();
            run_command(
                &mut *ui,
                &mut *list,
                &mut *outbox,
                &mut *pending,
                &mut *conversation,
                &mut *input,
                &mut *drafts,
                &mut *profile,
                session,
                cmd.trim(),
            )
        }
        PromptKind::Search => {
            let query = input.line.take();
            run_search(&mut *ui, &mut *conversation, &mut *outbox, query.trim());
            false
        }
        PromptKind::NewChat => {
            let query = input.line.take();
            submit_new_chat(&mut *ui, &mut *conversation, &mut *outbox, query.trim());
            false
        }
        // Unreachable: a sign-in field answers `Enter` itself, so that its
        // `⏎` can be refused while a request is on its way — which a submit
        // with no way to refuse is.
        PromptKind::Phone | PromptKind::Code | PromptKind::Password => false,
    };

    // A sign-in field is what the line is now: `:signin` opened it
    // pre-filled, and the reset below belongs to the commands that finish
    // on the conversation. Clearing it would empty the field the reader
    // came for, and handing the focus to the conversation would route
    // their keys to an arm with nothing to say about a flow in progress.
    // `:new` keeps its query line for the same reason: the reader is about
    // to type into it, and the reset below would empty it.
    if session.signin_field().is_some() || input.line.purpose() == PromptKind::NewChat {
        conversation.set_reply_to(None);
        conversation.set_editing(None);
        return opened;
    }

    ui.set_focus(Focus::Conversation);
    input.line.clear();
    conversation.set_reply_to(None);
    conversation.set_editing(None);
    opened
}

/// Runs a `:` command.
///
/// Returns true when a card was opened and its highlight still needs sizing.
pub(crate) fn run_command(
    ui: &mut UiState,
    list: &mut ChatListState,
    outbox: &mut Outbox,
    pending: &mut Pending,
    conversation: &mut ConversationState,
    input: &mut InputState,
    drafts: &mut DraftStore,
    profile: &mut ProfileCard,
    session: &mut SessionState,
    cmd: &str,
) -> bool {
    match cmd {
        "q" | "quit" => {
            request_quit(&mut *ui, &mut *conversation);
            false
        }
        "settings" => open_profile(&mut *profile, &mut *ui, &mut *conversation, &mut *outbox),
        // The same path from the signed-out card and from a launch with no
        // session: they are the same question, and two entry points would be
        // two flows that could come to differ.
        "signin" => {
            begin_signin(
                &mut *ui,
                &mut *session,
                &mut *conversation,
                &mut *input,
                &mut *profile,
            );
            false
        }
        "retry" => {
            request_retry(&mut *ui, &mut *session);
            false
        }
        // The explicit route to the same prompt `/` opens from the chat
        // list. It opens the line rather than searching at once, so the
        // reader can edit the query before it goes anywhere.
        "new" => {
            begin_new_chat(&mut *ui, &mut *input, "");
            false
        }
        _ if cmd.starts_with("new ") => {
            begin_new_chat(&mut *ui, &mut *input, cmd[4..].trim());
            false
        }
        _ if cmd.starts_with("chat ") => {
            if let Ok(id) = cmd[5..].trim().parse::<i64>() {
                // Unknown ids stay a silent no-op here: only the launch
                // selection says where it landed instead.
                select_chat_by_id(
                    &mut *ui,
                    &mut *list,
                    &mut *outbox,
                    &mut *pending,
                    &mut *conversation,
                    &mut *input,
                    &mut *drafts,
                    id,
                );
            }
            false
        }
        _ => {
            ui.set_status(format!("unknown command: :{cmd}"));
            false
        }
    }
}

/// Handles a key while the line has the focus.
///
/// The line decides what the key means and says what the host has to do;
/// this only carries out the two answers that are the host's. Everything
/// else — motions, quick edits, a selection, the two stages of `Esc` — is
/// answered inside the line wrapper rather than a `String`.
///
/// Returns true when a card was opened and its highlight still needs sizing.
pub(crate) fn handle_line(
    ui: &mut UiState,
    conversation: &mut ConversationState,
    input: &mut InputState,
    outbox: &mut Outbox,
    pending: &mut Pending,
    session: &mut SessionState,
    list: &mut ChatListState,
    drafts: &mut DraftStore,
    profile: &mut ProfileCard,
    key: KeyEvent,
) -> bool {
    let opened = match input.line.feed(key) {
        LineVerdict::Submit => submit(
            &mut *ui,
            &mut *conversation,
            &mut *input,
            &mut *outbox,
            &mut *pending,
            session,
            &mut *list,
            &mut *drafts,
            &mut *profile,
        ),
        LineVerdict::LeftEditing => {
            leave_line(&mut *ui, &mut *conversation, &mut *input, &mut *profile);
            false
        }
        LineVerdict::TooLong => {
            ui.flash("message is too long");
            false
        }
        LineVerdict::Refused => {
            ui.flash("that motion on non-ASCII text is not built yet");
            false
        }
        LineVerdict::Edited | LineVerdict::Ignored => false,
    };

    // A yank in the line is a yank: the same slot, the same drain, and the
    // same OSC 52 write the conversation's goes through. One seam, two
    // producers.
    if let Some(yanked) = input.line.take_yanked() {
        outbox.store_clipboard(yanked);
    }

    // Last, because it is a function of what the line now holds: deriving
    // rather than maintaining is what keeps the popup from describing a
    // fragment the reader has already typed past.
    refresh_completion(&mut *ui, &mut *input);
    opened
}

/// A key with a sign-in field open.
///
/// `Enter` submits, `Esc` gives up the step — or the flow, at the phone step,
/// where the step *is* the flow — and everything else is the
/// line's: an insert-only prompt, so there is no line's Normal mode to
/// leave and the editor answers insert keys the same way it does for a
/// command line.
///
/// Returns true when a card was opened and its highlight still needs sizing.
pub(crate) fn handle_signin_field(
    session: &mut SessionState,
    input: &mut InputState,
    ui: &mut UiState,
    outbox: &mut Outbox,
    pending: &mut Pending,
    conversation: &mut ConversationState,
    profile: &mut ProfileCard,
    list: &mut ChatListState,
    drafts: &mut DraftStore,
    key: KeyEvent,
) -> bool {
    let field = session.signin_field();
    let control = key.modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        KeyCode::Enter => {
            submit_signin_field(
                &mut *session,
                &mut *input,
                &mut *ui,
                &mut *outbox,
                &mut *conversation,
                &mut *profile,
            );
            false
        }
        // `Esc` at the two steps that hold a secret asks Telegram for a new
        // code rather than leaving a typed password in a dimmed bar. At the
        // phone step there is nothing to discard but the flow itself, and
        // the hint names that key `cancel` — so it goes, rather than
        // pausing into an overlay every other key is swallowed by.
        KeyCode::Esc if matches!(field, Some(LoginField::Code | LoginField::Password)) => {
            signin_cancel(
                &mut *session,
                &mut *input,
                &mut *ui,
                &mut *outbox,
                &mut *conversation,
                &mut *profile,
            );
            false
        }
        // `Esc` at the phone step: the flow is done with, and so is the
        // surface. A pause would be a trap here, so the shape is
        // `login_complete`'s rather than a second copy of it.
        KeyCode::Esc => {
            login_complete(
                &mut *session,
                &mut *input,
                &mut *ui,
                &mut *conversation,
                &mut *profile,
            );
            false
        }
        KeyCode::Tab | KeyCode::BackTab => {
            signin_away(
                &mut *session,
                &mut *input,
                &mut *ui,
                &mut *conversation,
                &mut *profile,
            );
            false
        }
        _ if control && key.code == KeyCode::Char('w') => {
            signin_away(
                &mut *session,
                &mut *input,
                &mut *ui,
                &mut *conversation,
                &mut *profile,
            );
            false
        }
        _ => handle_line(
            &mut *ui,
            &mut *conversation,
            &mut *input,
            &mut *outbox,
            &mut *pending,
            &mut *session,
            &mut *list,
            &mut *drafts,
            &mut *profile,
            key,
        ),
    }
}

/// A key while the sign-in surface is up.
///
/// Returns true when a card was opened and its highlight still needs sizing.
pub(crate) fn handle_signin(
    session: &mut SessionState,
    ui: &mut UiState,
    conversation: &mut ConversationState,
    input: &mut InputState,
    outbox: &mut Outbox,
    profile: &mut ProfileCard,
    pending: &mut Pending,
    list: &mut ChatListState,
    drafts: &mut DraftStore,
    key: KeyEvent,
) -> bool {
    match ui.focus {
        Focus::Input => handle_signin_field(
            &mut *session,
            &mut *input,
            &mut *ui,
            &mut *outbox,
            &mut *pending,
            &mut *conversation,
            &mut *profile,
            &mut *list,
            &mut *drafts,
            key,
        ),
        // Paused, or waiting for an answer. Nothing here answers anything
        // except the way back and the stale session's offer.
        Focus::ChatList => {
            handle_signin_away(
                &mut *session,
                &mut *input,
                &mut *ui,
                &mut *outbox,
                &mut *conversation,
                &mut *profile,
                key,
            );
            false
        }
        // The no-credentials sentence: no field, so no flow to pause, and
        // the two keys it answers are the two the shell cards name.
        Focus::Conversation => {
            if session.signin.as_ref() == Some(&SignIn::NoCredentials) {
                match key.code {
                    KeyCode::Char('q') => {
                        request_quit(&mut *ui, &mut *conversation);
                        false
                    }
                    // A command line is the way to `:q` from a shell that has
                    // no session and no chat, so the key has to answer here
                    // rather than in a focus that does not exist yet.
                    KeyCode::Char(':') => {
                        session.clear_signin();
                        input.line.open(PromptKind::Command);
                        set_focus(
                            &mut *ui,
                            &mut *conversation,
                            &mut *input,
                            &mut *profile,
                            Focus::Input,
                        );
                        false
                    }
                    _ => false,
                }
            } else {
                false
            }
        }
    }
}

/// `Ctrl` chords over the conversation: paging and jumps.
///
/// A screenful at a time, which is what a terminal scrolls by. Bound here
/// rather than in the motion table because how much a page is depends on
/// how tall the panel turned out to be.
//
fn handle_normal_ctrl(
    ui: &mut UiState,
    pending: &mut Pending,
    conversation: &mut ConversationState,
    layout: &[RowSpan],
    key: KeyEvent,
) {
    match key.code {
        KeyCode::Char('d') => page(&*ui, &mut *conversation, layout, true),
        KeyCode::Char('u') => page(&*ui, &mut *conversation, layout, false),
        // `Ctrl-o` and `Ctrl-i`: back and forward through the places the
        // reader has jumped from. Not while a jump is on its way — one
        // fetch is in flight and the reader may have escaped it, and a
        // second jump would replace the one they are still waiting for.
        KeyCode::Char('o') if pending.pending_jump.is_none() => {
            jump_back(&mut *pending, &mut *conversation);
        }
        KeyCode::Char('i') if pending.pending_jump.is_none() => {
            jump_forward(&mut *pending, &mut *conversation);
        }
        _ => {}
    }
}

/// Normal-mode keys over the conversation.
///
/// Structure: `Ctrl` chords for paging and jumps, then the vim motion table,
/// then the plain-character match. Dispatch order and bindings are unchanged.
///
/// Returns true when a card was opened and its highlight still needs sizing.
pub(crate) fn handle_normal(
    ui: &mut UiState,
    pending: &mut Pending,
    conversation: &mut ConversationState,
    input: &mut InputState,
    list: &mut ChatListState,
    outbox: &mut Outbox,
    profile: &mut ProfileCard,
    layout: &[RowSpan],
    key: KeyEvent,
) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        handle_normal_ctrl(&mut *ui, &mut *pending, &mut *conversation, layout, key);
        return false;
    }

    // Every plain character goes to the motion table before anything else:
    // `gd` is a two-key sequence built on the same `g` as `gg`, and the
    // table owns that prefix. A character that never reached it left the `g`
    // armed for whatever key came next, so `g` then `d` then `G` deleted a
    // message instead of going to the end.
    let KeyCode::Char(c) = key.code else {
        return false;
    };

    if let Some(motion) = conversation.vim.handle_char(c) {
        match motion {
            // `gg` is where the unread messages start when there are
            // any, and the top of what is loaded when there are not. A
            // jump the window can answer is taken here; one it cannot is
            // left for the caller to fetch. Either way the reader has
            // asked for something, so whatever they asked for before is
            // replaced by it.
            Motion::First => {
                let jump = jump_to_unread(&mut *list, &mut *conversation, &mut *ui);
                pending.set_jump(jump);
            }

            // `G` is the reader overriding a jump with "take me to the
            // end". The page on its way is for a place they no longer
            // want to be, and it is dropped when it lands.
            Motion::Last => pending.set_jump(None),

            // `n` and `N` walk the search's matches. `VimState` reports
            // the motion but cannot answer it, because a match is a
            // place in a conversation and the list of them lives here.
            Motion::NextMatch => walk_search(&mut *pending, &mut *conversation, &mut *ui, true),
            Motion::PrevMatch => walk_search(&mut *pending, &mut *conversation, &mut *ui, false),

            // `gd`, likewise: which message this one quotes is a fact
            // about the conversation, and the window is what holds it.
            Motion::GotoReply => {
                let jump = jump_to_reply(&mut *list, &mut *conversation, &mut *ui);
                pending.set_jump(jump);
            }

            // `j` and `k` are the motions themselves, and the table has
            // already applied them.
            Motion::Down | Motion::Up => {}
        }

        conversation.settle_follow();
        return false;
    }

    match c {
        'i' | 'a' => {
            start_compose(&mut *ui, &mut *conversation, &mut *input);
            false
        }
        'r' => {
            start_reply(&mut *ui, &mut *conversation, &mut *input);
            false
        }
        'e' => {
            start_edit(&mut *ui, &mut *conversation, &mut *input);
            false
        }
        // A plain `d`: the table reports nothing for it, so a `d` that does
        // not follow a `g` deletes here.
        'd' => {
            request_delete(&mut *ui, &mut *conversation);
            false
        }
        'p' => {
            paste(&mut *ui, &mut *conversation, &mut *input);
            false
        }
        'D' => {
            conversation.dismiss_failed_at_cursor();
            false
        }
        'v' => {
            begin_selection(&mut *ui, &mut *conversation, Some(0));
            false
        }
        'V' => {
            begin_selection(&mut *ui, &mut *conversation, None);
            false
        }
        // The list is beside the conversation, so `h` is how the reader gets
        // to it. `l` has nothing to move to from here and is left unbound
        // rather than made to wrap.
        'h' => {
            set_focus(
                &mut *ui,
                &mut *conversation,
                &mut *input,
                &mut *profile,
                Focus::ChatList,
            );
            false
        }
        '/' => {
            ui.set_focus(Focus::Input);
            input.line.open(PromptKind::Search);
            false
        }
        ':' => {
            ui.set_focus(Focus::Input);
            input.line.open(PromptKind::Command);
            false
        }
        'q' => {
            request_quit(&mut *ui, &mut *conversation);
            false
        }
        'S' => open_profile(&mut *profile, &mut *ui, &mut *conversation, &mut *outbox),
        'A' => open_contact(
            &*list,
            &mut *profile,
            &mut *ui,
            &mut *conversation,
            &mut *outbox,
        ),
        _ => false,
    }
}

/// Handles a key while the chat list has the focus.
///
/// `j`, `k`, `gg` and `G` move the highlight and record the conversation it
/// now names; `Enter` opens it at once, because a reader who presses it is
/// not going to press anything else. `h` and `l` are the pane movement, and
/// both mean the same thing from here: the conversation is the only pane
/// beside this one, so there is nothing for the two of them to choose
/// between.
///
/// Returns true when a card was opened and its highlight still needs sizing.
pub(crate) fn handle_chat_list(
    list: &mut ChatListState,
    pending: &mut Pending,
    conversation: &mut ConversationState,
    ui: &mut UiState,
    input: &mut InputState,
    profile: &mut ProfileCard,
    outbox: &mut Outbox,
    drafts: &mut DraftStore,
    client_available: bool,
    key: KeyEvent,
) -> bool {
    let here = list.selected_chat;
    let bottom = list.list.chats.len().saturating_sub(1);

    match key.code {
        // The list is the only pane beside this one, so both keys are the
        // way into it.
        KeyCode::Char('h' | 'l') => {
            set_focus(
                &mut *ui,
                &mut *conversation,
                &mut *input,
                &mut *profile,
                Focus::Conversation,
            );
            false
        }

        // `/` here finds a person rather than a message: the conversation's
        // own `/` searches the window, and there is no window to hand this
        // one to, so the list gets its own search instead of borrowing a
        // scope it does not have.
        KeyCode::Char('/') => {
            pending.set_g(false);
            begin_new_chat(&mut *ui, &mut *input, "");
            false
        }

        // The account's own card, from the list as well as from the
        // conversation: a reader looking for settings has usually not opened a
        // conversation to look in.
        KeyCode::Char('S') => {
            pending.set_g(false);
            open_profile(&mut *profile, &mut *ui, &mut *conversation, &mut *outbox)
        }

        // `p` pins the highlighted chat, or unpins it. The list does not move
        // until Telegram has agreed, so a refused pin leaves it where it was.
        KeyCode::Char('p') => {
            pending.set_g(false);
            toggle_pin_highlighted(&*list, &mut *ui, &mut *outbox, client_available);
            false
        }

        // The contact the highlight is on. `A` and not `l`, because `l` is
        // already the way into the conversation on this pane, and a key that
        // means two things in two panes is a key a reader has to learn twice.
        KeyCode::Char('A') => {
            pending.set_g(false);
            open_contact(
                &*list,
                &mut *profile,
                &mut *ui,
                &mut *conversation,
                &mut *outbox,
            )
        }

        KeyCode::Char('j') => {
            pending.set_g(false);
            choose_chat(
                &mut *list,
                &mut *pending,
                here.saturating_add(1).min(bottom),
            );
            false
        }
        KeyCode::Char('k') => {
            pending.set_g(false);
            choose_chat(&mut *list, &mut *pending, here.saturating_sub(1));
            false
        }
        KeyCode::Char('g') => {
            if std::mem::take(&mut pending.pending_g) {
                choose_chat(&mut *list, &mut *pending, 0);
            } else {
                pending.set_g(true);
            }
            false
        }
        KeyCode::Char('G') => {
            pending.set_g(false);
            choose_chat(&mut *list, &mut *pending, bottom);
            false
        }

        KeyCode::Enter => {
            pending.set_g(false);
            select_chat(
                &mut *ui,
                &mut *list,
                &mut *outbox,
                &mut *pending,
                &mut *conversation,
                &mut *input,
                &mut *drafts,
                here,
            );
            set_focus(
                &mut *ui,
                &mut *conversation,
                &mut *input,
                &mut *profile,
                Focus::Conversation,
            );
            false
        }

        // Any other key ends the sequence, so a lone `g` does not become a
        // jump to the top the next time one is pressed.
        _ => {
            pending.set_g(false);
            false
        }
    }
}

/// Asks for the highlighted chat's pin to flip.
///
/// Refuses with a flash when no client is up, the way a sign-in step does, and
/// queues nothing: a queued pin would fire on its own if a client came up later.
fn toggle_pin_highlighted(
    list: &ChatListState,
    ui: &mut UiState,
    outbox: &mut Outbox,
    client_available: bool,
) {
    let Some(chat) = list.list.chats.get(list.selected_chat) else {
        return;
    };
    if !client_available {
        ui.flash("not connected yet — the client is not up");
        return;
    }
    queue_action(
        outbox,
        ui,
        Action::TogglePin {
            chat_id: chat.id,
            pinned: !chat.pinned,
        },
    );
}

/// Handles a key while the profile has the focus.
///
/// `h` goes to the chat list and `l` and `Esc` come back to the
/// conversation, and `j`/`k`/`gg`/`G` move the highlight. A key that is none
/// of those is the conversation's, and taking it is what stops a reader who
/// pressed `i` by reflex from having to press it twice.
///
/// `rows`, `lines`, `is_contact` and `cursor` are the card's drawn state,
/// precomputed by the caller from `&App`: the rows render from the whole
/// application and cannot move here. `layout` is the conversation's
/// precomputed frame geometry, for the same reason.
///
/// Returns true when a card was opened and its highlight still needs sizing.
pub(crate) fn handle_profile(
    profile: &mut ProfileCard,
    ui: &mut UiState,
    conversation: &mut ConversationState,
    outbox: &mut Outbox,
    list: &mut ChatListState,
    pending: &mut Pending,
    input: &mut InputState,
    rows: &[CardRow],
    lines: Vec<String>,
    is_contact: bool,
    cursor: usize,
    layout: &[RowSpan],
    key: KeyEvent,
) -> bool {
    // `Ctrl-w` is a prefix here, and bare it still leaves the input line: the
    // same prefix-with-a-bare-fallback shape `g`/`gg` already has.
    if profile.profile_pending_w {
        profile.profile_pending_w = false;
        match key.code {
            KeyCode::Char('h') => {
                set_focus(
                    &mut *ui,
                    &mut *conversation,
                    &mut *input,
                    &mut *profile,
                    Focus::ChatList,
                );
                return false;
            }
            // Nothing is drawn to the right of a card, so `Ctrl-w l` has no
            // destination. It is named nowhere on the card's hint for that
            // reason, and saying so here is what keeps the key from looking
            // broken to a reader who tries it.
            KeyCode::Char('l') => {
                ui.flash("nothing to the right of a card");
                return false;
            }
            _ => return false,
        }
    }

    match key.code {
        KeyCode::Esc => {
            escape_card(&mut *ui, &mut *profile);
            false
        }
        // `h` at the first cell is the way back, because that is where the
        // inline position has nowhere left to go. A card is one column of
        // values, so there is no column to the left of the first one, so the
        // motion is at its edge rather than the key being repurposed mid-word.
        KeyCode::Char('h') if profile.card_caret_at_start() => {
            close_card(&mut *ui, &mut *profile);
            false
        }
        KeyCode::Char(c @ ('l' | 'h' | 'w' | 'b' | 'e' | '0' | '$')) => {
            card_motion_char(&mut *profile, rows, c);
            false
        }
        KeyCode::Char(c @ ('j' | 'k' | 'g' | 'G')) => {
            card_motion_row(&mut *profile, rows, c);
            false
        }
        // `v`, and `j` to reach further. There is no `V`: one gesture for one
        // thing is one thing to learn, and the design's own entry table has
        // only `v` on a card — `V` there is the *mode* label for Visual. A
        // selection that grows to a second row becomes a set of whole rows by
        // itself, which is what rowwise meant.
        KeyCode::Char('v') => {
            start_card_visual(&mut *profile);
            false
        }
        KeyCode::Char('y') => {
            yank_card(
                &mut *profile,
                &mut *ui,
                &mut *outbox,
                &mut *conversation,
                lines,
            );
            false
        }
        KeyCode::Char('d') => {
            activate_profile_row(&mut *ui, &mut *conversation, is_contact, rows, cursor);
            false
        }
        KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
            profile.card_count(c);
            false
        }
        _ => {
            close_profile(&mut *ui, &mut *profile);
            handle_normal(
                &mut *ui,
                &mut *pending,
                &mut *conversation,
                &mut *input,
                &mut *list,
                &mut *outbox,
                &mut *profile,
                layout,
                key,
            )
        }
    }
}

/// Accepts the person the overlay is on.
///
/// With nothing to accept this does nothing, which is what an empty list
/// means. Otherwise the choice goes to `open_user`.
pub(crate) fn accept_user_search(
    ui: &mut UiState,
    list: &mut ChatListState,
    outbox: &mut Outbox,
    pending: &mut Pending,
    conversation: &mut ConversationState,
    input: &mut InputState,
    drafts: &mut DraftStore,
    profile: &mut ProfileCard,
) {
    let Some(candidate) = conversation.user_search.selected_candidate().cloned() else {
        return;
    };

    open_user(
        &mut *ui,
        &mut *list,
        &mut *outbox,
        &mut *pending,
        &mut *conversation,
        &mut *input,
        &mut *drafts,
        &mut *profile,
        &candidate,
    );
}

/// Handles a key while the new-conversation overlay is up.
///
/// Answers whether the key was consumed. Only the list's own keys are: the
/// arrows and `j`/`k` move the highlight, `Enter` accepts the person on it,
/// and `Esc` puts the list away. Everything else is passed through to the
/// pane, because the overlay is a short list drawn over the chat list and
/// not a mode of its own.
pub(crate) fn handle_user_search(
    ui: &mut UiState,
    list: &mut ChatListState,
    outbox: &mut Outbox,
    pending: &mut Pending,
    conversation: &mut ConversationState,
    input: &mut InputState,
    drafts: &mut DraftStore,
    profile: &mut ProfileCard,
    key: KeyEvent,
) -> bool {
    if !conversation.user_search.is_active() || key.modifiers != KeyModifiers::NONE {
        return false;
    }

    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            conversation.move_user_selection(false);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            conversation.move_user_selection(true);
        }
        KeyCode::Enter => accept_user_search(
            &mut *ui,
            &mut *list,
            &mut *outbox,
            &mut *pending,
            &mut *conversation,
            &mut *input,
            &mut *drafts,
            &mut *profile,
        ),
        KeyCode::Esc => conversation.dismiss_user_search(),
        _ => return false,
    }

    true
}

/// Keys something else owns before the panes see them: a jump in flight, the
/// completion, the new-conversation overlay.
///
/// True when the key was answered here and the panes never see it.
fn intercept_key(
    ui: &mut UiState,
    pending: &mut Pending,
    list: &mut ChatListState,
    outbox: &mut Outbox,
    conversation: &mut ConversationState,
    input: &mut InputState,
    drafts: &mut DraftStore,
    profile: &mut ProfileCard,
    key: KeyEvent,
) -> bool {
    // A jump in flight is a page on its way to replace the window under the
    // reader, so nothing else acts until it lands: every other key is
    // swallowed rather than answered, because a key that moves the cursor
    // would move it out from under the page. `Esc` is the one answer — it
    // drops the jump and leaves the reader where they were. The page may
    // still land, and is dropped when it does: nobody is waiting for it.
    if pending.pending_jump.is_some() {
        if key.code == KeyCode::Esc {
            pending.set_jump(None);
        }
        return true;
    }

    // A completion owns a few keys for as long as it is up. `Ctrl-C` above
    // stays first: a reader reaching for it to abandon a half-typed
    // shortcode gets out of the program, which is what they asked for.
    if handle_completion(&mut *input, &mut *ui, key) {
        return true;
    }

    // The new-conversation overlay owns the keys that would otherwise move
    // the pane under it, for as long as it is open: `j`/`k` and the arrows
    // walk the candidates, `Enter` accepts one, and `Esc` puts the list away.
    // It is skipped while the line has the focus, because the same letters
    // have to type into a fresh query. Every other key is passed through:
    // the overlay is a short list drawn over the chat list, not a mode.
    if ui.focus != Focus::Input
        && handle_user_search(
            &mut *ui,
            &mut *list,
            &mut *outbox,
            &mut *pending,
            &mut *conversation,
            &mut *input,
            &mut *drafts,
            &mut *profile,
            key,
        )
    {
        return true;
    }

    false
}

/// Pane movement, the one thing every pane answers the same way.
///
/// True when the key moved the focus and nothing else answers it.
fn handle_pane_key(
    ui: &mut UiState,
    conversation: &mut ConversationState,
    input: &mut InputState,
    profile: &mut ProfileCard,
    key: KeyEvent,
) -> bool {
    // Pane movement is the one thing every pane answers the same way, so it
    // is read here rather than bound in each of them.
    match key.code {
        KeyCode::Tab => {
            cycle_focus(
                &mut *ui,
                &mut *conversation,
                &mut *input,
                &mut *profile,
                true,
            );
            true
        }
        KeyCode::BackTab => {
            cycle_focus(
                &mut *ui,
                &mut *conversation,
                &mut *input,
                &mut *profile,
                false,
            );
            true
        }
        // `Ctrl-w` is the input line's way out, and a card's prefix for pane
        // movement — because the card's own `h`/`l` are an inline motion and a
        // key that is a motion in one place and a pane in the next is a key a
        // reader has to learn twice. The prefix is armed only on a card, so on
        // every other pane `Ctrl-w` is still exactly what it was.
        _ if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('w') => {
            if ui.pane.is_profile() {
                profile.profile_pending_w = true;
            } else {
                leave_line(&mut *ui, &mut *conversation, &mut *input, &mut *profile);
            }
            true
        }
        _ => false,
    }
}

/// Keys the top level answers before any pane: `Ctrl-C`, the confirmation,
/// the sign-in surface.
///
/// `Some` when the key was answered here, `None` when the panes see it.
fn preempt_key(
    ui: &mut UiState,
    session: &mut SessionState,
    conversation: &mut ConversationState,
    input: &mut InputState,
    outbox: &mut Outbox,
    profile: &mut ProfileCard,
    pending: &mut Pending,
    list: &mut ChatListState,
    drafts: &mut DraftStore,
    key: KeyEvent,
) -> Option<bool> {
    // Ctrl-C always quits.
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        ui.quit();
        return Some(false);
    }

    // A confirmation is a question about the whole screen rather than about
    // a pane, so it outranks the focus: it has to be answered before another
    // key is addressed anywhere.
    if ui.mode == Mode::Confirm {
        handle_confirm(&mut *ui, &mut *conversation, &mut *outbox, key);
        return Some(false);
    }

    // The sign-in surface answers every key itself, and it has to be asked
    // **before** the pane walk below: while it is up `Tab` pauses the flow
    // rather than walking the panes, and `q` is unbound, so a reader typing a
    // phone number cannot quit the program out from under themselves.
    if session.signin.is_some() {
        return Some(handle_signin(
            &mut *session,
            &mut *ui,
            &mut *conversation,
            &mut *input,
            &mut *outbox,
            &mut *profile,
            &mut *pending,
            &mut *list,
            &mut *drafts,
            key,
        ));
    }

    None
}

/// Keys for the conversation pane, matched on mode and content.
///
/// Matched on both axes rather than on `mode` alone: the profile is a
/// content of this pane, not a pane, and the wildcard that would save
/// the tuple here is the kind of arm that is right until the day it
/// is not.
fn handle_conversation_key(
    ui: &mut UiState,
    profile: &mut ProfileCard,
    conversation: &mut ConversationState,
    outbox: &mut Outbox,
    list: &mut ChatListState,
    pending: &mut Pending,
    input: &mut InputState,
    rows: &[CardRow],
    lines: Vec<String>,
    is_contact: bool,
    cursor: usize,
    layout: &[RowSpan],
    key: KeyEvent,
) -> bool {
    match (ui.mode, ui.pane) {
        (Mode::Normal, Pane::Profile(_)) => handle_profile(
            &mut *profile,
            &mut *ui,
            &mut *conversation,
            &mut *outbox,
            &mut *list,
            &mut *pending,
            &mut *input,
            rows,
            lines,
            is_contact,
            cursor,
            layout,
            key,
        ),
        (Mode::Normal, Pane::Conversation) => handle_normal(
            &mut *ui,
            &mut *pending,
            &mut *conversation,
            &mut *input,
            &mut *list,
            &mut *outbox,
            &mut *profile,
            layout,
            key,
        ),
        (Mode::Visual, _) => {
            handle_visual(
                &mut *ui,
                &mut *pending,
                &mut *conversation,
                &mut *outbox,
                key,
            );
            false
        }
        (Mode::Confirm, _) => {
            handle_confirm(&mut *ui, &mut *conversation, &mut *outbox, key);
            false
        }
    }
}

/// The top-level key dispatch.
///
/// Order: `Ctrl-C` always quits; a confirmation outranks every pane; the
/// sign-in surface answers before the pane walk; a jump in flight swallows
/// everything but `Esc`; the completion owns its keys; the new-conversation
/// overlay owns its keys off the input line; pane movement (`Tab`,
/// `BackTab`, `Ctrl-w`) is read here; then the focus match. That order is
/// load-bearing and unchanged.
///
/// `rows`, `lines`, `is_contact` and `cursor` are the card's drawn state and
/// `layout` the conversation's frame geometry, all precomputed by the caller
/// from `&App` for the arms that need them.
///
/// Returns true when a card was opened and its highlight still needs sizing.
pub(crate) fn handle_key(
    ui: &mut UiState,
    session: &mut SessionState,
    profile: &mut ProfileCard,
    pending: &mut Pending,
    list: &mut ChatListState,
    outbox: &mut Outbox,
    conversation: &mut ConversationState,
    input: &mut InputState,
    drafts: &mut DraftStore,
    rows: &[CardRow],
    lines: Vec<String>,
    is_contact: bool,
    cursor: usize,
    layout: &[RowSpan],
    key: KeyEvent,
) -> bool {
    if let Some(answered) = preempt_key(
        &mut *ui,
        &mut *session,
        &mut *conversation,
        &mut *input,
        &mut *outbox,
        &mut *profile,
        &mut *pending,
        &mut *list,
        &mut *drafts,
        key,
    ) {
        return answered;
    }

    if intercept_key(
        &mut *ui,
        &mut *pending,
        &mut *list,
        &mut *outbox,
        &mut *conversation,
        &mut *input,
        &mut *drafts,
        &mut *profile,
        key,
    ) {
        return false;
    }

    if handle_pane_key(
        &mut *ui,
        &mut *conversation,
        &mut *input,
        &mut *profile,
        key,
    ) {
        return false;
    }

    match ui.focus {
        Focus::ChatList => handle_chat_list(
            &mut *list,
            &mut *pending,
            &mut *conversation,
            &mut *ui,
            &mut *input,
            &mut *profile,
            &mut *outbox,
            &mut *drafts,
            session.client_available,
            key,
        ),
        Focus::Conversation => handle_conversation_key(
            &mut *ui,
            &mut *profile,
            &mut *conversation,
            &mut *outbox,
            &mut *list,
            &mut *pending,
            &mut *input,
            rows,
            lines,
            is_contact,
            cursor,
            layout,
            key,
        ),
        Focus::Input => handle_line(
            &mut *ui,
            &mut *conversation,
            &mut *input,
            &mut *outbox,
            &mut *pending,
            &mut *session,
            &mut *list,
            &mut *drafts,
            &mut *profile,
            key,
        ),
    }
}
