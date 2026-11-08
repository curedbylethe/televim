use std::{borrow::Cow, path::PathBuf};

use demo_support::{DemoSimulator, DemoWorld, Effect};
use domain::{
    global_search::{MediaFilter, SearchSpec},
    message::{MediaKind, Message, MessageStatus},
    presence::Presence,
    report::ReportReason,
};
use tui::{Action, App, LoginField, state::update};

fn simulator() -> DemoSimulator {
    DemoSimulator::new(DemoWorld::new(1_730_000_000))
}

fn app_for(sim: &DemoSimulator) -> App {
    let mut app = App::new();
    app.set_chats(sim.world.active().chats.clone());
    app.select_chat(0);
    app
}

fn message(chat_id: i64, id: i64, text: &str, timestamp: i64, media: Option<MediaKind>) -> Message {
    Message {
        id,
        chat_id,
        text: Cow::Owned(text.to_owned()),
        timestamp,
        status: MessageStatus::Received,
        is_outgoing: false,
        reply_to: None,
        media,
        media_id: None,
    }
}

#[test]
fn every_action_has_an_effect() {
    let mut sim = simulator();
    let app = app_for(&sim);
    let chat_id = sim.world.active().default_chat;
    let other_chat = sim
        .world
        .active()
        .chats
        .iter()
        .find(|chat| chat.id != chat_id)
        .map_or(chat_id, |chat| chat.id);
    let peer_id = sim
        .world
        .active()
        .contacts
        .first()
        .map_or(123, |candidate| candidate.user_id);
    let mut actions = update_and_message_actions(chat_id, other_chat);
    actions.extend(search_actions(chat_id, peer_id));
    actions.extend(native_actions(chat_id, peer_id, sim.world.accounts[1].id));
    assert_eq!(actions.len(), 38);
    for action in actions {
        let effects = sim.dispatch(&app, action);
        assert!(!effects.is_empty(), "each action gets a result");
        assert!(effects.iter().all(
            |effect| !matches!(effect, Effect::Status(text) if text.is_empty() || text == "ok")
        ));
    }
}

#[test]
fn retrying_picker_failure_requests_an_attachment_again() {
    let mut sim = simulator();
    let app = app_for(&sim);
    let chat_id = sim.world.active().default_chat;
    let temp_id = -700;
    sim.record_failed_upload(chat_id, temp_id, MediaKind::Photo);

    let effects = sim.dispatch(&app, Action::RetryUpload { chat_id, temp_id });
    assert!(
        matches!(effects.as_slice(), [Effect::RequestFilePicker { chat_id: id, temp_id: pending, kind: MediaKind::Photo }] if *id == chat_id && *pending == temp_id)
    );
    assert!(
        sim.advance(500).is_empty(),
        "retry waits for a file selection"
    );
}

fn update_and_message_actions(chat_id: i64, other_chat: i64) -> Vec<Action> {
    vec![
        Action::Update(update::Action::Open),
        Action::Update(update::Action::Apply {
            available: "2.0".into(),
        }),
        Action::Update(update::Action::Later {
            available: "2.0".into(),
        }),
        Action::Update(update::Action::Restart),
        Action::Update(update::Action::InstalledLater),
        Action::Update(update::Action::Cancel),
        Action::Send {
            chat_id,
            temp_id: -1,
            text: "maple note".into(),
            reply_to: None,
        },
        Action::SendAttachment {
            chat_id,
            temp_id: -2,
            path: PathBuf::from("/ignored"),
            kind: MediaKind::Photo,
        },
        Action::CancelUpload { temp_id: -2 },
        Action::RetryUpload {
            chat_id,
            temp_id: -2,
        },
        Action::DismissUpload { temp_id: -2 },
        Action::Edit {
            chat_id,
            message_id: 1,
            text: "edited".into(),
        },
        Action::Delete {
            chat_id,
            message_ids: vec![1],
        },
        Action::Forward {
            chat_id,
            message_ids: vec![1],
            dest_chat_id: other_chat,
        },
    ]
}

fn search_actions(chat_id: i64, peer_id: i64) -> Vec<Action> {
    vec![
        Action::FetchContact { peer_id },
        Action::FetchContacts,
        Action::Search {
            chat_id,
            query: "map".into(),
        },
        Action::GlobalSearch {
            spec: SearchSpec::plain("map"),
        },
        Action::MoreGlobal {
            spec: SearchSpec::plain("map"),
        },
        Action::ResolveUser {
            query: "fictional".into(),
        },
        Action::TogglePin {
            chat_id,
            pinned: true,
        },
    ]
}

