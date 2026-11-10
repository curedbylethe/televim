use std::{
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use telegram_framework::{FileStore, KeyringStore, SessionError, SessionStore};

use crate::accounts::{self, AccountSlot, SessionTarget};
use crate::config::Config;

pub fn migrate_derived_key(config_path: &Path, cfg: &Config) {
    let (Some(key_path), Some(session_path)) = (&cfg.session_key_file, &cfg.session_path) else {
        return;
    };
    if !cfg.session_key_file_derived
        || cfg.passphrase().is_some()
        || key_path.exists()
        || !session_path.exists()
    {
        return;
    }
    let result = telegram_framework::sealed::KeyringKeyProvider::default()
        .existing_key()
        .and_then(|key| match key {
            Some(key) => telegram_framework::sealed::KeyFileProvider::new(key_path).store(&key),
            None => Ok(()),
        });
    match result {
        Ok(()) if key_path.exists() => {
            tracing::info!("copied legacy session key to {}", key_path.display());
        }
        Ok(()) => {}
        Err(error) => tracing::warn!(
            "could not migrate session key for {}: {error}",
            config_path.display()
        ),
    }
}

pub(crate) fn migrate_legacy_session(config_path: &Path, cfg: &Config, slot: AccountSlot) {
    let paths = accounts::slot_paths(config_path, cfg, slot.account());
    let SessionTarget::File(path) = paths.session else {
        return;
    };
    let legacy = KeyringStore::new("televim", accounts::session_name(slot));
    let primary = FileStore::with_key_provider(path.clone(), crate::net::file_key_provider(cfg));
    if let Err(error) = migrate_session(&primary, &legacy, path.exists()) {
        tracing::warn!(
            "could not migrate legacy session to {}: {error}",
            path.display()
        );
    }
}

fn migrate_session(
    primary: &dyn SessionStore,
    legacy: &dyn SessionStore,
    file_exists: bool,
) -> Result<(), SessionError> {
    if file_exists {
        return Ok(());
    }
    let Some(session) = legacy.load()? else {
        return Ok(());
    };
    primary.save(&session)?;
    if let Err(error) = legacy.clear() {
        tracing::warn!("could not clear migrated keychain session: {error}");
    }
    Ok(())
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use telegram_framework::{MemoryStore, SessionData};

    struct Legacy {
        inner: MemoryStore,
        loads: AtomicUsize,
        fail_load: bool,
        fail_clear: bool,
    }

    impl SessionStore for Legacy {
        fn load(&self) -> Result<Option<SessionData>, SessionError> {
            self.loads.fetch_add(1, Ordering::Relaxed);
            if self.fail_load {
                return Err(SessionError::Corrupt("bad snapshot".into()));
            }
            self.inner.load()
        }

        fn save(&self, session: &SessionData) -> Result<(), SessionError> {
            self.inner.save(session)
        }

        fn clear(&self) -> Result<(), SessionError> {
            if self.fail_clear {
                return Err(SessionError::Clear("denied".into()));
            }
            self.inner.clear()
        }
    }

    fn legacy(session: Option<SessionData>) -> Legacy {
        let inner = MemoryStore::new();
        if let Some(session) = session {
            inner.save(&session).expect("seed memory store");
        }
        Legacy {
            inner,
            loads: AtomicUsize::new(0),
            fail_load: false,
            fail_clear: false,
        }
    }

    #[test]
    fn migrates_then_clears_legacy_session() {
        let old = legacy(Some(SessionData::default()));
        let new = MemoryStore::new();
        migrate_session(&new, &old, false).expect("migration succeeds");
        assert_eq!(new.load().expect("new loads"), Some(SessionData::default()));
        assert_eq!(old.inner.load().expect("old loads"), None);
    }

    #[test]
    fn existing_file_skips_keychain_read() {
        let old = legacy(Some(SessionData::default()));
        migrate_session(&MemoryStore::new(), &old, true).expect("skip succeeds");
        assert_eq!(old.loads.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn empty_or_corrupt_legacy_does_not_create_primary() {
        let empty = legacy(None);
        let new = MemoryStore::new();
        migrate_session(&new, &empty, false).expect("empty is ignored");
        assert_eq!(new.load().expect("new loads"), None);

        let mut corrupt = legacy(None);
        corrupt.fail_load = true;
        assert!(migrate_session(&new, &corrupt, false).is_err());
        assert_eq!(new.load().expect("new loads"), None);
    }

    #[test]
    fn failed_clear_keeps_migrated_primary() {
        let mut old = legacy(Some(SessionData::default()));
        old.fail_clear = true;
        let new = MemoryStore::new();
        migrate_session(&new, &old, false).expect("clear failure is best effort");
        assert_eq!(new.load().expect("new loads"), Some(SessionData::default()));
        assert!(old.inner.load().expect("old loads").is_some());
    }
}

pub fn resolve_config(explicit: Option<&Path>) -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    if let Some(path) = explicit {
        return Ok(if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        });
    }
    let data_root = dirs::data_local_dir().context("finding the local data directory")?;
    resolve_with(None, &cwd, &data_root)
}

pub fn resolve_with(explicit: Option<&Path>, cwd: &Path, data_root: &Path) -> Result<PathBuf> {
    ensure!(cwd.is_absolute(), "current directory must be absolute");
    if let Some(path) = explicit {
        return Ok(if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        });
    }

    ensure!(
        data_root.is_absolute(),
        "local data directory must be absolute"
    );
    let profile = data_root.join("televim");
    let config = profile.join("televim.toml");
    let pointer = profile.join("profile");

    match std::fs::read_to_string(&pointer) {
        Ok(value) if !value.trim().is_empty() => {
            let path = PathBuf::from(value.trim());
            if path.is_absolute() {
                return Ok(path);
            }
            eprintln!(
                "warning: ignoring relative profile pointer at {}",
                pointer.display()
            );
        }
        Ok(_) => eprintln!(
            "warning: ignoring empty profile pointer at {}",
            pointer.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => eprintln!(
            "warning: cannot read profile pointer at {}: {error}",
            pointer.display()
        ),
    }

    if ["televim.toml", "session", "televim.accounts.json"]
        .iter()
        .any(|name| profile.join(name).exists())
    {
        return Ok(config);
    }

    if [
        "televim.toml",
        "session",
        "session.key",
        "televim.accounts.json",
    ]
    .iter()
    .any(|name| cwd.join(name).exists())
    {
        if let Some(value) = cwd.join("televim.toml").to_str() {
            if let Err(error) = write_pointer(&pointer, value) {
                eprintln!(
                    "warning: cannot save adopted profile pointer at {}: {error}",
                    pointer.display()
                );
            }
            return Ok(cwd.join("televim.toml"));
        }
        eprintln!("warning: cannot adopt profile path that is not valid UTF-8");
    }

    Ok(config)
}

fn write_pointer(pointer: &Path, value: &str) -> Result<()> {
    let parent = pointer.parent().context("profile pointer has no parent")?;
    std::fs::create_dir_all(parent).context("creating profile directory")?;
    let temporary = parent.join(format!("profile.{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .context("creating temporary profile pointer")?;
    file.write_all(value.as_bytes())
        .context("writing profile pointer")?;
    drop(file);
    std::fs::rename(&temporary, pointer).context("installing profile pointer")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_path_is_absolute_and_does_not_touch_pointer() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let cwd = temp.path().join("cwd");
        let data = temp.path().join("data");
        std::fs::create_dir_all(&cwd).expect("cwd exists");
        let path = resolve_with(Some(Path::new("custom.toml")), &cwd, &data)
            .expect("explicit path resolves");
        assert_eq!(path, cwd.join("custom.toml"));
        assert!(!data.join("televim/profile").exists());
    }

    #[test]
    fn pointer_wins_across_working_directories() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let data = temp.path().join("data");
        let chosen = temp.path().join("chosen/televim.toml");
        let pointer = data.join("televim/profile");
        std::fs::create_dir_all(pointer.parent().expect("parent")).expect("profile directory");
        std::fs::write(&pointer, chosen.to_str().expect("chosen path is UTF-8"))
            .expect("pointer written");
        let first = resolve_with(None, Path::new("/first"), &data).expect("first resolves");
        let second = resolve_with(None, Path::new("/second"), &data).expect("second resolves");
        assert_eq!(first, chosen);
        assert_eq!(second, chosen);
    }

    #[test]
    fn existing_default_profile_wins_over_legacy_cwd() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let cwd = temp.path().join("cwd");
        let data = temp.path().join("data");
        std::fs::create_dir_all(&cwd).expect("cwd exists");
        std::fs::create_dir_all(data.join("televim")).expect("profile directory");
        std::fs::write(data.join("televim/session"), b"session").expect("session exists");
        std::fs::write(cwd.join("session.key"), b"key").expect("legacy key exists");
        assert_eq!(
            resolve_with(None, &cwd, &data).expect("resolves"),
            data.join("televim/televim.toml")
        );
    }

    #[test]
    fn legacy_cwd_is_adopted_without_copying_or_removing_files() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let cwd = temp.path().join("cwd");
        let data = temp.path().join("data");
        std::fs::create_dir_all(&cwd).expect("cwd exists");
        std::fs::write(cwd.join("session"), b"session").expect("legacy session");
        let resolved = resolve_with(None, &cwd, &data).expect("legacy profile resolves");
        assert_eq!(resolved, cwd.join("televim.toml"));
        assert_eq!(
            std::fs::read(cwd.join("session")).expect("session remains"),
            b"session"
        );
        assert!(!data.join("televim/session").exists());
        assert_eq!(
            std::fs::read_to_string(data.join("televim/profile")).expect("pointer saved"),
            resolved.to_str().expect("utf8 path")
        );
    }

    #[test]
    fn empty_directory_uses_default_and_corrupt_pointer_is_ignored() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let cwd = temp.path().join("cwd");
        let data = temp.path().join("data");
        std::fs::create_dir_all(&cwd).expect("cwd exists");
        let pointer = data.join("televim/profile");
        std::fs::create_dir_all(pointer.parent().expect("parent")).expect("profile directory");
        std::fs::write(&pointer, "relative/path").expect("bad pointer");
        assert_eq!(
            resolve_with(None, &cwd, &data).expect("resolves"),
            data.join("televim/televim.toml")
        );
    }
}
