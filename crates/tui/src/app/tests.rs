use super::*;

/// A message in the sample conversation.
fn message(id: i64, text: &'static str) -> Message {
    Message {
        id,
        chat_id: MOCK_CHAT,
        text: Cow::Borrowed(text),
        timestamp: 1_730_000_000 + id,
        status: MessageStatus::Received,
        is_outgoing: false,
        reply_to: None,
        media: None,
    }
}

/// Messages of the sample conversation, with these identifiers.
fn page(ids: &[i64]) -> Vec<Message> {
    ids.iter().map(|id| message(*id, "text")).collect()
}

/// The same, however the identifiers are spelled.
fn numbered(ids: impl IntoIterator<Item = i64>) -> Vec<Message> {
    ids.into_iter().map(|id| message(id, "text")).collect()
}

/// `to` messages of a screenful each, which is a window of rows rather
/// than of lines.
fn tall_page(to: i64) -> Vec<Message> {
    (0..to)
        .map(|id| Message {
            text: Cow::Owned("x".repeat(400)),
            ..message(id, "text")
        })
        .collect()
}

/// A message in a conversation the sample data does not hold.
fn stranger(id: i64) -> Message {
    Message {
        chat_id: MOCK_CHAT + 1,
        ..message(id, "stranger")
    }
}

/// A message in a conversation the client holds nowhere at all.
fn unknown(id: i64) -> Message {
    Message {
        chat_id: 999,
        ..message(id, "unknown")
    }
}

/// How many unread messages the list holds for a conversation.
fn unread(app: &App, chat_id: i64) -> u32 {
    app.list
        .list
        .chats
        .iter()
        .find(|chat| chat.id == chat_id)
        .expect("the chat is in the list")
        .unread_count
}

/// The identifier of the message the cursor is on.
fn reading(app: &App) -> Option<i64> {
    app.conversation
        .conversation
        .window
        .get(app.conversation.vim.cursor())
        .map(|message| message.id)
}

/// The text the open conversation holds for a message.
fn text_of(app: &App, id: i64) -> Option<&str> {
    app.conversation
        .conversation
        .window
        .iter()
        .find(|message| message.id == id)
        .map(|message| message.text.as_ref())
}

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn press_ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn type_text(app: &mut App, text: &str) {
    for ch in text.chars() {
        app.handle_key(press(KeyCode::Char(ch)));
    }
}

/// The cursor to the top of what is loaded, which is where `gg` leaves it.
fn go_to_top(app: &mut App) {
    app.handle_key(press(KeyCode::Char('g')));
    app.handle_key(press(KeyCode::Char('g')));
}

/// The sample conversation, with `unread` messages waiting in it and its
/// newest message numbered `last`.
///
/// The sample conversation has nothing unread — it is the one the reader is
/// in — so the tests that are about where the unread messages start say how
/// many there are, and how the conversation is numbered.
fn with_unread(unread: u32, last: i64) -> App {
    let mut app = App::mock();
    let chat = app
        .list
        .list
        .chats
        .iter_mut()
        .find(|chat| chat.id == MOCK_CHAT)
        .expect("the sample conversation is in the list");
    chat.unread_count = unread;
    chat.last_message_id = Some(last);

    app
}

/// The sample conversation with its unread messages in front of what is
/// loaded: the conversation runs to 20, and the window stops at 8.
fn with_unread_out_of_reach(unread: u32) -> App {
    let mut app = with_unread(unread, 20);
    app.apply_latest(page(&[1, 2, 3, 4, 5, 6, 7, 8]));
    app
}

// ---- the frame -----------------------------------------------------

#[test]
fn a_new_application_holds_nothing_it_has_not_fetched() {
    let app = App::new();

    assert!(app.chats().is_empty());
    assert!(app.conversation.conversation.window.is_empty());
    assert_eq!(app.current_chat_id(), 0);
    assert!(!app.has_conversation());
    assert!(!app.wants_older(), "there is nothing to page through yet");
    assert!(!app.wants_newer());
}

#[test]
fn opening_a_chat_replaces_the_conversation_on_show() {
    let mut app = App::mock();
    app.select_chat(1);

    assert_eq!(app.list.selected_chat, 1);
    assert_eq!(app.current_chat_id(), 2);
    assert_eq!(
        app.conversation.conversation.window.chat_id, 2,
        "the window belongs to the chat that was opened"
    );
    assert!(
        app.conversation.conversation.window.is_empty(),
        "nothing has been fetched for it yet"
    );
    assert!(app.conversation.conversation.auto_follow());
    assert_eq!(app.conversation.vim.total(), 0);
}

/// The fetched list replaces whatever was there, and the reader's place
/// comes back inside it rather than pointing past the end of a shorter one.
#[test]
fn a_fetched_list_replaces_the_one_before_it() {
    let mut app = App::mock();
    app.select_chat(2);

    app.set_chats(mock_chats().into_iter().take(2).collect());

    assert_eq!(app.chats().len(), 2);
    assert_eq!(app.list.selected_chat, 1, "clamped into the shorter list");
    assert_eq!(app.current_chat_id(), 2);
}

/// A fetch that returns nobody leaves no conversation to be in: the window
/// belongs to a chat the list no longer holds.
#[test]
fn an_empty_fetch_closes_the_conversation() {
    let mut app = App::mock();

    app.set_chats(Vec::new());

    assert!(app.chats().is_empty());
    assert_eq!(app.current_chat_id(), 0);
    assert!(app.conversation.conversation.window.is_empty());
}

/// A refresh installs a new list around the reader's place, rather than
/// through the reset a chat switch runs: the conversation, the selection and
/// the draft stay, and the highlight follows the open conversation's id into
/// a list whose order has changed.
#[test]
fn a_refresh_keeps_the_conversation_selection_and_draft_purpose() {
    let mut app = App::mock();
    app.start_reply();
    let reply_to = app
        .conversation
        .reply_to
        .expect("the sample cursor is on a message");
    spanning(&mut app, 3, 5);

    // The same conversations, reversed: an index would point at a different
    // one, so only restoring by id can keep the highlight where it was.
    let reversed: Vec<_> = app.chats().iter().rev().cloned().collect();
    let restored = app.refresh_chats(reversed);

    assert!(
        restored,
        "the open conversation is still in the fetched list"
    );
    assert_eq!(
        app.current_chat_id(),
        MOCK_CHAT,
        "the conversation on show is untouched"
    );
    assert!(
        !app.conversation.conversation.window.is_empty(),
        "and so is its window"
    );
    assert_eq!(
        app.list.selected_chat, 2,
        "the highlight followed the id to the end of the reversed list"
    );
    assert_eq!(
        app.input.line.purpose(),
        PromptKind::Reply,
        "the draft is still a reply, not reset to a plain message"
    );
    assert_eq!(app.conversation.reply_to, Some(reply_to));
    assert_eq!(
        app.selection()
            .map(|selection| (selection.anchor.message_id, selection.focus.message_id)),
        Some((3, 5)),
        "the selection survives the list being replaced"
    );
}

/// Regression: every keystroke must be applied exactly once. Previously
/// the reader thread in `runtime.rs` dropped every other event, so typing
/// `s` then `q` produced only `q`.
#[test]
fn entering_insert_mode_then_typing_records_every_key() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('i')));
    assert_eq!(app.ui.focus, Focus::Input);

    type_text(&mut app, "hello");
    assert_eq!(app.input.line.text(), "hello");
}

#[test]
fn escape_returns_to_the_line_and_then_to_the_conversation() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('i')));
    type_text(&mut app, "hi");

    app.handle_key(press(KeyCode::Esc));

    assert_eq!(
        app.ui.focus,
        Focus::Input,
        "one escape stops typing, and the reader is still in the line"
    );
    assert_eq!(
        app.input.line.text(),
        "hi",
        "and the text is not thrown away"
    );
}

#[test]
fn backspace_removes_exactly_one_char_per_press() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('i')));
    type_text(&mut app, "abc");
    app.handle_key(press(KeyCode::Backspace));

    assert_eq!(app.input.line.text(), "ab");
}

/// The headline behaviour, in the reader's words. Every Vim user presses
/// `Esc` to stop typing and look at the conversation, and losing four lines
/// to it with no warning was the most complaint-worthy thing this program
/// did.
#[test]
fn a_typed_message_survives_two_escapes() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('i')));
    type_text(&mut app, "half a thought\nand the rest of it");

    app.handle_key(press(KeyCode::Esc));
    assert_eq!(
        app.ui.focus,
        Focus::Input,
        "the first escape stops typing: the reader is still in the line"
    );

    app.handle_key(press(KeyCode::Esc));
    assert_eq!(
        app.ui.focus,
        Focus::Conversation,
        "and the second looks away"
    );
    assert_eq!(
        app.input.line.text(),
        "half a thought\nand the rest of it",
        "with every word of it"
    );
}

/// A draft is per conversation: leaving one behind does not leak its words
/// into the next, which starts with whatever that conversation was left with,
/// and the reader's own conversation has its sentence back on return.
#[test]
fn a_draft_survives_a_conversation_switch() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('i')));
    type_text(&mut app, "half a th");
    app.handle_key(press(KeyCode::Esc));
    app.handle_key(press(KeyCode::Esc));

    app.select_chat(1);

    assert!(
        app.input.line.is_empty(),
        "another conversation starts with its own draft, and has none"
    );

    app.select_chat(0);

    assert_eq!(
        app.input.line.text(),
        "half a th",
        "and the reader's own conversation has theirs again"
    );
}

/// A conversation nobody has typed in starts empty, rather than inheriting
/// the words of the one before it.
#[test]
fn another_conversation_starts_with_its_own_draft() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('i')));
    type_text(&mut app, "half a th");
    app.handle_key(press(KeyCode::Esc));
    app.handle_key(press(KeyCode::Esc));

    app.select_chat(1);

    assert!(
        app.input.line.is_empty(),
        "the next conversation's own draft is nothing yet"
    );
}

/// The draft's *subject* does not survive, though, because it names something
/// in the conversation that has been closed. The words stay; what they were
/// written against does not.
#[test]
fn a_reply_that_outlives_its_conversation_becomes_a_message() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('r')));
    let replied_to = app
        .conversation
        .reply_to
        .expect("a reply answers the message on the cursor");
    assert_eq!(app.input.line.purpose(), PromptKind::Reply);
    type_text(&mut app, "sure");

    app.select_chat(1);
    app.select_chat(0);

    assert_eq!(app.input.line.text(), "sure", "the words");
    assert_eq!(
        app.input.line.purpose(),
        PromptKind::Message,
        "but not the subject"
    );
    assert_eq!(
        app.conversation.reply_to, None,
        "and nothing to reply to any more: {replied_to} was in the other chat"
    );
}

/// The rule for what a draft is: the bar is always a draft, so switching away
/// from it and coming back finds the text rather than a blank field.
#[test]
fn a_draft_is_still_there_after_a_submit_that_sent_it() {
    let mut app = App::mock();
    submit(&mut app, "ping");

    assert!(
        app.input.line.is_empty(),
        "a sent message leaves nothing behind, or `Enter` would send it twice"
    );

    app.handle_key(press(KeyCode::Char('i')));
    type_text(&mut app, "next");

    assert_eq!(
        app.input.line.text(),
        "next",
        "and the next one starts clean"
    );
}

/// Types `text` and submits it, leaving a placeholder in flight.
fn submit(app: &mut App, text: &str) {
    app.handle_key(press(KeyCode::Char('i')));
    type_text(app, text);
    app.handle_key(press(KeyCode::Enter));
}

#[test]
fn enter_shows_the_typed_message_while_it_is_on_its_way() {
    let mut app = App::mock();
    let before = app.conversation.conversation.window.len();

    submit(&mut app, "ping");

    assert_eq!(app.conversation.conversation.window.len(), before + 1);
    let id = app.conversation.sending.expect("the send is in flight");
    assert_eq!(id, -1, "the first placeholder is minus one");
    assert_eq!(text_of(&app, id), Some("ping"));
    assert_eq!(
        reading(&app),
        Some(id),
        "a message just typed is the one on screen"
    );
    assert_eq!(app.ui.mode, Mode::Normal);

    assert_eq!(
        app.take_action(),
        Some(Action::Send {
            chat_id: MOCK_CHAT,
            temp_id: id,
            text: "ping".to_owned(),
            reply_to: None,
        })
    );
    assert_eq!(app.take_action(), None, "an action is taken once");
}

#[test]
fn typing_with_no_conversation_open_composes_nothing() {
    let mut app = App::new();
    app.handle_key(press(KeyCode::Char('i')));
    type_text(&mut app, "ping");
    app.handle_key(press(KeyCode::Enter));

    assert!(app.conversation.conversation.window.is_empty());
}

#[test]
fn a_second_send_is_refused_while_one_is_on_its_way() {
    let mut app = App::mock();
    submit(&mut app, "first");
    let before = app.conversation.conversation.window.len();

    submit(&mut app, "second");

    assert_eq!(
        app.conversation.conversation.window.len(),
        before,
        "the second message is not shown, because it was not queued"
    );
    assert!(
        app.ui.status.contains("already on its way"),
        "a refusal has to say so: {:?}",
        app.ui.status
    );
}

/// Two requests made between two ticks are two requests. A single slot
/// would let the second replace the first, and the first would never be
/// sent — the bug this queue exists to prevent.
#[test]
fn two_actions_queued_together_are_taken_in_order() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('k')));
    assert_eq!(reading(&app), Some(9), "an outgoing message");

    // First edit.
    app.handle_key(press(KeyCode::Char('e')));
    type_text(&mut app, " one");
    app.handle_key(press(KeyCode::Enter));
    // Second edit, before the caller has taken the first.
    app.handle_key(press(KeyCode::Char('e')));
    type_text(&mut app, " two");
    app.handle_key(press(KeyCode::Enter));

    let first = app.take_action().expect("the first edit is queued");
    let second = app.take_action().expect("the second edit is queued");

    assert!(
        first != second,
        "the two edits must be distinct operations, not one twice"
    );
    assert_eq!(app.take_action(), None, "and the queue is drained");
}

#[test]
fn a_queued_forward_is_taken_with_its_fields_in_order() {
    let mut app = App::mock();
    app.outbox.actions.push_back(Action::Forward {
        chat_id: 1,
        message_ids: vec![10, 11],
        dest_chat_id: 2,
    });

    assert_eq!(
        app.take_action(),
        Some(Action::Forward {
            chat_id: 1,
            message_ids: vec![10, 11],
            dest_chat_id: 2,
        }),
    );
    assert_eq!(app.take_action(), None, "and the queue is drained");
}

// ---- focus and the panes --------------------------------------------

/// The sample data, with the focus on the chat list.
fn on_the_chat_list() -> App {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('h')));
    app
}

#[test]
fn a_new_application_has_the_conversation_focused() {
    assert_eq!(App::new().ui.focus, Focus::Conversation);
}

#[test]
fn h_leaves_the_conversation_for_the_chat_list_and_l_comes_back() {
    let mut app = App::mock();
    assert_eq!(app.ui.focus, Focus::Conversation);

    app.handle_key(press(KeyCode::Char('h')));
    assert_eq!(app.ui.focus, Focus::ChatList);

    // The conversation's own motions are not the list's: `k` up there moved
    // the cursor, and here it moves the highlight.
    app.handle_key(press(KeyCode::Char('k')));
    assert_eq!(app.list.selected_chat, 0);
    assert_eq!(reading(&app), Some(10), "the cursor did not move");

    app.handle_key(press(KeyCode::Char('l')));
    assert_eq!(app.ui.focus, Focus::Conversation);
}

#[test]
fn tab_walks_the_panes_in_the_order_they_are_drawn_and_wraps() {
    let mut app = App::mock();

    let mut seen = vec![app.ui.focus];
    for _ in 0..3 {
        app.handle_key(press(KeyCode::Tab));
        seen.push(app.ui.focus);
    }

    assert_eq!(
        seen,
        vec![
            Focus::Conversation,
            Focus::Input,
            Focus::ChatList,
            Focus::Conversation
        ],
        "one full turn of Tab is back where it started"
    );
}

#[test]
fn backtab_walks_the_other_way() {
    let mut app = App::mock();

    app.handle_key(press(KeyCode::BackTab));
    assert_eq!(app.ui.focus, Focus::ChatList);

    app.handle_key(press(KeyCode::BackTab));
    assert_eq!(app.ui.focus, Focus::Input);
}

/// `Esc` abandons the line and `Ctrl+w` only looks away from it, because a
/// reader stepping between panes should not lose a half-written sentence.
#[test]
fn ctrl_w_leaves_the_line_keeping_what_was_typed() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('i')));
    type_text(&mut app, "half a th");

    app.handle_key(press_ctrl('w'));

    assert_eq!(app.ui.focus, Focus::Conversation);
    assert_eq!(
        app.input.line.text(),
        "half a th",
        "the line is not thrown away"
    );

    // And it is still there to come back to, rather than lost.
    app.handle_key(press(KeyCode::Tab));
    assert_eq!(app.input.line.text(), "half a th");
}

#[test]
fn ctrl_w_outside_the_line_does_nothing() {
    let mut app = App::mock();

    app.handle_key(press_ctrl('w'));

    assert_eq!(app.ui.focus, Focus::Conversation);
}

/// A selection belongs to the conversation, so a pane that is not the
/// conversation cannot be entered over the top of one.
#[test]
fn leaving_the_conversation_drops_a_visual_selection() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('v')));
    assert_eq!(app.ui.mode, Mode::Visual);

    app.handle_key(press(KeyCode::Tab));

    assert_eq!(app.ui.focus, Focus::Input);
    assert_eq!(app.ui.mode, Mode::Normal);
}

#[test]
fn the_chat_list_highlight_moves_and_stops_at_both_ends() {
    let mut app = on_the_chat_list();

    app.handle_key(press(KeyCode::Char('j')));
    assert_eq!(app.list.selected_chat, 1);
    app.handle_key(press(KeyCode::Char('j')));
    assert_eq!(app.list.selected_chat, 2);
    app.handle_key(press(KeyCode::Char('j')));
    assert_eq!(app.list.selected_chat, 2, "and clamps at the end");

    app.handle_key(press(KeyCode::Char('k')));
    app.handle_key(press(KeyCode::Char('k')));
    app.handle_key(press(KeyCode::Char('k')));
    assert_eq!(app.list.selected_chat, 0, "and at the start");
}

/// A moment long enough after any keystroke this test could have pressed for
/// the highlight to have settled.
fn settled() -> Instant {
    Instant::now() + CHAT_SWITCH_DELAY
}

/// Moving the highlight asks for the conversation it names, but not before
/// the reader has stopped: a held `j` would otherwise fetch every chat it
/// scrolled past.
#[test]
fn a_moving_highlight_waits_for_the_reader_to_stop() {
    let mut app = on_the_chat_list();

    app.handle_key(press(KeyCode::Char('j')));
    assert!(
        app.take_pending_chat(Instant::now()).is_none(),
        "nothing is asked for while the key is still moving"
    );

    app.handle_key(press(KeyCode::Char('j')));
    assert_eq!(
        app.list.selected_chat, 2,
        "the highlight follows the key at once"
    );

    assert_eq!(
        app.take_pending_chat(settled()),
        Some(2),
        "and the conversation it landed on is asked for once it settles"
    );
    assert_eq!(
        app.take_pending_chat(settled()),
        None,
        "taken once, like every other hand-over"
    );
}

#[test]
fn a_debounce_elapsed_but_the_highlight_moving_again_defers_the_open() {
    let mut app = on_the_chat_list();

    app.handle_key(press(KeyCode::Char('j')));
    let settled = settled();
    assert_eq!(app.take_pending_chat(settled), Some(1));

    app.handle_key(press(KeyCode::Char('j')));
    assert_eq!(
        app.take_pending_chat(settled),
        None,
        "the second press restarts the wait rather than slipping through it"
    );
}

#[test]
fn gg_and_g_reach_both_ends_of_the_chat_list() {
    let mut app = on_the_chat_list();

    app.handle_key(press(KeyCode::Char('G')));
    assert_eq!(app.list.selected_chat, 2);
    assert_eq!(app.take_pending_chat(settled()), Some(2));

    app.handle_key(press(KeyCode::Char('g')));
    app.handle_key(press(KeyCode::Char('g')));
    assert_eq!(app.list.selected_chat, 0);
    assert_eq!(app.take_pending_chat(settled()), Some(0));
}

/// A lone `g` is a key with no meaning of its own, so it must not still be
/// waiting to be the first half of a `gg` several keys later.
#[test]
fn a_lone_g_does_not_wait_to_be_the_first_half_of_gg() {
    let mut app = on_the_chat_list();

    app.handle_key(press(KeyCode::Char('G')));
    app.handle_key(press(KeyCode::Char('g')));
    app.handle_key(press(KeyCode::Char('x')));
    app.handle_key(press(KeyCode::Char('g')));
    assert_eq!(
        app.list.selected_chat, 2,
        "the `g` after the `x` starts a sequence rather than finishing one"
    );

    app.handle_key(press(KeyCode::Char('g')));
    assert_eq!(app.list.selected_chat, 0);
}

/// `Enter` is a reader saying "this one", not a movement, so it opens at once
/// and takes the focus to the messages.
#[test]
fn enter_in_the_chat_list_opens_it_and_moves_to_the_conversation() {
    let mut app = on_the_chat_list();
    app.handle_key(press(KeyCode::Char('j')));

    app.handle_key(press(KeyCode::Enter));

    assert_eq!(app.ui.focus, Focus::Conversation);
    assert_eq!(app.conversation.conversation.window.chat_id, 2);
    assert_eq!(app.list.selected_chat, 1);
    assert_eq!(
        app.take_pending_chat(settled()),
        None,
        "and there is nothing left to open afterwards"
    );
}

/// A movement in one pane must not answer for the other: the sample
/// conversation's `k` walks messages, and the list's walks conversations.
#[test]
fn a_pane_only_answers_for_itself() {
    let mut app = on_the_chat_list();

    app.handle_key(press(KeyCode::Char('g')));
    assert_eq!(
        app.list.selected_chat, 0,
        "`gg` in the list, not the top of a window"
    );
    assert_eq!(app.conversation.conversation.window.chat_id, MOCK_CHAT);
}

// ---- a selection the window can move under -------------------------

/// A selection spanning two whole messages, made directly.
///
/// The keys that make one are bound in a later step, and the question this
/// section is about is what survives the window moving rather than how a
/// selection was made — so it is built here rather than pressed.
fn spanning(app: &mut App, anchor: i64, focus: i64) {
    app.conversation.selection = Some(Selection {
        anchor: Mark::whole(anchor),
        focus: Mark::whole(focus),
    });
}

/// The invariant a selection is most likely to break: a page landing under a
/// live one shifts every index in the window, so restoring the cursor alone
/// would leave the selection covering different messages — and the next `d`
/// would delete something the reader did not select.
#[test]
fn a_page_landing_under_a_selection_keeps_both_of_its_ends() {
    let mut app = App::mock();
    app.apply_latest(numbered(10..=20));
    spanning(&mut app, 11, 15);

    assert!(app.apply_older(numbered(1..=9)));

    let selection = app.selection().expect("both messages are still loaded");
    assert_eq!(
        (selection.anchor.message_id, selection.focus.message_id),
        (11, 15),
        "the ends are identifiers, so the nine that went in front of them cannot move them"
    );
    assert_eq!(
        app.covered(app.selection()),
        10..15,
        "and it still covers exactly what it did"
    );
}

