use std::borrow::Cow;
use std::fmt::Write as _;

use demo_support::DemoWorld;
use domain::{
    message::{Message, MessageStatus},
    search::word_prefix_match,
};
use ratatui::{Terminal, backend::TestBackend};
use tui::{
    App,
    app::FetchDirection,
    bidi::BidiMode,
    key::{KeyCode, KeyEvent, KeyModifiers},
};

const NOW: i64 = 1_730_000_000;

fn app_with_messages(texts: &[&str]) -> App {
    let world = DemoWorld::new(NOW);
    let account = world.active();
    let chat_id = account.default_chat;
    let mut app = App::new();
    app.set_client_available(true);
    app.set_account(Ok(account.account.clone()));
    app.set_known_accounts(account.known_accounts.clone());
    app.set_chats(account.chats.clone());
    app.select_chat_by_id(chat_id);
    let messages = texts
        .iter()
        .enumerate()
        .map(|(index, text)| Message {
            id: i64::try_from(index + 1).expect("test message id fits"),
            chat_id,
            text: Cow::Owned((*text).to_owned()),
            timestamp: NOW + i64::try_from(index).expect("test timestamp fits"),
            status: MessageStatus::Received,
            is_outgoing: false,
            is_service: false,
            reply_to: None,
            media: None,
            media_id: None,
        })
        .collect();
    assert!(app.apply_latest(messages));
    app.exhaust(FetchDirection::Newer);
    app.with_bidi(BidiMode::Visual)
}

fn key(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn type_text(app: &mut App, text: &str) {
    for character in text.chars() {
        key(app, KeyCode::Char(character));
    }
}

fn rendered(app: &App) -> String {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("create test terminal");
    terminal
        .draw(|frame| app.render(frame))
        .expect("render app");
    let buffer = terminal.backend().buffer();
    let mut text = String::new();
    for y in buffer.area.y..buffer.area.bottom() {
        for x in buffer.area.x..buffer.area.right() {
            let _ = write!(text, "{}", buffer[(x, y)].symbol());
        }
        text.push('\n');
    }
    text
}

#[test]
fn app_renders_unicode_corpus_without_losing_glyphs() {
    let corpus = [
        "سلام، حالت چطوره؟",
        "مرحبا، كيف حالك؟",
        "Hello دنیا",
        "abc سلام xyz",
        "cafe\u{301} עברית",
        "👩‍🔬 ❤️ 👍🏽 🇮🇷 1️⃣ 漢 Ａ",
    ];
    let app = app_with_messages(&corpus);
    let screen = rendered(&app);

    for text in corpus {
        for character in text.chars().filter(|character| !character.is_whitespace()) {
            assert!(
                screen.contains(character),
                "missing {character:?} from {screen:?}"
            );
        }
    }
    assert!(app.body_width() <= 80);
    assert!(app.row_layout().iter().all(|span| span.len > 0));
}

#[test]
fn visual_render_is_stable_and_differs_from_terminal_order() {
    let text = "abc سلام xyz";
    let visual = rendered(&app_with_messages(&[text]));
    let repeated = rendered(&app_with_messages(&[text]));
    let terminal = app_with_messages(&[text]).with_bidi(BidiMode::Terminal);
    let terminal = rendered(&terminal);
    let words = app_with_messages(&[text]).with_bidi(BidiMode::Words);
    let words = rendered(&words);

    assert_eq!(visual, repeated, "Visual mode stays stable across frames");
    assert_ne!(
        visual, terminal,
        "Visual mode reorders a mixed-direction row"
    );
    assert_ne!(visual, words, "Words mode uses different RTL word order");
}

#[test]
fn composer_keeps_rtl_text_and_deletes_one_grapheme() {
    let mut app = app_with_messages(&["plain message"]);
    key(&mut app, KeyCode::Char('i'));
    type_text(&mut app, "سلام e\u{301}👩‍🔬x");
    assert_eq!(app.input.line.text(), "سلام e\u{301}👩‍🔬x");
    assert!(
        app.input
            .line
            .text()
            .is_char_boundary(app.input.line.caret())
    );

    key(&mut app, KeyCode::Backspace);
    assert_eq!(app.input.line.text(), "سلام e\u{301}👩‍🔬");
    key(&mut app, KeyCode::Backspace);
    assert_eq!(app.input.line.text(), "سلام e\u{301}");
}

#[test]
fn character_selection_yanks_original_logical_text() {
    let logical = "abc سلام xyz";
    let selected = "abc سلام xy";
    let mut app = app_with_messages(&[logical]);
    key(&mut app, KeyCode::Char('0'));
    key(&mut app, KeyCode::Char('v'));
    key(&mut app, KeyCode::Char('$'));
    assert!(app.selection().is_some());
    key(&mut app, KeyCode::Char('y'));

    assert_eq!(app.take_clipboard().as_deref(), Some(selected));
}

#[test]
fn arabic_search_uses_character_prefixes() {
    assert!(word_prefix_match("مرحبا، كيف حالك؟", "كيف"));
    assert!(word_prefix_match("سلام، حالت چطوره؟", "حا"));
    assert!(!word_prefix_match("سلام، حالت چطوره؟", "لات"));
}

#[test]
fn emoji_shortcode_inserts_full_sequence_before_following_text() {
    let mut app = app_with_messages(&["plain message"]);
    key(&mut app, KeyCode::Char('i'));
    type_text(&mut app, ":woman_scientist");
    assert!(app.completion().is_some());
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('x'));

    assert!(app.input.line.text().ends_with("👩‍🔬x"));
    assert!(
        app.input
            .line
            .text()
            .is_char_boundary(app.input.line.caret())
    );
}
