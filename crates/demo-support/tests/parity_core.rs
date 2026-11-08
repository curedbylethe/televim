use demo_support::{DemoSimulator, DemoWorld, Effect};
use domain::{
    message::{MediaKind, MessageStatus},
    updates::UpdateEvent,
};
use tui::{
    Action, App, Focus, Mode, PromptKind,
    key::{KeyCode, KeyEvent, KeyModifiers},
};

const NOW: i64 = 1_730_000_000;

fn app_for(sim: &DemoSimulator) -> App {
    let account = sim.world.active();
    let mut app = App::new();
    app.set_client_available(true);
    app.set_account(Ok(account.account.clone()));
    app.set_known_accounts(account.known_accounts.clone());
    app.set_chats(account.chats.clone());
    app.set_archived_chats(account.archived.clone());
    for (peer_id, name) in &account.local_names {
        app.set_local_name(*peer_id, Some(name));
    }
    app.select_chat_by_id(account.default_chat);
    assert!(app.apply_latest(account.messages(account.default_chat).to_vec()));
    app.record_now(NOW, 0);
    app
}

fn fixture() -> (DemoSimulator, App) {
    let sim = DemoSimulator::new(DemoWorld::new(NOW));
    let app = app_for(&sim);
    (sim, app)
}

fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn press_char(app: &mut App, ch: char) {
    press(app, KeyCode::Char(ch));
}

fn type_text(app: &mut App, text: &str) {
    for ch in text.chars() {
        press_char(app, ch);
    }
}

#[test]
fn modes_counts_motion_and_focus_use_native_handlers() {
    let (_, mut app) = fixture();
    assert_eq!(app.ui.mode, Mode::Normal);
    assert_eq!(app.ui.focus, Focus::Conversation);

    press_char(&mut app, 'i');
    assert_eq!(app.ui.focus, Focus::Input);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.ui.mode, Mode::Normal);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.ui.focus, Focus::Conversation);

    type_text(&mut app, "gg");
    let before = app.conversation.vim.cursor();
    type_text(&mut app, "3j");
    assert!(app.conversation.vim.cursor() > before);

    press(&mut app, KeyCode::Tab);
    assert_eq!(app.ui.focus, Focus::Input);
}

#[test]
fn emoji_completion_and_draft_survive_chat_switch() {
    let (_, mut app) = fixture();
    press_char(&mut app, 'i');
    type_text(&mut app, ":smile");
    assert!(app.completion().is_some());
    press(&mut app, KeyCode::Enter);
    assert!(app.input.line.text().contains('😄'));

    type_text(&mut app, " draft");
    let draft = app.input.line.text().to_owned();
    press(&mut app, KeyCode::Esc);
    app.select_chat(1);
    app.select_chat(0);
    assert_eq!(app.input.line.text(), draft);
}

#[test]
fn selection_yank_and_quit_set_native_state() {
    let (_, mut app) = fixture();
    type_text(&mut app, "gg");
    press_char(&mut app, 'V');
    assert_eq!(app.ui.mode, Mode::Visual);
    press_char(&mut app, 'j');
    press_char(&mut app, 'y');
    assert_eq!(app.ui.mode, Mode::Normal);
    assert!(app.take_clipboard().is_some());

    app.ui.should_quit = false;
    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(app.ui.should_quit);
}

#[test]
fn command_quit_sets_native_quit_state() {
    let (_, mut app) = fixture();
    press_char(&mut app, ':');
    type_text(&mut app, "q");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.ui.mode, Mode::Confirm);
    press_char(&mut app, 'y');
    assert!(app.ui.should_quit);
}

#[test]
fn invalid_count_clears_and_visual_delete_asks_for_confirmation() {
    let (_, mut app) = fixture();
    type_text(&mut app, "gg");
    let top = app.conversation.vim.cursor();
    press_char(&mut app, '2');
    press_char(&mut app, 'x');
    press_char(&mut app, 'j');
    assert_eq!(app.conversation.vim.cursor(), top + 1);

    type_text(&mut app, "gg");
    press_char(&mut app, 'V');
    press_char(&mut app, 'j');
    press_char(&mut app, 'd');
    assert_eq!(app.ui.mode, Mode::Confirm);
    press_char(&mut app, 'y');
    assert!(
        matches!(app.take_action(), Some(Action::Delete { message_ids, .. }) if message_ids.len() >= 2)
    );
}

#[test]
fn discard_command_confirms_and_clears_the_draft() {
    let (_, mut app) = fixture();
    press_char(&mut app, 'i');
    type_text(&mut app, "unsent words");
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Esc);
    press_char(&mut app, ':');
    type_text(&mut app, "discard");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.ui.mode, Mode::Confirm);
    press_char(&mut app, 'y');
    assert!(app.input.line.is_empty());
}