/// The same, from the other end: an arrival pushes the oldest messages out of
/// a window that is at its cap.
#[test]
fn an_arrival_that_pushes_an_end_out_of_the_window_drops_the_selection() {
    let cap = i64::try_from(CONVERSATION_WINDOW).expect("the cap fits an identifier");
    let mut app = App::mock();
    app.apply_latest(numbered(1..=cap));
    spanning(&mut app, 1, 3);

    assert!(app.apply_newer(numbered(cap + 1..=cap + 2)));
    assert!(
        app.conversation
            .conversation
            .window
            .position_of(1)
            .is_none(),
        "the oldest message has been pushed out of a window at its cap"
    );

    assert_eq!(
        app.selection(),
        None,
        "half a selection is worse than none: `d` on it would be a one-message \
         deletion the reader did not ask for"
    );
}

#[test]
fn a_window_that_is_replaced_takes_the_selection_with_it() {
    let mut app = App::mock();
    spanning(&mut app, 2, 4);

    app.apply_latest(numbered(1..=10));

    assert_eq!(
        app.selection(),
        None,
        "a page that replaces the window has moved every message in it"
    );
}

#[test]
fn a_jump_replaces_the_window_and_the_selection_with_it() {
    let mut app = with_unread(2, 20);
    go_to_top(&mut app);
    let jump = app.pending_jump().expect("the target is not loaded");
    spanning(&mut app, 1, 2);

    assert!(app.apply_jump(&numbered(1..=8), jump.target_id));

    assert_eq!(app.selection(), None);
}

#[test]
fn opening_another_conversation_takes_the_selection_with_it() {
    let mut app = App::mock();
    spanning(&mut app, 2, 4);

    app.select_chat(1);

    assert_eq!(app.selection(), None);
}

/// A mark on a message the window does not hold is a mark nothing can draw
/// and `d` cannot act on, so it is refused rather than recorded.
#[test]
fn a_selection_can_only_be_started_on_a_message_that_is_loaded() {
    let mut app = App::mock();

    assert!(app.select(4, Some(0)));
    assert_eq!(
        app.selection().and_then(Selection::text_range),
        Some((4, 0..0)),
        "and it starts collapsed, which is what `v` leaves behind"
    );

    assert!(!app.select(999, Some(0)));
    assert_eq!(
        app.selection().and_then(Selection::text_range),
        Some((4, 0..0)),
        "and a refused mark leaves the selection that was there"
    );
}

// ---- the motions ----------------------------------------------------

/// The characters of the message under the cursor, for a motion's answer.
fn selected_chars(app: &App) -> Option<(i64, Range<usize>)> {
    app.selection().and_then(Selection::text_range)
}

/// The messages the selection covers, oldest first.
fn selected_messages(app: &App) -> Vec<i64> {
    let covered = app.covered(app.selection());

    app.conversation
        .conversation
        .window
        .iter()
        .skip(covered.start)
        .take(covered.len())
        .map(|message| message.id)
        .collect()
}

/// The focus, as a message and a character position.
fn focused(app: &App) -> Option<(i64, Option<usize>)> {
    app.selection()
        .map(|selection| (selection.focus.message_id, selection.focus.char))
}

fn key(app: &mut App, c: char) {
    app.handle_key(press(KeyCode::Char(c)));
}

#[test]
fn v_starts_a_charwise_selection_at_the_first_character() {
    let mut app = App::mock();
    go_to_top(&mut app);

    key(&mut app, 'v');

    assert_eq!(app.ui.mode, Mode::Visual);
    assert_eq!(
        selected_chars(&app),
        Some((1, 0..0)),
        "a position, which is a span of no characters yet"
    );
}

#[test]
fn v_then_l_selects_one_character() {
    let mut app = App::mock();
    go_to_top(&mut app);

    key(&mut app, 'v');
    key(&mut app, 'l');

    assert_eq!(selected_chars(&app), Some((1, 0..1)));
}

#[test]
fn v_then_j_selects_two_messages() {
    let mut app = App::mock();
    go_to_top(&mut app);

    key(&mut app, 'v');
    key(&mut app, 'j');

    assert_eq!(
        selected_messages(&app),
        vec![1, 2],
        "two ends in two messages are a set of messages"
    );
    assert_eq!(selected_chars(&app), None);
    assert_eq!(reading(&app), Some(2), "and the cursor followed the focus");
}

/// The first sample message, whose words are `Hey,` `is` `the` `build`
/// `green?`.
const SAMPLE: &str = "Hey, is the build green?";

/// A selection dropped and Normal restored, the way a reader leaves one.
fn escape(app: &mut App) {
    app.handle_key(press(KeyCode::Esc));
}

#[test]
fn v_then_selecting_upward_covers_the_same_messages() {
    let mut app = App::mock();
    go_to_top(&mut app);
    key(&mut app, 'j');
    key(&mut app, 'j');

    key(&mut app, 'V');
    assert_eq!(selected_messages(&app), vec![3]);
    key(&mut app, 'k');
    key(&mut app, 'k');

    assert_eq!(
        selected_messages(&app),
        vec![1, 2, 3],
        "a selection dragged upwards covers the same messages as one dragged down"
    );
}

#[test]
fn capital_v_selects_a_whole_message() {
    let mut app = App::mock();
    go_to_top(&mut app);

    key(&mut app, 'V');

    assert_eq!(selected_messages(&app), vec![1]);
    assert_eq!(
        focused(&app),
        Some((1, None)),
        "and there is no place inside it"
    );
}

/// A linewise selection has nowhere inside it to move, so the character
/// motions do nothing — which is what Vim does, and what the status line's
/// count already tells the reader.
#[test]
fn a_character_motion_over_a_whole_message_does_nothing() {
    let mut app = App::mock();
    go_to_top(&mut app);
    key(&mut app, 'V');

    for motion in ['h', 'l', 'w', 'b', 'e', '0', '$'] {
        key(&mut app, motion);
        assert_eq!(focused(&app), Some((1, None)), "after {motion}");
    }

    assert_eq!(
        selected_messages(&app),
        vec![1],
        "and the selection is intact"
    );
}

#[test]
fn o_swaps_the_ends_without_changing_the_selection() {
    let mut app = App::mock();
    go_to_top(&mut app);
    key(&mut app, 'v');
    for _ in 0..3 {
        key(&mut app, 'l');
    }
    let before = selected_chars(&app);
    let anchor = app.selection().expect("held").anchor;
    let focus = focused(&app).expect("held");

    key(&mut app, 'o');

    assert_eq!(selected_chars(&app), before, "only the direction changed");
    assert_eq!(focused(&app), Some((anchor.message_id, anchor.char)));
    assert_eq!(app.selection().expect("held").focus.message_id, focus.0);

    key(&mut app, 'o');
    assert_eq!(focused(&app), Some(focus), "and twice is the original");
}

#[test]
fn escape_drops_the_selection_and_returns_to_normal() {
    let mut app = App::mock();
    go_to_top(&mut app);
    key(&mut app, 'v');
    key(&mut app, 'j');

    escape(&mut app);

    assert_eq!(app.ui.mode, Mode::Normal);
    assert_eq!(app.selection(), None);
}

#[test]
fn a_selection_does_not_outlive_the_status_that_announced_it() {
    let mut app = App::mock();
    go_to_top(&mut app);
    key(&mut app, 'v');
    let selected = app.status_text();
    assert!(
        selected.contains("selected"),
        "a selection says so: {selected:?}"
    );

    escape(&mut app);

    assert!(
        !app.status_text().contains("selected"),
        "so the status line does not keep describing a selection that is gone: {:?}",
        app.status_text()
    );
}

#[test]
fn the_word_motions_walk_the_words_of_a_message() {
    let mut app = App::mock();
    go_to_top(&mut app);
    key(&mut app, 'v');

    key(&mut app, 'w');
    assert_eq!(focused(&app), Some((1, Some(5))), "over `is`");
    key(&mut app, 'w');
    assert_eq!(focused(&app), Some((1, Some(8))), "and over `the`");
    key(&mut app, 'e');
    assert_eq!(
        focused(&app),
        Some((1, Some(10))),
        "and `e` to the end of it"
    );
    key(&mut app, 'b');
    assert_eq!(
        focused(&app),
        Some((1, Some(8))),
        "and `b` back to its start"
    );
}

#[test]
fn zero_and_the_end_are_the_ends_of_the_message() {
    let mut app = App::mock();
    go_to_top(&mut app);
    key(&mut app, 'v');

    key(&mut app, '$');
    let last = SAMPLE.chars().count() - 1;
    assert_eq!(focused(&app), Some((1, Some(last))));

    key(&mut app, '0');
    assert_eq!(focused(&app), Some((1, Some(0))));
}

#[test]
fn f_and_t_find_a_character_in_the_message() {
    let mut app = App::mock();
    go_to_top(&mut app);
    key(&mut app, 'v');

    key(&mut app, 'f');
    key(&mut app, 'i');
    assert_eq!(
        focused(&app),
        Some((1, Some(5))),
        "`fi` lands on the `i` of `is`"
    );

    key(&mut app, 'f');
    key(&mut app, 'i');
    assert_eq!(focused(&app), Some((1, Some(14))), "and the next one");

    key(&mut app, 'F');
    key(&mut app, 'i');
    assert_eq!(focused(&app), Some((1, Some(5))), "`Fi` goes back");
}

/// The key after `f` is the character to look for, whatever it is — that is
/// what `fw` means. A `w` there is a letter to find, not a motion, and this
/// message has no `w` in it.
#[test]
fn the_key_after_f_is_the_character_and_not_another_motion() {
    let mut app = App::mock();
    go_to_top(&mut app);
    key(&mut app, 'v');

    key(&mut app, 'f');
    key(&mut app, 'w');

    assert_eq!(
        focused(&app),
        Some((1, Some(0))),
        "nothing was found, so nothing moved — and `w` was not a motion"
    );
}

/// A `f` whose character has not been typed must not still be waiting when an
/// unrelated key arrives, or that key would be read as the character.
#[test]
fn a_find_waiting_for_its_character_is_forgotten_by_another_key() {
    let mut app = App::mock();
    go_to_top(&mut app);
    key(&mut app, 'v');

    key(&mut app, 'f');
    app.handle_key(press(KeyCode::Enter));
    key(&mut app, 'l');

    assert_eq!(
        focused(&app),
        Some((1, Some(1))),
        "the `l` was a motion, so the `f` was not half of one"
    );
}

/// A message's own text is the boundary, whatever the motion: crossing into
/// the next message is `j`'s job, and a motion that did it silently would
/// change what the reader thinks they selected.
#[test]
fn a_character_motion_never_leaves_the_message() {
    let mut app = App::mock();
    go_to_top(&mut app);
    key(&mut app, 'v');
    let last = SAMPLE.chars().count() - 1;

    for motion in ['w', 'e', '$', 'b'] {
        for _ in 0..40 {
            key(&mut app, motion);
        }

        let (id, at) = focused(&app).expect("still a selection");
        assert_eq!(id, 1, "{motion} stayed on the message it started on");
        assert!(at.is_some_and(|at| at <= last), "{motion} landed on {at:?}");
    }

    // Where each of them does end up, so the test above is not satisfied by a
    // motion that simply does nothing.
    for (motion, bound) in [('b', 0), ('e', last), ('$', last), ('w', 18)] {
        let mut app = App::mock();
        go_to_top(&mut app);
        key(&mut app, 'v');
        for _ in 0..40 {
            key(&mut app, motion);
        }
        assert_eq!(focused(&app), Some((1, Some(bound))), "{motion}");
    }
}

#[test]
fn a_character_motion_moves_over_a_multibyte_character_rather_than_inside_it() {
    let mut app = App::mock();
    app.apply_latest(vec![Message {
        text: Cow::Borrowed("é😀x"),
        ..message(1, "unused")
    }]);
    go_to_top(&mut app);
    key(&mut app, 'v');

    key(&mut app, 'l');
    assert_eq!(
        focused(&app),
        Some((1, Some(1))),
        "over the two-byte character"
    );
    key(&mut app, 'l');
    assert_eq!(focused(&app), Some((1, Some(2))), "and the four-byte one");
    key(&mut app, 'l');
    assert_eq!(focused(&app), Some((1, Some(2))), "and stops at the last");
    key(&mut app, 'h');
    assert_eq!(focused(&app), Some((1, Some(1))));
}

/// The character position rides along to the next message, clamped, so that a
/// selection which has been moved within a message keeps its relative place.
#[test]
fn a_character_position_carries_to_the_next_message_and_clamps() {
    let mut app = App::mock();
    app.apply_latest(vec![message(1, "a long first message"), message(2, "hi")]);
    go_to_top(&mut app);
    key(&mut app, 'v');
    for _ in 0..10 {
        key(&mut app, 'l');
    }
    assert_eq!(focused(&app), Some((1, Some(10))));

    key(&mut app, 'j');

    assert_eq!(
        focused(&app),
        Some((2, Some(1))),
        "clamped to the end of a two-character message"
    );
}

/// Leaving the pane drops a selection, because a selection for a conversation
/// nobody is looking at would leave `d` holding something invisible.
#[test]
fn leaving_the_conversation_drops_the_selection() {
    let mut app = App::mock();
    go_to_top(&mut app);
    key(&mut app, 'v');
    key(&mut app, 'j');

    app.handle_key(press(KeyCode::Tab));

    assert_eq!(app.selection(), None);
    assert_eq!(app.ui.mode, Mode::Normal);
}

// ---- yanking and pasting --------------------------------------------

/// The sample conversation, at the top, with the first message selected.
fn selecting_message_one(charwise: bool) -> App {
    let mut app = App::mock();
    go_to_top(&mut app);
    key(&mut app, if charwise { 'v' } else { 'V' });
    app
}

/// What the register holds.
fn yanked(app: &App) -> Vec<String> {
    app.register().lines().to_vec()
}

#[test]
fn y_pulls_a_text_selection_into_the_register_and_ends_the_selection() {
    let mut app = selecting_message_one(true);
    for _ in 0..8 {
        key(&mut app, 'l');
    }

    key(&mut app, 'y');

    assert_eq!(yanked(&app), vec!["Hey, is ".to_owned()]);
    assert_eq!(
        app.ui.mode,
        Mode::Normal,
        "a yank ends the selection, as in Vim"
    );
    assert_eq!(app.selection(), None);
}

#[test]
fn y_pulls_one_line_per_message_oldest_first() {
    let mut app = selecting_message_one(false);
    key(&mut app, 'j');
    key(&mut app, 'j');

    key(&mut app, 'y');

    assert_eq!(
        yanked(&app),
        vec![
            "Hey, is the build green?".to_owned(),
            "Yes — clippy is happy.".to_owned(),
            "Nice. Did you pin the toolchain?".to_owned(),
        ],
        "three messages, oldest first, whatever order they were selected in"
    );
}

/// A yank with no motion behind it is a position, not a span, and there is
/// nothing in it to take. Said rather than silently emptying the register.
#[test]
fn a_selection_that_covers_nothing_says_so_rather_than_yanking_nothing() {
    let mut app = selecting_message_one(true);

    key(&mut app, 'y');

    assert!(
        app.ui.status.contains("nothing to yank"),
        "got {:?}",
        app.ui.status
    );
    assert_eq!(
        app.status_text(),
        app.ui.status,
        "and the refusal is on the screen: a selection's own note outranks a \
         transient status, so leaving Visual is what makes it visible at all"
    );
    assert!(yanked(&app).is_empty());
    assert_eq!(app.ui.mode, Mode::Normal);
}

/// A yank with no paste is a one-way trip to the system clipboard, which is
/// not somewhere a message can be sent from.
#[test]
fn p_opens_the_line_with_what_was_yanked() {
    let mut app = selecting_message_one(false);
    key(&mut app, 'j');
    key(&mut app, 'y');

    key(&mut app, 'p');

    assert_eq!(app.ui.focus, Focus::Input);
    assert_eq!(app.input.line.purpose(), PromptKind::Message);
    assert_eq!(
        app.input.line.text(),
        "Hey, is the build green?\nYes — clippy is happy.",
        "two messages, pasted as two lines"
    );
}

/// The register holds owned text, so a page landing between the yank and the
/// paste cannot pull the text out from under it.
#[test]
fn what_was_yanked_survives_a_page_landing() {
    let mut app = App::mock();
    app.apply_latest(numbered(10..=20));
    go_to_top(&mut app);
    key(&mut app, 'V');
    key(&mut app, 'j');
    key(&mut app, 'y');
    let before = yanked(&app);

    assert!(app.apply_older(numbered(1..=9)));
    key(&mut app, 'p');

    assert_eq!(yanked(&app), before, "and the paste is the same text");
    assert_eq!(app.input.line.text(), before.join("\n"));
}

#[test]
fn p_with_nothing_yanked_says_so() {
    let mut app = App::mock();

    key(&mut app, 'p');

    assert!(
        app.ui.status.contains("nothing has been yanked"),
        "got {:?}",
        app.ui.status
    );
    assert_eq!(app.ui.focus, Focus::Conversation, "and no line was opened");
}

/// A yank is about this conversation, so opening another one forgets it —
/// the same discipline as the search and the selection.
#[test]
fn opening_another_conversation_forgets_what_was_yanked() {
    let mut app = selecting_message_one(false);
    key(&mut app, 'y');

    app.select_chat(1);

    assert!(yanked(&app).is_empty());
}

/// A yank is offered to the system clipboard as well as kept in the register,
/// and taking it is a hand-over like every other one here: once, not twice.
#[test]
fn a_yank_is_offered_to_the_clipboard_and_taken_once() {
    let mut app = App::mock();
    assert_eq!(app.take_clipboard(), None, "nothing has been yanked yet");

    go_to_top(&mut app);
    key(&mut app, 'V');
    key(&mut app, 'j');
    key(&mut app, 'y');

    assert_eq!(
        app.take_clipboard().as_deref(),
        Some("Hey, is the build green?\nYes — clippy is happy."),
        "the whole of the register, which is what a yank is for"
    );
    assert_eq!(app.take_clipboard(), None, "and it is gone once taken");
}

/// A yank in the line is a yank: the line's `y` fills the same slot the
/// conversation's does, and the caller that owns the terminal drains it the
/// same way. One seam, two producers.
#[test]
fn a_yank_in_the_line_is_offered_to_the_clipboard_and_taken_once() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('i')));
    type_text(&mut app, "héllo");
    app.handle_key(press(KeyCode::Esc));
    app.handle_key(press(KeyCode::Char('0')));
    app.handle_key(press(KeyCode::Char('v')));
    app.handle_key(press(KeyCode::Char('l')));
    app.handle_key(press(KeyCode::Char('l')));
    app.handle_key(press(KeyCode::Char('y')));

    assert_eq!(
        app.take_clipboard().as_deref(),
        Some("hél"),
        "the line's yank, multi-byte characters and all"
    );
    assert_eq!(app.take_clipboard(), None, "and it is drained once");
}

/// A word motion on multi-byte text runs, and the caret it leaves is on a
/// character boundary — the snap after every key is what makes that so.
#[test]
fn a_word_motion_on_non_ascii_text_runs_and_says_nothing() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('i')));
    type_text(&mut app, "héllo wörld");
    app.handle_key(press(KeyCode::Esc));

    app.handle_key(press(KeyCode::Char('w')));

    assert_eq!(app.input.line.text(), "héllo wörld", "and it only moved");
    assert!(
        app.input
            .line
            .text()
            .is_char_boundary(app.input.line.caret()),
        "onto a character: {:?}",
        app.input.line.caret()
    );
    assert!(
        !app.ui.status.contains("not built yet"),
        "and nothing is owed the reader: {:?}",
        app.ui.status
    );
}

/// Behind an operator the motion and the slice happen inside one key, which
/// is the one place snapping cannot help. It is refused — and it says so,
/// because a key that does nothing and says nothing reads as a hang.
#[test]
fn a_word_motion_behind_an_operator_on_non_ascii_text_is_refused_and_says_so() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('i')));
    type_text(&mut app, "héllo wörld");
    app.handle_key(press(KeyCode::Esc));
    app.handle_key(press(KeyCode::Char('d')));

    app.handle_key(press(KeyCode::Char('w')));

    assert_eq!(app.input.line.text(), "héllo wörld", "nothing ran");
    assert!(
        app.ui.status.contains("not built yet"),
        "and the refusal says so: {:?}",
        app.ui.status
    );
}

/// The register is the half that always works; the clipboard is a courtesy
/// whose terminal may or may not honour it, so nothing about a yank depends on
/// the offer having been taken.
#[test]
fn a_yank_survives_its_clipboard_offer_being_never_taken() {
    let mut app = selecting_message_one(false);
    key(&mut app, 'y');

    let _offer = app.take_clipboard();

    assert_eq!(
        yanked(&app),
        vec!["Hey, is the build green?".to_owned()],
        "the register is what is left whatever happened to the offer"
    );
}

/// Two refusals, and a reader who cannot tell them apart cannot tell what to
/// select instead.
#[test]
fn a_visual_r_is_refused_and_says_which_way() {
    let mut app = App::mock();
    go_to_top(&mut app);
    key(&mut app, 'v');
    for _ in 0..5 {
        key(&mut app, 'l');
    }

    key(&mut app, 'r');

    assert_eq!(app.ui.mode, Mode::Normal, "a refusal still answers the key");
    assert_eq!(app.selection(), None);
    assert_eq!(
        app.status_text(),
        "quoting a reply is not built yet",
        "a selection inside one message is the case a quote would serve"
    );
    assert_eq!(app.ui.focus, Focus::Conversation, "and no line was opened");
}

#[test]
fn a_visual_r_over_several_messages_is_refused_for_the_other_reason() {
    let mut app = App::mock();
    go_to_top(&mut app);
    key(&mut app, 'V');
    key(&mut app, 'j');

    key(&mut app, 'r');

    assert_eq!(app.ui.mode, Mode::Normal);
    assert_eq!(
        app.status_text(),
        "a reply can only quote words inside one message",
        "there is no wire representation for quoting five messages"
    );
}

/// `r` in Normal is a plain reply with no quote, and is not affected by any of
/// the above.
#[test]
fn a_normal_r_still_opens_a_plain_reply() {
    let mut app = App::mock();
    go_to_top(&mut app);

    key(&mut app, 'r');

    assert_eq!(app.ui.focus, Focus::Input);
    assert_eq!(app.input.line.purpose(), PromptKind::Reply);
    assert_eq!(app.conversation.reply_to, Some(1));
}

#[test]
fn p_is_not_bound_in_visual_mode() {
    let mut app = selecting_message_one(false);

    key(&mut app, 'p');

    assert_eq!(
        app.ui.focus,
        Focus::Conversation,
        "replacing a selection with the reader's own text is a destructive reading \
         of a key that looks additive, so it does nothing here"
    );
    assert!(yanked(&app).is_empty());
    assert_eq!(app.ui.mode, Mode::Visual, "and the selection is untouched");
}

// ---- deleting, and the confirm --------------------------------------

// ---- deleting, and the confirm --------------------------------------

/// A conversation holding exactly these turns: an identifier and whose it is.
///
/// The sample data alternates which side sent each message, so a run of two
/// from the same side — which is the ordinary shape of a conversation, and the
/// only way to reach the "all yours" and "all theirs" wordings — is not
/// something it can say.
fn conversation(turns: &[(i64, bool)]) -> App {
    let mut app = App::new();
    app.set_chats(mock_chats());
    app.select_chat(0);
    app.apply_latest(
        turns
            .iter()
            .map(|(id, outgoing)| Message {
                id: *id,
                chat_id: MOCK_CHAT,
                text: Cow::Borrowed("text"),
                timestamp: 0,
                status: MessageStatus::Received,
                is_outgoing: *outgoing,
                reply_to: None,
                media: None,
            })
            .collect(),
    );
    app
}

/// Three of the reader's own, then two of theirs.
fn one_way_then_the_other() -> App {
    conversation(&[(1, true), (2, true), (3, true), (4, false), (5, false)])
}

