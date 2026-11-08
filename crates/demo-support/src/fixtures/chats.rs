use domain::chat::{Chat, ChatKind};
use domain::presence::Presence;

pub(super) fn build(account_index: usize, base_timestamp: i64) -> (Vec<Chat>, Vec<Chat>) {
    let account_offset = i64::try_from(account_index).expect("three fixture accounts fit i64");
    let first_peer = 20_001 + account_offset * 1_000;
    let chats = (0_usize..5)
        .map(|index| {
            let peer_offset = i64::try_from(index).expect("six fixture peers fit i64");
            let id = first_peer + peer_offset;
            let presence = match index {
                0 => Some(Presence::Online),
                1 => Some(Presence::Offline {
                    was_online: i32::try_from(base_timestamp.saturating_sub(3_600))
                        .unwrap_or(i32::MAX),
                }),
                2 => Some(Presence::Recently),
                _ => None,
            };
            Chat {
                read_outbox_max_id: Some(0),
                id,
                title: peer_name(account_index, index).to_owned(),
                kind: ChatKind::Private,
                last_message: None,
                unread_count: if index == 1 { 4 } else { 0 },
                last_message_id: None,
                last_timestamp: None,
                pinned: index == 0,
                muted: false,
                unread_mark: false,
                presence,
                deleted: false,
            }
        })
        .collect();
    let archived = vec![Chat {
        read_outbox_max_id: None,
        id: first_peer + 5,
        title: peer_name(account_index, 5).to_owned(),
        kind: ChatKind::Private,
        last_message: None,
        unread_count: 0,
        last_message_id: None,
        last_timestamp: None,
        pinned: false,
        muted: false,
        unread_mark: false,
        presence: None,
        deleted: false,
    }];
    // The last active row is deliberately empty; it opens a real empty composer.
    (chats, archived)
}

fn peer_name(account_index: usize, peer_index: usize) -> &'static str {
    const NAMES: [[&str; 6]; 3] = [
        [
            "Juniper Reed",
            "Rowan Hale",
            "Ibis North",
            "Cedar Vale",
            "Moss Bell",
            "Pine West",
        ],
        [
            "Tala Moss",
            "Rafi Noor",
            "Dara Finch",
            "Laleh Stone",
            "Omid Lake",
            "Roya Field",
        ],
        [
            "Sora Ash",
            "Nuri Dawn",
            "Kian Reed",
            "Mina Shore",
            "Eren Vale",
            "Ari Grove",
        ],
    ];
    NAMES[account_index][peer_index]
}
