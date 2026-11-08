use std::borrow::Cow;

use demo_support::{DemoSimulator, DemoWorld, Effect};
use domain::{
    global_search::{MediaFilter, SearchSpec},
    message::{MediaKind, Message, MessageStatus},
    report::ReportReason,
    updates::UpdateEvent,
};
use tui::{
    Action, App, FetchDirection,
    state::ui::{GraphicsMode, StickerMode},
};

const NOW: i64 = 1_730_000_000;

fn app_for(sim: &DemoSimulator, chat_id: i64, messages: Vec<Message>) -> App {
    let account = sim.world.active();
    let mut app = App::new();
    app.set_client_available(true);
    app.set_account(Ok(account.account.clone()));
    app.set_known_accounts(account.known_accounts.clone());
    app.set_chats(account.chats.clone());
    app.set_archived_chats(account.archived.clone());
    app.select_chat_by_id(chat_id);
    assert!(app.apply_latest(messages));
    app
}

fn message(chat_id: i64, id: i64, body: &str, media: Option<MediaKind>) -> Message {
    Message {
        id,
        chat_id,
        text: Cow::Owned(body.to_owned()),
        timestamp: NOW.saturating_add(id),
        status: MessageStatus::Received,
        is_outgoing: false,
        reply_to: None,
        media,
        media_id: None,
    }
}

fn fixture() -> (DemoSimulator, App) {
    let sim = DemoSimulator::new(DemoWorld::new(NOW));
    let chat_id = sim.world.active().default_chat;
    let app = app_for(&sim, chat_id, sim.world.active().messages(chat_id).to_vec());
    (sim, app)
}

fn assert_active_chat(sim: &DemoSimulator, chat_id: i64) {
    assert!(
        sim.world
            .active()
            .chats
            .iter()
            .any(|chat| chat.id == chat_id)
    );
}

#[test]
fn pins_archives_and_account_switches_update_the_fictional_world() {
    let (mut sim, mut app) = fixture();
    let chat_id = sim.world.active().default_chat;
    let chat_title = app.chats()[app.list.selected_chat].title.clone();
    let second = sim
        .world
        .active()
        .chats
        .iter()
        .find(|chat| chat.id != chat_id)
        .expect("fixture has another chat")
        .id;
    let selected = app.chats()[app.list.selected_chat].id;

    let pin = sim.dispatch(
        &app,
        Action::TogglePin {
            chat_id: second,
            pinned: true,
        },
    );
    assert!(
        matches!(pin.as_slice(), [Effect::PinToggled { chat_id: id, pinned: true }] if *id == second)
    );
    app.set_pinned(second, true);
    assert_eq!(app.chats()[0].id, second);
    assert_eq!(app.chats()[app.list.selected_chat].id, selected);
    assert!(app.select_chat_by_id(chat_id));

    let archived = sim.dispatch(
        &app,
        Action::SetArchived {
            chat_id,
            archived: true,
        },
    );
    assert!(matches!(
        archived.as_slice(),
        [Effect::Archived { archived: true, .. }]
    ));
    assert_eq!(
        app.set_archived(chat_id, true).as_deref(),
        Some(chat_title.as_str())
    );
    assert_eq!(
        app.conversation.conversation.window.chat_id, 0,
        "archiving open chat closes it"
    );
    assert!(!app.chats().iter().any(|chat| chat.id == chat_id));
    let fallback_chat = sim.world.active().default_chat;
    assert_ne!(fallback_chat, chat_id);
    assert_active_chat(&sim, fallback_chat);

    let restored = sim.dispatch(
        &app,
        Action::SetArchived {
            chat_id,
            archived: false,
        },
    );
    assert!(matches!(
        restored.as_slice(),
        [Effect::Archived {
            archived: false,
            ..
        }]
    ));
    assert_eq!(
        app.set_archived(chat_id, false).as_deref(),
        Some(chat_title.as_str())
    );
    assert!(app.chats().iter().any(|chat| chat.id == chat_id));

    let original_account = sim.world.active().id;
    let other_account = sim
        .world
        .accounts
        .iter()
        .find(|account| account.id != original_account)
        .expect("fixture has another account")
        .id;
    sim.schedule_arrival(chat_id, "pending account message", None, 5_000);
    assert!(matches!(
        sim.dispatch(&app, Action::SwitchAccount { id: other_account }).as_slice(),
        [Effect::SwitchAccount { id }] if *id == other_account
    ));
    assert!(
        sim.advance(5_000).is_empty(),
        "switch clears old account events"
    );
    assert_eq!(sim.world.active().id, other_account);
    assert!(sim.world.switch_to(original_account));
    assert_active_chat(&sim, fallback_chat);
    assert!(
        sim.world
            .active()
            .chats
            .iter()
            .any(|chat| chat.id == second && chat.pinned)
    );
}