/// Moves the cursor down onto the message with this identifier.
///
/// Forward only, which is all these tests need: they all start at the top.
/// Bounded, so a target that is behind the cursor fails the test rather than
/// hanging the suite.
fn cursor_onto(app: &mut App, id: i64) {
    for _ in 0..=app.conversation.conversation.window.len() {
        if reading(app) == Some(id) {
            return;
        }
        key(app, 'j');
    }

    panic!(
        "no message {id} below the cursor: it holds {:?}",
        reading(app)
    );
}

/// A linewise selection from the first message to the last.
fn spanning_all(app: &mut App) {
    go_to_top(app);
    key(app, 'V');
    while app.conversation.vim.cursor() + 1 < app.conversation.conversation.window.len() {
        key(app, 'j');
    }
}

/// The identifiers a pending deletion would ask the server for.
fn asked_to_delete(app: &App) -> Vec<i64> {
    match &app.conversation.confirm {
        Some(ConfirmKind::DeleteMessages { ids, .. }) => ids.clone(),
        other => panic!("expected a deletion to be waiting, got {other:?}"),
    }
}

#[test]
fn q_asks_before_quitting() {
    let mut app = App::mock();

    key(&mut app, 'q');

    assert_eq!(app.ui.mode, Mode::Confirm);
    assert_eq!(app.conversation.confirm, Some(ConfirmKind::Quit));
    assert!(!app.ui.should_quit);
    assert_eq!(app.status_text(), QUIT_PROMPT);
}

/// The command asks the same question as the key, because they are the same
/// request written down.
#[test]
fn the_command_quit_asks_too() {
    let mut app = App::mock();

    run_command_line(&mut app, "q");

    assert_eq!(app.ui.mode, Mode::Confirm);
    assert_eq!(app.conversation.confirm, Some(ConfirmKind::Quit));
    assert!(!app.ui.should_quit);
}

#[test]
fn y_on_the_quit_prompt_quits_and_asks_the_network_for_nothing() {
    let mut app = App::mock();

    key(&mut app, 'q');
    key(&mut app, 'y');

    assert!(app.ui.should_quit);
    assert_eq!(app.conversation.confirm, None);
    assert_eq!(app.ui.mode, Mode::Normal);
    assert!(app.outbox.actions.is_empty());
}

/// Either way of saying no, and neither of them is a third key: `Esc` is
/// how a reader abandons a question they have read past the end of.
#[test]
fn n_and_esc_keep_the_program_running() {
    for answer in [
        KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
    ] {
        let mut app = App::mock();

        key(&mut app, 'q');
        app.handle_key(answer);

        assert!(!app.ui.should_quit, "{answer:?} quit");
        assert_eq!(
            app.conversation.confirm, None,
            "{answer:?} left the prompt up"
        );
        assert_eq!(app.ui.mode, Mode::Normal, "{answer:?} left the mode alone");
    }
}

/// The one key that does not ask, and the reason it is not asked about.
#[test]
fn ctrl_c_still_quits_at_once() {
    let mut app = App::mock();

    app.handle_key(press_ctrl('c'));

    assert!(app.ui.should_quit);
    assert_eq!(app.conversation.confirm, None);
}

#[test]
fn d_asks_to_delete_the_message_under_the_cursor() {
    let mut app = App::mock();
    // The newest sample message is one of theirs.
    assert_eq!(reading(&app), Some(10));

    key(&mut app, 'd');

    assert_eq!(app.ui.mode, Mode::Confirm);
    assert_eq!(asked_to_delete(&app), vec![10]);
    assert_eq!(app.status_text(), DELETE_INCOMING_PROMPT);
}

/// `dd` is `d` with no second press to distinguish, so a reader who types it
/// gets the same answer.
#[test]
fn dd_asks_about_the_same_message_d_does() {
    let mut app = App::mock();

    key(&mut app, 'd');
    assert_eq!(app.ui.mode, Mode::Confirm);

    app.handle_key(press(KeyCode::Char('n')));
    key(&mut app, 'd');
    assert_eq!(app.ui.mode, Mode::Confirm);
    key(&mut app, 'd');

    assert_eq!(asked_to_delete(&app), vec![10], "and so does `dd`");
}

#[test]
fn a_motion_then_dd_deletes_the_message_the_cursor_is_on() {
    let mut app = App::mock();
    go_to_top(&mut app);

    key(&mut app, 'j');
    key(&mut app, 'd');
    assert_eq!(asked_to_delete(&app), vec![2]);
    key(&mut app, 'y');

    assert_eq!(
        app.take_action(),
        Some(Action::Delete {
            chat_id: MOCK_CHAT,
            message_ids: vec![2],
        }),
        "the message the cursor was on when the `d` landed, and not the one \
         it was on before the `j`"
    );
}

/// The asymmetry this whole feature is built on: a deletion accepts the other
/// side's message, and the prompt says which side it is about.
#[test]
fn a_deletion_accepts_an_outgoing_message_and_the_prompt_names_that_side() {
    let mut app = App::mock();
    key(&mut app, 'k');
    assert_eq!(reading(&app), Some(9), "an outgoing message");

    key(&mut app, 'd');

    assert_eq!(app.status_text(), DELETE_OUTGOING_PROMPT);
}

/// With no latch there is nothing for a stray key to disturb, and a second `d`
/// is a second question about whatever is under the cursor then — not the end
/// of a two-key sequence over the first one's message.
#[test]
fn two_ds_are_two_questions_and_nothing_else_is_half_of_one() {
    let mut app = App::mock();

    key(&mut app, 'x');
    assert_eq!(
        app.ui.mode,
        Mode::Normal,
        "an unbound key is not half of a `dd`"
    );

    key(&mut app, 'd');
    assert_eq!(asked_to_delete(&app), vec![10]);
    key(&mut app, 'n');

    key(&mut app, 'k');
    key(&mut app, 'd');
    assert_eq!(asked_to_delete(&app), vec![9]);
}

#[test]
fn a_page_then_d_deletes_where_the_page_landed() {
    let mut app = App::mock();
    go_to_top(&mut app);

    app.handle_key(press_ctrl('d'));
    let after = reading(&app).expect("a message is on screen");

    key(&mut app, 'd');

    assert_eq!(asked_to_delete(&app), vec![after]);
}

#[test]
fn a_visual_d_asks_for_every_message_the_selection_covers() {
    let mut app = App::mock();
    go_to_top(&mut app);
    key(&mut app, 'V');
    cursor_onto(&mut app, 3);

    key(&mut app, 'd');

    assert_eq!(asked_to_delete(&app), vec![1, 2, 3], "oldest first");
    assert_eq!(
        app.status_text(),
        delete_mixed_prompt(2, 1),
        "a range that mixes both sides says which is which"
    );
}

/// A selection inside one message deletes the whole of it: a partial message
/// is not something the protocol can do, and half a deletion is not something
/// the reader would recognise afterwards.
#[test]
fn a_text_selection_deletes_the_whole_message_it_is_in() {
    let mut app = App::mock();
    go_to_top(&mut app);
    key(&mut app, 'v');
    for _ in 0..5 {
        key(&mut app, 'l');
    }

    key(&mut app, 'd');

    assert_eq!(
        asked_to_delete(&app),
        vec![1],
        "all of it, not the five characters"
    );
}

#[test]
fn a_selection_of_only_the_reader_s_own_messages_counts_them() {
    let mut app = conversation(&[(1, true), (2, true), (3, true)]);
    spanning_all(&mut app);

    key(&mut app, 'd');

    assert_eq!(app.status_text(), delete_yours_prompt(3));
}

#[test]
fn a_selection_of_only_their_messages_counts_them() {
    let mut app = conversation(&[(1, false), (2, false), (3, false), (4, false)]);
    spanning_all(&mut app);

    key(&mut app, 'd');

    assert_eq!(app.status_text(), delete_theirs_prompt(4));
}

/// A placeholder is a local stand-in for a send the server has not
/// acknowledged, so naming one would have the whole request refused and take
/// the real messages down with it. It is left out — and said.
#[test]
fn a_selection_with_some_placeholders_skips_them_and_says_how_many() {
    let mut app = conversation(&[(1, false), (2, false), (3, false)]);
    submit(&mut app, "hi");
    let placeholder = app.conversation.sending.expect("the send is in flight");
    go_to_top(&mut app);
    key(&mut app, 'V');
    cursor_onto(&mut app, placeholder);

    key(&mut app, 'd');

    assert_eq!(asked_to_delete(&app), vec![1, 2, 3]);
    assert!(
        app.status_text().ends_with("· 1 never sent"),
        "and the prompt says what it left out: {}",
        app.status_text()
    );
}

/// A selection of nothing but placeholders has nothing to ask for, and the
/// refusal keeps the distinction between a send still on its way and one that
/// failed: only the second has a `D`.
#[test]
fn a_selection_of_only_placeholders_is_refused() {
    let mut app = conversation(&[(1, false)]);
    submit(&mut app, "hi");
    let placeholder = app.conversation.sending.expect("the send is in flight");
    go_to_top(&mut app);
    cursor_onto(&mut app, placeholder);
    key(&mut app, 'V');

    key(&mut app, 'd');

    assert_eq!(app.ui.mode, Mode::Normal, "no confirm is raised");
    assert!(
        app.ui.status.contains("still on its way"),
        "got {:?}",
        app.ui.status
    );

    app.fail_send(placeholder, "boom".to_owned());
    key(&mut app, 'd');

    assert_eq!(app.ui.mode, Mode::Normal, "and still none");
    assert!(
        app.ui.status.contains("D dismisses"),
        "a failed message points at the key that clears it: {:?}",
        app.ui.status
    );
}

/// A placeholder for a send in flight is numbered below zero and sits at the
/// *end* of the window, so the numbers between the two ends of a selection say
/// something different from what the selection covers. Reading coverage off the
/// identifiers deleted the wrong messages.
#[test]
fn a_selection_reaching_a_placeholder_covers_the_window_positions() {
    let mut app = App::mock();
    submit(&mut app, "hi");
    let placeholder = app.conversation.sending.expect("the send is in flight");
    go_to_top(&mut app);
    key(&mut app, 'V');
    cursor_onto(&mut app, placeholder);

    assert_eq!(
        app.covered(app.selection()),
        0..11,
        "from the first message to the placeholder, which is the last position"
    );

    key(&mut app, 'd');

    assert_eq!(
        asked_to_delete(&app),
        (1..=10).collect::<Vec<i64>>(),
        "the ten between them, and not the one the identifier span names"
    );
}

/// Everything the wording needs is captured when the prompt is raised, so
/// nothing that happens to the selection while the prompt is up can change
/// what `y` deletes. A page landing under a range, a focus change, a
/// conversation change: all of it moves what the reader is looking at, and
/// re-deriving the selection at `y` time would delete that instead.
#[test]
fn a_confirmation_hands_over_what_it_captured_and_not_the_selection_now() {
    let mut app = one_way_then_the_other();
    spanning_all(&mut app);
    key(&mut app, 'd');
    assert_eq!(asked_to_delete(&app), vec![1, 2, 3, 4, 5]);

    app.set_selection(Selection::at(5, None));

    key(&mut app, 'y');

    assert_eq!(
        app.take_action(),
        Some(Action::Delete {
            chat_id: MOCK_CHAT,
            message_ids: vec![1, 2, 3, 4, 5],
        })
    );
}

#[test]
fn a_forward_covers_every_message_in_the_selection_oldest_first() {
    let mut app = App::mock();
    go_to_top(&mut app);
    key(&mut app, 'V');
    cursor_onto(&mut app, 3);

    let selection = *app.selection().expect("a selection is up");
    let forwarding = app
        .conversation
        .forwardable(&selection)
        .expect("three numbered messages can be forwarded");

    assert_eq!(forwarding.ids, vec![1, 2, 3]);
    assert_eq!(forwarding.skipped, 0);
}

#[test]
fn a_forward_leaves_out_placeholders_and_counts_them() {
    let mut app = App::mock();
    submit(&mut app, "hi");
    let placeholder = app.conversation.sending.expect("the send is in flight");
    go_to_top(&mut app);
    key(&mut app, 'V');
    cursor_onto(&mut app, placeholder);

    let selection = *app.selection().expect("a selection is up");
    let forwarding = app
        .conversation
        .forwardable(&selection)
        .expect("the numbered messages can be forwarded");

    assert_eq!(forwarding.ids, (1..=10).collect::<Vec<i64>>());
    assert_eq!(
        forwarding.skipped, 1,
        "the placeholder is counted, not sent"
    );
}

#[test]
fn a_forward_of_nothing_but_placeholders_is_refused_with_the_reason() {
    let mut app = App::mock();
    submit(&mut app, "hi");
    let placeholder = app.conversation.sending.expect("the send is in flight");
    app.set_selection(Selection::at(placeholder, None));

    let selection = *app.selection().expect("a selection is up");

    assert_eq!(app.conversation.forwardable(&selection), None);
    assert_eq!(
        app.conversation.refuse_forward(&selection),
        "that message is still on its way, so it cannot be forwarded"
    );
}

#[test]
fn editing_a_message_that_has_not_been_sent_or_is_not_yours_is_refused() {
    let mut app = App::mock();
    submit(&mut app, "hi");
    let id = app.conversation.sending.expect("the send is in flight");

    app.handle_key(press(KeyCode::Char('e')));
    assert_eq!(app.ui.mode, Mode::Normal);
    assert!(
        app.ui.status.contains("hasn't been sent yet"),
        "got {:?}",
        app.ui.status
    );

    app.fail_send(id, "boom".to_owned());
    app.handle_key(press(KeyCode::Char('e')));
    assert!(
        app.ui.status.contains("hasn't been sent yet"),
        "a failed send is refused on the same fact: {:?}",
        app.ui.status
    );

    // Step back off the placeholder to a message that came from them.
    app.handle_key(press(KeyCode::Char('k')));
    assert_eq!(
        reading(&app),
        Some(10),
        "the newest sample message is incoming"
    );
    app.handle_key(press(KeyCode::Char('e')));
    assert_eq!(app.ui.mode, Mode::Normal);
    assert!(
        app.ui.status.contains("only edit your own"),
        "got {:?}",
        app.ui.status
    );
}

// ---- composing a reply and an edit ---------------------------------

#[test]
fn r_opens_a_reply_to_the_message_under_the_cursor() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('k')));
    assert_eq!(reading(&app), Some(9));

    app.handle_key(press(KeyCode::Char('r')));
    assert_eq!(app.ui.focus, Focus::Input);
    assert_eq!(app.input.line.purpose(), PromptKind::Reply);
    assert_eq!(app.conversation.reply_to, Some(9));

    type_text(&mut app, "sure");
    app.handle_key(press(KeyCode::Enter));

    assert_eq!(
        app.take_action(),
        Some(Action::Send {
            chat_id: MOCK_CHAT,
            temp_id: -1,
            text: "sure".to_owned(),
            reply_to: Some(9),
        })
    );
}

#[test]
fn e_opens_the_cursor_s_own_message_for_editing() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('k')));
    assert_eq!(reading(&app), Some(9), "an outgoing message");

    app.handle_key(press(KeyCode::Char('e')));
    assert_eq!(app.ui.focus, Focus::Input);
    assert_eq!(app.input.line.purpose(), PromptKind::Edit);
    assert_eq!(app.conversation.editing, Some(9));
    assert!(
        app.input.line.text().starts_with("No pressure then :)"),
        "the buffer opens with the message's text: {:?}",
        app.input.line.text()
    );

    type_text(&mut app, "!");
    app.handle_key(press(KeyCode::Enter));

    let Some(Action::Edit {
        chat_id,
        message_id,
        text,
    }) = app.take_action()
    else {
        panic!("an edit is handed to the caller");
    };
    assert_eq!(chat_id, MOCK_CHAT);
    assert_eq!(message_id, 9);
    assert!(text.starts_with("No pressure then :)"), "got {text:?}");
}

// ---- confirming, dismissing, and the status line -------------------

#[test]
fn confirming_a_delete_hands_the_captured_message_over() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('d')));
    app.handle_key(press(KeyCode::Char('d')));

    app.handle_key(press(KeyCode::Char('y')));

    assert_eq!(app.ui.mode, Mode::Normal);
    assert_eq!(app.conversation.confirm, None);
    assert_eq!(
        app.take_action(),
        Some(Action::Delete {
            chat_id: MOCK_CHAT,
            message_ids: vec![10],
        })
    );
}

#[test]
fn cancelling_a_delete_leaves_no_side_effect() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('d')));
    app.handle_key(press(KeyCode::Char('d')));

    app.handle_key(press(KeyCode::Esc));

    assert_eq!(app.ui.mode, Mode::Normal);
    assert_eq!(app.conversation.confirm, None);
    assert_eq!(app.take_action(), None);
}

#[test]
fn the_dismiss_key_clears_a_failed_message() {
    let mut app = App::mock();
    submit(&mut app, "hi");
    let id = app.conversation.sending.expect("the send is in flight");
    app.fail_send(id, "boom".to_owned());

    app.handle_key(press(KeyCode::Char('D')));

    assert_eq!(app.conversation.conversation.window.len(), 10);
    assert!(app.conversation.conversation.message(id).is_none());
}

/// The row only has room for a short reason; the whole of it is on the
/// status line while the cursor is on the message.
#[test]
fn the_full_reason_a_send_failed_is_on_the_status_line() {
    let mut app = App::mock();
    submit(&mut app, "hi");
    let id = app.conversation.sending.expect("the send is in flight");

    app.fail_send(id, "flood wait, retry in 42s".to_owned());

    assert_eq!(app.status_text(), "flood wait, retry in 42s");
}

#[test]
fn a_flash_reverts_once_its_time_is_up() {
    let mut app = App::mock();

    app.flash("something went wrong");
    assert_eq!(app.ui.status, "something went wrong");
    assert!(
        !app.expire_status(Instant::now()),
        "the deadline has not passed"
    );
    assert_eq!(app.ui.status, "something went wrong");

    assert!(app.expire_status(Instant::now() + FLASH_FOR));
    assert_eq!(app.ui.status, IDLE_STATUS);
    assert!(!app.expire_status(Instant::now() + FLASH_FOR), "only once");
}

fn run_command_line(app: &mut App, command: &str) {
    app.handle_key(press(KeyCode::Char(':')));
    type_text(app, command);
    app.handle_key(press(KeyCode::Enter));
}

// ---- the profile panel ------------------------------------------------

/// The profile, opened from a mock application.
fn profile() -> App {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('S')));
    app
}

#[test]
fn settings_command_opens_the_profile() {
    let mut app = App::mock();
    run_command_line(&mut app, "settings");

    assert_eq!(app.ui.pane, Pane::Profile(ProfileId::SelfAccount));
}

/// A `:` line is a command line, not a message, so it is not a buffer and
/// never opens a shortcode completion — which is why the catalog cannot
/// intercept `Enter` here. The test is here because that is the reason, and
/// a reason nobody has checked is a reason that stops being true.
#[test]
fn a_command_line_never_completes_a_shortcode() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char(':')));
    type_text(&mut app, "sett");

    assert!(
        app.completion().is_none(),
        "a `:shortcode` is a message's, and this line is a command's"
    );
}

#[test]
fn the_profile_opens_from_the_chat_list_too() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('h')));
    assert_eq!(app.ui.focus, Focus::ChatList);

    app.handle_key(press(KeyCode::Char('S')));
    assert_eq!(app.ui.pane, Pane::Profile(ProfileId::SelfAccount));
    assert_eq!(app.ui.focus, Focus::Conversation);
}

/// The hint row is the one thing that says which keys the panel answers, and
/// naming the conversation's would tell the reader they are wrong.
#[test]
fn the_profile_has_its_own_hint() {
    let mut app = App::mock();
    assert!(
        app.status_text().contains("dd:del"),
        "the conversation's row"
    );

    app.handle_key(press(KeyCode::Char('S')));
    let hint = app.status_text();
    assert!(hint.contains("j/k: row"), "{hint}");
    assert!(!hint.contains("dd:del"), "the conversation's keys: {hint}");
}

/// `d` on a row with nothing to do must change nothing at all: no mode, no
/// prompt, and nothing handed to the caller to put on the wire. A key that
/// quietly armed the conversation's delete would make the *next* `d`,
/// wherever the reader had been by then, a deletion.
#[test]
fn a_profile_row_with_nothing_to_do_changes_nothing() {
    let mut app = profile();
    assert!(app.on_card_row("name"));

    app.handle_key(press(KeyCode::Char('d')));

    assert_eq!(app.ui.mode, Mode::Normal);
    assert_eq!(app.conversation.confirm, None);
    assert!(app.take_action().is_none(), "nothing was queued");
}

/// The same for the row that does have something to do: it confirms, and what
/// it queues is not a deletion. A card is a place to read, and the only thing
/// `d` can destroy from one is the reader's own session — which is what the
/// confirmation is asking about.
#[test]
fn a_profile_row_never_queues_a_deletion() {
    let mut app = profile();
    while !app.on_card_row(crate::card::LOGOUT) {
        app.handle_key(press(KeyCode::Char('j')));
    }

    app.handle_key(press(KeyCode::Char('d')));
    assert_eq!(app.conversation.confirm, Some(ConfirmKind::Logout));

    app.handle_key(press(KeyCode::Char('y')));
    assert!(
        !matches!(app.take_action(), Some(Action::Delete { .. })),
        "a card destroys no messages"
    );
}

/// The keys walk the rows the panel is drawing.
///
/// There used to be a second enumeration of the card's rows — a bare enum the
/// keys asked instead of the panel — and the two agreed only because both were
/// built from the same field. The failure was never a crash: it was a key that
/// acted on a row the panel was not showing, or a row nothing could reach.
/// Walking the card and comparing the two lists is the assertion that catches
/// it, and the reason this test walks by `j` rather than by index: an index
/// would match even if the two lists were ordered differently.
#[test]
fn the_keys_walk_the_rows_the_panel_is_drawing() {
    let mut app = profile();
    let drawn: Vec<&'static str> = crate::card::rows(&app)
        .iter()
        .map(|row| row.label)
        .collect();
    assert!(drawn.len() > 2, "the mock account has rows: {drawn:?}");

    let mut walked = Vec::new();
    for _ in 0..drawn.len() {
        walked.push(crate::card::rows(&app)[app.profile_cursor()].label);
        app.handle_key(press(KeyCode::Char('j')));
    }

    assert_eq!(
        walked, drawn,
        "`j` walked a different list than the panel drew"
    );
}

/// Two cursors, one type. With one value shared, the conversation's total —
/// the window's length — would size the profile's highlight too, and the
/// profile's row count would stand in for it. Neither test fails on its own:
/// each one only sees its own half move.
#[test]
fn the_profile_and_the_conversation_cursors_are_independent() {
    let mut app = profile();
    let in_the_conversation = app.conversation.vim.cursor();
    for _ in 0..3 {
        app.handle_key(press(KeyCode::Char('j')));
    }
    assert!(app.profile_cursor() > 0, "the card's highlight moved");
    assert_eq!(
        app.conversation.vim.cursor(),
        in_the_conversation,
        "and the conversation's did not"
    );

    // `Esc` out and `k` on the conversation: `k` rather than `j`, because the
    // mock conversation opens pinned to its newest message and `j` there is
    // the clamp rather than the motion.
    app.handle_key(press(KeyCode::Esc));
    app.handle_key(press(KeyCode::Char('k')));
    assert!(
        app.conversation.vim.cursor() < in_the_conversation,
        "the conversation's highlight moves on its own"
    );
}

