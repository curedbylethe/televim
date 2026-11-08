use domain::message::{MediaKind, MessageStatus};

use demo_support::DemoWorld;

const SEEDS: [[&str; 3]; 3] = [
    ["lantern", "harbor", "orchard"],
    ["atlas", "meadow", "signal"],
    ["cobalt", "willow", "ember"],
];

#[test]
fn world_has_three_complete_fictional_accounts() {
    let world = DemoWorld::new(1_700_000_000);
    assert_eq!(world.accounts.len(), 3);

    for (account_index, account) in world.accounts.iter().enumerate() {
        assert_eq!(account.chats.len(), 5);
        assert_eq!(account.archived.len(), 1);
        assert_eq!(account.profiles.len(), 6);
        assert_eq!(account.contacts.len(), 6);
        assert!(account.account.phone.is_none());

        let all_chats = account
            .chats
            .iter()
            .chain(&account.archived)
            .collect::<Vec<_>>();
        let mut ids = all_chats.iter().map(|chat| chat.id).collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all_chats.len(), "chat ids are unique");

        for chat in all_chats {
            assert_eq!(
                account.profile(chat.id).map(|profile| profile.user_id),
                Some(chat.id)
            );
            assert!(
                account
                    .contacts
                    .iter()
                    .any(|contact| contact.user_id == chat.id)
            );
            let messages = account.messages(chat.id);
            if chat.id == account.chats[4].id {
                assert!(messages.is_empty(), "one active chat stays empty");
                continue;
            }
            assert!((15..=30).contains(&messages.len()));
            assert!(messages.iter().all(|message| message.chat_id == chat.id));
            assert!(messages.windows(2).all(|pair| pair[0].id < pair[1].id));
            assert!(
                messages
                    .windows(2)
                    .all(|pair| pair[0].timestamp < pair[1].timestamp)
            );
        }

        for (seed_index, seed) in SEEDS[account_index].iter().enumerate() {
            let chat = &account.chats[seed_index];
            assert!(
                account
                    .messages(chat.id)
                    .iter()
                    .any(|message| message.text.contains(seed))
            );
        }

        assert_eq!(account.chats.iter().filter(|chat| chat.pinned).count(), 1);
        assert_eq!(account.archived.len(), 1);
        assert!(account.chats.iter().any(|chat| chat.unread_count > 0));
        assert_eq!(account.blocked.len(), 1);
        assert!(account.chats[0]
            .last_message_id
            .and_then(|id| account.messages(account.chats[0].id).iter().find(|m| m.id == id))
            .is_some_and(|message| message.is_outgoing && message.status == MessageStatus::Sent));
    }
}

#[test]
fn fixture_text_covers_unicode_and_media_is_local() {
    let world = DemoWorld::new(1_700_000_000);
    let text = world
        .accounts
        .iter()
        .flat_map(|account| account.messages.values())
        .flatten()
        .map(|message| message.text.as_ref())
        .collect::<Vec<_>>()
        .join(" ");

    for sample in [
        "سلام، حالت چطوره؟",
        "مرحبا، كيف حالك؟",
        "cafe\u{301}",
        "👩‍🔬",
        "❤️",
        ":wave:",
        "https://example.invalid/field-note",
    ] {
        assert!(text.contains(sample), "missing fixture text {sample:?}");
    }

    assert!(world.accounts.iter().any(|account| {
        account.media.values().any(|media| {
            media.kind == MediaKind::Photo
                && media.mime == "image/svg+xml"
                && media.bytes.starts_with(b"<svg")
        })
    }));
    assert!(world.accounts.iter().any(|account| {
        account
            .media
            .values()
            .any(|media| media.kind == MediaKind::Sticker && !media.downloadable)
    }));
    assert!(
        world
            .accounts
            .iter()
            .flat_map(|account| account.profiles.values())
            .all(|p| p.phone.is_none())
    );
}

#[test]
fn same_timestamp_builds_same_world_and_account_switch_is_by_id() {
    let first = DemoWorld::new(123);
    let second = DemoWorld::new(123);
    let first_summary = first
        .accounts
        .iter()
        .map(|account| {
            (
                account.id,
                account.chats.iter().map(|chat| chat.id).collect::<Vec<_>>(),
                account
                    .messages
                    .values()
                    .flatten()
                    .map(|message| (message.id, message.text.to_string(), message.timestamp))
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    let second_summary = second
        .accounts
        .iter()
        .map(|account| {
            (
                account.id,
                account.chats.iter().map(|chat| chat.id).collect::<Vec<_>>(),
                account
                    .messages
                    .values()
                    .flatten()
                    .map(|message| (message.id, message.text.to_string(), message.timestamp))
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(first_summary, second_summary);

    let mut world = first;
    assert!(world.switch_to(10_002));
    assert_eq!(world.active().id, 10_002);
    assert!(!world.switch_to(99_999));
    assert_eq!(
        world.account(10_003).map(|account| account.label.as_str()),
        Some("Samir Vale")
    );
}
