//! Downloaded media kept on disk, so a re-open does not download it again.
//!
//! One file per message, named `<chat>-<message>.<suffix>` — the ids are the
//! server's, so the name is the same after a restart, where the old temp-dir
//! name carried the process id. The directory is bounded by a byte total and an
//! entry count; past either, the oldest file by modification time goes first.
//!
//! **Plaintext, like the history and drafts files.** Files are restricted to
//! their owner and the directory is tagged with the account, but nothing is
//! encrypted. The directory is emptied when the account changes or signs out.
//!
//! Every operation is best-effort, the history file's discipline: a missing
//! directory is an empty cache, a failed write is warned about and skipped, and
//! nothing here fails the run. Diagnostics go to `tracing`, never the terminal.
//!
//! The atomic-write helper is a fourth copy of the drafts and history ones, on
//! purpose: those live in sibling modules of a binary crate, and sharing one
//! would widen a security-sensitive surface for thirty lines.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use anyhow::{Context as _, Result};
use domain::message::MediaKind;
use telegram_framework::MEDIA_LIMIT;

/// The most bytes the cache keeps on disk. Per entry, [`MEDIA_LIMIT`] is the
/// ceiling; this is the total, so a few hundred large files cannot grow
/// without bound. It is a disk figure and does not count against the memory
/// budget.
pub const MEDIA_CACHE_MAX_BYTES: u64 = 1024 * 1024 * 1024;

/// The most files the cache keeps, whatever their size.
pub const MEDIA_CACHE_MAX_ENTRIES: usize = 256;

/// The file beside the media that records which account wrote it.
const ACCOUNT_FILE: &str = "account";

/// Every extension a cached file can carry, one per [`suffix`]. A name with any
/// other extension is not ours and is left alone.
const SUFFIXES: [&str; 6] = ["jpg", "mp4", "gif", "ogg", "webp", "bin"];

/// The extension a kind of media is stored under.
pub(crate) fn suffix(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Photo => "jpg",
        MediaKind::Video => "mp4",
        MediaKind::Gif => "gif",
        MediaKind::Voice => "ogg",
        MediaKind::Sticker => "webp",
        MediaKind::File => "bin",
    }
}

/// The ids a cached file's name encodes, or `None` for anything that is not one
/// of this cache's files — temps, the account file, a stranger's file.
fn parse_name(name: &str) -> Option<(i64, i64)> {
    let (stem, extension) = name.rsplit_once('.')?;
    if !SUFFIXES.contains(&extension) {
        return None;
    }
    // Split on the last dash so a negative chat id keeps its sign.
    let (chat, message) = stem.rsplit_once('-')?;
    Some((chat.parse().ok()?, message.parse().ok()?))
}

struct Entry {
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
}

/// A store's claim on its file, taken under the lock before any I/O.
struct Reservation {
    dir: PathBuf,
    path: PathBuf,
    size: u64,
    generation: u64,
}

/// The media cache in one directory, with its index in memory.
pub(crate) struct MediaCache {
    dir: PathBuf,
    max_bytes: u64,
    max_entries: usize,
    entries: BTreeMap<(i64, i64), Entry>,
    /// Bumped by [`MediaCache::clear`], so a store that began before a clear
    /// knows not to index its file after it.
    generation: u64,
}

impl MediaCache {
    /// Opens the cache in `dir` for `account`, clearing it first if it was
    /// written for another account or for none. An unwritable `dir` opens a
    /// per-launch fallback in the temp directory instead.
    pub(crate) fn open(dir: PathBuf, account: Option<&str>) -> Self {
        Self::open_in(dir, &std::env::temp_dir(), account)
    }

    /// [`MediaCache::open`] with the fallback root passed in, so tests can
    /// point it at a temp dir of their own.
    fn open_in(dir: PathBuf, fallback_root: &Path, account: Option<&str>) -> Self {
        let dir = usable_dir(dir, fallback_root, account);
        Self::with_limits(dir, MEDIA_CACHE_MAX_BYTES, MEDIA_CACHE_MAX_ENTRIES, account)
    }