/// Leaving a pane is one rule, and `Tab` is one of the keys that does it: the
/// card is not a stack, so the conversation is what comes back.
///
/// `Esc` and `l` no longer both leave, and that is the design: `h`/`l` are an
/// inline motion now, so a card is a column of values with no column to the
/// right of the last one, and `l` at the end of a value has nowhere to go. The
/// way out is `Esc`, and `h` at the *start* of a value — the two edges, each
/// key doing the one thing its own edge allows.
#[test]
fn every_way_out_of_the_card_lands_on_the_conversation() {
    // `Esc` with nothing selected: one press leaves.
    let mut app = profile();
    app.handle_key(press(KeyCode::Esc));
    assert_eq!(app.ui.pane, Pane::Conversation, "Esc closes the card");

    // `l` at the end of a value is not a way out, because it is a motion with
    // nowhere to go and saying otherwise would teach a key to lie.
    let mut app = profile();
    while app.card_caret() < 3 {
        app.handle_key(press(KeyCode::Char('l')));
    }
    app.handle_key(press(KeyCode::Char('l')));
    assert!(
        app.ui.pane.is_profile(),
        "l past the end of a value is a motion, not a way out"
    );

    // `h` at the start of a value is the way back, and the way back is the
    // conversation. The chat list is `Ctrl-w h`, because `h` is a motion here
    // and a key that is a motion in one place and a pane in the next is a key
    // a reader has to learn twice.
    let mut app = profile();
    app.handle_key(press(KeyCode::Char('h')));
    assert_eq!(app.ui.pane, Pane::Conversation, "h at the start goes back");

    let mut app = profile();
    app.handle_key(press_ctrl('w'));
    app.handle_key(press(KeyCode::Char('h')));
    assert_eq!(app.ui.focus, Focus::ChatList, "Ctrl-w h is the chat list");

    // Nothing is drawn to the right of a card, so `Ctrl-w l` has nowhere to
    // go and says so rather than doing nothing.
    let mut app = profile();
    app.handle_key(press_ctrl('w'));
    app.handle_key(press(KeyCode::Char('l')));
    assert!(
        app.ui.pane.is_profile(),
        "the card stays, and the reason is on show"
    );

    let mut app = profile();
    app.handle_key(press(KeyCode::Tab));
    assert_eq!(app.ui.pane, Pane::Conversation, "Tab walks the panes");
    assert_eq!(app.ui.focus, Focus::Input);
}

/// `Esc` is a ladder and not a switch: a selection, then the card, then out.
/// Four presses from a selection on a second row, with no special case
/// anywhere in it.
#[test]
fn escape_on_a_card_is_a_ladder_and_not_a_switch() {
    let mut app = profile();
    app.handle_key(press(KeyCode::Char('j')));
    app.handle_key(press(KeyCode::Char('v')));
    app.handle_key(press(KeyCode::Char('j')));
    assert!(
        app.card_selection().is_some(),
        "a selection across two rows"
    );

    // One press drops the selection and keeps the card.
    app.handle_key(press(KeyCode::Esc));
    assert!(app.card_selection().is_none(), "the selection went");
    assert!(app.ui.pane.is_profile(), "and the card stayed");

    // The next press leaves.
    app.handle_key(press(KeyCode::Esc));
    assert_eq!(app.ui.pane, Pane::Conversation, "and the next one leaves");
}

/// A key the panel does not answer is the conversation's, and taking it is
/// what stops a reader who pressed `i` by reflex from pressing it twice.
#[test]
fn a_conversation_key_from_the_profile_does_its_own_thing() {
    let mut app = profile();
    app.handle_key(press(KeyCode::Char('i')));

    assert_eq!(app.ui.pane, Pane::Conversation);
    assert_eq!(app.ui.focus, Focus::Input, "and the line is open");
}

#[test]
fn add_account_refuses_and_says_what_it_cannot_do() {
    let mut app = profile();
    while !app.on_card_row(crate::card::ADD_ACCOUNT) {
        app.handle_key(press(KeyCode::Char('j')));
    }
    app.handle_key(press(KeyCode::Char('d')));

    assert_eq!(app.ui.status, ADD_ACCOUNT_REFUSAL);
    assert_eq!(
        app.ui.mode,
        Mode::Normal,
        "and nothing was asked to be confirmed"
    );
}

/// A `logout` row that flashed a refusal instead of confirming would have
/// taught the reader the wrong thing about a key that throws away the only
/// secret this program holds.
#[test]
fn logout_confirms_before_anything_is_asked_for() {
    let mut app = profile();
    while !app.on_card_row(crate::card::LOGOUT) {
        app.handle_key(press(KeyCode::Char('j')));
    }
    app.handle_key(press(KeyCode::Char('d')));

    assert_eq!(app.ui.mode, Mode::Confirm);
    assert_eq!(app.conversation.confirm, Some(ConfirmKind::Logout));
    assert_eq!(app.status_text(), LOGOUT_PROMPT);
    assert_eq!(app.take_action(), None, "nothing is asked for yet");

    app.handle_key(press(KeyCode::Char('y')));
    assert_eq!(
        app.take_action(),
        Some(Action::Logout),
        "`y` asks for the sign-out"
    );
    assert_eq!(app.conversation.confirm, None);
    assert_eq!(app.ui.mode, Mode::Normal);
}

/// `n` and `Esc` are the other half of a confirmation, and here they mean
/// *do not*: the session this program holds is the reader's, and declining is
/// not a step towards doing it.
#[test]
fn declining_a_sign_out_asks_for_nothing() {
    for answer in [KeyCode::Char('n'), KeyCode::Esc] {
        let mut app = profile();
        while !app.on_card_row(crate::card::LOGOUT) {
            app.handle_key(press(KeyCode::Char('j')));
        }
        app.handle_key(press(KeyCode::Char('d')));
        assert_eq!(app.conversation.confirm, Some(ConfirmKind::Logout));

        app.handle_key(press(answer));

        assert_eq!(
            app.conversation.confirm, None,
            "{answer:?} drops the question"
        );
        assert_eq!(app.ui.mode, Mode::Normal);
        assert_eq!(app.take_action(), None, "{answer:?} asks for nothing");
    }
}

/// The rows that exist are the ones the account has something to say about.
#[test]
fn a_row_exists_only_when_the_account_says_something() {
    let mut app = App::mock();
    app.set_account(Ok(Account {
        username: None,
        phone: None,
        birthday: None,
        bio: None,
        ..mock_account()
    }));
    app.handle_key(press(KeyCode::Char('S')));

    let rows = crate::card::rows(&app);
    assert!(!rows.iter().any(|row| row.label == "bio"));
    assert!(!rows.iter().any(|row| row.label == "birthday"));
    assert!(rows.iter().any(|row| row.label == "name"));
    assert_eq!(
        rows.last().map(|row| row.label),
        Some(crate::card::LOGOUT),
        "the actions are last"
    );
    // The highlight counts the rows that exist, not the ones that would: six
    // rows here, and the two that are missing leave no gap for `j` to land in.
    assert_eq!(rows.len(), 6);
    assert!(app.on_card_row("name"));
}

#[test]
fn chat_command_selects_the_matching_chat() {
    let mut app = App::mock();
    run_command_line(&mut app, "chat 2");

    let expected = app
        .chats()
        .iter()
        .position(|c| c.id == 2)
        .expect("chat 2 is part of the mock data");
    assert_eq!(app.list.selected_chat, expected);
    assert_eq!(
        app.conversation.conversation.window.chat_id, 2,
        "the panel follows the chat list"
    );
}

/// Both halves of the `chat <id>` guard must hold: a malformed id and a
/// well-formed-but-unknown id must both leave the selection untouched.
#[test]
fn chat_command_ignores_unparseable_or_unknown_ids() {
    let mut app = App::mock();
    let before = app.list.selected_chat;

    run_command_line(&mut app, "chat not-a-number");
    assert_eq!(app.list.selected_chat, before);

    run_command_line(&mut app, "chat 999");
    assert_eq!(app.list.selected_chat, before);
}

#[test]
fn select_chat_by_id_opens_the_matching_chat_and_says_so() {
    let mut app = App::mock();

    assert!(app.select_chat_by_id(3));

    let expected = app
        .chats()
        .iter()
        .position(|c| c.id == 3)
        .expect("chat 3 is part of the mock data");
    assert_eq!(app.list.selected_chat, expected);
    assert_eq!(
        app.conversation.conversation.window.chat_id, 3,
        "the panel follows the chat list"
    );
}

/// The `:chat` half of the contract: an unknown id reports `false` and
/// leaves the screen as it was — saying where the reader landed instead is
/// the launch selection's job, not this one's.
#[test]
fn select_chat_by_id_leaves_an_unknown_id_where_the_reader_was() {
    let mut app = App::mock();
    let before = app.list.selected_chat;

    assert!(!app.select_chat_by_id(999));

    assert_eq!(app.list.selected_chat, before);
    assert_eq!(
        app.conversation.conversation.window.chat_id, MOCK_CHAT,
        "still the conversation that was open"
    );
}

/// The pending `--chat` id applies once: taken when the list lands, and
/// gone afterwards so a later refresh keeps the reader where they are.
#[test]
fn the_pending_initial_chat_is_taken_once() {
    let mut app = App::mock();

    assert_eq!(app.take_initial_chat(), None, "nothing pending at first");

    app.set_initial_chat(2);
    assert_eq!(app.take_initial_chat(), Some(2));
    assert_eq!(
        app.take_initial_chat(),
        None,
        "a second list lands with nothing to apply"
    );
}

#[test]
fn unknown_command_sets_the_status_line() {
    let mut app = App::mock();
    run_command_line(&mut app, "frobnicate");

    assert!(app.ui.status.contains("unknown command"));
    assert_eq!(app.ui.mode, Mode::Normal);
}

/// `:retry` asks for the client again and takes the `offline:` line down with
/// it: the sentence left up would be a failure that has just been acted on.
#[test]
fn retry_command_asks_for_the_client_again() {
    let mut app = App::mock();
    app.ui.status = "offline: connection reset".to_owned();

    run_command_line(&mut app, "retry");

    assert!(
        app.take_retry_request(),
        "the request is what the network side acts on"
    );
    assert_ne!(app.ui.status, IDLE_STATUS, "got {:?}", app.ui.status);
    assert!(
        !app.ui.status.contains("offline:"),
        "the failure it answers must not still be up: {:?}",
        app.ui.status
    );
}

/// One slot rather than a queue: a request taken is a request being carried out,
/// so the pass after the one that took it finds nothing. What stops a *second*
/// bring-up is not this slot — pressing `:retry` twice before either is taken
/// is still one request — but the network side's own in-flight guard.
#[test]
fn a_retry_request_is_spent_once_taken() {
    let mut app = App::mock();

    run_command_line(&mut app, "retry");
    assert!(app.take_retry_request(), "the command records the request");
    assert!(!app.take_retry_request(), "and taking it spends it");
}

/// Not a flash: a bring-up does not pass on its own, it ends in an event that
/// brings its own sentence — so this one must not expire back to idle while
/// the client is still being built.
#[test]
fn a_retry_outlives_a_transient_status() {
    let mut app = App::mock();
    run_command_line(&mut app, "retry");

    assert!(
        !app.expire_status(Instant::now() + FLASH_FOR),
        "got {:?}",
        app.ui.status
    );
    assert_ne!(app.ui.status, IDLE_STATUS);
}

/// The flash deadline from before the command goes with it: `:retry`
/// answers the `offline:` line, so a transient timer must not take the
/// persistent sentence down on the next tick.
#[test]
fn retry_command_clears_a_flash_deadline() {
    let mut app = App::mock();
    app.flash("something went wrong");

    run_command_line(&mut app, "retry");

    assert!(app.ui.status_until.is_none(), "got {:?}", app.ui.status);
    assert!(
        !app.expire_status(Instant::now() + FLASH_FOR),
        "got {:?}",
        app.ui.status
    );
    assert_eq!(app.ui.status, "reconnecting");
}

/// A feed retry sentence lives at the persistent rank: above the hint, so
/// the reader sees the wait — and never above a confirmation, which is the
/// question that cannot wait.
#[test]
fn a_feed_retry_sentence_sits_at_the_persistent_rank() {
    let mut app = App::mock();
    app.ui.status = "the update feed failed (telegram returned rpc error 420 FLOOD_WAIT); \
        retrying in 31s (attempt 1/3)"
        .to_owned();

    assert_eq!(app.status_text(), app.ui.status);

    key(&mut app, 'q');

    assert_eq!(app.status_text(), QUIT_PROMPT);
}

fn run_search_line(app: &mut App, query: &str) {
    app.handle_key(press(KeyCode::Char('/')));
    type_text(app, query);
    app.handle_key(press(KeyCode::Enter));
}

#[test]
fn search_finds_matches_in_the_open_conversation() {
    let mut app = App::mock();
    run_search_line(&mut app, "benchmarks");

    assert_eq!(app.search_query(), Some("benchmarks"));
    assert_eq!(
        app.conversation.vim.cursor(),
        6,
        "the cursor lands on the only match, at its index in the window"
    );
    assert!(
        !app.conversation.conversation.auto_follow(),
        "the reader moved off the end"
    );
}

/// The local pass is provisional: it can only see what is loaded, so the
/// label says so and a request is queued for the authoritative answer.
#[test]
fn a_search_is_provisional_until_the_server_answers() {
    let mut app = App::mock();
    run_search_line(&mut app, "benchmarks");

    assert_eq!(
        app.search().label(),
        "/benchmarks — 1 loaded — searching…",
        "a count from the loaded window is not an answer"
    );
    assert_eq!(
        app.take_action(),
        Some(Action::Search {
            chat_id: MOCK_CHAT,
            query: "benchmarks".to_owned(),
        }),
        "so the server is asked"
    );
}

/// A conversation the window holds in full cannot be searched better, so no
/// round trip is spent asking.
#[test]
fn a_search_over_a_complete_window_asks_nothing() {
    let mut app = App::mock();
    app.exhaust(FetchDirection::Older);
    app.exhaust(FetchDirection::Newer);

    run_search_line(&mut app, "benchmarks");

    assert_eq!(app.take_action(), None, "there is nobody to ask");
    assert_eq!(
        app.search().label(),
        "/benchmarks — 1 loaded",
        "and the local list stands as the answer"
    );
}

/// The broad reading of "the window holds everything" is wrong: the cap
/// drops the oldest messages, so both ends being reached says nothing.
#[test]
fn a_window_at_the_cap_is_not_the_whole_conversation() {
    let mut window = ConversationWindow::new(MOCK_CHAT);
    let over = i64::try_from(CONVERSATION_WINDOW).expect("the cap fits an identifier") + 5;
    window.replace((1..=over).map(|id| message(id, "text")).collect::<Vec<_>>());
    window.exhausted_older = true;
    window.exhausted_newer = true;

    assert_eq!(window.len(), CONVERSATION_WINDOW);
    assert!(
        !coordinate::holds_everything(&window),
        "the cap is the only thing that drops messages, and here it did"
    );

    let mut small = ConversationWindow::new(MOCK_CHAT);
    small.replace(page(&[1, 2, 3]));
    small.exhausted_older = true;
    small.exhausted_newer = true;
    assert!(
        coordinate::holds_everything(&small),
        "nothing was ever dropped"
    );
}

#[test]
fn an_empty_query_repeats_the_last_search() {
    let mut app = App::mock();
    run_search_line(&mut app, "benchmarks");
    let _ = app.take_action();

    run_search_line(&mut app, "");

    assert_eq!(
        app.search_query(),
        Some("benchmarks"),
        "as in Vim, an empty `/` runs the last search again"
    );
    assert_eq!(
        app.take_action(),
        Some(Action::Search {
            chat_id: MOCK_CHAT,
            query: "benchmarks".to_owned(),
        }),
        "and asks again, because the answer may have changed"
    );
}

/// The regression the bounded queue exists for: a search made inside the
/// tick a send was made in must not replace the send.
#[test]
fn sending_and_searching_in_one_tick_both_happen() {
    let mut app = App::mock();

    submit(&mut app, "ping");
    run_search_line(&mut app, "benchmarks");

    let first = app.take_action().expect("the send is still queued");
    let second = app.take_action().expect("and so is the search");
    assert!(
        matches!(first, Action::Send { .. }),
        "the send goes out first"
    );
    assert!(matches!(second, Action::Search { .. }));
}

#[test]
fn an_empty_query_with_nothing_to_repeat_says_so() {
    let mut app = App::mock();

    run_search_line(&mut app, "");

    assert!(!app.search().is_active());
    assert!(
        app.ui.status.contains("no previous search"),
        "a key that does nothing reads as a hang: {:?}",
        app.ui.status
    );
}

#[test]
fn n_with_no_search_says_so() {
    let mut app = App::mock();
    let before = reading(&app);

    app.handle_key(press(KeyCode::Char('n')));

    assert_eq!(reading(&app), before, "nothing moves");
    assert!(
        app.ui.status.contains("no previous search"),
        "and the key explains itself: {:?}",
        app.ui.status
    );
}

#[test]
fn opening_another_conversation_clears_the_search() {
    let mut app = App::mock();
    run_search_line(&mut app, "benchmarks");
    assert!(app.search().is_active());

    app.select_chat(1);

    assert!(
        !app.search().is_active(),
        "a match is a place in the conversation that was open"
    );
    assert_eq!(app.search_query(), None);
}

/// The label is state, not a flash: a transient status must not outrank it,
/// and its expiry must not take it away.
#[test]
fn the_search_label_outlives_a_transient_status() {
    let mut app = App::mock();
    run_search_line(&mut app, "benchmarks");

    app.flash("something that passes");

    assert!(
        app.status_text().contains("/benchmarks"),
        "the search line is not replaced by a passing message: {:?}",
        app.status_text()
    );

    app.expire_status(Instant::now() + FLASH_FOR);

    assert_eq!(app.ui.status, IDLE_STATUS, "the flash did expire");
    assert!(
        app.status_text().contains("/benchmarks"),
        "and the search line is still there: {:?}",
        app.status_text()
    );
}

#[test]
fn wrapping_the_walk_is_announced_and_loops_within_the_page() {
    let mut app = App::mock();
    // `the` starts a word — or a word beginning with it, like `then` — in
    // six of the sample messages.
    run_search_line(&mut app, "the");
    for _ in 0..5 {
        app.handle_key(press(KeyCode::Char('n')));
    }
    assert_eq!(reading(&app), Some(10), "the newest match");
    assert!(
        !app.search().label().contains("hit"),
        "a step that did not wrap says nothing"
    );

    app.handle_key(press(KeyCode::Char('n')));

    assert_eq!(reading(&app), Some(1), "the walk loops within the page");
    assert!(
        app.search()
            .label()
            .contains("search hit BOTTOM, continuing at TOP"),
        "and says so: {}",
        app.search().label()
    );
}

#[test]
fn a_result_for_a_replaced_query_changes_nothing() {
    let mut app = App::mock();
    run_search_line(&mut app, "benchmarks");
    run_search_line(&mut app, "slides");
    let before = app.search().ids().to_vec();
    assert_eq!(before, vec![6], "the second search stands");

    assert!(
        !app.apply_searched(MOCK_CHAT, "benchmarks", vec![7], 1),
        "an answer for the query the reader has left must not land"
    );

    assert_eq!(app.search().ids().to_vec(), before);
    assert_eq!(app.search_query(), Some("slides"));
}

#[test]
fn a_result_for_another_conversation_changes_nothing() {
    let mut app = App::mock();
    run_search_line(&mut app, "benchmarks");

    assert!(!app.apply_searched(MOCK_CHAT + 1, "benchmarks", vec![7], 1));

    assert_eq!(
        app.search().source(),
        domain::search::SearchSource::Local,
        "the local list is what is still on screen"
    );
}

/// The server's answer replaces the local one rather than being merged with
/// it, and the cursor moves with it.
#[test]
fn a_server_result_replaces_the_local_matches() {
    let mut app = App::mock();
    run_search_line(&mut app, "benchmarks");
    assert_eq!(reading(&app), Some(7));

    assert!(app.apply_searched(MOCK_CHAT, "benchmarks", vec![3, 7], 1_000));

    assert_eq!(app.search().source(), domain::search::SearchSource::Server);
    assert_eq!(app.search().total(), 1_000);
    assert_eq!(app.search().ids().to_vec(), vec![3, 7]);
    assert_eq!(
        reading(&app),
        Some(7),
        "the cursor was on a match the server confirmed, so it stays"
    );
}

// ---- where the reader is -------------------------------------------

#[test]
fn opening_a_conversation_starts_pinned_to_the_newest_message() {
    let app = App::mock();

    assert!(app.conversation.conversation.auto_follow());
    assert_eq!(reading(&app), Some(10));
}

#[test]
fn stepping_up_disengages_following_and_the_end_re_engages_it() {
    let mut app = App::mock();

    app.handle_key(press(KeyCode::Char('k')));
    assert_eq!(reading(&app), Some(9));
    assert!(
        !app.conversation.conversation.auto_follow(),
        "the reader has moved away"
    );

    app.handle_key(press(KeyCode::Char('G')));
    assert_eq!(reading(&app), Some(10));
    assert!(
        app.conversation.conversation.auto_follow(),
        "`G` means the newest message"
    );
}

#[test]
fn a_page_moves_a_screenful_and_the_bottom_re_engages_following() {
    let mut app = App::mock();
    assert_eq!(app.conversation.vim.cursor(), 9);

    app.handle_key(press_ctrl('u'));
    assert_eq!(
        app.conversation.vim.cursor(),
        0,
        "a screenful up from the newest message"
    );
    assert!(!app.conversation.conversation.auto_follow());

    app.handle_key(press_ctrl('d'));
    assert_eq!(app.conversation.vim.cursor(), 9);
    assert!(
        app.conversation.conversation.auto_follow(),
        "back at the newest message"
    );
}

/// The page step is the panel's height, so a taller terminal pages further.
/// Until a frame has been drawn the panel has no measurement, which is why
/// the fallback has to be a sensible size rather than zero.
#[test]
fn a_page_is_as_tall_as_the_panel_was() {
    let mut app = App::mock();
    app.record_rows(4);

    app.handle_key(press_ctrl('u'));
    assert_eq!(
        app.conversation.vim.cursor(),
        5,
        "one panel's worth up from the end"
    );

    app.handle_key(press_ctrl('d'));
    assert_eq!(app.conversation.vim.cursor(), 9);
}

/// A page is a screenful of rows, not a screenful of messages: a message
/// wider than the panel is more than one row, and four rows of one are four
/// rows the reader has moved.
#[test]
fn a_page_moves_by_rows_and_lands_on_a_message() {
    let mut app = App::mock();
    app.record_body(53);
    app.record_rows(4);
    app.apply_latest(vec![
        message(0, "a"),
        Message {
            id: 1,
            text: Cow::Owned("y".repeat(400)),
            ..message(1, "text")
        },
        message(2, "b"),
        message(3, "c"),
    ]);
    go_to_top(&mut app);
    assert_eq!(app.conversation.vim.cursor(), 0);

    app.handle_key(press_ctrl('d'));

    assert_eq!(
        app.conversation.vim.cursor(),
        1,
        "row 4 is inside the message at row 1, which is what the cursor stands on"
    );
}

/// A page steps over rows and lands on a message, so wherever it lands there
/// is a message there: the cursor is what the fetch triggers measure, and a
/// cursor pointing at a row rather than a message is not a thing it can
/// answer.
#[test]
fn a_page_never_leaves_the_cursor_off_a_message() {
    let mut app = App::mock();
    app.record_body(53);
    app.record_rows(4);
    let total = app.conversation.conversation.window.len();
    go_to_top(&mut app);

    for _ in 0..4 {
        app.handle_key(press_ctrl('d'));

        assert!(
            app.row_layout().iter().any(|span| span.kind
                == RowKind::Message {
                    index: app.conversation.vim.cursor()
                }),
            "the layout has a message for the cursor at {}",
            app.conversation.vim.cursor()
        );
    }

    assert_eq!(
        app.conversation.vim.cursor(),
        total - 1,
        "and four pages down of one screenful each is the newest message"
    );
}

