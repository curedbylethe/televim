//! Shared harness for the PTY tests: a scratch sandbox and an offline spawn.
//!
//! "Offline" means no credentials reach the binary, so it takes the
//! deterministic no-credentials path and never touches the network. The
//! sandbox keeps the session file and the log out of the developer's own
//! directories; `current_dir` is the sandbox so the binary's `dotenvy` lookup
//! starts there and never finds the repository's `.env`.

use std::path::PathBuf;
use std::time::Duration;

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
