//! Account-scoped paths, and the store the session goes in.
//!
//! One resolver, so the files a run writes, the keyring entry it uses and the
//! description the profile panel shows cannot disagree about which account they
//! belong to.
//!
//! This is groundwork and changes no behaviour: every caller passes `None`, the
//! legacy slot, which reproduces the layout runs have today byte for byte.
//! `Some(id)` is an account added later.
//!
//! `None` is a plain `Option` here rather than an `AccountSlot` enum: the typed
//! enum arrives in stage 01, when both variants are constructed. An
//! unconstructed variant is a `dead_code` failure under `-D warnings`, which
//! this binary-only crate has nowhere to keep quiet.

use std::path::{Path, PathBuf};

use crate::config::Config;

/// Every path one account owns.
pub(crate) struct SlotPaths {
    pub(crate) drafts: PathBuf,
    pub(crate) history: PathBuf,
    pub(crate) media: PathBuf,
    pub(crate) session: SessionTarget,

    /// What the profile panel says the session is. Derived here, beside the
    /// target, so the panel and the store cannot name different things.
    pub(crate) description: tui::SessionStore,
}

/// Where the session goes.
pub(crate) enum SessionTarget {
    /// The OS credential store, under this account name.
    Keyring(String),

    /// A file on disk, encrypted at rest.
    File(PathBuf),
}

/// The paths for `account`, derived from the configuration and its file.
///
/// `config_path` is the configuration file's own path: the drafts, history and
/// media files sit beside it. `None` is the legacy slot and is unchanged from
/// the code this replaced.
pub(crate) fn slot_paths(config_path: &Path, cfg: &Config, account: Option<i64>) -> SlotPaths {
    let (drafts, history, media) = match account {
        None => (
            config_path.with_extension("drafts.json"),
            config_path.with_extension("history.json"),
            cfg.media_cache_dir
                .clone()
                .unwrap_or_else(|| config_path.with_extension("media")),
        ),
        Some(id) => (
            config_path.with_extension(format!("{id}.drafts.json")),
            config_path.with_extension(format!("{id}.history.json")),
            cfg.media_cache_dir.clone().map_or_else(
                || config_path.with_extension(format!("{id}.media")),
                |dir| suffixed(&dir, id),
            ),
        ),
    };

    let session = match (&cfg.session_path, account) {
        (None, _) => SessionTarget::Keyring(
            account.map_or_else(|| "session".to_owned(), |id| format!("session-{id}")),
        ),
        (Some(path), None) => SessionTarget::File(path.clone()),
        (Some(path), Some(id)) => SessionTarget::File(suffixed_session(path, id)),
    };

    let description = match &session {
        SessionTarget::Keyring(_) => tui::SessionStore::Keyring,
        SessionTarget::File(path) => tui::SessionStore::EncryptedFile(path.clone()),
    };

    SlotPaths {
        drafts,
        history,
        media,
        session,
        description,
    }
}

/// `path` with `.<id>` appended to its name: a directory or file keeps the
/// spelling it was configured with, and only gains the suffix.
fn suffixed(path: &Path, id: i64) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{id}"));
    PathBuf::from(name)
}

/// A session file with `.<id>` before its extension, so the file keeps its
/// readable name and its extension.
fn suffixed_session(path: &Path, id: i64) -> PathBuf {
    match path.extension() {
        Some(extension) => path.with_extension(format!("{id}.{}", extension.to_string_lossy())),
        None => path.with_extension(id.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_legacy_slot_reproduces_todays_paths() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let config_path = dir.path().join("televim.toml");
        let paths = slot_paths(&config_path, &Config::default(), None);

        assert_eq!(paths.drafts, dir.path().join("televim.drafts.json"));
        assert_eq!(paths.history, dir.path().join("televim.history.json"));
        assert_eq!(paths.media, dir.path().join("televim.media"));
        assert_eq!(paths.description, tui::SessionStore::Keyring);
        let SessionTarget::Keyring(account) = &paths.session else {
            panic!("the legacy slot keeps the session in the keyring");
        };
        assert_eq!(account, "session");
    }

    #[test]
    fn the_legacy_slot_takes_the_overrides_verbatim() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let config_path = dir.path().join("televim.toml");
        let media = dir.path().join("cache");
        let session = dir.path().join("session.bin");
        let cfg = Config {
            media_cache_dir: Some(media.clone()),
            session_path: Some(session.clone()),
            ..Config::default()
        };

        let paths = slot_paths(&config_path, &cfg, None);

        assert_eq!(paths.media, media);
        assert_eq!(
            paths.description,
            tui::SessionStore::EncryptedFile(session.clone())
        );
        let SessionTarget::File(path) = &paths.session else {
            panic!("a configured file is the session's");
        };
        assert_eq!(path, &session, "the configured path is used as it is");
    }

    /// A configured media directory is not the legacy slot's to rename: the
    /// resolver leaves it exactly where the reader put it.
    #[test]
    fn the_legacy_slot_leaves_a_configured_media_directory_alone() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let config_path = dir.path().join("televim.toml");
        let media = dir.path().join("cache");
        let cfg = Config {
            media_cache_dir: Some(media.clone()),
            ..Config::default()
        };

        assert_eq!(slot_paths(&config_path, &cfg, None).media, media);
    }

    #[test]
    fn an_added_account_suffixes_every_path() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let config_path = dir.path().join("televim.toml");
        let media = dir.path().join("cache");
        let session = dir.path().join("televim.session");
        let cfg = Config {
            media_cache_dir: Some(media),
            session_path: Some(session),
            ..Config::default()
        };

        let paths = slot_paths(&config_path, &cfg, Some(7));

        assert_eq!(paths.drafts, dir.path().join("televim.7.drafts.json"));
        assert_eq!(paths.history, dir.path().join("televim.7.history.json"));
        assert_eq!(paths.media, dir.path().join("cache.7"));
        let SessionTarget::File(path) = &paths.session else {
            panic!("a configured file is the session's");
        };
        assert_eq!(path, &dir.path().join("televim.7.session"));

        for path in [&paths.drafts, &paths.history, &paths.media, path] {
            assert!(
                path.starts_with(dir.path()),
                "{} stays under the configuration's directory",
                path.display()
            );
        }

        let keyring = slot_paths(&config_path, &Config::default(), Some(7));
        let SessionTarget::Keyring(account) = &keyring.session else {
            panic!("no configured file means the keyring");
        };
        assert_eq!(account, "session-7");
    }
}