    fn with_limits(
        dir: PathBuf,
        max_bytes: u64,
        max_entries: usize,
        account: Option<&str>,
    ) -> Self {
        let mut cache = Self {
            dir,
            max_bytes,
            max_entries,
            entries: BTreeMap::new(),
            generation: 0,
        };
        cache.claim(account);
        cache.scan();
        cache
    }

    /// The file a message's media is cached in, if it is.
    pub(crate) fn lookup(&self, chat_id: i64, message_id: i64) -> Option<PathBuf> {
        self.entries
            .get(&(chat_id, message_id))
            .map(|entry| entry.path.clone())
    }

    /// Caches `bytes` for a message and returns the file they landed in.
    ///
    /// Takes the lock twice and writes between: the file I/O never runs under
    /// the lock, so a clear waits out a lock-held swap, not a 16 MiB write.
    pub(crate) fn store_shared(
        cache: &Mutex<Self>,
        chat_id: i64,
        message_id: i64,
        kind: MediaKind,
        bytes: &[u8],
    ) -> Option<PathBuf> {
        let reservation = cache
            .lock()
            .expect("the media cache lock is not poisoned")
            .reserve(chat_id, message_id, kind, bytes.len())?;
        write_reserved(&reservation, bytes)?;
        cache
            .lock()
            .expect("the media cache lock is not poisoned")
            .commit(reservation, chat_id, message_id)
    }

    /// The single-owner store, for tests: the same three steps as
    /// [`MediaCache::store_shared`] with nothing between them to interleave.
    #[cfg(test)]
    fn store(
        &mut self,
        chat_id: i64,
        message_id: i64,
        kind: MediaKind,
        bytes: &[u8],
    ) -> Option<PathBuf> {
        let reservation = self.reserve(chat_id, message_id, kind, bytes.len())?;
        write_reserved(&reservation, bytes)?;
        self.commit(reservation, chat_id, message_id)
    }

    /// Claims a file for a store of `size` bytes. Refuses what could never fit:
    /// bytes over [`MEDIA_LIMIT`], or over the byte cap. No I/O.
    fn reserve(
        &self,
        chat_id: i64,
        message_id: i64,
        kind: MediaKind,
        size: usize,
    ) -> Option<Reservation> {
        let size = u64::try_from(size).ok()?;
        if size > MEDIA_LIMIT as u64 || size > self.max_bytes {
            return None;
        }
        Some(Reservation {
            dir: self.dir.clone(),
            path: self
                .dir
                .join(format!("{chat_id}-{message_id}.{}", suffix(kind))),
            size,
            generation: self.generation,
        })
    }

    /// Indexes a written file, unless a clear ran since it was reserved. A
    /// failed write has no reservation to commit, so no index entry is made
    /// for a file that does not exist.
    fn commit(
        &mut self,
        reservation: Reservation,
        chat_id: i64,
        message_id: i64,
    ) -> Option<PathBuf> {
        let Reservation {
            path,
            size,
            generation,
            ..
        } = reservation;
        if generation != self.generation {
            // The clear already swept the cache; the late file must not survive
            // it. A later store may own this name by now, so leave indexed files.
            if !self.entries.values().any(|entry| entry.path == path) {
                let _ = fs::remove_file(&path);
            }
            return None;
        }

        let key = (chat_id, message_id);
        let entry = Entry {
            path: path.clone(),
            bytes: size,
            modified: SystemTime::now(),
        };
        if let Some(old) = self.entries.insert(key, entry) {
            // One file per message: a re-store under another kind replaces it.
            if old.path != path {
                let _ = fs::remove_file(&old.path);
            }
        }
        self.evict(Some(key));
        Some(path)
    }