/// The first message the panel shows, given a panel `budget` rows tall.
fn shown_from(app: &App, budget: usize) -> usize {
    app.viewport(&app.row_layout(), budget).start
}

#[test]
fn the_viewport_is_a_windowful_ending_at_a_pinned_view() {
    let app = App::mock();

    assert_eq!(
        shown_from(&app, 4),
        6,
        "pinned to the bottom, the slice is the last screenful"
    );
    assert_eq!(shown_from(&app, 99), 0, "a panel taller than the window");
    assert_eq!(
        shown_from(&app, 0),
        9,
        "a panel with no room still shows the newest row"
    );
}

#[test]
fn the_viewport_centres_on_a_cursor_that_is_not_pinned() {
    let mut app = App::mock();
    go_to_top(&mut app);

    assert_eq!(
        shown_from(&app, 4),
        0,
        "the top of the window is the top of the slice"
    );

    app.conversation.vim.set_cursor(5);
    assert_eq!(
        shown_from(&app, 4),
        3,
        "half a panel either side of the cursor"
    );

    app.conversation.vim.set_cursor(9);
    assert_eq!(
        shown_from(&app, 4),
        6,
        "a slice is never taller than the panel, nor starts past the end"
    );
}

// ---- pages ---------------------------------------------------------

#[test]
fn an_older_page_leaves_the_reader_on_the_message_they_were_reading() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('k')));
    assert_eq!(reading(&app), Some(9));

    assert!(app.apply_older(page(&[-1, 0])));

    assert_eq!(app.conversation.conversation.window.len(), 12);
    assert_eq!(
        reading(&app),
        Some(9),
        "the window moved under the reader, not the reader with it"
    );
    assert!(!app.conversation.conversation.auto_follow());
}

#[test]
fn an_older_page_keeps_a_pinned_view_pinned() {
    let mut app = App::mock();

    assert!(app.apply_older(page(&[-1, 0])));

    assert_eq!(reading(&app), Some(10), "the end did not move");
    assert!(app.conversation.conversation.auto_follow());
}

#[test]
fn a_newer_page_stays_behind_what_the_window_holds() {
    let mut app = App::mock();
    go_to_top(&mut app);

    assert!(app.apply_newer(page(&[11, 12])));

    assert_eq!(app.conversation.conversation.window.newest_id(), Some(12));
    assert_eq!(reading(&app), Some(1), "the reader is still at the top");
}

#[test]
fn a_page_for_another_conversation_is_refused() {
    let mut app = App::mock();
    let before = app.conversation.conversation.window.len();

    assert!(!app.apply_latest(vec![stranger(1)]));
    assert!(!app.apply_older(vec![stranger(1)]));
    assert!(!app.apply_newer(vec![stranger(1)]));

    assert_eq!(
        app.conversation.conversation.window.len(),
        before,
        "a page that arrives late must not empty the window it does not belong to"
    );
    assert_eq!(app.conversation.conversation.window.chat_id, MOCK_CHAT);
}

// ---- events from the feed ------------------------------------------

#[test]
fn an_arrival_lands_at_the_bottom_of_a_pinned_view() {
    let mut app = App::mock();
    assert!(app.conversation.conversation.auto_follow());

    assert!(app.apply_update(&UpdateEvent::NewMessage(message(11, "ping"))));

    assert_eq!(
        reading(&app),
        Some(11),
        "a pinned view follows the conversation"
    );
    assert!(app.conversation.conversation.auto_follow());
}

#[test]
fn an_arrival_does_not_move_a_reader_who_scrolled_away() {
    let mut app = App::mock();
    go_to_top(&mut app);
    let reading_before = reading(&app);
    let len_before = app.conversation.conversation.window.len();

    assert!(app.apply_update(&UpdateEvent::NewMessage(message(11, "ping"))));

    assert_eq!(app.conversation.conversation.window.len(), len_before + 1);
    assert_eq!(
        reading(&app),
        reading_before,
        "the reader stays where they were"
    );
    assert!(!app.conversation.conversation.auto_follow());
}

/// An arrival the conversation already holds leaves it alone: the window
/// deduplicates by identifier, so a message cannot sit in it twice.
///
/// The flat window underneath is a different thing — a record of what the
/// client has been sent, which does not deduplicate — so the event still
/// reports a change. That difference is why the two are fed separately
/// rather than one being derived from the other, and it is why the
/// assertion here is about the conversation rather than about the report.
#[test]
fn an_arrival_the_conversation_already_holds_leaves_it_alone() {
    let mut app = App::mock();
    let before = app.conversation.conversation.window.len();

    let moved = app.apply_update(&UpdateEvent::NewMessage(message(10, "again")));

    assert_eq!(
        app.conversation.conversation.window.len(),
        before,
        "the open window holds one copy of the message"
    );
    assert_eq!(
        text_of(&app, 10),
        Some("See you at the demo."),
        "and the message on show keeps the text it arrived with"
    );
    assert!(moved, "while the flat window recorded what it was sent");
}

/// One event, two windows: the message lands in the conversation on show,
/// and the conversation's unread count moves in the list behind it.
#[test]
fn one_arrival_reaches_both_the_window_and_the_list() {
    let mut app = App::mock();
    let before = unread(&app, MOCK_CHAT);

    assert!(app.apply_update(&UpdateEvent::NewMessage(message(11, "ping"))));

    assert_eq!(reading(&app), Some(11), "the window took the message");
    assert_eq!(
        unread(&app, MOCK_CHAT),
        before + 1,
        "and the list counted it, which is what the panel shows"
    );
}

/// The newest message the peer has read, for `chat_id`.
fn read(chat_id: i64, max_id: i64) -> UpdateEvent {
    UpdateEvent::ReadReceipt { chat_id, max_id }
}

/// A read acknowledgement reaching the feed moves the conversation's watermark
/// with nothing else happening: no message arrives, changes or leaves, so the
/// window, the cursor and the reader's place in it are all as they were.
#[test]
fn a_read_acknowledgement_advances_the_open_conversation_and_moves_nothing_else() {
    let mut app = App::mock();
    // Two of the reader's own messages, as the server leaves them: the
    // sample conversation's are every one `Received`, which no sent message
    // ever is.
    app.apply_latest(vec![
        Message {
            id: 20,
            chat_id: MOCK_CHAT,
            text: Cow::Borrowed("mine"),
            timestamp: 1_730_000_600,
            status: MessageStatus::Sent,
            is_outgoing: true,
            reply_to: None,
            media: None,
        },
        Message {
            id: 21,
            chat_id: MOCK_CHAT,
            text: Cow::Borrowed("also mine"),
            timestamp: 1_730_000_660,
            status: MessageStatus::Sent,
            is_outgoing: true,
            reply_to: None,
            media: None,
        },
    ]);
    let before = (
        reading(&app),
        app.conversation.vim.cursor(),
        app.conversation.conversation.window.len(),
    );
    let newest = before.0.expect("the window holds messages");

    assert_eq!(
        rows::group_of(&app, 1).receipt,
        rows::Receipt::None,
        "nothing has been read yet, so the group claims nothing"
    );

    assert!(app.apply_update(&read(MOCK_CHAT, newest)));

    assert_eq!(
        app.conversation.conversation.read_watermark(),
        Some(newest),
        "so the group's state is derived from it on the next frame"
    );
    assert_eq!(
        rows::group_of(&app, 1).receipt,
        rows::Receipt::Read,
        "and the state the panel draws follows the feed, without anything else changing"
    );
    assert_eq!(
        (
            reading(&app),
            app.conversation.vim.cursor(),
            app.conversation.conversation.window.len()
        ),
        before,
        "and nothing about the reader's place moved"
    );
}

/// A receipt about a conversation the reader is not in does not touch the one
/// that is — but it is still remembered, so opening that chat shows how far it
/// has been read (AC-15's read half).
#[test]
fn a_read_acknowledgement_for_another_conversation_is_remembered_and_not_applied() {
    let mut app = App::mock();
    let other = MOCK_CHAT + 1;

    assert!(app.apply_update(&read(other, 7)));

    assert_eq!(
        app.conversation.conversation.read_watermark(),
        None,
        "the conversation on show is a different one"
    );

    app.select_chat(1);
    assert_eq!(
        app.conversation.conversation.read_watermark(),
        Some(7),
        "and the chat that owns it is opened carrying what it was told"
    );
}

/// The watermark is a fact about a conversation and not about the page on
/// show, so looking away and coming back finds it as it was.
#[test]
fn a_read_watermark_survives_a_switch_away_and_back() {
    let mut app = App::mock();
    assert!(app.apply_update(&read(MOCK_CHAT, 6)));

    app.select_chat(1);
    app.select_chat(0);

    assert_eq!(
        app.conversation.conversation.read_watermark(),
        Some(6),
        "the view is new and the conversation's reading is not"
    );
}

/// The wire's watermark can repeat or arrive late, so a lower one is dropped
/// rather than applied, and an acknowledgement that named nothing real is not
/// recorded at all. Both report no change, which is what tells the loop there
/// is nothing to redraw for.
#[test]
fn a_read_acknowledgement_that_says_nothing_new_changes_nothing() {
    let mut app = App::mock();
    assert!(app.apply_update(&read(MOCK_CHAT, 9)));

    for (chat_id, max_id) in [(MOCK_CHAT, 9), (MOCK_CHAT, 4), (MOCK_CHAT, 0)] {
        assert!(
            !app.apply_update(&read(chat_id, max_id)),
            "{chat_id} read up to {max_id} says nothing the reader has not been shown"
        );
    }
    assert_eq!(app.conversation.conversation.read_watermark(), Some(9));
}

/// A receipt while the reader is scrolled back up moves neither the cursor nor
/// the reader's place: a read is not a message arriving, and it is not entitled
/// to move anyone (US-X4).
#[test]
fn a_read_acknowledgement_leaves_a_scrolled_up_reader_where_they_are() {
    let mut app = App::mock();
    app.handle_key(press_ctrl('u'));
    let before = (
        app.conversation.vim.cursor(),
        app.conversation.conversation.auto_follow(),
    );
    assert!(
        !app.conversation.conversation.auto_follow(),
        "the fixture is scrolled back up"
    );

    assert!(app.apply_update(&read(MOCK_CHAT, 10)));

    assert_eq!(
        (
            app.conversation.vim.cursor(),
            app.conversation.conversation.auto_follow()
        ),
        before,
        "and still there after it"
    );
}

// ---- what a window with separators does to search and to motions -----

/// A window spanning three days, with a separator between each and a word
/// worth searching for in more than one of them.
///
/// Every message is the reader's own, a minute apart within its day, so a day
/// is three messages in one group — the shape PR 8 puts on the screen at once:
/// a group, a separator, and (with a watermark) a receipt.
fn across_three_days() -> App {
    let mut app = App::mock();
    app.record_body(53);
    app.conversation
        .conversation
        .window
        .replace((1..=9).map(message_of_day));
    app.conversation.vim.set_total(9);
    app.conversation.vim.set_cursor(8);

    app
}

/// A search finds messages and lands on messages, whatever else is in the
/// window: a separator names no message, carries no identifier, and cannot be
/// walked onto however the walk arrived (AC-20).
#[test]
fn a_search_in_a_window_with_separators_matches_and_lands_on_messages_only() {
    let mut app = across_three_days();
    let layout = app.row_layout();
    let separators: Vec<usize> = layout
        .iter()
        .filter(|span| !span.kind.is_message())
        .map(|span| span.first)
        .collect();
    assert_eq!(separators.len(), 3, "one for each of the three days");

    app.run_search("benchmarks");
    let matched = app.search().ids().to_vec();
    assert_eq!(
        matched,
        vec![102, 104, 106, 108],
        "only message identifiers, which is all a match can be"
    );

    // Every landing the walk can produce, in both directions and across the
    // wrap, is a message row rather than one of the separator rows.
    for key in ['n', 'n', 'n', 'N', 'N'] {
        app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
        let on = app.conversation.vim.cursor();
        assert!(
            !separators.contains(&rows::first_row_of_message(&layout, on).expect("a message")),
            "{key} landed on a separator row at message {on}"
        );
        assert!(
            layout
                .iter()
                .any(|span| { span.kind.index() == Some(on) && !span.first.eq(&separators[0]) }),
            "{key} left the cursor on a message"
        );
    }
}

/// The landing positions are the messages themselves, which is the whole claim:
/// walking a search crosses separators because it never had to stop on one.
#[test]
fn every_search_landing_is_the_position_of_a_message_it_matched() {
    let mut app = across_three_days();

    app.run_search("benchmarks");
    for _ in 0..5 {
        let on = app.conversation.vim.cursor();
        let id = app
            .conversation
            .conversation
            .window
            .get(on)
            .expect("the cursor names a message the window holds")
            .id;
        assert!(
            app.search().is_match(id),
            "the cursor is on message {id}, which matched"
        );
        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    }
}

/// Every motion in the vocabulary lands on a message. The cursor counts
/// messages, so this is structural — but a separator is a row on the screen
/// and only a test proves the two never come apart (AC-21, US-X6).
#[test]
fn no_motion_lands_on_a_separator_row() {
    let mut app = across_three_days();
    let separator_rows: Vec<usize> = app
        .row_layout()
        .iter()
        .filter(|span| !span.kind.is_message())
        .map(|span| span.first)
        .collect();

    let keys = [
        KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        press_ctrl('d'),
        press_ctrl('u'),
        press_ctrl('d'),
    ];
    for key in keys {
        app.handle_key(key);
        let on = app.conversation.vim.cursor();
        assert!(
            app.conversation.conversation.window.get(on).is_some(),
            "the cursor at {on} names a message the window holds"
        );
        assert!(
            !separator_rows.contains(
                &rows::first_row_of_message(&app.row_layout(), on)
                    .expect("the message is laid out")
            ),
            "and that message is not a separator row"
        );
    }
}

/// `gg` and `G` are the two ends of the window, and each is a message: the
/// first message of the window is below the first day's separator, and the
/// last is the last message rather than a row after it.
#[test]
fn the_ends_of_the_window_are_messages_and_not_separators() {
    let mut app = across_three_days();
    app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    assert_eq!(
        app.conversation.vim.cursor(),
        0,
        "gg lands on the first message"
    );

    app.handle_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE));
    assert_eq!(
        app.conversation.vim.cursor(),
        8,
        "and G on the newest, neither of which is a separator row"
    );
}

/// Grouping, separators and read state in one window, at the layer that
/// draws them: three groups of three, a separator before each, and the newest
/// group read.
#[test]
fn grouping_separators_and_read_state_agree_in_one_window() {
    let mut app = across_three_days();
    assert!(app.apply_update(&read(MOCK_CHAT, 109)));

    let layout = app.row_layout();
    assert_eq!(
        layout.len(),
        12,
        "nine messages and three separators: {:?}",
        layout.iter().map(|span| span.first).collect::<Vec<_>>()
    );
    // Each separator is the entry immediately before a message, so a day
    // opens with a message one row below its own separator.
    for separator in [0_usize, 4, 8] {
        assert!(
            !layout[separator].kind.is_message(),
            "row {separator} is the separator"
        );
        let under = &layout[separator + 1];
        assert_eq!(
            under.kind.index(),
            Some((separator / 4) * 3),
            "and the message below it opens that day"
        );
    }
    assert!(
        rows::group_of(&app, 0).first,
        "a day boundary is a group break as well as a separator"
    );
    assert_eq!(
        rows::group_of(&app, 8).receipt,
        rows::Receipt::Read,
        "and the newest group is read"
    );
    assert_eq!(
        rows::group_of(&app, 4).receipt,
        rows::Receipt::None,
        "while the first is not: the watermark covers only the last"
    );
}

/// Looking away and coming back leaves everything PR 8 derives from the window
/// as it was, and restores the one thing the window does not carry (US-X1).
///
/// The window itself is replaced by the switch, so grouping and the separators
/// are worked out again from the same messages and the read state is put back
/// from what the feed said — which is what makes this a test of the
/// arrangement rather than of a value that never moved.
#[test]
fn grouping_separators_and_read_state_survive_a_switch_away_and_back() {
    let mut app = across_three_days();
    // Through the feed rather than the view's setter: what the feed says is
    // what survives the switch, and a figure nobody was told is not recorded.
    assert!(app.apply_update(&read(MOCK_CHAT, 109)));
    let before = app.row_layout();

    app.select_chat(1);
    app.select_chat(0);
    // The same messages, in the same order, as the conversation being reopened
    // is filled.
    app.conversation
        .conversation
        .window
        .replace((1..=9).map(message_of_day));
    app.conversation.vim.set_total(9);

    assert_eq!(
        app.row_layout(),
        before,
        "the same rows in the same places, worked out again rather than kept"
    );
    assert_eq!(
        app.conversation.conversation.read_watermark(),
        Some(109),
        "and the reading of the conversation, which the switch replaced"
    );
}

/// The messages [`across_three_days`] is built from, so a test can put the
/// same conversation back after a switch without spelling them out twice.
fn message_of_day(id: i64) -> Message {
    let day = (id - 1) / 3;
    Message {
        id: 100 + id,
        chat_id: MOCK_CHAT,
        text: if id % 2 == 0 {
            Cow::Borrowed("benchmarks and more")
        } else {
            Cow::Borrowed("text")
        },
        timestamp: 1_730_000_000 + day * 86_400 + (id - 1) % 3 * 60,
        status: MessageStatus::Sent,
        is_outgoing: true,
        reply_to: None,
        media: None,
    }
}

/// What PR 8 added to memory is bounded, and this is the part of AC-22 that
/// can be asserted rather than audited.
///
/// **No RSS claim is made.** The project has no measurement harness — no
/// `heaptrack`, no `massif` target, and `docs/memory.md` records the 50 MB
/// ceiling as unmeasured (OQ-09) — so what is checked here is the property
/// that makes the ceiling plausible: nothing PR 8 added grows with history
/// beyond the window that was already bounded.
///
/// Two claims, then: the layout is rebuilt every frame rather than kept, and
/// what it holds is one entry per message plus one per day; and the read state
/// is a single number per conversation however many acknowledgements arrive.
#[test]
fn what_pr_eight_holds_is_bounded_by_the_window_and_the_conversation_count() {
    let mut app = App::mock();
    app.record_body(53);

    // A full window with a day per message: the worst case for separators,
    // and still one row each.
    app.conversation
        .conversation
        .window
        .replace((0..CONVERSATION_WINDOW).map(|day| {
            let day = i64::try_from(day).expect("a window index fits a timestamp");
            Message {
                id: 1_000 + day,
                chat_id: MOCK_CHAT,
                text: Cow::Borrowed("text"),
                timestamp: 1_730_000_000 + day * 86_400,
                status: MessageStatus::Sent,
                is_outgoing: true,
                reply_to: None,
                media: None,
            }
        }));
    app.conversation.vim.set_total(CONVERSATION_WINDOW);

    let layout = app.row_layout();
    assert_eq!(
        layout.len(),
        CONVERSATION_WINDOW * 2,
        "one entry per message and one separator per day, and nothing else"
    );
    assert_eq!(
        rows::total_rows(&layout),
        CONVERSATION_WINDOW * 2,
        "and one row each: no message grew and no separator did"
    );

    // Read state: one number per conversation, whatever arrives.
    for chat_id in [MOCK_CHAT, MOCK_CHAT + 1] {
        for max_id in [1, 5, 3, 9] {
            let _ = app.apply_update(&read(chat_id, max_id));
        }
    }
    assert_eq!(
        app.drafts.read_receipts.borrow().len(),
        2,
        "four acknowledgements for each of two conversations, and two numbers"
    );
}

/// An event for a conversation the client does not hold has nowhere to go:
/// neither window can apply it, so nothing observable moved.
#[test]
fn an_arrival_for_an_unknown_conversation_changes_nothing() {
    let mut app = App::mock();

    assert!(!app.apply_update(&UpdateEvent::NewMessage(unknown(11))));
    assert_eq!(app.conversation.conversation.window.len(), 10);
}

/// A conversation other than the one on show is still one the list holds,
/// so the arrival reaches the list and stops there.
#[test]
fn an_arrival_for_another_conversation_reaches_the_list_alone() {
    let mut app = App::mock();
    let before = app.conversation.conversation.window.len();

    assert!(
        app.apply_update(&UpdateEvent::NewMessage(stranger(11))),
        "the list holds the conversation the message belongs to"
    );
    assert_eq!(
        app.conversation.conversation.window.len(),
        before,
        "but the window on show is a different conversation"
    );
}

#[test]
fn an_edit_reaches_the_open_conversation() {
    let mut app = App::mock();
    let edit = UpdateEvent::MessageEdited {
        chat_id: MOCK_CHAT,
        message_id: 3,
        new_text: Cow::Borrowed("corrected"),
    };

    assert!(app.apply_update(&edit));
    assert_eq!(text_of(&app, 3), Some("corrected"));
    assert!(
        !app.apply_update(&edit),
        "the same text twice is not a change"
    );
}

#[test]
fn a_deletion_takes_the_message_out_of_the_open_conversation() {
    let mut app = App::mock();

    assert!(app.apply_update(&UpdateEvent::MessagesDeleted {
        message_ids: vec![3],
    }));

    assert_eq!(text_of(&app, 3), None);
    assert_eq!(app.conversation.conversation.window.len(), 9);
    assert_eq!(
        reading(&app),
        Some(10),
        "the reader was on the newest message and still is"
    );
}

/// Three same-side messages a minute apart: one group, three identifiers.
fn a_group_of_three() -> Vec<Message> {
    [0, 60, 120]
        .into_iter()
        .map(|seconds| Message {
            id: 100 + seconds,
            chat_id: MOCK_CHAT,
            text: Cow::Borrowed("text"),
            timestamp: 1_730_000_000 + seconds,
            status: MessageStatus::Sent,
            is_outgoing: true,
            reply_to: None,
            media: None,
        })
        .collect()
}

/// Where each message of the window stands in its group.
fn places(app: &App) -> Vec<rows::Grouped> {
    (0..app.conversation.conversation.window.len())
        .map(|index| rows::group_of(app, index))
        .collect()
}

/// An edit is a new text for a message that is already there, so it moves no
/// group boundary: membership is decided by who is talking, when, and what
/// the message is about — none of which an edit touches.
#[test]
fn an_edit_inside_a_group_changes_no_group_boundary() {
    let mut app = App::mock();
    app.conversation
        .conversation
        .window
        .replace(a_group_of_three());
    app.record_body(53);

    assert!(app.apply_update(&UpdateEvent::MessageEdited {
        chat_id: MOCK_CHAT,
        message_id: 160,
        new_text: Cow::Borrowed("corrected"),
    }));

    assert_eq!(text_of(&app, 160), Some("corrected"));
    assert_eq!(
        places(&app),
        vec![
            rows::Grouped {
                first: true,
                last: false,
                receipt: rows::Receipt::None
            },
            rows::Grouped {
                first: false,
                last: false,
                receipt: rows::Receipt::None
            },
            rows::Grouped {
                first: false,
                last: true,
                receipt: rows::Receipt::None
            },
        ],
        "the group is the same one it was"
    );
}