#[test]
fn send_failure_dismiss_and_attachment_retry_follow_native_actions() {
    let (mut sim, mut app) = fixture();
    press_char(&mut app, 'i');
    type_text(&mut app, "!fail demo send");
    press(&mut app, KeyCode::Enter);
    let Some(Action::Send {
        chat_id,
        temp_id,
        text,
        reply_to,
    }) = app.take_action()
    else {
        panic!("composer queues a send");
    };
    let effects = sim.dispatch(
        &app,
        Action::Send {
            chat_id,
            temp_id,
            text,
            reply_to,
        },
    );
    assert!(matches!(effects.as_slice(), [Effect::Status(_)]));
    let failed = sim.advance(300);
    let Some(Effect::SendFailed {
        temp_id, reason, ..
    }) = failed.into_iter().next()
    else {
        panic!("simulator returns the scheduled failure");
    };
    assert!(app.fail_send(temp_id, reason));
    assert_eq!(
        app.conversation
            .conversation
            .message(temp_id)
            .map(|message| message.status),
        Some(MessageStatus::Failed)
    );
    press_char(&mut app, 'D');
    assert!(app.conversation.conversation.message(temp_id).is_none());
    assert!(
        app.take_action().is_none(),
        "text failure needs no upload action"
    );

    let (_, mut app) = fixture();
    let id = app
        .conversation
        .conversation
        .queue_media_send(MediaKind::File, None);
    app.conversation
        .vim
        .set_total(app.conversation.conversation.window.len());
    app.conversation
        .vim
        .set_cursor(app.conversation.conversation.window.len() - 1);
    app.fail_send(id, "upload failed".to_owned());
    press_char(&mut app, 'o');
    assert!(
        matches!(app.take_action(), Some(Action::RetryUpload { temp_id, .. }) if temp_id == id)
    );
    assert_eq!(
        app.conversation
            .conversation
            .message(id)
            .map(|message| message.status),
        Some(MessageStatus::Sending)
    );
    app.fail_send(id, "upload failed again".to_owned());
    press_char(&mut app, 'D');
    assert!(matches!(app.take_action(), Some(Action::DismissUpload { temp_id }) if temp_id == id));
}

#[test]
fn send_and_reply_use_fixture_state() {
    let (mut sim, mut app) = fixture();
    press_char(&mut app, 'i');
    type_text(&mut app, "hello from the demo");
    press(&mut app, KeyCode::Enter);
    let action = app.take_action().expect("composer queues send");
    assert!(matches!(action, Action::Send { .. }));
    let effects = sim.dispatch(&app, action);
    assert!(matches!(effects.as_slice(), [Effect::Status(_)]));
    let sent = sim.advance(1_000);
    let Some(Effect::Sent {
        temp_id, message, ..
    }) = sent.into_iter().next()
    else {
        panic!("simulator settles the send");
    };
    assert!(app.confirm_sent(temp_id, message));

    let (_, mut app) = fixture();
    press_char(&mut app, 'r');
    assert_eq!(app.input.line.purpose(), PromptKind::Reply);
    type_text(&mut app, "a reply");
    press(&mut app, KeyCode::Enter);
    assert!(matches!(
        app.take_action(),
        Some(Action::Send {
            reply_to: Some(_),
            ..
        })
    ));
}

#[test]
fn edit_uses_fixture_state() {
    let (mut sim, mut app) = fixture();
    let chat_id = app.current_chat_id();
    let outgoing = sim
        .world
        .active()
        .messages(chat_id)
        .iter()
        .rposition(|message| message.is_outgoing)
        .expect("fixture has an outgoing message");
    for _ in 0..app.conversation.conversation.window.len() {
        let cursor = app.conversation.vim.cursor();
        if cursor == outgoing {
            break;
        }
        press_char(&mut app, if cursor < outgoing { 'j' } else { 'k' });
    }
    assert_eq!(app.conversation.vim.cursor(), outgoing);
    press_char(&mut app, 'e');
    assert_eq!(app.input.line.purpose(), PromptKind::Edit);
    type_text(&mut app, " revised");
    press(&mut app, KeyCode::Enter);
    let action = app.take_action().expect("edit queues action");
    assert!(matches!(action, Action::Edit { .. }));
    let Some(Effect::Edited {
        chat_id,
        message_id,
        text,
    }) = sim.dispatch(&app, action).into_iter().next()
    else {
        panic!("simulator edits fixture message");
    };
    assert!(app.apply_update(&UpdateEvent::MessageEdited {
        chat_id,
        message_id,
        new_text: text.into()
    }));
}

#[test]
fn rename_sets_and_clears_local_name() {
    let (mut sim, mut app) = fixture();
    press_char(&mut app, ':');
    type_text(&mut app, "rename Ada");
    press(&mut app, KeyCode::Enter);
    let action = app.take_local_name_action().expect("rename action");
    let Action::SetLocalName { peer_id, name } = action else {
        panic!("local-name action");
    };
    assert!(matches!(
        sim.dispatch(
            &app,
            Action::SetLocalName {
                peer_id,
                name: name.clone()
            }
        )
        .as_slice(),
        [Effect::LocalNameSaved { .. }]
    ));
    app.set_local_name(peer_id, name.as_deref());
    assert_eq!(
        app.chats()
            .iter()
            .find(|chat| chat.id == peer_id)
            .map(|chat| chat.title.as_str()),
        Some("Ada")
    );
    let original_name = sim
        .world
        .active()
        .chats
        .iter()
        .find(|chat| chat.id == peer_id)
        .expect("fixture contact exists")
        .title
        .clone();
    app.set_local_name(peer_id, None);
    assert_eq!(
        app.chats()
            .iter()
            .find(|chat| chat.id == peer_id)
            .map(|chat| chat.title.as_str()),
        Some(original_name.as_str())
    );
}

#[test]
fn delete_uses_fixture_state() {
    let (mut sim, mut app) = fixture();
    press_char(&mut app, 'd');
    assert_eq!(app.ui.mode, Mode::Confirm);
    press_char(&mut app, 'y');
    let action = app.take_action().expect("confirmed delete action");
    assert!(matches!(action, Action::Delete { .. }));
    let effects = sim.dispatch(&app, action);
    let Some(Effect::Deleted { message_ids, .. }) = effects.into_iter().next() else {
        panic!("simulator deletes fixture message");
    };
    assert!(app.apply_update(&UpdateEvent::MessagesDeleted { message_ids }));
}
