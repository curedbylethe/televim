use std::{borrow::Cow, cell::RefCell, rc::Rc};

use domain::message::{Message, MessageStatus};
use ratzilla::{
    event::{KeyCode, KeyEvent},
    ratatui::{Terminal, backend::TestBackend},
};
use tui::{
    app::Action,
    key::{KeyCode as TuiKeyCode, KeyEvent as TuiKeyEvent, KeyModifiers},
};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

use super::{DRIVER, driver::Driver, input};

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn key_mapping_covers_letters_arrows_tab_escape_and_modifiers() {
    let key = |code, ctrl, alt, shift| KeyEvent {
        code,
        ctrl,
        alt,
        shift,
    };

    assert_eq!(
        input::to_key(key(KeyCode::Char('j'), false, false, false)),
        Some(TuiKeyEvent::new(TuiKeyCode::Char('j'), KeyModifiers::NONE))
    );
    assert_eq!(
        input::to_key(key(KeyCode::Up, true, true, true)),
        Some(TuiKeyEvent::new(
            TuiKeyCode::Up,
            KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT
        ))
    );
    assert_eq!(
        input::to_key(key(KeyCode::Tab, false, false, true)),
        Some(TuiKeyEvent::new(TuiKeyCode::BackTab, KeyModifiers::SHIFT))
    );
    assert_eq!(
        input::to_key(key(KeyCode::Esc, false, false, false)),
        Some(TuiKeyEvent::new(TuiKeyCode::Esc, KeyModifiers::NONE))
    );
}

#[wasm_bindgen_test]
fn driver_open_chat_id_does_not_follow_chat_list_highlight() {
    let mut driver = Driver::new();
    let open_chat_id = driver.open_chat_id();

    driver.handle_key(TuiKeyEvent::new(TuiKeyCode::Char('h'), KeyModifiers::NONE));
    driver.handle_key(TuiKeyEvent::new(TuiKeyCode::Char('j'), KeyModifiers::NONE));

    assert_ne!(driver.app.current_chat_id(), open_chat_id);
    assert_eq!(driver.open_chat_id(), open_chat_id);
}

#[wasm_bindgen_test]
fn retry_command_reports_that_the_offline_demo_cannot_reconnect() {
    let mut driver = Driver::new();
    driver.handle_key(TuiKeyEvent::new(TuiKeyCode::Char(':'), KeyModifiers::NONE));
    for ch in "retry".chars() {
        driver.handle_key(TuiKeyEvent::new(TuiKeyCode::Char(ch), KeyModifiers::NONE));
    }
    driver.handle_key(TuiKeyEvent::new(TuiKeyCode::Enter, KeyModifiers::NONE));
    driver.tick();

    assert_eq!(
        driver.app.ui.status,
        "Retry is unavailable: the offline demo has no Telegram connection."
    );
}

#[wasm_bindgen_test]
fn app_renders_fixture_into_test_backend() {
    let driver = Driver::new();
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("test backend creates terminal");
    terminal
        .draw(|frame| driver.app.render(frame))
        .expect("app renders to test backend");

    let text = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(text.contains("Offline demo"), "fixture status is rendered");
}

#[wasm_bindgen_test]
fn simulator_send_applies_sent_effect_to_app() {
    let mut driver = Driver::new();
    let chat_id = driver.app.current_chat_id();
    let effects = driver.sim.dispatch(
        &driver.app,
        Action::Send {
            chat_id,
            temp_id: -700,
            text: "wasm test message".to_owned(),
            reply_to: None,
        },
    );
    assert_eq!(effects.len(), 1);

    for effect in driver.sim.advance(1_000) {
        if let demo_support::Effect::Sent {
            temp_id, message, ..
        } = effect
        {
            driver.app.end_send(temp_id);
            driver.app.confirm_sent(temp_id, message);
        }
    }

    let rows = &driver.app.conversation.conversation.window;
    let sent = rows
        .iter()
        .find(|message| message.text == "wasm test message")
        .expect("simulated send appears in app history");
    assert_eq!(sent.status, MessageStatus::Sent);
}

#[wasm_bindgen_test]
fn sent_and_edited_effects_keep_chat_preview_message_cache() {
    let mut driver = Driver::new();
    let chat_id = driver.open_chat_id();
    let message = Message {
        id: 70_001,
        chat_id,
        text: Cow::Borrowed("preview before edit"),
        timestamp: driver.sim.world.base_timestamp + 60,
        status: MessageStatus::Sent,
        is_outgoing: true,
        reply_to: None,
        media: None,
        media_id: None,
    };
    driver.sim.schedule(
        0,
        demo_support::Effect::Sent {
            chat_id,
            temp_id: -70_001,
            message,
        },
    );
    driver.tick();
    assert_eq!(
        driver
            .app
            .chats()
            .iter()
            .find(|chat| chat.id == chat_id)
            .and_then(|chat| chat.last_message.as_deref()),
        Some("preview before edit")
    );

    driver.sim.schedule(
        0,
        demo_support::Effect::Edited {
            chat_id,
            message_id: 70_001,
            text: "preview after edit".into(),
        },
    );
    driver.tick();
    assert_eq!(
        driver
            .app
            .chats()
            .iter()
            .find(|chat| chat.id == chat_id)
            .and_then(|chat| chat.last_message.as_deref()),
        Some("preview after edit")
    );
}

#[wasm_bindgen_test]
fn reset_demo_restores_fixture_cast() {
    let driver = Rc::new(RefCell::new(Driver::new()));
    let original_label = driver.borrow().sim.world.active().label.clone();
    driver.borrow_mut().sim.world.active_mut().label = "mutated cast".into();
    DRIVER.with(|slot| *slot.borrow_mut() = Some(Rc::clone(&driver)));

    super::reset_demo();

    assert_eq!(driver.borrow().sim.world.active().label, original_label);
    DRIVER.with(|slot| *slot.borrow_mut() = None);
}