/// A deletion takes the message out and leaves the survivors grouped; a
/// group with nothing left in it leaves no row behind either.
#[test]
fn a_deletion_inside_a_group_leaves_the_survivors_grouped() {
    let mut app = App::mock();
    app.conversation
        .conversation
        .window
        .replace(a_group_of_three());
    app.record_body(53);

    assert!(app.apply_update(&UpdateEvent::MessagesDeleted {
        message_ids: vec![160],
    }));

    assert_eq!(
        places(&app),
        vec![
            rows::Grouped {
                first: true,
                last: false,
                receipt: rows::Receipt::None
            },
            rows::Grouped {
                first: false,
                last: true,
                receipt: rows::Receipt::None
            },
        ],
        "the two that are left are still one group"
    );
    let layout = app.row_layout();
    assert_eq!(layout.len(), 3, "two messages and the day's separator");
    assert_eq!(
        rows::total_rows(&layout),
        3,
        "and no row for what was deleted"
    );

    // An emptied group leaves nothing at all: no entry of its own, and no
    // rows for the scrollbar to count.
    assert!(app.apply_update(&UpdateEvent::MessagesDeleted {
        message_ids: vec![100, 220],
    }));
    assert!(app.row_layout().is_empty());
    assert_eq!(rows::total_rows(&app.row_layout()), 0);
}

// ---- fetching ------------------------------------------------------

/// The margin is what stops a fetch from being asked for at every
/// keystroke: near an end, once, and not again while one is in flight.
#[test]
fn a_page_is_asked_for_near_an_end_and_not_before() {
    let mut app = App::mock();
    app.apply_latest(page(&(1..=60).collect::<Vec<_>>()));

    assert!(
        !app.wants_older(),
        "the reader is at the end, not the start"
    );
    assert!(
        !app.wants_newer(),
        "a view pinned to the newest message has nothing to catch up on"
    );

    app.handle_key(press(KeyCode::Char('k')));
    assert!(!app.wants_older());
    assert!(
        app.wants_newer(),
        "the reader has stepped away from the end"
    );

    go_to_top(&mut app);
    assert!(
        app.wants_older(),
        "the reader is at the top of what is loaded"
    );
    assert!(!app.wants_newer());
}

/// The margin is counted in rows, which is what a reader scrolling upwards
/// is counting. Twenty messages that came to fill four rows each is eighty
/// rows of conversation, and a reader on the fourth of them is nowhere near
/// the top of it.
#[test]
fn a_page_is_asked_for_by_rows_rather_than_by_messages() {
    let mut app = App::mock();
    app.record_body(53);
    app.apply_latest(tall_page(10));
    app.conversation.vim.set_cursor(3);

    assert!(
        !app.wants_older(),
        "message 4 begins at row {}, and what is in front of it is a screenful of text rather than one line of window",
        rows::first_row_of_message(&app.row_layout(), 3).expect("the message is laid out")
    );

    app.conversation.vim.set_cursor(0);
    assert!(
        app.wants_older(),
        "and the reader on the first message is near the top of both"
    );
}

#[test]
fn a_fetch_in_flight_is_not_asked_for_twice() {
    let mut app = App::mock();
    assert!(
        app.wants_older(),
        "a window shorter than the margin is near its start"
    );

    app.begin_fetch(FetchDirection::Older);
    assert!(app.is_fetching(FetchDirection::Older));
    assert!(!app.wants_older(), "one page per direction at a time");

    app.end_fetch(FetchDirection::Older);
    assert!(!app.is_fetching(FetchDirection::Older));
    assert!(app.wants_older(), "the direction is open again");
}

/// The directions are tracked apart, so a page in flight in one of them
/// does not hold up the others.
#[test]
fn the_directions_are_tracked_apart() {
    let mut app = App::mock();

    app.begin_fetch(FetchDirection::Latest);
    app.begin_fetch(FetchDirection::Older);

    assert!(app.is_fetching(FetchDirection::Latest));
    assert!(app.is_fetching(FetchDirection::Older));
    assert!(!app.is_fetching(FetchDirection::Newer));

    app.end_fetch(FetchDirection::Latest);
    assert!(!app.is_fetching(FetchDirection::Latest));
    assert!(
        app.is_fetching(FetchDirection::Older),
        "releasing one says nothing about the rest"
    );
}

/// Opening another conversation forgets what was in flight for the old one:
/// the page is coming for a window that is no longer on screen.
#[test]
fn opening_a_conversation_forgets_what_was_in_flight() {
    let mut app = App::mock();
    app.begin_fetch(FetchDirection::Latest);
    app.begin_fetch(FetchDirection::Older);

    app.select_chat(1);

    for direction in [
        FetchDirection::Latest,
        FetchDirection::Older,
        FetchDirection::Newer,
    ] {
        assert!(
            !app.is_fetching(direction),
            "{direction:?} is still in flight"
        );
    }
}

/// A conversation with nothing loaded has no message to count a page from,
/// so the newest page is the only one it can be given.
#[test]
fn an_empty_conversation_is_near_neither_of_its_ends() {
    let mut app = App::mock();
    app.select_chat(1);

    assert!(app.conversation.conversation.window.is_empty());
    assert!(!app.wants_older());
    assert!(!app.wants_newer());
}

#[test]
fn a_direction_the_conversation_has_run_out_of_is_not_asked_for() {
    let mut app = App::mock();
    assert!(app.wants_older());

    app.exhaust(FetchDirection::Older);
    assert!(
        !app.wants_older(),
        "there is nothing in front of the oldest message"
    );

    app.exhaust(FetchDirection::Newer);
    assert!(!app.wants_newer(), "nor behind the newest one");
}

// ---- a conversation opened from the cache --------------------------

/// The conversation the cache tests open: the second in the sample list,
/// which has nothing loaded for it.
const CACHED_CHAT: i64 = 2;

/// Messages of [`CACHED_CHAT`], as the cache would hand them over.
fn cached(ids: &[i64]) -> Vec<Message> {
    ids.iter()
        .map(|id| Message {
            chat_id: CACHED_CHAT,
            ..message(*id, "cached")
        })
        .collect()
}

/// The identifiers the window holds, oldest first.
fn window_ids(app: &App) -> Vec<i64> {
    app.conversation
        .conversation
        .window
        .iter()
        .map(|message| message.id)
        .collect()
}

/// [`CACHED_CHAT`] opened, seeded from the cache, and its newest page asked
/// for — the order the driver does them in.
fn revalidating(ids: &[i64]) -> App {
    let mut app = App::mock();
    app.select_chat(1);
    assert!(
        app.seed_from_cache(CACHED_CHAT, cached(ids)),
        "the seed was expected to land"
    );
    app.begin_fetch(FetchDirection::Latest);
    app
}

/// A warm cache paints the conversation before the network has answered: the
/// rows are there, the reader is at the newest of them, and the page that will
/// replace them is still on its way.
#[test]
fn a_warm_cache_fills_a_conversation_before_its_page_lands() {
    let mut app = App::mock();
    app.select_chat(1);
    assert!(app.apply_update(&read(CACHED_CHAT, 6)));

    assert!(app.seed_from_cache(CACHED_CHAT, cached(&[5, 6, 7])));

    assert_eq!(window_ids(&app), [5, 6, 7]);
    assert_eq!(app.conversation.vim.total(), 3);
    assert_eq!(app.conversation.vim.cursor(), 2, "at the newest message");
    assert!(app.conversation.conversation.auto_follow());
    assert_eq!(
        app.conversation.conversation.read_watermark(),
        Some(6),
        "the watermark is the feed's, and the cache does not move it"
    );
    assert!(
        !app.is_revalidating(),
        "nothing is on its way until the page is asked for"
    );

    app.begin_fetch(FetchDirection::Latest);

    assert!(app.is_revalidating());
    assert_eq!(app.status_text(), REVALIDATING_LABEL);
}

/// The server's newest page replaces the cached rows through the path every
/// newest page takes, and the wait is over.
#[test]
fn the_newest_page_replaces_what_the_cache_showed() {
    let mut app = revalidating(&[5, 6, 7]);
    app.conversation.vim.set_cursor(0);

    app.end_fetch(FetchDirection::Latest);
    assert!(app.apply_latest(cached(&[6, 7, 8, 9])));

    assert_eq!(
        window_ids(&app),
        [6, 7, 8, 9],
        "nothing of the seed is left"
    );
    assert_eq!(app.conversation.vim.total(), 4);
    assert_eq!(app.conversation.vim.cursor(), 3, "at the newest message");
    assert!(!app.conversation.cached);
    assert!(!app.is_revalidating());
    assert_ne!(app.status_text(), REVALIDATING_LABEL);
}

/// A seed is only for a conversation that has just been opened and has
/// nothing better on show: any other is refused and leaves the window as it
/// was.
#[test]
fn a_seed_is_refused_where_it_would_cover_something() {
    // Nothing open.
    let mut app = App::new();
    assert!(!app.seed_from_cache(CACHED_CHAT, cached(&[1])));

    // Another conversation than the one on show.
    let mut app = App::mock();
    app.select_chat(1);
    assert!(!app.seed_from_cache(3, cached(&[1])));
    assert!(app.conversation.conversation.window.is_empty());

    // The right identifier, but messages from somewhere else.
    assert!(!app.seed_from_cache(CACHED_CHAT, page(&[1, 2])));
    assert!(app.conversation.conversation.window.is_empty());
    assert!(!app.conversation.cached);

    // A window that already holds the conversation.
    let mut app = App::mock();
    assert!(!app.seed_from_cache(MOCK_CHAT, page(&[1, 2])));
    assert_eq!(window_ids(&app), (1..=10).collect::<Vec<_>>());

    // The newest page has already landed.
    let mut app = App::mock();
    app.select_chat(1);
    assert!(app.apply_latest(cached(&[8, 9])));
    assert!(!app.seed_from_cache(CACHED_CHAT, cached(&[1, 2, 3])));
    assert_eq!(window_ids(&app), [8, 9]);
    assert!(!app.conversation.cached);
}

/// An empty cache is no seed at all, and the conversation opens exactly as
/// it did before there was one: empty, and waiting on the `Loading…` row.
#[test]
fn a_cold_cache_opens_a_conversation_as_before() {
    let mut app = App::mock();
    app.select_chat(1);

    assert!(!app.seed_from_cache(CACHED_CHAT, Vec::new()));
    app.begin_fetch(FetchDirection::Latest);

    assert!(app.conversation.conversation.window.is_empty());
    assert!(!app.conversation.cached);
    assert!(!app.is_revalidating(), "the `Loading…` row says this one");
    assert_ne!(app.status_text(), REVALIDATING_LABEL);
}

/// The feed does not wait for the revalidation: an arrival lands on the
/// cached rows the way it lands on any window, and the newest page then
/// replaces both.
#[test]
fn an_arrival_lands_on_a_cached_window() {
    let mut app = revalidating(&[5, 6, 7]);

    assert!(app.apply_update(&UpdateEvent::NewMessage(Message {
        chat_id: CACHED_CHAT,
        ..message(8, "live")
    })));

    assert_eq!(window_ids(&app), [5, 6, 7, 8]);
    assert_eq!(
        app.conversation.vim.cursor(),
        3,
        "a following view follows it"
    );
    assert!(app.is_revalidating(), "an arrival is not the newest page");

    app.end_fetch(FetchDirection::Latest);
    assert!(app.apply_latest(cached(&[6, 7, 8])));
    assert_eq!(window_ids(&app), [6, 7, 8]);
}

/// A cached window is not paged from while its newest page is on its way:
/// that page replaces it, and the cursor that would anchor a page describes
/// nothing yet. Once the page lands, paging is what it always was.
#[test]
fn a_cached_window_is_not_paged_from_until_its_page_lands() {
    let mut app = revalidating(&[5]);

    assert!(!app.wants_older());
    app.conversation.conversation.unfollow();
    assert!(!app.wants_newer());
    app.conversation.conversation.follow();

    app.end_fetch(FetchDirection::Latest);
    assert!(app.apply_latest(cached(&[5])));

    assert!(
        app.wants_older(),
        "a one-message window is near its start, and asks"
    );
}

/// A revalidation that fails says so like any other page, and the cached
/// rows stay readable underneath: the cache never talks over a failure.
#[test]
fn a_failed_revalidation_still_says_so() {
    let mut app = revalidating(&[5, 6, 7]);

    // Written while the page is still on its way, the failure outranks the
    // wait.
    app.ui.status = "history: flood wait".to_owned();
    assert_eq!(app.status_text(), "history: flood wait");

    app.end_fetch(FetchDirection::Latest);

    assert_eq!(app.status_text(), "history: flood wait");
    assert!(!app.is_revalidating(), "nothing is on its way now");
    assert_eq!(window_ids(&app), [5, 6, 7], "and the rows are still there");
}

/// Leaving a cached conversation leaves its flag behind: the next one opens
/// as though nothing had been seeded.
#[test]
fn leaving_a_cached_conversation_forgets_where_it_came_from() {
    let mut app = revalidating(&[5, 6, 7]);

    app.select_chat(2);
    assert!(!app.conversation.cached);

    app.select_chat(1);
    app.begin_fetch(FetchDirection::Latest);
    assert!(!app.is_revalidating());
}

// ---- `gg` and the unread messages ----------------------------------

/// `gg` is Vim's top-of-buffer when there is nothing unread to be taken to,
/// which is the conversation the reader is already in.
#[test]
fn gg_with_nothing_unread_is_the_top_of_the_window() {
    let mut app = App::mock();

    go_to_top(&mut app);

    assert_eq!(app.conversation.vim.cursor(), 0);
    assert_eq!(app.pending_jump(), None, "there is nowhere to be taken to");
    assert!(!app.conversation.conversation.auto_follow());
}

#[test]
fn gg_with_no_conversation_open_moves_nothing() {
    let mut app = App::new();

    go_to_top(&mut app);

    assert_eq!(app.conversation.vim.cursor(), 0);
    assert_eq!(app.pending_jump(), None);
}

/// The unread messages are the newest ones there are, so a window that ends
/// where the conversation does holds them: `gg` lands on the first of them
/// without a round trip.
#[test]
fn gg_with_unread_loaded_lands_on_the_first_of_them() {
    let mut app = with_unread(2, 10);

    go_to_top(&mut app);

    assert_eq!(
        reading(&app),
        Some(9),
        "the newest message is 10, and two of them are unread"
    );
    assert_eq!(app.pending_jump(), None, "so no page was needed");
    assert!(
        !app.conversation.conversation.auto_follow(),
        "the reader moved off the end"
    );
}

/// Identifiers have gaps wherever messages were deleted, so counting back
/// from the newest by number can name a message that does not exist. A window
/// that ends where the conversation does is the exception: the unread
/// messages are the newest ones there are, so they are counted back by
/// position and land exactly.
#[test]
fn a_window_that_ends_the_conversation_lands_where_the_numbers_do_not() {
    let mut app = with_unread(3, 20);
    app.apply_latest(page(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 20]));

    go_to_top(&mut app);

    assert_eq!(
        reading(&app),
        Some(8),
        "the third from the end — counting back three from 20 would name 18, \
         which is not a message this conversation has"
    );
    assert_eq!(app.pending_jump(), None);
}

/// Counting from the end is only an answer when the window reaches the end,
/// and only when the unread messages fit inside it.
#[test]
fn counting_from_the_end_needs_a_window_that_holds_the_unread_ones() {
    assert_eq!(coordinate::landing_position(10, 0), None, "nothing unread");
    assert_eq!(coordinate::landing_position(10, 3), Some(7));
    assert_eq!(
        coordinate::landing_position(3, 3),
        Some(0),
        "the whole window"
    );
    assert_eq!(
        coordinate::landing_position(3, 4),
        None,
        "they reach past the window, so counting them from the end would land \
         on a message that is not one of them"
    );
    assert_eq!(
        coordinate::landing_position(0, 1),
        None,
        "and an empty window"
    );
}

/// A target the window does not hold is handed to the caller, and asking
/// again while it is on its way produces the same intent rather than another
/// one: holding the key must not stack requests.
#[test]
fn a_jump_the_window_cannot_answer_is_asked_for_once() {
    let mut app = with_unread_out_of_reach(2);

    go_to_top(&mut app);

    let expected = Jump {
        peer_id: MOCK_CHAT,
        target_id: 19,
        kind: JumpKind::Unread,
    };
    assert_eq!(
        app.pending_jump(),
        Some(expected),
        "counting back two from 20"
    );

    go_to_top(&mut app);
    assert_eq!(app.pending_jump(), Some(expected), "the same place, once");
}

/// The completion puts the reader on the message they jumped to, and the
/// window it landed in is surrounded by the unknown on both sides.
#[test]
fn a_jump_lands_the_reader_on_the_message_it_was_for() {
    let mut app = with_unread_out_of_reach(2);
    go_to_top(&mut app);
    assert!(app.pending_jump().is_some());

    assert!(app.apply_jump(&page(&[16, 17, 18, 19, 20]), 19));

    assert_eq!(reading(&app), Some(19));
    assert_eq!(app.pending_jump(), None, "the jump is over");
    assert!(!app.conversation.conversation.auto_follow());
    assert!(
        !app.conversation.conversation.window.exhausted_older
            && !app.conversation.conversation.window.exhausted_newer,
        "a window that jumped has no edge the one before it can vouch for"
    );
}

/// A page that does not hold the target: the reader is put on the first
/// message after it, which is the nearest the page came.
#[test]
fn a_jump_that_missed_its_target_lands_on_the_nearest_message_after_it() {
    let mut app = with_unread_out_of_reach(2);
    go_to_top(&mut app);

    assert!(app.apply_jump(&page(&[16, 17, 20, 21]), 19));

    assert_eq!(reading(&app), Some(20));
}

/// And an estimate past everything the page holds lands on the newest of it:
/// an estimate that outran the conversation, which the nearest survivor
/// answers honestly.
#[test]
fn a_jump_past_the_page_lands_on_its_newest_message() {
    let mut app = with_unread_out_of_reach(2);
    go_to_top(&mut app);

    assert!(app.apply_jump(&page(&[1, 2, 3]), 19));

    assert_eq!(reading(&app), Some(3));
}

/// However it ended, the jump is over: an empty page leaves the reader where
/// they were rather than wedging the key.
#[test]
fn a_jump_that_came_back_empty_leaves_the_reader_where_they_were() {
    let mut app = with_unread_out_of_reach(2);
    go_to_top(&mut app);
    let before = app.conversation.conversation.window.len();

    assert!(!app.apply_jump(&[], 19));

    assert_eq!(app.pending_jump(), None, "the key is free again");
    assert_eq!(
        app.conversation.conversation.window.len(),
        before,
        "and the window is untouched"
    );
}

/// A page for a jump the reader has abandoned: opening another conversation
/// is the reader saying they are no longer going there.
#[test]
fn a_jump_for_a_conversation_that_is_no_longer_open_is_dropped() {
    let mut app = with_unread_out_of_reach(2);
    go_to_top(&mut app);
    assert!(app.pending_jump().is_some());

    app.select_chat(1);

    assert!(!app.apply_jump(&page(&[16, 17, 18, 19, 20]), 19));
    assert_eq!(app.pending_jump(), None);
    assert!(
        app.conversation.conversation.window.is_empty(),
        "the conversation that was opened kept its empty window"
    );
}

/// A page naming another conversation is refused even when the target
/// matches: a window belongs to one conversation.
#[test]
fn a_jump_page_for_another_conversation_is_refused() {
    let mut app = with_unread_out_of_reach(2);
    go_to_top(&mut app);
    let before = app.conversation.conversation.window.len();

    assert!(!app.apply_jump(&[stranger(19)], 19));

    assert_eq!(app.conversation.conversation.window.len(), before);
    assert_eq!(app.pending_jump(), None, "and the jump is over");
}

/// A page for a target nobody is waiting for: the reader asked for one
/// place, and the fetch that comes back is for another.
#[test]
fn a_jump_page_for_another_target_is_refused() {
    let mut app = with_unread_out_of_reach(2);
    go_to_top(&mut app);
    let before = app.conversation.conversation.window.len();

    assert!(!app.apply_jump(&page(&[16, 17, 18, 19, 20]), 18));

    assert_eq!(app.conversation.conversation.window.len(), before);
    assert_eq!(
        app.pending_jump(),
        Some(Jump {
            peer_id: MOCK_CHAT,
            target_id: 19,
            kind: JumpKind::Unread,
        }),
        "the jump the reader did ask for is still the one being waited on"
    );
}

/// While a jump is on its way only `Esc` answers: a key that moved the cursor
/// would move it out from under the page that is coming, so `G` no longer
/// gets to say "take me to the end instead". `Esc` drops the jump and leaves
/// the reader where they were, and the page is dropped when it lands.
#[test]
fn escape_is_the_only_answer_while_a_jump_is_in_flight() {
    let mut app = with_unread_out_of_reach(2);
    go_to_top(&mut app);
    assert!(app.pending_jump().is_some());
    let before = app.conversation.vim.cursor();

    app.handle_key(press(KeyCode::Char('G')));

    assert_eq!(
        app.pending_jump().map(|jump| jump.target_id),
        Some(19),
        "another key is swallowed rather than answered"
    );
    assert_eq!(app.conversation.vim.cursor(), before, "and moves nothing");

    app.handle_key(press(KeyCode::Esc));
    assert_eq!(app.pending_jump(), None, "`Esc` drops the jump");
    assert_eq!(
        app.conversation.vim.cursor(),
        before,
        "and leaves the reader put"
    );
    assert!(
        !app.apply_jump(&page(&[16, 17, 18, 19, 20]), 19),
        "the page that was on its way has nobody waiting for it"
    );
}

/// `G` after the jump is over still means what it always meant.
#[test]
fn the_end_of_the_conversation_still_follows_the_reader() {
    let mut app = with_unread_out_of_reach(2);
    go_to_top(&mut app);
    go_to_top(&mut app);
    assert!(app.pending_jump().is_some());

    app.handle_key(press(KeyCode::Esc));

    app.handle_key(press(KeyCode::Char('G')));
    assert!(app.conversation.conversation.auto_follow());
}

/// The contrapositive of what used to hold: a window that was replaced no
/// longer invalidates the match list, because a match is a message
/// identifier rather than a position in the window that was on screen.
#[test]
fn a_jump_keeps_the_match_list() {
    let mut app = with_unread_out_of_reach(2);
    run_search_line(&mut app, "text");
    assert!(
        app.search().is_match(3),
        "the loaded window matched message 3"
    );

    go_to_top(&mut app);
    assert!(app.apply_jump(&page(&[16, 17, 18, 19, 20]), 19));

    assert!(app.search().is_active(), "the search survives the jump");
    assert!(
        app.search().is_match(3),
        "and still remembers the places it found"
    );
}

/// The other half: loading the newest page replaces the window, and the
/// match list is places, which survive that too.
#[test]
fn a_latest_page_keeps_the_match_list() {
    let mut app = App::mock();
    run_search_line(&mut app, "benchmarks");
    assert!(app.search().is_match(7), "the sample match is message 7");

    assert!(app.apply_latest(page(&[5, 6, 7, 8, 9, 10])));

    assert!(app.search().is_active());
    assert!(app.search().is_match(7));
}

/// The test that fails the moment someone puts a `clear` back into
/// `apply_jump`: `n` walks across the boundary instead of restarting.
#[test]
fn n_crosses_a_jump_boundary() {
    let mut app = with_unread_out_of_reach(2);
    run_search_line(&mut app, "text");
    assert!(app.apply_searched(MOCK_CHAT, "text", vec![3, 7, 19, 25], 4));
    assert_eq!(
        reading(&app),
        Some(3),
        "the walk starts at the oldest match"
    );

    app.handle_key(press(KeyCode::Char('n')));
    assert_eq!(reading(&app), Some(7), "a loaded match is a cursor move");

    app.handle_key(press(KeyCode::Char('n')));
    assert_eq!(
        app.pending_jump(),
        Some(Jump {
            peer_id: MOCK_CHAT,
            target_id: 19,
            kind: JumpKind::Unread,
        }),
        "an unloaded match is a jump, the same path `gg` takes"
    );

    assert!(app.apply_jump(&page(&[19, 20, 21, 22, 23, 24, 25]), 19));
    assert_eq!(reading(&app), Some(19));

    app.handle_key(press(KeyCode::Char('n')));
    assert_eq!(
        reading(&app),
        Some(25),
        "the walk continues from the jumped-to match, not from the top"
    );
}

