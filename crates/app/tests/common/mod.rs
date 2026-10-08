//! Shared harness for the PTY tests: a scratch sandbox and an offline spawn.
//!
//! "Offline" means no credentials reach the binary, so it takes the
//! deterministic no-credentials path and never touches the network. The
//! sandbox keeps the session file and the log out of the developer's own
//! directories; `current_dir` is the sandbox so the binary's `dotenvy` lookup
//! starts there and never finds the repository's `.env`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use tempfile::TempDir;
use termlens::Terminal;

/// A scratch directory holding an empty config and a session path.
///
/// Keep it alive for as long as the spawned process runs: the log is written
/// beside the config, inside this directory.
pub struct Sandbox {
    dir: TempDir,
    config: PathBuf,
    session: PathBuf,
}

pub fn sandbox() -> Sandbox {
    let dir = tempfile::tempdir().expect("create scratch sandbox");
    let config = dir.path().join("televim.toml");
    std::fs::write(&config, "").expect("write empty config");
    let session = dir.path().join("session.bin");
    Sandbox {
        dir,
        config,
        session,
    }
}

/// Spawn the `televim` binary with no credentials, 80x24, in `sandbox`.
pub fn spawn_offline(sandbox: &Sandbox) -> Terminal {
    let config = sandbox
        .config
        .to_str()
        .expect("sandbox config path is UTF-8");
    Terminal::builder()
        .size(80, 24)
        .env_clear()
        .timeout(Duration::from_secs(5))
        .current_dir(sandbox.dir.path())
        .env("TELEVIM_SESSION_PATH", &sandbox.session)
        .args(["--config", config])
        .spawn(env!("CARGO_BIN_EXE_televim"))
        .expect("spawn televim in a PTY")
}

/// Writes the warm-start cache beside the sandbox config, before launch.
///
/// The file is `history.json` with the config's extension replaced, the path
/// `runtime.rs` loads from. The sandbox sets no phone number, so the account
/// is `null`, the unnamed account `history_acceptable` accepts. `chats` is
/// `(id, title)`; `peers` is `(peer id, message texts)`, numbered from 1 in
/// the order given, each one sent by the peer and stamped with its number.
pub fn seed_history(sandbox: &Sandbox, chats: &[(i64, &str)], peers: &[(i64, Vec<&str>)]) {
    let file = SeedFile {
        account: None,
        peers: peers
            .iter()
            .map(|(peer, texts)| {
                let rows = texts
                    .iter()
                    .zip(1_i64..)
                    .map(|(text, id)| SeedMessage {
                        id,
                        text,
                        timestamp: id,
                        is_outgoing: false,
                    })
                    .collect();
                (*peer, rows)
            })
            .collect(),
        chats: chats
            .iter()
            .map(|(id, title)| SeedChat { id: *id, title })
            .collect(),
    };
    let bytes = serde_json::to_vec(&file).expect("serialise seeded history");
    let path = sandbox.config.with_extension("history.json");
    std::fs::write(&path, bytes).expect("write seeded history.json");
}

/// The shape `runtime.rs` reads back: `HistoryFile`'s payload, written by hand.
#[derive(Serialize)]
struct SeedFile<'a> {
    account: Option<&'a str>,
    peers: BTreeMap<i64, Vec<SeedMessage<'a>>>,
    chats: Vec<SeedChat<'a>>,
}

#[derive(Serialize)]
struct SeedChat<'a> {
    id: i64,
    title: &'a str,
}

#[derive(Serialize)]
struct SeedMessage<'a> {
    id: i64,
    text: &'a str,
    timestamp: i64,
    is_outgoing: bool,
}