fn native_actions(chat_id: i64, peer_id: i64, account_id: i64) -> Vec<Action> {
    vec![
        Action::Login {
            field: LoginField::Phone,
            value: "+15550000000".into(),
        },
        Action::LoginCancelled,
        Action::AddAccount,
        Action::SwitchAccount { id: account_id },
        Action::Logout,
        Action::ExportSession {
            path: PathBuf::from("/ignored"),
        },
        Action::ImportSession {
            path: PathBuf::from("/ignored"),
        },
        Action::SubmitSessionPassphrase {
            value: tui::app::Secret::new("fake".into()),
        },
        Action::ChangeSessionPassphrase {
            new: tui::app::Secret::new("fake".into()),
        },
        Action::OpenMedia {
            chat_id,
            message_id: 1,
        },
        Action::SaveMedia {
            chat_id,
            message_id: 1,
            path: PathBuf::from("/ignored"),
            typed: String::new(),
        },
        Action::CancelMediaDownload {
            chat_id,
            message_id: 1,
        },
        Action::SetArchived {
            chat_id,
            archived: true,
        },
        Action::SetBlocked {
            peer_id,
            name: "Fictional peer".into(),
            blocked: true,
        },
        Action::SetLocalName {
            peer_id,
            name: Some("Local name".into()),
        },
        Action::DeleteConversation {
            chat_id,
            name: "Fictional peer".into(),
            both: false,
        },
        Action::ReportPeer {
            peer_id,
            name: "Fictional peer".into(),
            reason: ReportReason::Spam,
        },
    ]
}

#[test]
fn send_success_and_failure_are_scheduled() {
    let mut sim = simulator();
    let app = app_for(&sim);
    let chat_id = sim.world.active().default_chat;
    let queued = sim.dispatch(
        &app,
        Action::Send {
            chat_id,
            temp_id: -1,
            text: "maple note".into(),
            reply_to: None,
        },
    );
    assert!(matches!(queued.as_slice(), [Effect::Status(_)]));
    assert!(matches!(
        sim.advance(1_000).as_slice(),
        [Effect::Sent { temp_id: -1, .. }]
    ));

    sim.dispatch(
        &app,
        Action::Send {
            chat_id,
            temp_id: -2,
            text: "!fail".into(),
            reply_to: None,
        },
    );
    assert!(matches!(sim.advance(299).as_slice(), []));
    assert!(
        matches!(sim.advance(1).as_slice(), [Effect::SendFailed { reason, .. }] if reason.ends_with("failure."))
    );
}

#[test]
fn selected_attachment_records_filename_without_claiming_file_bytes() {
    let mut sim = simulator();
    let chat_id = sim.world.active().default_chat;
    let previous_max = sim
        .world
        .active()
        .messages
        .values()
        .flatten()
        .map(|message| message.id)
        .max()
        .expect("fixtures contain messages");

    let message = sim
        .attachment_message(chat_id, "sketch.png".to_owned(), MediaKind::Photo)
        .expect("fixture chat exists");

    assert!(message.id > previous_max);
    assert_eq!(message.text, "sketch.png");
    assert_eq!(message.media, None);
    assert!(message.is_outgoing);
    assert!(sim.world.active().messages(chat_id).iter().any(|stored| {
        stored.id == message.id && stored.text == message.text && stored.media == message.media
    }));
}

#[test]
fn edit_delete_and_forward_mutate_world() {
    let mut sim = simulator();
    let app = app_for(&sim);
    let chat_id = sim.world.active().default_chat;
    let dest = sim
        .world
        .active()
        .chats
        .iter()
        .find(|chat| chat.id != chat_id)
        .map_or(chat_id, |chat| chat.id);
    let destination_count = sim.world.active().messages(dest).len();
    let media = sim
        .world
        .active()
        .media
        .values()
        .next()
        .cloned()
        .expect("fixtures contain media");
    sim.world
        .active_mut()
        .media
        .insert((chat_id, 50), media.clone());
    sim.world.active_mut().messages.insert(
        chat_id,
        vec![Message {
            is_outgoing: true,
            ..message(chat_id, 50, "original", 1_730_000_001, Some(media.kind))
        }],
    );
    assert!(matches!(
        sim.dispatch(
            &app,
            Action::Edit {
                chat_id,
                message_id: 50,
                text: "changed".into()
            }
        )
        .as_slice(),
        [Effect::Edited { .. }]
    ));
    assert_eq!(
        sim.world.active().messages(chat_id)[0].display_body(),
        "changed"
    );
    let forward_effects = sim.dispatch(
        &app,
        Action::Forward {
            chat_id,
            message_ids: vec![50],
            dest_chat_id: dest,
        },
    );
    let [Effect::Forwarded { messages, .. }] = forward_effects.as_slice() else {
        panic!("message forwards");
    };
    let forwarded_id = messages[0].id;
    assert_eq!(
        sim.world
            .active()
            .media
            .get(&(dest, forwarded_id))
            .map(|m| &m.bytes),
        Some(&media.bytes)
    );
    assert_eq!(
        sim.world.active().messages(dest).len(),
        destination_count + 1
    );
    assert!(
        matches!(sim.dispatch(&app, Action::Delete { chat_id, message_ids: vec![50] }).as_slice(), [Effect::Deleted { message_ids, .. }] if message_ids == &[50])
    );
    assert!(sim.world.active().messages(chat_id).is_empty());
    assert!(!sim.world.active().media.contains_key(&(chat_id, 50)));
    sim.schedule_delete(dest, vec![forwarded_id], 0);
    assert!(!sim.world.active().media.contains_key(&(dest, forwarded_id)));
}