#[test]
fn a_jump_in_flight_is_what_the_status_line_says() {
    let mut app = with_unread_out_of_reach(2);
    app.ui.status = "3 conversation(s)".to_string();

    assert_eq!(app.status_text(), "3 conversation(s)");

    go_to_top(&mut app);
    assert_eq!(app.status_text(), JUMP_LABEL);

    app.apply_jump(&page(&[16, 17, 18, 19, 20]), 19);
    assert_eq!(
        app.status_text(),
        "3 conversation(s)",
        "the line goes back to what it was saying once the jump is over"
    );
}

// ---- `gd`, the message a reply quotes ------------------------------

/// A message of the sample conversation that quotes `reply_to`.
fn reply(id: i64, reply_to: i64) -> Message {
    Message {
        reply_to: Some(reply_to),
        ..message(id, "text")
    }
}

/// A window whose middle message quotes 19, which is nowhere near it.
fn with_a_reply_to_19() -> App {
    let mut app = App::mock();
    app.apply_latest(vec![message(1, "text"), reply(2, 19), message(3, "text")]);
    app.handle_key(press(KeyCode::Char('k')));
    app
}

/// `gd`, as a reader types it.
fn go_to_reply(app: &mut App) {
    app.handle_key(press(KeyCode::Char('g')));
    app.handle_key(press(KeyCode::Char('d')));
}

/// A window whose middle message quotes the one before it, which is on
/// screen: `gd` here is a cursor move and nothing else, and `Ctrl-o` is the
/// way back.
fn with_a_loaded_quote() -> App {
    let mut app = App::mock();
    app.apply_latest(vec![message(1, "text"), reply(2, 1), message(3, "text")]);
    app.handle_key(press(KeyCode::Char('k')));
    app
}

/// A quote the window already holds is a cursor move and nothing else: a
/// round trip for a message on screen would put the reader through the same
/// window twice to arrive where they already were.
#[test]
fn gd_on_a_loaded_quote_is_a_cursor_move() {
    let mut app = App::mock();
    app.apply_latest(vec![message(1, "text"), reply(2, 1), message(3, "text")]);
    app.handle_key(press(KeyCode::Char('k')));
    assert_eq!(reading(&app), Some(2), "the reply is under the cursor");

    go_to_reply(&mut app);

    assert_eq!(
        reading(&app),
        Some(1),
        "and the reader is on what it quotes"
    );
    assert_eq!(
        app.pending_jump(),
        None,
        "so nothing was asked of the network"
    );
}

/// A quote the window does not hold is the same jump `gg` makes, and says so.
#[test]
fn gd_on_a_quote_out_of_the_window_asks_for_it() {
    let mut app = with_a_reply_to_19();

    go_to_reply(&mut app);

    assert_eq!(
        app.pending_jump(),
        Some(Jump {
            peer_id: MOCK_CHAT,
            target_id: 19,
            kind: JumpKind::Reply,
        })
    );
}

/// A message that quotes nothing has nowhere to go, and the refusal says
/// which key would have gone somewhere.
#[test]
fn gd_on_a_message_that_quotes_nothing_refuses() {
    let mut app = App::mock();
    app.apply_latest(page(&[1, 2, 3]));
    app.handle_key(press(KeyCode::Char('k')));

    go_to_reply(&mut app);

    assert_eq!(app.status_text(), NOT_A_REPLY);
    assert_eq!(app.pending_jump(), None);
    assert_eq!(reading(&app), Some(2), "and the reader stays put");
}

/// A quote the client does not hold at all: the fetch came back with
/// nothing, so there is no page to land in and the reader is told why the
/// jump ended rather than left waiting for a key that will never work again.
#[test]
fn a_reply_jump_that_came_back_empty_says_the_message_is_gone() {
    let mut app = with_a_reply_to_19();
    go_to_reply(&mut app);
    let before = app.conversation.conversation.window.len();

    assert!(!app.apply_jump(&[], 19));

    assert_eq!(app.status_text(), JUMP_UNAVAILABLE);
    assert_eq!(app.pending_jump(), None, "and the key is free again");
    assert_eq!(app.conversation.conversation.window.len(), before);
}

/// The label names where the reader is going, so it differs by jump: a jump
/// to a reply is not a jump to the first unread message, and saying so is
/// the difference between a fetch that was asked for and one that was not.
#[test]
fn the_label_says_which_jump_is_in_flight() {
    let mut app = with_unread_out_of_reach(2);

    go_to_top(&mut app);
    assert_eq!(app.status_text(), JUMP_LABEL, "`gg` is the first unread");

    app.handle_key(press(KeyCode::Esc));

    let mut app = with_a_reply_to_19();
    go_to_reply(&mut app);
    assert_eq!(app.status_text(), JUMP_REPLY_LABEL);
    assert_eq!(
        app.jump_label(),
        JUMP_REPLY_LABEL,
        "the panel says the same"
    );
}

// ---- `Ctrl-o` and `Ctrl-i`, back and forward -------------------------

/// A reply jump the window can answer, remembered: `gd` on a quote that is
/// on screen still moves the reader away from where they were standing, and
/// `Ctrl-o` is how they get back.
#[test]
fn a_reply_jump_records_where_the_reader_was() {
    let mut app = with_a_loaded_quote();

    go_to_reply(&mut app);
    assert_eq!(reading(&app), Some(1), "the reader is on the quote");

    app.handle_key(press_ctrl('o'));

    assert_eq!(reading(&app), Some(2), "and back where they were standing");
    assert_eq!(app.pending_jump(), None, "which needed no page");
}

/// And forward again, which is the other half of the same walk.
#[test]
fn ctrl_i_takes_the_reader_forward_again() {
    let mut app = with_a_loaded_quote();
    go_to_reply(&mut app);
    app.handle_key(press_ctrl('o'));

    app.handle_key(press_ctrl('i'));

    assert_eq!(reading(&app), Some(1), "back to the quoted message");
    assert_eq!(app.pending_jump(), None);
}

/// The jump that took the reader away replaced the window, so the mark they
/// left behind is not on screen any more: a return is then a jump of its own,
/// on the same terms as the one they asked for, and it says so.
#[test]
fn a_return_across_a_replaced_window_is_a_jump_of_its_own() {
    let mut app = with_a_reply_to_19();
    go_to_reply(&mut app);
    assert!(app.apply_jump(&page(&[16, 17, 18, 19, 20]), 19));

    app.handle_key(press_ctrl('o'));

    assert_eq!(
        app.pending_jump(),
        Some(Jump {
            peer_id: MOCK_CHAT,
            target_id: 2,
            kind: JumpKind::Back,
        }),
        "message 2 is not in the window the jump replaced"
    );
    assert_eq!(app.status_text(), JUMP_BACK_LABEL);

    assert!(app.apply_jump(&page(&[1, 2, 3, 4, 5]), 2));
    assert_eq!(reading(&app), Some(2), "the reader is back where they were");

    app.handle_key(press_ctrl('i'));
    assert_eq!(
        app.pending_jump(),
        Some(Jump {
            peer_id: MOCK_CHAT,
            target_id: 19,
            kind: JumpKind::Forward,
        })
    );
    assert_eq!(app.status_text(), JUMP_FORWARD_LABEL);
}

/// One fetch at a time: a return asked for while a page is on its way is
/// swallowed rather than replacing the jump the reader is still waiting for.
#[test]
fn a_return_asked_for_while_a_jump_is_on_its_way_is_ignored() {
    let mut app = with_a_reply_to_19();
    go_to_reply(&mut app);
    let waiting = app.pending_jump();

    app.handle_key(press_ctrl('o'));
    assert_eq!(app.pending_jump(), waiting, "the key is swallowed");

    app.jump_back();
    assert_eq!(
        app.pending_jump(),
        waiting,
        "and calling it directly does not walk the list either"
    );
}

/// A placeholder is a message the server has not seen, so a jump to one has
/// nothing to fetch: the reader is told the message cannot be reached and
/// left where they were, rather than watching a page arrive for an id that
/// does not exist. Recorded as a known limitation rather than fixed — a
/// placeholder has no server-side identity to fetch around.
#[test]
fn a_return_to_a_placeholder_says_so_and_leaves_the_reader_put() {
    let mut app = App::mock();
    // Numbered below zero, which is what an outgoing message looks like
    // before the server has given it an id. The window is in message order,
    // so the placeholder is the oldest row and two steps up from the newest.
    app.apply_latest(vec![reply(-7, 19), message(1, "text"), message(3, "text")]);
    app.handle_key(press(KeyCode::Char('k')));
    app.handle_key(press(KeyCode::Char('k')));
    go_to_reply(&mut app);
    assert!(app.apply_jump(&page(&[16, 17, 18, 19, 20]), 19));
    assert_eq!(reading(&app), Some(19));

    app.handle_key(press_ctrl('o'));
    assert_eq!(
        app.pending_jump().map(|jump| jump.target_id),
        Some(-7),
        "it was asked for: nothing here can say it will not land"
    );

    assert!(!app.apply_jump(&[], -7));
    assert_eq!(app.status_text(), JUMP_UNAVAILABLE);
    assert_eq!(reading(&app), Some(19), "and the reader is where they were");
}

/// Q5: `Ctrl-i` is the same byte as `Tab` on a terminal that does not report
/// modifiers. Bare `Tab` stays the pane switch — and stays *only* that, so
/// forward navigation is unreachable there and `Ctrl-o` carries the criterion
/// alone.
#[test]
fn a_bare_tab_cycles_panes_and_is_not_forward_navigation() {
    let mut app = with_a_loaded_quote();
    go_to_reply(&mut app);
    app.handle_key(press_ctrl('o'));
    assert_eq!(app.ui.focus, Focus::Conversation);

    app.handle_key(press(KeyCode::Tab));

    assert_eq!(
        app.ui.focus,
        Focus::Input,
        "`Tab` is the pane switch, whatever byte a terminal sent"
    );
    assert_eq!(reading(&app), Some(2), "and it walked nothing");

    app.handle_key(press(KeyCode::BackTab));
    assert_eq!(app.ui.focus, Focus::Conversation);
    assert_eq!(reading(&app), Some(2), "nor did the other way");

    app.handle_key(press_ctrl('i'));
    assert_eq!(reading(&app), Some(1), "a reported `Ctrl-i` still walks");
}

// ---- the :shortcode completion --------------------------------------

/// An application composing `draft`, which opens a completion if it names a
/// shortcode.
fn typing(draft: &str) -> App {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('i')));
    type_text(&mut app, draft);
    app
}

#[test]
fn a_shortcode_opens_the_completion() {
    let app = typing(":cr");

    let trigger = app.completion().expect("a completion is up");
    assert_eq!(trigger.query, "cr");
    assert_eq!(
        trigger.chosen().and_then(|emoji| emoji.shortcode()),
        Some("cry"),
        "the shortest prefix is the top candidate"
    );
}

#[test]
fn a_character_keeps_filtering_and_keeps_the_popup_open() {
    let app = typing(":cry");

    let trigger = app.completion().expect("a completion is up");
    assert!(
        trigger
            .candidates
            .iter()
            .all(|emoji| emoji.shortcode().is_some_and(|code| code.contains("cry"))),
        "a candidate is on the list without matching the query"
    );
}

/// `j` and `k` are letters, not candidate motion: binding them would make
/// `:joy` and `:jack_o_lantern` untypable, which is the feature refusing to
/// work.
#[test]
fn j_and_k_are_still_letters_while_the_popup_is_open() {
    // `k` after `:o` still matches (`ok_hand`), so the popup is up on both
    // sides of the key — the case where a candidate motion would be reached.
    let mut app = typing(":o");
    assert!(app.completion().is_some(), "`:o` opens it");

    app.handle_key(press(KeyCode::Char('k')));

    assert_eq!(app.input.line.text(), ":ok", "`k` went to the draft");
    let trigger = app.completion().expect("and the list keeps filtering");
    assert_eq!(trigger.query, "ok");
    assert_eq!(trigger.selected, 0, "and `k` did not move the candidate");

    // `j` is the same key one row over. What matters is that the letter
    // reached the draft rather than being taken as motion.
    let mut app = typing(":cr");
    app.handle_key(press(KeyCode::Char('j')));

    assert_eq!(app.input.line.text(), ":crj", "`j` went to the draft too");
}

#[test]
fn the_arrows_move_the_candidate_and_wrap_around_it() {
    let mut app = typing(":cry");
    assert_eq!(app.completion().expect("up").candidates.len(), 3);

    for _ in 0..3 {
        app.handle_key(press(KeyCode::Down));
    }
    assert_eq!(
        app.completion().expect("up").selected,
        0,
        "three downs over three candidates wrapped"
    );

    app.handle_key(press(KeyCode::Up));
    assert_eq!(app.completion().expect("up").selected, 2, "and up wrapped");
}

#[test]
fn the_arrows_are_caret_motions_again_once_it_is_closed() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('i')));
    type_text(&mut app, "hello");
    app.handle_key(press_ctrl('j'));
    type_text(&mut app, ":cr");
    assert!(app.completion().is_some(), "there is one to close");

    app.handle_key(press(KeyCode::Esc));
    assert!(app.completion().is_none());

    app.handle_key(press(KeyCode::Up));

    assert_eq!(app.input.line.caret(), 3, "the arrow moved the caret");
    assert_eq!(
        app.input.line.laid_out(78).row,
        0,
        "to the first row of the draft, not to a candidate"
    );
}

#[test]
fn tab_accepts_the_candidate() {
    let mut app = typing(":cr");

    app.handle_key(press(KeyCode::Tab));

    assert_eq!(app.input.line.text(), "😢");
    assert_eq!(app.input.line.caret(), 4, "after the glyph");
    assert!(app.completion().is_none(), "and the popup is away");
}

/// Two presses fifty milliseconds apart are accept-then-send, which is what
/// a reader who typed `:cry` and mashed `Enter` wanted.
#[test]
fn enter_accepts_the_candidate_and_the_next_enter_sends() {
    let mut app = typing(":cry");

    app.handle_key(press(KeyCode::Enter));

    assert_eq!(app.take_action(), None, "accepting did not send");
    assert_eq!(app.input.line.text(), "😢");
    assert_eq!(
        app.ui.focus,
        Focus::Input,
        "and the reader is still in the line"
    );

    app.handle_key(press(KeyCode::Enter));

    assert_eq!(
        app.take_action(),
        Some(Action::Send {
            chat_id: MOCK_CHAT,
            temp_id: -1,
            text: "😢".to_owned(),
            reply_to: None,
        })
    );
}

#[test]
fn escape_puts_the_completion_away_and_leaves_the_draft_alone() {
    let mut app = typing(":cry");

    app.handle_key(press(KeyCode::Esc));

    assert_eq!(app.input.line.text(), ":cry", "the words are the reader's");
    assert!(app.completion().is_none());
}

/// The popup's `Esc` is a third key in front of the line's own two-stage
/// `Esc`, and it does not shorten the rule.
#[test]
fn escape_twice_leaves_the_line_as_it_did_before() {
    let mut app = typing(":cry");

    app.handle_key(press(KeyCode::Esc));
    assert_eq!(
        app.ui.focus,
        Focus::Input,
        "the popup's escape only closes it"
    );
    assert!(app.completion().is_none());

    app.handle_key(press(KeyCode::Esc));
    assert_eq!(
        app.ui.focus,
        Focus::Input,
        "the line's own escape is still the first stage"
    );

    app.handle_key(press(KeyCode::Esc));
    assert_eq!(
        app.ui.focus,
        Focus::Conversation,
        "and the second stage leaves"
    );
}

#[test]
fn backspace_shortens_the_query_and_the_list_grows() {
    let mut app = typing(":cry");
    let before = app.completion().expect("up").candidates.len();

    app.handle_key(press(KeyCode::Backspace));

    let trigger = app.completion().expect("still open on a shorter query");
    assert_eq!(trigger.query, "cr");
    assert!(
        trigger.candidates.len() >= before,
        "{} < {before}",
        trigger.candidates.len()
    );
}

#[test]
fn a_newline_ends_the_shortcode() {
    let mut app = typing(":cry");

    app.handle_key(press_ctrl('j'));

    assert!(app.completion().is_none());
    assert_eq!(app.input.line.text(), ":cry\n");
}

#[test]
fn a_space_ends_the_shortcode() {
    let mut app = typing(":cry");

    app.handle_key(press(KeyCode::Char(' ')));

    assert!(app.completion().is_none(), "the query is now `cry `");
}

#[test]
fn leaving_the_line_puts_the_completion_away() {
    let mut app = typing(":cry");

    app.handle_key(press_ctrl('w'));

    assert_eq!(app.ui.focus, Focus::Conversation);
    assert!(app.completion().is_none());
}

#[test]
fn a_command_line_never_completes() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char(':')));
    assert_eq!(app.input.line.purpose(), PromptKind::Command);

    type_text(&mut app, "cr");

    assert!(app.completion().is_none(), "a command is not a shortcode");
}

#[test]
fn a_search_line_never_completes() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('/')));
    assert_eq!(app.input.line.purpose(), PromptKind::Search);

    type_text(&mut app, "cr");

    assert!(app.completion().is_none(), "a search is not a shortcode");
}

/// The gate is `is_buffer`, not `Message`: a reply and an edit are buffers
/// too, and a reader answering either can name an emoji.
#[test]
fn a_reply_and_an_edit_do_complete() {
    let mut reply = App::mock();
    reply.handle_key(press(KeyCode::Char('r')));
    assert_eq!(reply.input.line.purpose(), PromptKind::Reply);
    type_text(&mut reply, ":cr");
    assert!(reply.completion().is_some(), "a reply completes");

    // Only the reader's own messages can be edited, and the sample
    // conversation's newest is not one of them.
    let mut edit = App::mock();
    edit.handle_key(press(KeyCode::Char('k')));
    edit.handle_key(press(KeyCode::Char('e')));
    assert_eq!(edit.input.line.purpose(), PromptKind::Edit);
    type_text(&mut edit, ":cr");
    assert!(edit.completion().is_some(), "an edit completes");
}

// ---- the sign-in field through the command that opens it ---------------

/// `:signin` leaves the line holding the phone field, pre-filled, and keeps
/// the keys going there.
///
/// Driven through `run_command_line` rather than `begin_signin` on purpose:
/// the field was being emptied and unfocused by `submit`'s reset, which only
/// runs when the command is *run*, so a test that calls `begin_signin`
/// directly walks straight past the bug. The symptom it caused is the whole
/// surface locking: `handle_key` routes every key to `handle_signin` once
/// `signin` is up, and its `Conversation` arm has nothing to say about a
/// flow — so a wiped line also meant no digits and no `q`.
#[test]
fn the_signin_command_leaves_the_phone_field_open_and_typed_into() {
    let mut app = App::mock();
    run_command_line(&mut app, "signin");

    assert_eq!(app.signin_field(), Some(LoginField::Phone));
    assert_eq!(app.ui.focus, Focus::Input, "the field is what has the keys");
    assert_eq!(
        app.input.line.text(),
        "+44 7700 900142",
        "the number the configuration carries is still in the bar"
    );

    app.handle_key(press(KeyCode::Char('4')));

    assert_eq!(
        app.input.line.text(),
        "+44 7700 9001424",
        "a digit lands in the field rather than being swallowed"
    );
}

/// And the step after it: `login_advanced` opens the code field and the
/// focus with it, so the next thing the reader types is a code.
#[test]
fn the_code_step_after_the_phone_one_is_typed_into_too() {
    let mut app = App::mock();
    run_command_line(&mut app, "signin");
    app.login_advanced(
        domain::session::SessionState::AwaitingCode {
            phone: "+44 7700 900142".to_owned(),
        },
        None,
    );

    assert_eq!(app.signin_field(), Some(LoginField::Code));
    assert_eq!(app.ui.focus, Focus::Input);

    app.handle_key(press(KeyCode::Char('4')));

    assert_eq!(
        app.input.line.text(),
        "4",
        "the code is what they are typing"
    );
}

/// `Esc` at the phone step is `cancel`, which is what the hint calls it —
/// so it takes the flow down rather than pausing into an overlay that
/// swallows every key, `q` among them.
#[test]
fn escape_at_the_phone_step_takes_the_flow_down() {
    let mut app = App::mock();
    run_command_line(&mut app, "signin");
    assert!(app.signin().is_some(), "the flow is up to begin with");

    app.handle_key(press(KeyCode::Esc));

    assert!(app.signin().is_none(), "cancelled, not paused");
    assert_eq!(
        app.ui.focus,
        Focus::Conversation,
        "and the keys are ours again"
    );
    // The consequence of not doing this: a paused flow swallowed every key.
    app.handle_key(press(KeyCode::Char('q')));
    assert!(
        app.conversation.confirm.is_some(),
        "`q` asks to quit, so it reached the app"
    );
}

/// `Esc` at the code step is a *different* thing, and stays one: Telegram
/// has sent a code, so the step restarts at the phone and the status line
/// says what that cost. The reader asked to sign in, not to stop.
#[test]
fn escape_at_the_code_step_still_restarts_at_the_phone() {
    let mut app = App::mock();
    run_command_line(&mut app, "signin");
    app.login_advanced(
        domain::session::SessionState::AwaitingCode {
            phone: "+44 7700 900142".to_owned(),
        },
        None,
    );

    app.handle_key(press(KeyCode::Esc));

    assert!(app.signin().is_some(), "a code was sent, so the flow stays");
    assert_eq!(app.signin_field(), Some(LoginField::Phone));
    assert_eq!(app.ui.focus, Focus::Input);
    assert_eq!(
        app.ui.status,
        "cancelling discards the code Telegram sent; ⏎ asks for a new one"
    );
}

// ---- the flow against a client, or the lack of one -------------------

/// A `⏎` with no client up reports itself instead of claiming to be in
/// flight.
///
/// The flag is the whole subject: with nothing to carry the request, a
/// `waiting` of `true` is a panel saying "Checking…" for an answer that is
/// never coming, and the guard behind it then swallows every later `⏎` as a
/// second press. So nothing is queued, the draft stays, and the sentence is
/// what the key earns.
#[test]
fn a_signin_with_no_client_reports_rather_than_waits() {
    let mut app = App::mock();
    app.set_client_available(false);
    run_command_line(&mut app, "signin");
    type_text(&mut app, "7");
    let draft = app.input.line.text().to_owned();

    app.handle_key(press(KeyCode::Enter));

    assert!(
        !app.signin()
            .and_then(SignIn::flow)
            .expect("the flow is up")
            .waiting,
        "nothing is on its way, so nothing is in flight"
    );
    assert!(
        app.take_action().is_none(),
        "and nothing was queued: a login fired by a client arriving later \
         is an attempt nobody asked for"
    );
    assert_eq!(app.input.line.text(), draft, "the draft is the reader's");
    assert_eq!(app.ui.focus, Focus::Input, "and the field keeps the keys");
    assert_eq!(app.ui.status, "not connected yet — the client is not up");
}

/// With a client up the same key is the request it always was: queued, and
/// the flow told a request is on its way.
#[test]
fn a_signin_with_a_client_queues_and_waits() {
    let mut app = App::mock();
    app.set_client_available(true);
    run_command_line(&mut app, "signin");

    app.handle_key(press(KeyCode::Enter));

    assert!(
        matches!(app.take_action(), Some(Action::Login { .. })),
        "a client is there to carry it"
    );
    assert!(
        app.signin()
            .and_then(SignIn::flow)
            .expect("the flow is up")
            .waiting,
        "and the panel may say so"
    );
}