    /// Empties the cache: its files and its account tag. A missing directory
    /// is already empty. Any store still in flight is refused when it commits.
    pub(crate) fn clear(&mut self) {
        self.generation += 1;
        self.entries.clear();
        let Ok(read) = fs::read_dir(&self.dir) else {
            return;
        };
        for entry in read.flatten() {
            let name = entry.file_name();
            let ours = name == ACCOUNT_FILE || name.to_str().and_then(parse_name).is_some();
            if ours && let Err(error) = fs::remove_file(entry.path()) {
                tracing::warn!(%error, "a cached media file could not be removed");
            }
        }
    }

    /// Clears the directory unless it was written for `account`, then tags it.
    ///
    /// The history file's rule: accept only an exact match, so unnamed matches
    /// unnamed and anything else — including a directory with files and no tag
    /// — starts empty.
    fn claim(&mut self, account: Option<&str>) {
        let stored = fs::read_to_string(self.dir.join(ACCOUNT_FILE)).ok();
        if stored.as_deref() != account {
            self.clear();
        }
        if let Some(account) = account {
            let tagged = fs::create_dir_all(&self.dir)
                .context("creating the media cache directory")
                .and_then(|()| write_atomically(&self.dir.join(ACCOUNT_FILE), account.as_bytes()));
            if let Err(error) = tagged {
                tracing::warn!(%error, "the media cache could not be tagged");
            }
        }
    }

    /// Rebuilds the index from the directory, so the size bound holds for what
    /// was there before this launch.
    fn scan(&mut self) {
        let Ok(read) = fs::read_dir(&self.dir) else {
            return;
        };
        for entry in read.flatten() {
            let name = entry.file_name();
            let Some(key) = name.to_str().and_then(parse_name) else {
                continue;
            };
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let found = Entry {
                path: entry.path(),
                bytes: meta.len(),
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            };
            if let Some(old) = self.entries.insert(key, found) {
                // Two files for one message: keep the one indexed last and
                // remove the other, so no file exists outside the bound.
                let _ = fs::remove_file(&old.path);
            }
        }
        self.evict(None);
    }

    fn total(&self) -> u64 {
        self.entries.values().map(|entry| entry.bytes).sum()
    }

    /// Removes the oldest files until both caps hold, never the `keep` entry.
    ///
    /// A file that will not remove is dropped from the index anyway: the cap
    /// is then a count of what this cache still tracks, and the warning says
    /// which file survived.
    fn evict(&mut self, keep: Option<(i64, i64)>) {
        while self.entries.len() > self.max_entries || self.total() > self.max_bytes {
            let oldest = self
                .entries
                .iter()
                .filter(|(key, _)| Some(**key) != keep)
                .min_by_key(|(_, entry)| entry.modified)
                .map(|(key, _)| *key);
            let Some(key) = oldest else {
                break;
            };
            if let Some(entry) = self.entries.remove(&key)
                && let Err(error) = fs::remove_file(&entry.path)
            {
                tracing::warn!(%error, "an evicted media file could not be removed");
            }
        }
    }
}

/// `dir` if it can be written, else the temp-directory fallback for `account`.
///
/// The probe is a create and a write-and-remove of a pid-named file, which
/// catches what `claim`'s tag write would: a regular file where the directory
/// should be, or a read-only directory. The probe itself is not tagged; `claim`
/// tags whichever directory is chosen. The fallback is not probed: if it cannot
/// be written, the store warns and returns `None` as an unwritable `dir` does.
fn usable_dir(dir: PathBuf, fallback_root: &Path, account: Option<&str>) -> PathBuf {
    let probe = dir.join(format!(".probe-{}", std::process::id()));
    let writable = fs::create_dir_all(&dir).is_ok() && fs::write(&probe, b"").is_ok();
    let _ = fs::remove_file(&probe);
    if writable {
        return dir;
    }
    let fallback = fallback_root.join(format!("televim.media-{}", account_tag(account)));
    tracing::warn!(
        configured = %dir.display(),
        fallback = %fallback.display(),
        "the media cache directory is not writable; using a temporary one for this run"
    );
    fallback
}