#[test]
fn search_filters_and_global_pages_are_deterministic() {
    let mut sim = simulator();
    let app = app_for(&sim);
    let chat_id = sim.world.active().default_chat;
    sim.world.active_mut().messages.clear();
    let messages: Vec<_> = (1..=25)
        .map(|id| {
            message(
                chat_id,
                id,
                "maple ticket",
                100 + id,
                (id == 3).then_some(MediaKind::Photo),
            )
        })
        .collect();
    sim.world.active_mut().messages.insert(chat_id, messages);
    let local = sim.dispatch(
        &app,
        Action::Search {
            chat_id,
            query: "map".into(),
        },
    );
    assert!(matches!(&local[0], Effect::Searched { ids, total: 25, .. } if ids.len() == 25));

    let mut spec = SearchSpec::plain("map");
    spec.media = MediaFilter::PhotoVideo;
    spec.since = Some(103);
    spec.until = Some(103);
    let first = sim.dispatch(&app, Action::GlobalSearch { spec: spec.clone() });
    assert!(
        matches!(&first[0], Effect::GlobalSearched { hits, total: 1, has_more: false, .. } if hits[0].message_id == 3)
    );

    let spec = SearchSpec::plain("map");
    assert!(
        matches!(sim.dispatch(&app, Action::GlobalSearch { spec: spec.clone() }).as_slice(), [Effect::GlobalSearched { hits, total: 25, has_more: true, append: false, .. }] if hits.len() == 20)
    );
    assert!(
        matches!(sim.dispatch(&app, Action::MoreGlobal { spec }).as_slice(), [Effect::GlobalSearched { hits, total: 25, has_more: false, append: true, .. }] if hits.len() == 5)
    );
}

#[test]
fn contacts_profiles_and_account_switch_use_seeded_data() {
    let mut sim = simulator();
    let app = app_for(&sim);
    let peer_id = sim.world.active().contacts[0].user_id;
    assert!(matches!(
        &sim.dispatch(&app, Action::FetchContact { peer_id })[0],
        Effect::Contact { result: Ok(_), .. }
    ));
    assert!(
        matches!(&sim.dispatch(&app, Action::FetchContacts)[0], Effect::Contacts { result: Ok(items) } if !items.is_empty())
    );
    assert!(
        matches!(&sim.dispatch(&app, Action::FetchContact { peer_id: -99 })[0], Effect::Contact { result: Err(reason), .. } if !reason.is_empty())
    );
    let next = sim.world.accounts[1].id;
    assert!(
        matches!(sim.dispatch(&app, Action::SwitchAccount { id: next }).as_slice(), [Effect::SwitchAccount { id }] if *id == next)
    );
    assert_eq!(sim.world.active().id, next);
    assert!(matches!(
        sim.dispatch(&app, Action::SwitchAccount { id: -1 })
            .as_slice(),
        [Effect::Status(_)]
    ));
}