/// Losing the client takes the flow out of the wait, rather than leaving a
/// flag that outlived the thing it was about.
#[test]
fn losing_the_client_ends_the_wait() {
    let mut app = App::mock();
    app.set_client_available(true);
    run_command_line(&mut app, "signin");
    app.handle_key(press(KeyCode::Enter));
    app.take_action();

    app.set_client_available(false);

    assert!(
        !app.signin()
            .and_then(SignIn::flow)
            .expect("the flow is up")
            .waiting,
        "no client, no in-flight request"
    );
}

// ---- finding a person to start a conversation with --------------------

/// A person a user search offered.
fn candidate(user_id: i64) -> UserCandidate {
    UserCandidate {
        user_id,
        display_name: format!("user-{user_id}"),
        username: Some(format!("user{user_id}")),
    }
}

/// Runs `/`-on-the-chat-list and submits `query`, leaving the search open.
fn start_user_search(app: &mut App, query: &str) {
    app.handle_key(press(KeyCode::Char('h')));
    app.handle_key(press(KeyCode::Char('/')));
    type_text(app, query);
    app.handle_key(press(KeyCode::Enter));
}

/// `/` on the chat list opens the new-conversation query, not the message
/// search: the two are different questions and must not share a prompt.
#[test]
fn p_on_the_chat_list_asks_to_pin_the_highlighted_chat() {
    let mut app = on_the_chat_list();
    app.handle_key(press(KeyCode::Char('j')));
    let chat = app.list.list.chats[app.list.selected_chat].id;

    app.handle_key(press(KeyCode::Char('p')));

    assert_eq!(
        app.take_action(),
        Some(Action::TogglePin {
            chat_id: chat,
            pinned: true,
        })
    );
    assert_eq!(app.take_action(), None, "an action is taken once");
}

#[test]
fn p_without_a_client_flashes_and_queues_nothing() {
    let mut app = on_the_chat_list();
    app.set_client_available(false);

    app.handle_key(press(KeyCode::Char('p')));

    assert_eq!(
        app.take_action(),
        None,
        "nothing waits for a client to come up"
    );
    assert_eq!(
        app.status_text(),
        "not connected yet — the client is not up"
    );
}

#[test]
fn an_accepted_pin_moves_the_chat_and_the_highlight_stays_on_it() {
    let mut app = on_the_chat_list();
    app.handle_key(press(KeyCode::Char('j')));
    let chat = app.list.list.chats[1].id;

    app.set_pinned(chat, true);

    assert_eq!(app.list.list.chats[0].id, chat);
    assert_eq!(app.list.selected_chat, 0, "the highlight followed the chat");
}

#[test]
fn a_slash_on_the_chat_list_opens_the_new_chat_prompt() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('h')));
    assert_eq!(app.ui.focus, Focus::ChatList, "the list has the keys");

    app.handle_key(press(KeyCode::Char('/')));

    assert_eq!(app.ui.focus, Focus::Input);
    assert_eq!(app.input.line.purpose(), PromptKind::NewChat);
    assert_eq!(app.prompt_prefix(), "/", "it reads as a query");
    assert!(
        !app.input.line.purpose().is_buffer(),
        "one line, insert only"
    );
}

/// The conversation's own `/` is unchanged: it still searches the window.
#[test]
fn a_slash_on_the_conversation_still_opens_the_message_search() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('/')));

    assert_eq!(app.ui.focus, Focus::Input);
    assert_eq!(app.input.line.purpose(), PromptKind::Search);
}

/// Letters that are list keys on the overlay must still type into the query
/// while the prompt has the focus.
#[test]
fn j_and_k_type_into_the_new_chat_query() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('h')));
    app.handle_key(press(KeyCode::Char('/')));

    type_text(&mut app, "jk");

    assert_eq!(app.input.line.text(), "jk", "the letters are the reader's");
}

/// Submitting queues one lookup and nothing else: the network is the
/// caller's, and `tui` does not reach it.
#[test]
fn submitting_a_new_chat_query_queues_exactly_one_lookup() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('h')));
    app.handle_key(press(KeyCode::Char('/')));
    type_text(&mut app, "  ada  ");

    app.handle_key(press(KeyCode::Enter));

    assert_eq!(
        app.take_action(),
        Some(Action::ResolveUser {
            query: "ada".to_owned()
        }),
        "the query is trimmed on its way out"
    );
    assert_eq!(app.take_action(), None, "an action is taken once");
    assert_eq!(app.ui.focus, Focus::Conversation);
    assert!(
        app.input.line.is_empty(),
        "the prompt has given up its query"
    );
    assert!(app.user_search().is_active(), "and the search is on show");
    assert_eq!(app.user_search().query(), Some("ada"));
}

/// An empty query asks nothing and says so, rather than leaving the reader
/// on a line that looks like it did something.
#[test]
fn submitting_an_empty_new_chat_query_says_so() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('h')));
    app.handle_key(press(KeyCode::Char('/')));

    app.handle_key(press(KeyCode::Enter));

    assert!(app.take_action().is_none(), "nothing was asked");
    assert!(!app.user_search().is_active());
    assert!(app.status_text().contains("name"), "{}", app.status_text());
}

/// `:new` opens the same prompt with the query already in it, and the line
/// keeps the keys: the submit reset must not wipe what the reader came to
/// type into.
#[test]
fn the_new_command_leaves_the_query_line_open_and_typed_into() {
    let mut app = App::mock();
    run_command_line(&mut app, "new ada");

    assert_eq!(app.input.line.purpose(), PromptKind::NewChat);
    assert_eq!(app.ui.focus, Focus::Input, "the line keeps the keys");
    assert_eq!(app.input.line.text(), "ada", "the query is pre-filled");

    app.handle_key(press(KeyCode::Char(' ')));
    assert_eq!(app.input.line.text(), "ada ", "and typing continues in it");
}

/// The status line names the query and the count while an answer is in
/// flight, after it lands, and when it fails.
#[test]
fn the_status_line_names_the_user_search_through_every_state() {
    let mut app = App::mock();
    start_user_search(&mut app, "ada");
    assert_eq!(app.status_text(), "/ada — searching…");

    assert!(app.apply_users("ada", vec![candidate(1), candidate(2)]));
    assert_eq!(app.status_text(), "/ada — 2 candidates");

    assert!(app.fail_users("ada", "flood wait".to_owned()));
    assert_eq!(
        app.status_text(),
        "/ada — 2 candidates (search failed: flood wait)"
    );
}

/// A user search outranks the conversation's own, the rank the design gives
/// it: the wider question wins while both are live.
#[test]
fn the_user_search_label_outranks_the_message_search_label() {
    let mut app = App::mock();
    start_user_search(&mut app, "ada");
    assert!(app.apply_users("ada", vec![candidate(1)]));

    // A conversation search, so both states are live at once.
    app.handle_key(press(KeyCode::Char('/')));
    type_text(&mut app, "the");
    app.handle_key(press(KeyCode::Enter));
    assert!(app.search().is_active(), "the message search is running");

    assert_eq!(app.status_text(), "/ada — 1 candidate");
}

/// A result for a query the reader has replaced is dropped, the same
/// discipline the message search's server pass gets.
#[test]
fn a_user_result_for_a_replaced_query_is_refused() {
    let mut app = App::mock();
    start_user_search(&mut app, "bar");

    assert!(!app.apply_users("foo", vec![candidate(2)]));
    assert!(!app.fail_users("foo", "too late".to_owned()));
    assert_eq!(app.user_search().query(), Some("bar"));
}

/// The arrows and `j`/`k` walk the list while it is open, and it wraps at
/// both ends.
#[test]
fn the_user_search_selection_wraps_in_both_directions() {
    let mut app = App::mock();
    start_user_search(&mut app, "x");
    assert!(app.apply_users("x", vec![candidate(1), candidate(2), candidate(3)]));

    app.handle_key(press(KeyCode::Char('j')));
    assert_eq!(app.user_search().selected(), 1, "`j` moves the list down");
    app.handle_key(press(KeyCode::Down));
    assert_eq!(app.user_search().selected(), 2);
    app.handle_key(press(KeyCode::Down));
    assert_eq!(
        app.user_search().selected(),
        0,
        "and the last wraps to the first"
    );
    app.handle_key(press(KeyCode::Char('k')));
    assert_eq!(
        app.user_search().selected(),
        2,
        "`k` wraps back the other way"
    );
}

/// `Esc` puts the list away and forgets it, so the next search starts from
/// nothing rather than under the last one's results.
#[test]
fn escape_clears_the_user_search_overlay() {
    let mut app = App::mock();
    start_user_search(&mut app, "x");
    assert!(app.apply_users("x", vec![candidate(1)]));

    app.handle_key(press(KeyCode::Esc));

    assert!(!app.user_search().is_active());
    assert_eq!(app.user_search().candidates().len(), 0);
}

/// Accepting a person the list already holds focuses that chat rather than
/// opening a second one.
#[test]
fn accepting_a_candidate_that_is_already_a_chat_focuses_it() {
    let mut app = App::mock();
    let before = app.chats().len();
    start_user_search(&mut app, "ada");
    assert!(app.apply_users("ada", vec![candidate(MOCK_CHAT)]));

    app.handle_key(press(KeyCode::Enter));

    assert_eq!(app.ui.focus, Focus::Conversation);
    assert_eq!(app.current_chat_id(), MOCK_CHAT);
    assert_eq!(app.chats().len(), before, "no duplicate chat was made");
    assert!(!app.user_search().is_active(), "and the list is put away");
}

/// Accepting a person the list does not hold lists them once and opens the
/// conversation on them — the new-chat path, not a duplicate.
#[test]
fn accepting_a_candidate_that_is_not_a_chat_lists_and_opens_them() {
    let mut app = App::mock();
    let before = app.chats().len();
    let person = candidate(MOCK_CHAT + 99);
    start_user_search(&mut app, "ada");
    assert!(app.apply_users("ada", vec![person.clone()]));

    app.handle_key(press(KeyCode::Enter));

    assert_eq!(app.ui.focus, Focus::Conversation);
    assert_eq!(app.current_chat_id(), person.user_id, "their chat is open");
    assert_eq!(app.chats().len(), before + 1, "and listed exactly once");
    assert!(
        app.chats().iter().any(|chat| chat.id == person.user_id),
        "the listed chat is them"
    );
    assert!(!app.user_search().is_active(), "and the list is put away");
}

// ---- the open draft --------------------------------------------------

/// Types `text` into the open conversation's line, and leaves it the way the
/// reader does: `Esc` to normal mode, and `Esc` again back to the conversation.
fn draft_then_leave(app: &mut App, text: &str) {
    app.handle_key(press(KeyCode::Char('i')));
    type_text(app, text);
    app.handle_key(press(KeyCode::Esc));
    app.handle_key(press(KeyCode::Esc));
    assert_eq!(
        app.ui.focus,
        Focus::Conversation,
        "and the words stay in the line"
    );
}

/// Whether the layout's last entry is the draft.
fn drafted(app: &App) -> bool {
    app.row_layout()
        .last()
        .is_some_and(|span| span.kind == RowKind::Draft)
}

/// The slice the panel is given for every cursor and budget the tests try.
fn slices_of(app: &mut App) -> Vec<rows::Slice> {
    let mut slices = Vec::new();
    for cursor in 0..app.conversation.conversation.window.len() {
        app.conversation.vim.set_cursor(cursor);
        let layout = app.row_layout();
        for budget in 1..=8 {
            slices.push(app.viewport(&layout, budget));
        }
    }
    slices
}

/// An empty line has no row, and a line with words has one trailing row for
/// them, after every message, and counts nothing of it.
#[test]
fn a_draft_is_a_trailing_row_only_while_the_line_has_words() {
    let mut app = App::mock();
    app.record_body(53);
    let before = app.row_layout();
    assert!(!drafted(&app), "an empty line is no row");

    app.handle_key(press(KeyCode::Char('i')));
    type_text(&mut app, "on its way");
    let layout = app.row_layout();
    let draft = layout.last().expect("the draft is laid out");

    assert_eq!(draft.kind, RowKind::Draft);
    assert_eq!(draft.message_id, None, "and names no message");
    assert_eq!(draft.first, rows::total_rows(&before), "after the last row");
    assert_eq!(draft.len, 1);
    assert_eq!(draft.text, 0.."on its way".len(), "over the words it shows");
    assert_eq!(
        &layout[..layout.len() - 1],
        &before[..],
        "every other entry is as it was"
    );

    app.input.line.clear();
    assert_eq!(app.row_layout(), before, "and cleared, it is gone");
}

/// A `:` command is not a draft, whatever is typed into it.
#[test]
fn a_command_being_typed_is_not_a_draft() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char(':')));
    type_text(&mut app, "quit");

    assert_eq!(app.input.line.purpose(), PromptKind::Command);
    assert!(!drafted(&app));
}

/// A draft wraps at the panel's width less the `[you|draft] ` it is drawn
/// behind: thirty characters fit a 40-column panel on their own, and not beside
/// their name.
#[test]
fn a_draft_wraps_at_the_panel_width_less_its_name() {
    let mut app = App::mock();
    app.record_body(40);
    app.handle_key(press(KeyCode::Char('i')));
    type_text(&mut app, &"x".repeat(30));

    let draft = app.row_layout().last().expect("the draft is laid out").len;

    assert_eq!(
        draft, 2,
        "28 columns beside the name, and the rest below it"
    );
}

/// The draft is drawn, and no count sees it: the total, a message at a row past
/// the messages, and the number of entries are all what they are without it.
#[test]
fn a_draft_is_in_no_count_while_it_is_on_screen() {
    let mut app = App::mock();
    app.record_body(53);
    let without = app.row_layout();
    let total = rows::total_rows(&without);

    app.handle_key(press(KeyCode::Char('i')));
    type_text(&mut app, &"y".repeat(150));
    let with = app.row_layout();

    assert_eq!(with.len(), without.len() + 1, "one entry more");
    assert!(with.last().is_some_and(|span| span.len > 1));
    assert_eq!(
        rows::total_rows(&with),
        total,
        "and the rows it spans are not in the total"
    );
    assert_eq!(rows::message_at_row(&with, total), None);
    assert_eq!(rows::message_at_row_moving(&with, total, true), None);
}

/// The slice the panel is given is the same for every cursor and budget with a
/// draft as without one: the draft is drawn below it and is not in what it
/// shows, its total, or its scrollbar.
#[test]
fn the_slice_is_the_same_with_a_draft_as_without_one() {
    let mut app = App::mock();
    app.record_body(53);
    let without = slices_of(&mut app);

    draft_then_leave(&mut app, &"y".repeat(150));
    assert!(drafted(&app), "the draft is there to be ignored");

    assert_eq!(slices_of(&mut app), without);
}

/// The fetch triggers and the cursor's place in the window are what they are
/// without the draft, for every cursor the window has.
#[test]
fn the_fetch_margins_are_the_same_with_a_draft_as_without_one() {
    let mut app = App::mock();
    app.record_body(53);
    app.apply_latest(tall_page(10));
    let measure = |app: &mut App| -> Vec<(usize, usize, bool, bool, bool)> {
        (0..app.conversation.conversation.window.len())
            .map(|cursor| {
                app.conversation.vim.set_cursor(cursor);
                (
                    app.cursor_extent().0,
                    app.cursor_extent().1,
                    app.near_the_end(),
                    app.wants_older(),
                    app.wants_newer(),
                )
            })
            .collect()
    };
    let without = measure(&mut app);

    draft_then_leave(&mut app, "a draft that is only a little");
    assert!(drafted(&app));

    assert_eq!(measure(&mut app), without);
}

/// A page down and a page up land on the same messages with a draft as without
/// one, and never on the draft.
#[test]
fn a_page_lands_on_the_same_messages_with_a_draft_as_without_one() {
    let mut landings = Vec::new();

    for drafting in [false, true] {
        let mut app = App::mock();
        app.record_body(53);
        app.record_rows(4);
        if drafting {
            draft_then_leave(&mut app, &"draft ".repeat(20));
            assert!(drafted(&app));
        }
        go_to_top(&mut app);

        let mut cursors = Vec::new();
        for _ in 0..4 {
            app.handle_key(press_ctrl('d'));
            cursors.push(app.conversation.vim.cursor());
        }
        for _ in 0..2 {
            app.handle_key(press_ctrl('u'));
            cursors.push(app.conversation.vim.cursor());
        }
        landings.push(cursors);
    }

    assert_eq!(landings[0], landings[1]);
}

/// In follow mode a conversation taller than the panel gives the messages every
/// row it has, so the draft's rows come out of the panel first: the messages are
/// given what the draft leaves, and the draft is the rows below them. No count
/// moves for it.
#[test]
fn a_draft_takes_its_rows_from_the_panel_before_the_messages_do() {
    let mut app = App::mock();
    app.record_body(53);
    app.apply_latest(tall_page(10));
    let height = 8;
    let without = app.row_layout();
    let total = rows::total_rows(&without);
    assert!(total > height, "the conversation is taller than the panel");
    assert_eq!(
        app.reserved(&without, height).below(),
        0,
        "and with no draft nothing is reserved below the messages"
    );

    draft_then_leave(&mut app, &"x".repeat(150));
    assert!(
        app.conversation.conversation.auto_follow(),
        "still following"
    );
    let layout = app.row_layout();
    let draft = layout.last().expect("the draft is laid out").len;
    assert!(draft < height - 1, "a few rows, well under the cap");

    let reserved = app.reserved(&layout, height);
    assert_eq!(reserved.draft, draft, "the draft's rows are reserved");
    assert_eq!(reserved.below(), draft, "and they are the rows below");

    let view = app.viewport(&layout, height - reserved.above() - reserved.below());
    assert_eq!(view.budget + draft, height, "the messages get what is left");
    assert_eq!(
        view.rows, view.budget,
        "and fill it, the conversation being taller"
    );
    assert_eq!(view.total, total, "the total is the one without the draft");
}

/// A draft taller than the panel is cut back to every row but one, so the
/// messages keep a row of their own however long the draft is.
#[test]
fn a_draft_taller_than_the_panel_leaves_the_messages_one_row() {
    let mut app = App::mock();
    app.record_body(53);
    app.apply_latest(tall_page(10));
    let height = 6;

    draft_then_leave(&mut app, &"x".repeat(600));
    let layout = app.row_layout();
    let draft = layout.last().expect("the draft is laid out").len;
    assert!(draft > height, "taller than the panel");

    let reserved = app.reserved(&layout, height);
    assert_eq!(reserved.draft, height - 1, "every row of the panel but one");

    let view = app.viewport(&layout, height - reserved.below());
    assert_eq!(view.budget, 1, "and one is left for the messages");
    assert_eq!(view.rows, 1);
}

/// Scrolled up, the draft would be drawn under an older message, so it is not
/// drawn at all: it reserves no rows below the messages, and the messages have
/// the whole panel.
#[test]
fn a_draft_reserves_nothing_while_scrolled_up() {
    let mut app = App::mock();
    app.record_body(53);
    app.apply_latest(tall_page(10));
    let height = 8;

    draft_then_leave(&mut app, "see you there");
    go_to_top(&mut app);
    assert!(
        !app.conversation.conversation.auto_follow(),
        "the reader is scrolled up"
    );
    let layout = app.row_layout();
    assert!(drafted(&app), "the draft is still there, only not shown");

    let reserved = app.reserved(&layout, height);
    assert_eq!(reserved.draft, 0, "no rows for the draft");
    assert_eq!(reserved.below(), 0, "and none below the messages");

    let view = app.viewport(&layout, height - reserved.above() - reserved.below());
    assert_eq!(view.budget, height, "the messages get the full panel");
}

/// A peer's presence arriving is news for the title and the card, and for nothing
/// the reader is looking at: the cursor, a visual selection and a search all stay
/// where the reader left them, and a repeat of the same report redraws nothing.
#[test]
fn a_presence_update_leaves_the_cursor_selection_and_search_alone() {
    let mut app = App::mock();
    run_search_line(&mut app, "benchmarks");
    app.handle_key(press(KeyCode::Char('v')));
    app.handle_key(press(KeyCode::Char('j')));
    assert_eq!(app.ui.mode, Mode::Visual, "a selection is running");

    let before = (
        app.conversation.vim.cursor(),
        app.ui.mode,
        app.search_query().map(str::to_owned),
        app.conversation.conversation.window.len(),
        reading(&app),
    );

    let online = UpdateEvent::PeerStatus {
        chat_id: MOCK_CHAT,
        presence: domain::presence::Presence::Online,
    };
    assert!(app.apply_update(&online), "a new presence is a redraw");
    assert_eq!(
        app.peer_presence(MOCK_CHAT),
        Some(domain::presence::Presence::Online),
        "and the open view holds it"
    );
    assert_eq!(
        (
            app.conversation.vim.cursor(),
            app.ui.mode,
            app.search_query().map(str::to_owned),
            app.conversation.conversation.window.len(),
            reading(&app),
        ),
        before,
        "and the reader's place, selection and search did not move"
    );
    assert!(
        !app.apply_update(&online),
        "the same presence again changes nothing on screen"
    );
}

// ---- `o`, the media on the cursor message --------------------------------

/// A message of the sample conversation that carries `media`.
fn with_media(id: i64, text: &'static str, media: domain::message::MediaKind) -> Message {
    Message {
        media: Some(media),
        ..message(id, text)
    }
}

/// `o` on a message with media queues one open, carrying the cursor
/// message's conversation and identifier, and nothing else.
#[test]
fn o_on_a_media_message_queues_its_open() {
    let mut app = App::mock();
    app.apply_latest(vec![
        message(1, "text"),
        with_media(2, "", domain::message::MediaKind::Photo),
        message(3, "text"),
    ]);
    app.handle_key(press(KeyCode::Char('k')));
    assert_eq!(
        reading(&app),
        Some(2),
        "the media message is under the cursor"
    );

    key(&mut app, 'o');

    assert_eq!(
        app.take_action(),
        Some(Action::OpenMedia {
            chat_id: MOCK_CHAT,
            message_id: 2,
        })
    );
    assert_eq!(app.take_action(), None, "one open, not two");
}

/// A caption does not stop a photo from opening: the media is what is asked
/// for, and the text is only shown beside it.
#[test]
fn o_on_a_captioned_media_message_still_queues_its_open() {
    let mut app = App::mock();
    app.apply_latest(vec![with_media(
        1,
        "a caption",
        domain::message::MediaKind::Video,
    )]);

    key(&mut app, 'o');

    assert_eq!(
        app.take_action(),
        Some(Action::OpenMedia {
            chat_id: MOCK_CHAT,
            message_id: 1,
        })
    );
}

/// `o` on a message without media queues nothing and says which key would
/// have opened something.
#[test]
fn o_on_a_message_without_media_refuses() {
    let mut app = App::mock();
    app.apply_latest(page(&[1, 2, 3]));
    app.handle_key(press(KeyCode::Char('k')));

    key(&mut app, 'o');

    assert_eq!(app.status_text(), crate::state::coordinate::NO_ATTACHMENT);
    assert_eq!(app.take_action(), None, "nothing is queued");
    assert_eq!(reading(&app), Some(2), "and the reader stays put");
}

#[test]
fn ctrl_c_quits_from_the_normal_mode_and_the_quit_prompt() {
    let mut app = App::mock();
    app.handle_key(press_ctrl('c'));
    assert!(app.ui.should_quit);

    let mut app = App::mock();
    key(&mut app, 'q');
    assert_eq!(app.ui.mode, Mode::Confirm);
    app.handle_key(press_ctrl('c'));
    assert!(app.ui.should_quit, "Ctrl-C answers the prompt by quitting");
}

#[test]
fn a_plain_key_does_not_quit_on_the_global_path() {
    let mut app = App::mock();
    app.handle_key(press(KeyCode::Char('j')));
    assert!(!app.ui.should_quit);
}
