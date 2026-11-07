mod backend;
mod input;

use std::{borrow::Cow, cell::RefCell, rc::Rc};

use domain::{
    account::Account,
    chat::{Chat, ChatKind},
    message::{Message, MessageStatus},
};
use tui::App;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    console_error_panic_hook::set_once();
    backend::run(Rc::new(RefCell::new(seed_app())))
}

fn seed_app() -> App {
    let mut app = App::new();
    app.set_chats(vec![Chat {
        id: 1,
        title: "Feasibility".to_owned(),
        kind: ChatKind::Private,
        last_message: Some(Cow::Borrowed(
            "The real Televim interface is running in a browser.",
        )),
        unread_count: 0,
        last_message_id: Some(1),
        last_timestamp: Some(1_700_000_000),
        pinned: false,
        presence: None,
        deleted: false,
        read_outbox_max_id: None,
    }]);
    app.select_chat(0);
    app.apply_latest(vec![Message {
        id: 1,
        chat_id: 1,
        text: Cow::Borrowed("The real Televim interface is running in a browser."),
        timestamp: 1_700_000_000,
        status: MessageStatus::Received,
        is_outgoing: false,
        reply_to: None,
        media: None,
        media_id: None,
    }]);
    app.set_account(Ok(Account {
        user_id: 10_001,
        first_name: "Demo".to_owned(),
        last_name: "Account".to_owned(),
        ..Account::default()
    }));
    app.set_client_available(true);
    app
}
