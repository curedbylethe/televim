//! Writes a sealed warm-start archive for `make measure`'s populated-binary scenario.
//!
//! Usage: `seed_history <dir> <passphrase>`. Writes `<dir>/televim.history.db`,
//! the archive the binary loads when `--config <dir>/televim.toml` is given, and
//! prints one JSON object describing what it wrote.
//!
//! The app crate has no lib target, so this includes the store's source as a
//! module and uses its own writer — `HistoryFile::encode` then `try_write`, the
//! path `net.rs` takes — rather than mirroring the on-disk format here. Sizes
//! stay under the cache's bounds (32 peers, 200 rows each) so nothing is
//! dropped on load.

#[allow(dead_code)]
#[path = "../src/history_store.rs"]
mod history_store;

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use domain::chat::{Chat, ChatKind};
use domain::message::{Message, MessageStatus};
use history_store::{HistoryCache, HistoryFile, HistoryTag};
use serde::Serialize;
use telegram_framework::PassphraseProvider;

const CHATS: i64 = 60;
const PEERS: i64 = 32;
const ROWS: i64 = 200;

#[derive(Serialize)]
struct Summary {
    chats: i64,
    peers: i64,
    messages: usize,
    text_bytes: usize,
    file_bytes: u64,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("usage: seed_history <dir> <passphrase>"));
    let passphrase = args.next().expect("usage: seed_history <dir> <passphrase>");

    let mut cache = HistoryCache::default();

    let chats: Vec<Chat> = (1..=CHATS)
        .map(|id| Chat {
            id,
            title: format!("Seeded chat {id:02}"),
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
            read_outbox_max_id: None,
        })
        .collect();
    cache.set_chats(&chats);

    let mut text_bytes = 0_usize;
    let mut messages = 0_usize;
    for peer in 1..=PEERS {
        let rows: Vec<Message> = (1..=ROWS)
            .map(|id| {
                let text = format!(
                    "peer {peer} message {id}: the quick brown fox jumps over the lazy dog, \
                     then writes a few more words so a row has a realistic length"
                );
                text_bytes += text.len();
                messages += 1;
                Message {
                    id,
                    chat_id: peer,
                    text: Cow::Owned(text),
                    timestamp: 1_700_000_000 + id,
                    status: MessageStatus::Received,
                    is_outgoing: id % 3 == 0,
                    is_service: false,
                    reply_to: None,
                    media: None,
                    media_id: None,
                }
            })
            .collect();
        cache.put(peer, &rows);
    }

    let keys: Arc<dyn telegram_framework::KeyProvider> =
        Arc::new(PassphraseProvider::new(&passphrase));
    let file = HistoryFile::new(dir.join("televim.history.json"), keys);
    let tag = HistoryTag {
        account_id: None,
        phone: None,
    };
    let bytes = HistoryFile::encode(&cache, &tag).expect("encode seeded history");
    file.try_write(&bytes).expect("write seeded archive");

    let db = dir.join("televim.history.db");
    let file_bytes = std::fs::metadata(&db).expect("seeded archive exists").len();

    let summary = Summary {
        chats: CHATS,
        peers: PEERS,
        messages,
        text_bytes,
        file_bytes,
    };
    println!(
        "{}",
        serde_json::to_string(&summary).expect("serialise summary")
    );
}