/// The account's name for the fallback directory: its phone with every
/// character outside `[A-Za-z0-9_-]` stripped, or `none` for no account.
fn account_tag(account: Option<&str>) -> String {
    match account {
        Some(phone) => phone
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .collect(),
        None => "none".to_owned(),
    }
}

/// Writes a reserved file. A failed write is warned about and returns `None`.
fn write_reserved(reservation: &Reservation, bytes: &[u8]) -> Option<()> {
    let written = fs::create_dir_all(&reservation.dir)
        .context("creating the media cache directory")
        .and_then(|()| write_atomically(&reservation.path, bytes));
    if let Err(error) = written {
        tracing::warn!(%error, "media could not be cached");
        return None;
    }
    Some(())
}

/// Writes `bytes` so the target only ever appears as a whole file.
///
/// A sibling temp, restricted before a byte is in it, renamed over the target:
/// an interrupted write leaves the previous file, never a truncated one.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = temp_sibling(path);
    let result = (|| -> Result<()> {
        fs::File::create(&temp).context("creating the media temp file")?;
        restrict_permissions(&temp).context("restricting the media temp file")?;
        fs::write(&temp, bytes).context("writing the media temp file")?;
        fs::rename(&temp, path).context("replacing the media file")?;
        Ok(())
    })();
    if result.is_err() {
        // A half-written temp is not a cached file, and nothing else cleans it up.
        let _ = fs::remove_file(&temp);
    }
    result
}

