mod chats;
mod media;
mod messages;
mod people;

use std::collections::{BTreeMap, BTreeSet};

use tui::KnownAccount;

use crate::{DemoAccount, DemoMedia};

pub(super) fn accounts(base_timestamp: i64) -> Vec<DemoAccount> {
    let cast = people::cast();
    (0..cast.len())
        .map(|index| account(index, base_timestamp, &cast))
        .collect()
}

fn account(index: usize, base_timestamp: i64, cast: &[domain::account::Account]) -> DemoAccount {
    let account = cast[index].clone();
    let id = account.user_id;
    let label = account.display_name();
    let known_accounts = cast
        .iter()
        .map(|person| KnownAccount {
            id: person.user_id,
            label: person.display_name(),
            active: person.user_id == id,
        })
        .collect();
    let (mut chats, mut archived) = chats::build(index, base_timestamp);
    let chat_ids = chats
        .iter()
        .chain(&archived)
        .map(|chat| chat.id)
        .collect::<Vec<_>>();
    let (contacts, profiles) = people::peers(index, &chat_ids);
    let mut messages = BTreeMap::new();
    let mut media = BTreeMap::<(i64, i64), DemoMedia>::new();

    for (chat_index, chat) in chats.iter_mut().chain(&mut archived).enumerate() {
        let thread = messages::thread(index, chat.id, chat_index, base_timestamp);
        if let Some(latest) = thread.last() {
            chat.last_message = Some(latest.display_body().to_owned().into());
            chat.last_message_id = Some(latest.id);
            chat.last_timestamp = Some(latest.timestamp);
        }
        for message in &thread {
            if let Some(kind) = message.media {
                media.insert(
                    (chat.id, message.id),
                    media::fixture(kind, index, chat_index, message.id),
                );
            }
        }
        messages.insert(chat.id, thread);
    }

    let blocked = BTreeSet::from([chat_ids[2]]);
    let local_names = BTreeMap::from([(chat_ids[0], "Northstar notes".to_owned())]);
    DemoAccount {
        id,
        label,
        account,
        known_accounts,
        default_chat: chats[0].id,
        chats,
        archived,
        messages,
        contacts,
        profiles,
        blocked,
        local_names,
        media,
    }
}