#[test]
fn local_mutations_persist_until_reset() {
    let mut sim = simulator();
    let app = app_for(&sim);
    let chat_id = sim.world.active().default_chat;
    let peer_id = -90_001;
    sim.dispatch(
        &app,
        Action::TogglePin {
            chat_id,
            pinned: true,
        },
    );
    sim.dispatch(
        &app,
        Action::SetArchived {
            chat_id,
            archived: true,
        },
    );
    sim.dispatch(
        &app,
        Action::SetBlocked {
            peer_id,
            name: "Fictional".into(),
            blocked: true,
        },
    );
    sim.dispatch(
        &app,
        Action::SetLocalName {
            peer_id,
            name: Some("Local".into()),
        },
    );
    sim.dispatch(
        &app,
        Action::ReportPeer {
            peer_id,
            name: "Fictional".into(),
            reason: ReportReason::Spam,
        },
    );
    assert!(sim.world.active().chat(chat_id).is_some());
    assert!(
        sim.world
            .active()
            .archived
            .iter()
            .any(|chat| chat.id == chat_id)
    );
    assert!(sim.world.active().blocked.contains(&peer_id));
    assert_eq!(
        sim.world
            .active()
            .local_names
            .get(&peer_id)
            .map(String::as_str),
        Some("Local")
    );
    assert_eq!(sim.reports, vec![(peer_id, ReportReason::Spam)]);
    let reset = DemoWorld::new(sim.world.base_timestamp);
    assert!(!reset.active().blocked.contains(&peer_id));
    assert!(!reset.active().local_names.contains_key(&peer_id));
}

#[test]
fn scheduled_arrivals_respect_open_chat_and_timing() {
    let mut sim = simulator();
    let open = sim.world.active().default_chat;
    let closed = sim
        .world
        .active()
        .chats
        .iter()
        .find(|chat| chat.id != open)
        .map_or(open + 1, |chat| chat.id);
    sim.set_open_chat_id(Some(open));
    let open_unread = sim
        .world
        .active()
        .chat(open)
        .map_or(0, |chat| chat.unread_count);
    let closed_unread = sim
        .world
        .active()
        .chat(closed)
        .map_or(0, |chat| chat.unread_count);
    let open_messages = sim.world.active().messages(open).len();
    let closed_messages = sim.world.active().messages(closed).len();
    sim.schedule_arrival(open, "hello", None, 100);
    sim.schedule_arrival(closed, "hello", None, 100);
    sim.schedule_typing(closed, true, 200);
    sim.schedule_presence(closed, Presence::Online, 300);
    assert_eq!(
        sim.world.active().chat(open).map(|chat| chat.unread_count),
        Some(open_unread)
    );
    assert_eq!(sim.world.active().messages(open).len(), open_messages);
    assert_eq!(sim.world.active().messages(closed).len(), closed_messages);
    assert_eq!(
        sim.world
            .active()
            .chat(closed)
            .map(|chat| chat.unread_count),
        Some(closed_unread)
    );
    assert_eq!(sim.advance(99).len(), 0);
    assert_eq!(sim.advance(1).len(), 2);
    assert_eq!(
        sim.world.active().chat(open).map(|chat| chat.unread_count),
        Some(open_unread)
    );
    assert_eq!(
        sim.world
            .active()
            .chat(closed)
            .map(|chat| chat.unread_count),
        Some(closed_unread + 1)
    );
    assert_eq!(sim.world.active().messages(open).len(), open_messages + 1);
    assert_eq!(
        sim.world.active().messages(closed).len(),
        closed_messages + 1
    );
    assert!(matches!(
        sim.advance(100).as_slice(),
        [Effect::Update(domain::updates::UpdateEvent::PeerTyping {
            typing: true,
            ..
        })]
    ));
    assert!(matches!(
        sim.advance(100).as_slice(),
        [Effect::Update(domain::updates::UpdateEvent::PeerStatus {
            presence: Presence::Online,
            ..
        })]
    ));
}