#[test]
fn contacts_profiles_blocks_reports_and_empty_results_use_seeded_data() {
    let (mut sim, mut app) = fixture();
    let peer_id = app.current_chat_id();
    let blocked = sim.world.active().blocked.contains(&peer_id);
    let profile = sim
        .world
        .active()
        .profile(peer_id)
        .cloned()
        .expect("fixture profile exists");

    assert!(matches!(
        sim.dispatch(&app, Action::FetchContact { peer_id }).as_slice(),
        [Effect::Contact { peer_id: id, blocked: value, result: Ok(account) }]
            if *id == peer_id && *value == blocked && account == &profile
    ));
    app.set_contact(peer_id, Some(blocked), Ok(profile));
    let contacts = sim.dispatch(&app, Action::FetchContacts);
    let [
        Effect::Contacts {
            result: Ok(contacts),
        },
    ] = contacts.as_slice()
    else {
        panic!("fixture contacts load");
    };
    let candidate = contacts.first().expect("fixture has a contact");
    assert!(matches!(
        sim.dispatch(
            &app,
            Action::ResolveUser {
                query: candidate.display_name.clone(),
            }
        )
        .as_slice(),
        [Effect::UserSearch { query, result: Ok(results) }]
            if query == &candidate.display_name && results.iter().any(|result| result.user_id == candidate.user_id)
    ));

    sim.world.active_mut().contacts.clear();
    assert!(matches!(
        sim.dispatch(&app, Action::ResolveUser { query: "nobody".into() }).as_slice(),
        [Effect::UserSearch { query, result: Err(reason) }]
            if query == "nobody" && reason == "No demo contact matches that search."
    ));

    let updated = sim.dispatch(
        &app,
        Action::SetBlocked {
            peer_id,
            name: "Fixture contact".into(),
            blocked: !blocked,
        },
    );
    assert!(
        matches!(updated.as_slice(), [Effect::Blocked { blocked: value, .. }] if *value != blocked)
    );
    app.set_blocked(peer_id, !blocked);

    let reported = sim.dispatch(
        &app,
        Action::ReportPeer {
            peer_id,
            name: "Fixture contact".into(),
            reason: ReportReason::Spam,
        },
    );
    assert!(
        matches!(reported.as_slice(), [Effect::Reported { reason: ReportReason::Spam, .. }, Effect::Status(text)] if text == "Report recorded locally. No report was sent to Telegram.")
    );
    assert_eq!(sim.reports, vec![(peer_id, ReportReason::Spam)]);

    sim.world.active_mut().contacts.clear();
    assert!(matches!(
        sim.dispatch(&app, Action::FetchContacts).as_slice(),
        [Effect::Contacts { result: Ok(results) }] if results.is_empty()
    ));
}

#[test]
fn history_pages_and_read_receipts_apply_to_real_app_state() {
    let mut sim = DemoSimulator::new(DemoWorld::new(NOW));
    let chat_id = sim.world.active().default_chat;
    let all = sim.world.active().messages(chat_id).to_vec();
    assert!(
        all.len() >= 5,
        "fixture has messages on both sides of an anchor"
    );
    let anchor = all[all.len() / 2].clone();
    let mut app = app_for(&sim, chat_id, vec![anchor.clone()]);

    let older = sim
        .history_page(chat_id, FetchDirection::Older, anchor.id, 2)
        .expect("older page exists");
    let Effect::HistoryPage {
        direction,
        messages,
        exhausted,
        ..
    } = older
    else {
        panic!("simulator returns a history page");
    };
    assert_eq!(direction, FetchDirection::Older);
    assert!(!exhausted);
    app.begin_fetch(FetchDirection::Older);
    assert!(app.apply_older(messages));
    app.end_fetch(FetchDirection::Older);

    let newer = sim
        .history_page(chat_id, FetchDirection::Newer, anchor.id, 2)
        .expect("newer page exists");
    let Effect::HistoryPage {
        direction,
        messages,
        exhausted,
        ..
    } = newer
    else {
        panic!("simulator returns a history page");
    };
    assert_eq!(direction, FetchDirection::Newer);
    assert!(!exhausted);
    app.begin_fetch(FetchDirection::Newer);
    assert!(app.apply_newer(messages));
    app.end_fetch(FetchDirection::Newer);

    sim.schedule_read_receipt(chat_id, anchor.id, 50);
    let due = sim.advance(50);
    let [
        Effect::Update(
            receipt @ UpdateEvent::ReadReceipt {
                chat_id: id,
                max_id,
            },
        ),
    ] = due.as_slice()
    else {
        panic!("simulator schedules read receipts");
    };
    assert_eq!((*id, *max_id), (chat_id, anchor.id));
    assert!(app.apply_update(receipt));
    assert_eq!(app.conversation.conversation.window.len(), 5);
    assert!(matches!(
        sim.history_page(chat_id, FetchDirection::Older, all[0].id, 2),
        Some(Effect::HistoryPage { exhausted: true, messages, .. }) if messages.is_empty()
    ));
}