/// The temp file [`write_atomically`] writes through, tagged with the process
/// id so two processes sharing a directory do not write the same temp.
fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache_in(dir: &Path, account: Option<&str>) -> MediaCache {
        MediaCache::open(dir.to_path_buf(), account)
    }

    #[test]
    fn a_stored_file_is_found_again_after_a_restart_under_a_pid_free_name() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), Some("+15550001"));
        let path = cache
            .store(7, 9, MediaKind::Photo, b"picture")
            .expect("a small file is cached");
        drop(cache);

        let reopened = cache_in(dir.path(), Some("+15550001"));
        assert_eq!(reopened.lookup(7, 9), Some(path.clone()));
        assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("7-9.jpg"));
        assert_eq!(fs::read(&path).expect("the file is there"), b"picture");
    }

    #[test]
    fn the_oldest_file_by_modification_goes_first_when_the_byte_cap_is_passed() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = MediaCache::with_limits(dir.path().to_path_buf(), 10, 256, None);
        cache.store(1, 1, MediaKind::File, b"aaaa");
        cache.store(1, 2, MediaKind::File, b"bbbb");
        cache
            .entries
            .get_mut(&(1, 1))
            .expect("the first file is indexed")
            .modified = SystemTime::UNIX_EPOCH;

        cache.store(1, 3, MediaKind::File, b"cccc");

        assert_eq!(cache.lookup(1, 1), None, "the oldest is evicted");
        assert!(cache.lookup(1, 2).is_some());
        assert!(cache.lookup(1, 3).is_some());
        assert!(cache.total() <= 10);
        assert!(
            !dir.path().join("1-1.bin").exists(),
            "and removed from disk"
        );
    }

    #[test]
    fn the_entry_cap_holds_whatever_the_size() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = MediaCache::with_limits(dir.path().to_path_buf(), 1024, 2, None);
        for message in 1..=3 {
            cache.store(1, message, MediaKind::Voice, b"x");
        }
        assert_eq!(cache.entries.len(), 2);
        assert_eq!(cache.lookup(1, 1), None);
    }

    #[test]
    fn a_file_that_could_never_fit_is_refused_and_nothing_is_evicted_for_it() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = MediaCache::with_limits(dir.path().to_path_buf(), 4, 8, None);
        cache.store(1, 1, MediaKind::File, b"ok");

        assert_eq!(cache.store(1, 2, MediaKind::File, b"too big"), None);
        assert!(cache.lookup(1, 1).is_some(), "the resident file is kept");
    }

    #[test]
    fn the_production_caps_are_a_gibibyte_and_256_files() {
        assert_eq!(MEDIA_CACHE_MAX_BYTES, 1024 * 1024 * 1024);
        assert_eq!(MEDIA_CACHE_MAX_ENTRIES, 256);
        assert!(MEDIA_LIMIT as u64 <= MEDIA_CACHE_MAX_BYTES);
    }

    #[test]
    fn strangers_temps_and_malformed_names_are_ignored_and_left_alone() {
        let dir = tempfile::tempdir().expect("a temp dir");
        for name in ["notes.txt", "x-y.jpg", "1-2.jpg.99.tmp", "1-2.exe"] {
            fs::write(dir.path().join(name), b"not ours").expect("a file can be written");
        }

        let cache = cache_in(dir.path(), None);

        assert!(cache.entries.is_empty());
        assert!(dir.path().join("notes.txt").exists());
        assert!(dir.path().join("1-2.exe").exists());
    }

    #[test]
    fn a_different_account_clears_the_directory_first() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), Some("+15550001"));
        let path = cache
            .store(7, 9, MediaKind::Photo, b"theirs")
            .expect("cached");

        let other = cache_in(dir.path(), Some("+15550002"));

        assert_eq!(other.lookup(7, 9), None);
        assert!(!path.exists(), "the other account's media is removed");
    }

    #[test]
    fn clearing_a_missing_directory_succeeds_and_clearing_removes_our_files() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut missing = cache_in(&dir.path().join("never-made"), None);
        missing.clear();

        let mut cache = cache_in(dir.path(), Some("+15550001"));
        let path = cache.store(1, 2, MediaKind::Gif, b"gif").expect("cached");
        cache.clear();

        assert_eq!(cache.lookup(1, 2), None);
        assert!(!path.exists());
        assert!(!dir.path().join(ACCOUNT_FILE).exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_cached_file_is_owner_only_before_it_is_renamed_into_place() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        let path = cache
            .store(1, 2, MediaKind::Voice, b"voice")
            .expect("cached");

        let mode = fs::metadata(&path)
            .expect("the file is there")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_clear_between_a_store_write_and_its_commit_refuses_the_late_file() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), Some("+15550001"));
        let reservation = cache
            .reserve(7, 9, MediaKind::Photo, 6)
            .expect("the file is reserved");
        let path = reservation.path.clone();
        write_reserved(&reservation, b"late!!").expect("the file is written");

        cache.clear();

        assert_eq!(cache.commit(reservation, 7, 9), None, "refused");
        assert_eq!(cache.lookup(7, 9), None);
        assert!(!path.exists(), "and the late file is removed");
    }

    #[test]
    fn a_refused_store_leaves_a_file_a_later_store_committed_under_the_same_name() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        let stale = cache.reserve(7, 9, MediaKind::Photo, 3).expect("reserved");
        write_reserved(&stale, b"old").expect("written");
        cache.clear();

        let fresh = cache.store(7, 9, MediaKind::Photo, b"new").expect("cached");
        assert_eq!(
            cache.commit(stale, 7, 9),
            None,
            "the stale store is refused"
        );

        assert_eq!(cache.lookup(7, 9), Some(fresh.clone()));
        assert_eq!(fs::read(&fresh).expect("the fresh file is there"), b"new");
    }

    #[test]
    fn a_store_after_a_clear_is_indexed_as_usual() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        cache.clear();

        let path = cache.store(1, 2, MediaKind::Gif, b"gif").expect("cached");

        assert_eq!(cache.lookup(1, 2), Some(path.clone()));
        assert!(path.exists());
    }

    /// A configured directory that cannot exist: its parent is a regular file.
    fn unwritable_dir(root: &Path) -> PathBuf {
        let blocker = root.join("blocker");
        fs::write(&blocker, b"a file").expect("a file can be written");
        blocker.join("media")
    }

    #[test]
    fn an_unwritable_configured_dir_opens_a_working_fallback_in_the_temp_root() {
        let root = tempfile::tempdir().expect("a temp dir");
        let fallback_root = root.path().join("tmp");
        fs::create_dir(&fallback_root).expect("the fallback root is made");

        let mut cache = MediaCache::open_in(
            unwritable_dir(root.path()),
            &fallback_root,
            Some("+15550001"),
        );
        let path = cache
            .store(7, 9, MediaKind::Photo, b"picture")
            .expect("the fallback caches it");

        assert!(path.starts_with(fallback_root.join("televim.media-15550001")));
        assert_eq!(cache.lookup(7, 9), Some(path.clone()));
        assert_eq!(fs::read(&path).expect("the file is there"), b"picture");
        assert_eq!(cache.max_bytes, MEDIA_CACHE_MAX_BYTES);
        assert_eq!(cache.max_entries, MEDIA_CACHE_MAX_ENTRIES);
    }

    #[test]
    fn nothing_is_created_under_the_unwritable_configured_path() {
        let root = tempfile::tempdir().expect("a temp dir");
        let fallback_root = root.path().join("tmp");
        fs::create_dir(&fallback_root).expect("the fallback root is made");
        let configured = unwritable_dir(root.path());

        let cache = MediaCache::open_in(configured.clone(), &fallback_root, None);

        assert!(cache.entries.is_empty());
        assert!(!configured.exists(), "the configured path stays absent");
        assert_eq!(
            fs::read(root.path().join("blocker")).expect("the blocker is there"),
            b"a file",
            "and the file in its way is untouched"
        );
    }

    #[test]
    fn the_fallback_keeps_the_byte_cap_eviction_and_owner_only_files() {
        let root = tempfile::tempdir().expect("a temp dir");
        let fallback_root = root.path().join("tmp");
        fs::create_dir(&fallback_root).expect("the fallback root is made");
        let dir = usable_dir(unwritable_dir(root.path()), &fallback_root, None);

        let mut cache = MediaCache::with_limits(dir.clone(), 10, 256, None);
        cache.store(1, 1, MediaKind::File, b"aaaa");
        cache.store(1, 2, MediaKind::File, b"bbbb");
        cache
            .entries
            .get_mut(&(1, 1))
            .expect("the first file is indexed")
            .modified = SystemTime::UNIX_EPOCH;
        cache.store(1, 3, MediaKind::File, b"cccc");

        assert_eq!(cache.lookup(1, 1), None, "the oldest is evicted");
        assert!(cache.total() <= 10);
        assert!(!dir.join("1-1.bin").exists(), "and removed from disk");
    }

    #[cfg(unix)]
    #[test]
    fn a_fallback_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().expect("a temp dir");
        let fallback_root = root.path().join("tmp");
        fs::create_dir(&fallback_root).expect("the fallback root is made");
        let mut cache = MediaCache::open_in(unwritable_dir(root.path()), &fallback_root, None);
        let path = cache
            .store(1, 2, MediaKind::Voice, b"voice")
            .expect("cached");

        let mode = fs::metadata(&path)
            .expect("the file is there")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn an_unwritable_fallback_too_degrades_to_no_cache_without_panicking() {
        let root = tempfile::tempdir().expect("a temp dir");
        let configured = unwritable_dir(root.path());
        // The fallback root is under the same regular file, so it is unwritable too.
        let fallback_root = root.path().join("blocker").join("tmp");

        let mut cache = MediaCache::open_in(configured, &fallback_root, None);

        assert_eq!(cache.store(1, 2, MediaKind::Gif, b"gif"), None);
        assert_eq!(cache.lookup(1, 2), None);
    }
}