#[test]
fn blocked_peer_arrivals_are_suppressed_until_unblocked() {
    let mut sim = simulator();
    let app = app_for(&sim);
    let chat_id = *sim
        .world
        .active()
        .blocked
        .iter()
        .next()
        .expect("blocked fixture");
    let before_messages = sim.world.active().messages(chat_id).len();
    let before_unread = sim
        .world
        .active()
        .chat(chat_id)
        .expect("blocked peer has a chat")
        .unread_count;

    sim.schedule_arrival(chat_id, "suppressed message", None, 100);

    assert!(sim.advance(100).is_empty());
    assert_eq!(sim.world.active().messages(chat_id).len(), before_messages);
    assert_eq!(
        sim.world
            .active()
            .chat(chat_id)
            .map(|chat| chat.unread_count),
        Some(before_unread)
    );

    let effects = sim.dispatch(
        &app,
        Action::SetBlocked {
            peer_id: chat_id,
            name: "Fixture peer".into(),
            blocked: false,
        },
    );
    assert!(matches!(
        effects.as_slice(),
        [Effect::Blocked { blocked: false, .. }]
    ));
    sim.schedule_arrival(chat_id, "permitted message", None, 100);

    assert!(matches!(
        sim.advance(100).as_slice(),
        [Effect::Update(domain::updates::UpdateEvent::NewMessage(message))]
            if message.text == "permitted message"
    ));
    assert_eq!(
        sim.world.active().messages(chat_id).len(),
        before_messages + 1
    );
    assert_eq!(
        sim.world
            .active()
            .chat(chat_id)
            .map(|chat| chat.unread_count),
        Some(before_unread + 1)
    );

    let unblocked_chat = sim
        .world
        .active()
        .chats
        .iter()
        .find(|chat| chat.id != chat_id)
        .expect("fixture has an unblocked chat")
        .id;
    let before_messages = sim.world.active().messages(unblocked_chat).len();
    let before_unread = sim
        .world
        .active()
        .chat(unblocked_chat)
        .expect("chat exists")
        .unread_count;
    sim.schedule_arrival(unblocked_chat, "blocked before delivery", None, 100);
    sim.dispatch(
        &app,
        Action::SetBlocked {
            peer_id: unblocked_chat,
            name: "Fixture peer".into(),
            blocked: true,
        },
    );
    assert!(sim.advance(100).is_empty());
    assert_eq!(
        sim.world.active().messages(unblocked_chat).len(),
        before_messages
    );
    assert_eq!(
        sim.world
            .active()
            .chat(unblocked_chat)
            .expect("chat exists")
            .unread_count,
        before_unread
    );
}

#[test]
fn cancelled_upload_retry_reopens_picker_without_a_fake_message() {
    let mut sim = simulator();
    let app = app_for(&sim);
    let chat_id = sim.world.active().default_chat;
    let before = sim.world.active().messages(chat_id).len();
    let temp_id = -900;
    sim.schedule(
        100,
        Effect::UploadProgress {
            chat_id,
            temp_id,
            sent: 1,
            total: 2,
        },
    );
    sim.schedule(
        300,
        Effect::SendFailed {
            chat_id,
            temp_id,
            reason: "old failure".into(),
        },
    );

    assert!(matches!(
        sim.dispatch(&app, Action::CancelUpload { temp_id }).as_slice(),
        [Effect::SendFailed { reason, .. }] if reason == "Upload cancelled; nothing was sent."
    ));
    assert!(matches!(
        sim.dispatch(&app, Action::RetryUpload { chat_id, temp_id }).as_slice(),
        [Effect::RequestFilePicker { chat_id: id, temp_id: pending, kind: MediaKind::File }]
            if *id == chat_id && *pending == temp_id
    ));
    assert!(sim.advance(500).is_empty());
    assert_eq!(sim.world.active().messages(chat_id).len(), before);

    sim.schedule(
        100,
        Effect::SendFailed {
            chat_id,
            temp_id: -901,
            reason: "old failure".into(),
        },
    );
    sim.record_failed_upload(chat_id, -901, MediaKind::File);
    sim.dispatch(&app, Action::DismissUpload { temp_id: -901 });
    assert!(sim.advance(100).is_empty());
}

#[test]
fn deleting_default_chat_selects_an_active_fallback() {
    let mut sim = simulator();
    let app = app_for(&sim);
    let deleted = sim.world.active().default_chat;

    assert!(matches!(
        sim.dispatch(
            &app,
            Action::DeleteConversation {
                chat_id: deleted,
                name: "Fixture peer".into(),
                both: false,
            }
        )
        .as_slice(),
        [Effect::ConversationDeleted { chat_id }] if *chat_id == deleted
    ));
    let fallback = sim.world.active().default_chat;
    assert_ne!(fallback, deleted);
    assert!(
        sim.world
            .active()
            .chats
            .iter()
            .any(|chat| chat.id == fallback)
    );
}

#[test]
fn unsupported_native_actions_say_why() {
    let mut sim = simulator();
    let app = app_for(&sim);
    for action in [
        Action::Update(update::Action::Apply {
            available: "2.0".into(),
        }),
        Action::Login {
            field: LoginField::Phone,
            value: "secret".into(),
        },
        Action::ExportSession {
            path: PathBuf::from("/ignored"),
        },
        Action::ImportSession {
            path: PathBuf::from("/ignored"),
        },
        Action::Logout,
        Action::AddAccount,
    ] {
        let effects = sim.dispatch(&app, action);
        assert!(effects.iter().any(|effect| matches!(effect, Effect::Status(sentence) if !sentence.is_empty() && sentence != "ok") || matches!(effect, Effect::UpdateUnavailable)));
    }
}