#[test]
fn filtered_global_search_pages_append_to_real_search_state() {
    let mut sim = DemoSimulator::new(DemoWorld::new(NOW));
    let chat_id = sim.world.active().default_chat;
    let messages: Vec<_> = (1..=25)
        .map(|id| {
            message(
                chat_id,
                id,
                "maple ticket",
                (id == 3).then_some(MediaKind::Photo),
            )
        })
        .collect();
    sim.world.active_mut().messages.clear();
    sim.world
        .active_mut()
        .messages
        .insert(chat_id, messages.clone());
    let app = app_for(&sim, chat_id, messages);
    let mut spec = SearchSpec::plain("map");
    spec.media = MediaFilter::PhotoVideo;
    spec.since = Some(NOW + 3);
    spec.until = Some(NOW + 3);
    assert!(matches!(
        sim.dispatch(&app, Action::GlobalSearch { spec: spec.clone() }).as_slice(),
        [Effect::GlobalSearched { hits, total: 1, has_more: false, append: false, .. }]
            if hits[0].message_id == 3
    ));

    let spec = SearchSpec::plain("map");
    let first_page = sim.dispatch(&app, Action::GlobalSearch { spec: spec.clone() });
    let [
        Effect::GlobalSearched {
            hits,
            total,
            dropped,
            has_more,
            append,
            ..
        },
    ] = first_page.as_slice()
    else {
        panic!("first search page");
    };
    assert_eq!(hits.len(), 20);
    assert_eq!((*total, *dropped, *has_more, *append), (25, 0, true, false));
    let mut app = app;
    app.begin_global_search(&spec);
    assert!(app.adopt_global_search(&spec, hits.clone(), *total, *dropped, *has_more));

    app.begin_global_search(&spec);
    let next_page = sim.dispatch(&app, Action::MoreGlobal { spec: spec.clone() });
    let [
        Effect::GlobalSearched {
            hits,
            total,
            dropped,
            has_more,
            append,
            ..
        },
    ] = next_page.as_slice()
    else {
        panic!("second search page");
    };
    assert_eq!(hits.len(), 5);
    assert!((*append) && !(*has_more));
    assert!(app.append_global_search(&spec, hits.clone(), *total, *dropped, *has_more));
    assert_eq!(app.global_search().hits().len(), 25);
}

#[test]
fn media_open_save_cancel_and_token_sticker_use_fixture_bytes() {
    let (mut sim, app) = fixture();
    let ((chat_id, message_id), expected_filename, expected_bytes) = {
        let (&key, media) = sim
            .world
            .active()
            .media
            .iter()
            .find(|(_, media)| media.downloadable)
            .expect("fixture has downloadable media");
        (key, media.filename.clone(), media.bytes.clone())
    };
    assert!(matches!(
        sim.dispatch(&app, Action::OpenMedia { chat_id, message_id }).as_slice(),
        [Effect::RequestOpenMedia { chat_id: id, message_id: mid }] if (*id, *mid) == (chat_id, message_id)
    ));
    assert!(matches!(
        sim.dispatch(
            &app,
            Action::SaveMedia {
                chat_id,
                message_id,
                path: "ignored".into(),
                typed: String::new(),
            }
        )
        .as_slice(),
        [Effect::RequestSaveMedia { filename, bytes, .. }]
            if filename == &expected_filename && bytes == &expected_bytes
    ));

    assert!(matches!(
        sim.dispatch(
            &app,
            Action::CancelMediaDownload {
                chat_id,
                message_id
            }
        )
        .as_slice(),
        [Effect::Status(text)] if text == "No media download is running."
    ));

    let sticker = sim
        .world
        .active()
        .media
        .values()
        .find(|media| media.kind == MediaKind::Sticker)
        .expect("fixture has a sticker");
    assert_eq!(sticker.mime, "text/plain");
    assert_eq!(sticker.bytes, b"[fictional sticker token]");
    let token_app = App::new()
        .with_stickers(StickerMode::Token)
        .with_graphics(GraphicsMode::Halfblocks);
    assert_eq!(token_app.sticker_mode(), StickerMode::Token);
    assert_eq!(token_app.graphics(), GraphicsMode::Halfblocks);
}

#[test]
fn browser_only_actions_keep_offline_claims_honest() {
    let (mut sim, app) = fixture();
    for (action, expected) in [
        (
            Action::AddAccount,
            "The demo has a fixed cast. Use the account card to switch.",
        ),
        (
            Action::Logout,
            "Logout is disabled. Use Reset demo to start over.",
        ),
        (
            Action::ExportSession {
                path: "ignored".into(),
            },
            "Session export is unavailable in the browser demo.",
        ),
        (
            Action::ImportSession {
                path: "ignored".into(),
            },
            "Session import is unavailable in the browser demo.",
        ),
        (
            Action::SubmitSessionPassphrase {
                value: tui::app::Secret::new("fictional".into()),
            },
            "There is no session passphrase in the demo.",
        ),
    ] {
        assert!(matches!(
            sim.dispatch(&app, action).as_slice(),
            [Effect::Status(text)] if text == expected
        ));
    }
    assert!(matches!(
        sim.dispatch(&app, Action::Update(tui::state::update::Action::Later { available: "1.0".into() })).as_slice(),
        [Effect::Status(text)] if text == "Updates are disabled in the offline demo."
    ));
}
