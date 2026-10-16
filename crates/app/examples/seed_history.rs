//! Writes a sealed warm-start cache for `make measure`'s populated-binary scenario.
//!
//! Usage: `seed_history <dir> <passphrase>`. Writes `<dir>/televim.history.json`,
//! the file the binary loads when `--config <dir>/televim.toml` is given, and
//! prints one JSON object describing what it wrote.
//!
//! The app crate has no lib target, so this cannot reuse `history_store`; it
//! serialises the same payload shape `tests/common/mod.rs` does and seals it
//! with the framework's own `sealed::seal`. Sizes stay under the cache's bounds
//! (32 peers, 200 rows each, 8 MiB of text) so nothing is evicted on load.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;
use telegram_framework::{PassphraseProvider, sealed};

const CHATS: i64 = 60;
const PEERS: i64 = 32;
const ROWS: i64 = 200;

#[derive(Serialize)]
struct SeedFile {
    account: Option<String>,
    peers: BTreeMap<i64, Vec<SeedMessage>>,
    chats: Vec<SeedChat>,
}

#[derive(Serialize)]
struct SeedChat {
    id: i64,
    title: String,
}

#[derive(Serialize)]
struct SeedMessage {
    id: i64,
    text: String,
    timestamp: i64,
    is_outgoing: bool,
}

#[derive(Serialize)]
struct Summary {
    chats: i64,
    peers: i64,
    messages: usize,
    text_bytes: usize,
    file_bytes: usize,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("usage: seed_history <dir> <passphrase>"));
    let passphrase = args.next().expect("usage: seed_history <dir> <passphrase>");

    let chats = (1..=CHATS)
        .map(|id| SeedChat {
            id,
            title: format!("Seeded chat {id:02}"),
        })
        .collect();
    let mut text_bytes = 0_usize;
    let mut messages = 0_usize;
    let peers = (1..=PEERS)
        .map(|peer| {
            let rows = (1..=ROWS)
                .map(|id| {
                    let text = format!(
                        "peer {peer} message {id}: the quick brown fox jumps over the lazy dog, \
                         then writes a few more words so a row has a realistic length"
                    );
                    text_bytes += text.len();
                    messages += 1;
                    SeedMessage {
                        id,
                        text,
                        timestamp: 1_700_000_000 + id,
                        is_outgoing: id % 3 == 0,
                    }
                })
                .collect();
            (peer, rows)
        })
        .collect();

    let file = SeedFile {
        account: None,
        peers,
        chats,
    };
    let plain = serde_json::to_vec(&file).expect("serialise seeded history");
    let bytes =
        sealed::seal(&plain, &PassphraseProvider::new(&passphrase)).expect("seal seeded history");
    std::fs::write(dir.join("televim.history.json"), &bytes).expect("write seeded history");

    let summary = Summary {
        chats: CHATS,
        peers: PEERS,
        messages,
        text_bytes,
        file_bytes: bytes.len(),
    };
    println!(
        "{}",
        serde_json::to_string(&summary).expect("serialise summary")
    );
}
