use std::fmt::Write as _;

use demo_support::DemoWorld;
use ratatui::{Terminal, backend::TestBackend};
use tui::{App, app::FetchDirection};

const HISTORY_PAGE_SIZE: usize = 8;

#[test]
fn render_real_app_fixture_poster() {
    let world = DemoWorld::new(1_730_000_000);
    let account = world.active();
    let chat_id = account.default_chat;
    let mut app = App::new();
    app.set_client_available(true);
    app.set_account(Ok(account.account.clone()));
    app.set_known_accounts(account.known_accounts.clone());
    app.set_chats(account.chats.clone());
    app.set_archived_chats(account.archived.clone());
    app.select_chat_by_id(chat_id);
    let messages = account.messages(chat_id);
    assert!(
        app.apply_latest(
            messages
                .iter()
                .skip(messages.len().saturating_sub(HISTORY_PAGE_SIZE))
                .cloned()
                .collect()
        )
    );
    app.exhaust(FetchDirection::Newer);
    if messages.len() <= HISTORY_PAGE_SIZE {
        app.exhaust(FetchDirection::Older);
    }

    let backend = TestBackend::new(96, 30);
    let mut terminal = Terminal::new(backend).expect("create poster terminal");
    terminal
        .draw(|frame| app.render(frame))
        .expect("render fixture app");

    let buffer = terminal.backend().buffer();
    let area = buffer.area;
    let mut poster = String::new();
    for y in area.y..area.bottom() {
        let mut row = String::new();
        for x in area.x..area.right() {
            let _ = write!(row, "{}", buffer[(x, y)].symbol());
        }
        poster.push_str(row.trim_end());
        poster.push('\n');
    }
    assert!(!poster.trim().is_empty());
    println!("TELEVIM_POSTER_BEGIN\n{poster}TELEVIM_POSTER_END");
}
