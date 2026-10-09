//! Downloaded media kept on disk, so a re-open does not download it again.
//!
//! Each distinct file is stored once, as a blob named `<sha256>.<suffix>` for
//! the hash of its bytes. Each key has a pointer file holding the hash of its
//! blob. A key is either the `(chat, message)` a file was downloaded for,
//! `<chat>-<message>.ref`, or the Telegram media id it carries, `m<id>.ref`, which
//! is the same for a file forwarded into any chat. The pointers are the index: a
//! restart rebuilds it from their names and contents. The directory is bounded by
//! the bytes of its distinct blobs and by its pointer count; past either, the
//! least recently used pointer goes first, and its blob with it once no other
//! pointer names it.
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
use sha2::{Digest, Sha256};
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

/// The hash a blob's name carries, or `None` for anything that is not one of
/// this cache's blobs.
fn parse_blob(name: &str) -> Option<String> {
    let (stem, extension) = name.rsplit_once('.')?;
    let hash_shaped = stem.len() == 64
        && stem
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    (SUFFIXES.contains(&extension) && hash_shaped).then(|| stem.to_owned())
}

/// What a cached file is looked up by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Key {
    /// The message a file was downloaded for: `<chat>-<message>.ref`.
    Message(i64, i64),
    /// Telegram's stable media id, which a forwarded copy shares: `m<id>.ref`.
    Media(i64),
}

impl Key {
    /// The pointer file's name. The `m` keeps a media id apart from a chat id,
    /// which may be negative.
    fn pointer_name(self) -> String {
        match self {
            Self::Message(chat_id, message_id) => format!("{chat_id}-{message_id}.ref"),
            Self::Media(media_id) => format!("m{media_id}.ref"),
        }
    }
}

/// The key a pointer's name carries, or `None` for anything else.
fn parse_pointer(name: &str) -> Option<Key> {
    let stem = name.strip_suffix(".ref")?;
    if let Some(media_id) = stem.strip_prefix('m') {
        return media_id.parse().ok().map(Key::Media);
    }
    let (chat_id, message_id) = parse_ids(stem)?;
    Some(Key::Message(chat_id, message_id))
}

/// The ids a pre-content-addressing file's name encodes, or `None` for anything
/// that is not one. Those names are `<chat>-<message>.<suffix>`.
fn parse_legacy(name: &str) -> Option<(i64, i64)> {
    let (stem, extension) = name.rsplit_once('.')?;
    if !SUFFIXES.contains(&extension) {
        return None;
    }
    parse_ids(stem)
}

/// `<chat>-<message>`, split on the last dash so a negative chat id keeps its
/// sign.
fn parse_ids(stem: &str) -> Option<(i64, i64)> {
    let (chat, message) = stem.rsplit_once('-')?;
    Some((chat.parse().ok()?, message.parse().ok()?))
}

/// Whether a name is one this cache wrote, in any of its three forms.
fn is_cache_file(name: &str) -> bool {
    parse_blob(name).is_some() || parse_pointer(name).is_some() || parse_legacy(name).is_some()
}

fn blob_name(hash: &str, kind: MediaKind) -> String {
    format!("{hash}.{}", suffix(kind))
}

/// The sha256 of `bytes`, as lowercase hex: the name a blob is stored under.
fn content_hash(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut hex = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hex
}

/// One key: the blob its pointer names, and when it was last used.
struct Entry {
    hash: String,
    modified: SystemTime,
}

/// One blob on disk, shared by every key whose pointer names its hash.
struct Blob {
    path: PathBuf,
    bytes: u64,
    /// The pointers that name this blob, plus the stores holding it while they
    /// write. The blob goes when this reaches zero.
    refs: usize,
}

/// A store's claim on its blob, taken under the lock before any I/O.
struct Reservation {
    dir: PathBuf,
    hash: String,
    path: PathBuf,
    size: u64,
    generation: u64,
    /// The references this store holds: one per pointer it will write. Taken now
    /// so an eviction cannot unlink a blob that a store is about to name.
    refs: usize,
    /// Whether this store writes the blob. `false` means the blob was already
    /// here and this store holds its references to it instead.
    write: bool,
}

/// The media cache in one directory, with its index in memory.
pub(crate) struct MediaCache {
    dir: PathBuf,
    max_bytes: u64,
    max_entries: usize,
    entries: BTreeMap<Key, Entry>,
    blobs: BTreeMap<String, Blob>,
    /// Bumped by [`MediaCache::clear`], so a store that began before a clear
    /// knows not to index its file after it.
    generation: u64,
    /// Blob paths reserved for writing since the last clear, with how many
    /// reservations hold each. A stale store must not remove a file a current
    /// reservation owns.
    pending: BTreeMap<PathBuf, usize>,
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
            blobs: BTreeMap::new(),
            generation: 0,
            pending: BTreeMap::new(),
        };
        cache.claim(account);
        cache.scan();
        cache
    }

    /// The file a message's media is cached in, if it is.
    pub(crate) fn lookup(&mut self, chat_id: i64, message_id: i64) -> Option<PathBuf> {
        self.hit(Key::Message(chat_id, message_id))
    }

    /// The file a Telegram media id is cached in, if it is: whichever message it
    /// was downloaded for, so a copy forwarded from another chat is found too.
    pub(crate) fn lookup_media(&mut self, media_id: i64) -> Option<PathBuf> {
        self.hit(Key::Media(media_id))
    }

    /// A lookup is a use: the key's recency moves to now, and so does its
    /// pointer's mtime, which is what carries the order across a restart.
    fn hit(&mut self, key: Key) -> Option<PathBuf> {
        let pointer = self.pointer_path(key);
        let entry = self.entries.get_mut(&key)?;
        let now = SystemTime::now();
        entry.modified = now;
        let blob = self.blobs.get(&entry.hash)?;
        // ponytail: the pointer's mtime is set under the cache lock, one syscall
        // per hit. Ceiling: a slow filesystem stalls the lock, so a concurrent
        // store waits, not the UI loop. Upgrade path: move this touch off-lock
        // with spawn_blocking, as the store's write already is.
        if let Err(error) = set_mtime(&pointer, now) {
            tracing::warn!(%error, "a media pointer's recency could not be recorded");
        }
        Some(blob.path.clone())
    }

    /// Caches `bytes` under each of `keys` and returns the file they landed in.
    ///
    /// Takes the lock twice and writes between: the file I/O never runs under
    /// the lock, so a clear waits out a lock-held swap, not a 16 MiB write. The
    /// hash is taken before the first lock for the same reason.
    pub(crate) fn store_shared(
        cache: &Mutex<Self>,
        keys: &[Key],
        kind: MediaKind,
        bytes: &[u8],
    ) -> Option<PathBuf> {
        let hash = content_hash(bytes);
        let reservation = cache
            .lock()
            .expect("the media cache lock is not poisoned")
            .reserve(&hash, kind, bytes.len(), keys.len())?;
        if write_reserved(&reservation, keys, bytes).is_none() {
            cache
                .lock()
                .expect("the media cache lock is not poisoned")
                .abandon(&reservation);
            return None;
        }
        cache
            .lock()
            .expect("the media cache lock is not poisoned")
            .commit(reservation, keys)
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
        self.store_keys(&[Key::Message(chat_id, message_id)], kind, bytes)
    }

    /// [`MediaCache::store`] under any set of keys.
    #[cfg(test)]
    fn store_keys(&mut self, keys: &[Key], kind: MediaKind, bytes: &[u8]) -> Option<PathBuf> {
        let reservation = self.reserve(&content_hash(bytes), kind, bytes.len(), keys.len())?;
        if write_reserved(&reservation, keys, bytes).is_none() {
            self.abandon(&reservation);
            return None;
        }
        self.commit(reservation, keys)
    }

    /// Claims the blob for `hash`, a store of `size` bytes that will name it
    /// under `refs` pointers. Refuses what could never fit: bytes over
    /// [`MEDIA_LIMIT`], or over the byte cap. A blob already here is held rather
    /// than rewritten. No I/O.
    fn reserve(
        &mut self,
        hash: &str,
        kind: MediaKind,
        size: usize,
        refs: usize,
    ) -> Option<Reservation> {
        let size = u64::try_from(size).ok()?;
        if size > MEDIA_LIMIT as u64 || size > self.max_bytes {
            return None;
        }
        // The holds are taken now, so an eviction cannot unlink the blob before
        // this store's pointers name it.
        let (path, write) = if let Some(blob) = self.blobs.get_mut(hash) {
            blob.refs += refs;
            (blob.path.clone(), false)
        } else {
            let path = self.dir.join(blob_name(hash, kind));
            *self.pending.entry(path.clone()).or_insert(0) += 1;
            (path, true)
        };
        Some(Reservation {
            dir: self.dir.clone(),
            hash: hash.to_owned(),
            path,
            size,
            generation: self.generation,
            refs,
            write,
        })
    }

    /// Gives back a reservation whose write failed: its pending name, or its
    /// hold on a blob that was already cached.
    fn abandon(&mut self, reservation: &Reservation) {
        if reservation.generation != self.generation {
            return;
        }
        if reservation.write {
            self.release(&reservation.path);
            if !self.owns(&reservation.path, &reservation.hash) {
                let _ = fs::remove_file(&reservation.path);
            }
        } else {
            for _ in 0..reservation.refs {
                self.drop_ref(&reservation.hash);
            }
        }
    }

    /// Drops one reservation of `path`. Only current-generation reservations
    /// are counted, so a clear, which drops them all, needs no care here.
    fn release(&mut self, path: &Path) {
        if let Some(count) = self.pending.get_mut(path) {
            *count -= 1;
            if *count == 0 {
                self.pending.remove(path);
            }
        }
    }

    /// Whether a newer reservation or a blob already owns this blob's name.
    fn owns(&self, path: &Path, hash: &str) -> bool {
        self.pending.contains_key(path) || self.blobs.contains_key(hash)
    }

    /// Gives back one reference to a blob, and unlinks the blob at zero.
    fn drop_ref(&mut self, hash: &str) {
        let Some(blob) = self.blobs.get_mut(hash) else {
            return;
        };
        blob.refs = blob.refs.saturating_sub(1);
        if blob.refs == 0
            && let Some(blob) = self.blobs.remove(hash)
            && let Err(error) = fs::remove_file(&blob.path)
        {
            tracing::warn!(%error, "an unreferenced media file could not be removed");
        }
    }

    fn pointer_path(&self, key: Key) -> PathBuf {
        self.dir.join(key.pointer_name())
    }

    /// Indexes a written store under each of `keys`, unless a clear ran since it
    /// was reserved. A late store's files are removed unless a newer reservation
    /// owns them.
    fn commit(&mut self, reservation: Reservation, keys: &[Key]) -> Option<PathBuf> {
        let Reservation {
            hash,
            path,
            size,
            generation,
            write,
            ..
        } = reservation;
        if generation != self.generation {
            // The clear already swept the cache; the late files must not survive
            // it. A newer store may own them by now, so leave what it owns.
            if !self.owns(&path, &hash) {
                let _ = fs::remove_file(&path);
            }
            for key in keys {
                if !self.entries.contains_key(key) {
                    let _ = fs::remove_file(self.pointer_path(*key));
                }
            }
            return None;
        }
        if write {
            self.release(&path);
            // Each new pointer takes a reference to the blob it names.
            self.blobs
                .entry(hash.clone())
                .or_insert_with(|| Blob {
                    path,
                    bytes: size,
                    refs: 0,
                })
                .refs += keys.len();
        } else if !self.blobs.contains_key(&hash) {
            // A hold keeps its blob indexed, so this is not reached in practice.
            return None;
        }

        let now = SystemTime::now();
        for key in keys {
            let entry = Entry {
                hash: hash.clone(),
                modified: now,
            };
            if let Some(old) = self.entries.insert(*key, entry) {
                // The key's old pointer no longer names its old blob.
                self.drop_ref(&old.hash);
            }
        }
        self.evict(keys);
        self.blobs.get(&hash).map(|blob| blob.path.clone())
    }

    /// Empties the cache: its files and its account tag. A missing directory
    /// is already empty. Any store still in flight is refused when it commits.
    pub(crate) fn clear(&mut self) {
        self.generation += 1;
        self.entries.clear();
        self.blobs.clear();
        self.pending.clear();
        let Ok(read) = fs::read_dir(&self.dir) else {
            return;
        };
        for entry in read.flatten() {
            let name = entry.file_name();
            let ours = name == ACCOUNT_FILE || name.to_str().is_some_and(is_cache_file);
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
        let mut pointers = Vec::new();
        for entry in read.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            if let Some(key) = parse_pointer(name) {
                let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                pointers.push((key, entry.path(), modified));
            } else if let Some(hash) = parse_blob(name) {
                let blob = Blob {
                    path: entry.path(),
                    bytes: meta.len(),
                    refs: 0,
                };
                if let Some(old) = self.blobs.insert(hash, blob) {
                    // Two blobs for one hash, under two suffixes: keep one.
                    let _ = fs::remove_file(&old.path);
                }
            } else if parse_legacy(name).is_some()
                && let Err(error) = fs::remove_file(entry.path())
            {
                // Named by message, before content addressing. Nothing indexes
                // these any more, and the cache is disposable, so they go.
                tracing::warn!(%error, "a legacy media file could not be removed");
            }
        }
        // The pointers are read after the blobs, so each can be checked against
        // the blob it names. A pointer naming no blob is dangling and goes.
        for (key, path, modified) in pointers {
            let hash = fs::read_to_string(&path)
                .ok()
                .map(|text| text.trim().to_owned())
                .filter(|hash| self.blobs.contains_key(hash));
            match hash {
                Some(hash) => {
                    if let Some(blob) = self.blobs.get_mut(&hash) {
                        blob.refs += 1;
                    }
                    self.entries.insert(key, Entry { hash, modified });
                }
                None => {
                    let _ = fs::remove_file(&path);
                }
            }
        }
        // A blob no pointer names was left by an interrupted store, or by a
        // pointer that is gone. Nothing would ever evict it, so it goes now.
        self.blobs.retain(|_, blob| {
            if blob.refs > 0 {
                return true;
            }
            if let Err(error) = fs::remove_file(&blob.path) {
                tracing::warn!(%error, "an orphaned media file could not be removed");
            }
            false
        });
        self.evict(&[]);
    }

    fn total(&self) -> u64 {
        self.blobs.values().map(|blob| blob.bytes).sum()
    }

    /// Removes the least recently used keys until both caps hold, never a `keep`
    /// key. Each removed key drops its pointer and its reference to its blob.
    ///
    /// A file that will not remove is dropped from the index anyway: the cap
    /// is then a count of what this cache still tracks, and the warning says
    /// which file survived.
    fn evict(&mut self, keep: &[Key]) {
        while self.entries.len() > self.max_entries || self.total() > self.max_bytes {
            let oldest = self
                .entries
                .iter()
                .filter(|(key, _)| !keep.contains(key))
                .min_by_key(|(_, entry)| entry.modified)
                .map(|(key, _)| *key);
            let Some(key) = oldest else {
                break;
            };
            if let Some(entry) = self.entries.remove(&key) {
                if let Err(error) = fs::remove_file(self.pointer_path(key)) {
                    tracing::warn!(%error, "an evicted media pointer could not be removed");
                }
                self.drop_ref(&entry.hash);
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

/// Writes what a reserved store owes the disk: the blob, when this store is the
/// one writing it, then a pointer for each key. A failed write is warned about
/// and returns `None`.
fn write_reserved(reservation: &Reservation, keys: &[Key], bytes: &[u8]) -> Option<()> {
    let written = fs::create_dir_all(&reservation.dir)
        .context("creating the media cache directory")
        .and_then(|()| {
            if reservation.write {
                write_atomically(&reservation.path, bytes)?;
            }
            let pointer = format!("{}\n", reservation.hash);
            keys.iter().try_for_each(|key| {
                write_atomically(
                    &reservation.dir.join(key.pointer_name()),
                    pointer.as_bytes(),
                )
            })
        });
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

/// Sets a file's modification time to `when`.
fn set_mtime(path: &Path, when: SystemTime) -> std::io::Result<()> {
    fs::File::open(path)?.set_modified(when)
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

    /// Sets a key's pointer mtime, to make one key older than another on disk.
    fn set_pointer_time(dir: &Path, chat_id: i64, message_id: i64, when: SystemTime) {
        let pointer = Key::Message(chat_id, message_id).pointer_name();
        set_mtime(&dir.join(pointer), when).expect("the mtime is set");
    }

    #[test]
    fn a_stored_file_is_found_again_after_a_restart_under_a_pid_free_name() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), Some("+15550001"));
        let path = cache
            .store(7, 9, MediaKind::Photo, b"picture")
            .expect("a small file is cached");
        drop(cache);

        let mut reopened = cache_in(dir.path(), Some("+15550001"));
        assert_eq!(reopened.lookup(7, 9), Some(path.clone()));
        let name = format!("{}.jpg", content_hash(b"picture"));
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some(name.as_str())
        );
        assert_eq!(fs::read(&path).expect("the file is there"), b"picture");
    }

    #[test]
    fn the_file_is_named_by_the_hash_of_its_bytes() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        // sha256("abc"), from FIPS 180-2.
        let path = cache.store(1, 2, MediaKind::Photo, b"abc").expect("cached");

        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad.jpg")
        );
    }

    #[test]
    fn two_messages_with_the_same_bytes_share_one_file() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        let first = cache
            .store(1, 1, MediaKind::Photo, b"same")
            .expect("cached");
        let second = cache
            .store(1, 2, MediaKind::Photo, b"same")
            .expect("cached");

        assert_eq!(first, second);
        assert_eq!(cache.lookup(1, 1), Some(first.clone()));
        assert_eq!(cache.lookup(1, 2), Some(second));
        let files = fs::read_dir(dir.path())
            .expect("the directory is there")
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|e| e == "jpg"))
            .count();
        assert_eq!(files, 1, "one blob for both messages");
    }

    #[test]
    fn a_pointer_is_rebuilt_on_restart() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), Some("+15550001"));
        let path = cache
            .store(7, 9, MediaKind::Photo, b"same")
            .expect("cached");
        cache
            .store(7, 10, MediaKind::Photo, b"same")
            .expect("cached");
        drop(cache);

        let mut reopened = cache_in(dir.path(), Some("+15550001"));

        assert_eq!(reopened.lookup(7, 9), Some(path.clone()));
        assert_eq!(reopened.lookup(7, 10), Some(path));
        let hash = content_hash(b"same");
        assert_eq!(reopened.blobs[&hash].refs, 2, "both pointers are counted");
        assert!(dir.path().join("7-9.ref").exists());
    }

    #[test]
    fn a_shared_blob_is_removed_only_when_its_last_entry_goes() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = MediaCache::with_limits(dir.path().to_path_buf(), 1024, 2, None);
        let shared = cache.store(1, 1, MediaKind::File, b"same").expect("cached");
        cache.store(1, 2, MediaKind::File, b"same").expect("cached");
        cache
            .entries
            .get_mut(&Key::Message(1, 1))
            .expect("the first key is indexed")
            .modified = SystemTime::UNIX_EPOCH;

        cache
            .store(1, 3, MediaKind::File, b"other")
            .expect("cached");
        assert!(shared.exists(), "one of its two keys is left");
        assert!(cache.entries.contains_key(&Key::Message(1, 2)));

        cache.store(1, 4, MediaKind::File, b"more").expect("cached");
        assert_eq!(cache.lookup(1, 2), None, "the second key goes next");
        assert!(
            !shared.exists(),
            "and with it the last reference to the blob"
        );
    }

    #[test]
    fn a_file_is_also_named_by_its_media_id_and_each_name_is_a_reference() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        let path = cache
            .store_keys(
                &[Key::Message(7, 9), Key::Media(77)],
                MediaKind::Video,
                b"clip",
            )
            .expect("cached");

        assert_eq!(cache.lookup_media(77), Some(path.clone()));
        assert_eq!(cache.lookup(7, 9), Some(path));
        assert_eq!(cache.blobs[&content_hash(b"clip")].refs, 2);
        assert!(
            dir.path().join("m77.ref").exists(),
            "the id has its own name"
        );
    }

    #[test]
    fn an_id_pointer_is_rebuilt_on_restart() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        let path = cache
            .store_keys(&[Key::Media(77)], MediaKind::Video, b"clip")
            .expect("cached");
        drop(cache);

        let mut reopened = cache_in(dir.path(), None);

        assert_eq!(reopened.lookup_media(77), Some(path));
    }

    #[test]
    fn an_id_pointer_is_dropped_with_its_blob() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = MediaCache::with_limits(dir.path().to_path_buf(), 1024, 2, None);
        let shared = cache
            .store_keys(
                &[Key::Message(1, 1), Key::Media(77)],
                MediaKind::File,
                b"same",
            )
            .expect("cached");
        for key in [Key::Message(1, 1), Key::Media(77)] {
            cache
                .entries
                .get_mut(&key)
                .expect("the key is indexed")
                .modified = SystemTime::UNIX_EPOCH;
        }

        // The message key is the older of the two, so it goes first; the id
        // outlives it, and the blob stays for the one reference left.
        cache
            .store(1, 2, MediaKind::File, b"other")
            .expect("cached");
        assert!(cache.entries.contains_key(&Key::Media(77)));
        assert!(shared.exists(), "one reference is left");

        cache
            .entries
            .get_mut(&Key::Media(77))
            .expect("the id is indexed")
            .modified = SystemTime::UNIX_EPOCH;
        cache.store(1, 3, MediaKind::File, b"more").expect("cached");

        assert_eq!(
            cache.lookup_media(77),
            None,
            "the id goes with its last reference"
        );
        assert!(!shared.exists(), "and so does the blob");
        assert!(!dir.path().join("m77.ref").exists());
    }

    #[test]
    fn a_hit_protects_a_file_from_eviction() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = MediaCache::with_limits(dir.path().to_path_buf(), 10, 256, None);
        let kept = cache.store(1, 1, MediaKind::File, b"aaaa").expect("cached");
        let evicted = cache.store(1, 2, MediaKind::File, b"bbbb").expect("cached");
        cache
            .entries
            .get_mut(&Key::Message(1, 2))
            .expect("the second file is indexed")
            .modified = SystemTime::UNIX_EPOCH;

        assert!(cache.lookup(1, 1).is_some());
        cache.store(1, 3, MediaKind::File, b"cccc");

        assert_eq!(cache.lookup(1, 2), None, "the file no one asked for goes");
        assert!(!evicted.exists());
        assert_eq!(cache.lookup(1, 1), Some(kept.clone()), "the hit is kept");
        assert!(kept.exists());
    }

    #[test]
    fn a_hit_survives_a_restart() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = MediaCache::with_limits(dir.path().to_path_buf(), 10, 256, None);
        let kept = cache.store(1, 1, MediaKind::File, b"aaaa").expect("cached");
        let evicted = cache.store(1, 2, MediaKind::File, b"bbbb").expect("cached");
        // Without a hit the first key would be the older; the hit moves it to now.
        set_pointer_time(dir.path(), 1, 1, SystemTime::UNIX_EPOCH);
        let later = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        set_pointer_time(dir.path(), 1, 2, later);
        assert!(cache.lookup(1, 1).is_some());
        drop(cache);

        let mut reopened = MediaCache::with_limits(dir.path().to_path_buf(), 10, 256, None);
        reopened.store(1, 3, MediaKind::File, b"cccc");

        assert_eq!(reopened.lookup(1, 1), Some(kept), "the hit is kept");
        assert_eq!(
            reopened.lookup(1, 2),
            None,
            "the file no one asked for goes"
        );
        assert!(!evicted.exists());
    }

    #[test]
    fn the_oldest_file_by_modification_goes_first_when_the_byte_cap_is_passed() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = MediaCache::with_limits(dir.path().to_path_buf(), 10, 256, None);
        let first = cache.store(1, 1, MediaKind::File, b"aaaa").expect("cached");
        cache.store(1, 2, MediaKind::File, b"bbbb");
        cache
            .entries
            .get_mut(&Key::Message(1, 1))
            .expect("the first file is indexed")
            .modified = SystemTime::UNIX_EPOCH;

        cache.store(1, 3, MediaKind::File, b"cccc");

        assert_eq!(cache.lookup(1, 1), None, "the oldest is evicted");
        assert!(cache.lookup(1, 2).is_some());
        assert!(cache.lookup(1, 3).is_some());
        assert!(cache.total() <= 10);
        assert!(!first.exists(), "and removed from disk");
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
    fn legacy_message_named_files_are_removed_on_open() {
        let dir = tempfile::tempdir().expect("a temp dir");
        for name in ["-7-9.jpg", "1-2.mp4", "notes.txt"] {
            fs::write(dir.path().join(name), b"old").expect("a file can be written");
        }

        let _cache = cache_in(dir.path(), None);

        assert!(
            !dir.path().join("-7-9.jpg").exists(),
            "a negative chat id too"
        );
        assert!(!dir.path().join("1-2.mp4").exists());
        assert!(dir.path().join("notes.txt").exists(), "a stranger is kept");
    }

    #[test]
    fn a_blob_without_a_pointer_is_removed_on_open() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        let kept = cache
            .store(7, 9, MediaKind::Photo, b"kept")
            .expect("cached");
        drop(cache);
        let orphan = dir
            .path()
            .join(blob_name(&content_hash(b"orphan"), MediaKind::File));
        fs::write(&orphan, b"orphan").expect("a file can be written");

        let mut reopened = cache_in(dir.path(), None);

        assert!(!orphan.exists(), "nothing names it, so it goes");
        assert!(kept.exists(), "a named blob stays");
        assert_eq!(reopened.lookup(7, 9), Some(kept));
    }

    #[test]
    fn a_different_account_clears_the_directory_first() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), Some("+15550001"));
        let path = cache
            .store(7, 9, MediaKind::Photo, b"theirs")
            .expect("cached");

        let mut other = cache_in(dir.path(), Some("+15550002"));

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
            .reserve(&content_hash(b"late!!"), MediaKind::Photo, 6, 1)
            .expect("the file is reserved");
        let path = reservation.path.clone();
        write_reserved(&reservation, &[Key::Message(7, 9)], b"late!!")
            .expect("the file is written");

        cache.clear();

        assert_eq!(
            cache.commit(reservation, &[Key::Message(7, 9)]),
            None,
            "refused"
        );
        assert_eq!(cache.lookup(7, 9), None);
        assert!(!path.exists(), "and the late file is removed");
    }

    #[test]
    fn a_refused_store_leaves_a_file_a_later_store_committed_under_the_same_name() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        let stale = cache
            .reserve(&content_hash(b"new"), MediaKind::Photo, 3, 1)
            .expect("reserved");
        write_reserved(&stale, &[Key::Message(7, 9)], b"new").expect("written");
        cache.clear();

        let fresh = cache.store(7, 9, MediaKind::Photo, b"new").expect("cached");
        assert_eq!(
            cache.commit(stale, &[Key::Message(7, 9)]),
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
        let first = cache.store(1, 1, MediaKind::File, b"aaaa").expect("cached");
        cache.store(1, 2, MediaKind::File, b"bbbb");
        cache
            .entries
            .get_mut(&Key::Message(1, 1))
            .expect("the first file is indexed")
            .modified = SystemTime::UNIX_EPOCH;
        cache.store(1, 3, MediaKind::File, b"cccc");

        assert_eq!(cache.lookup(1, 1), None, "the oldest is evicted");
        assert!(cache.total() <= 10);
        assert!(!first.exists(), "and removed from disk");
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

    #[test]
    fn a_late_store_leaves_the_file_a_newer_reservation_is_about_to_index() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        let stale = cache
            .reserve(&content_hash(b"new"), MediaKind::Photo, 3, 1)
            .expect("reserved");
        write_reserved(&stale, &[Key::Message(7, 9)], b"new").expect("written");
        cache.clear();
        let fresh = cache
            .reserve(&content_hash(b"new"), MediaKind::Photo, 3, 1)
            .expect("reserved");
        let path = fresh.path.clone();
        write_reserved(&fresh, &[Key::Message(7, 9)], b"new").expect("written over the late file");

        assert_eq!(
            cache.commit(stale, &[Key::Message(7, 9)]),
            None,
            "the late store is refused"
        );
        assert_eq!(fs::read(&path).expect("the newer file survives"), b"new");

        assert_eq!(
            cache.commit(fresh, &[Key::Message(7, 9)]),
            Some(path.clone())
        );
        assert_eq!(cache.lookup(7, 9), Some(path));
    }

    #[test]
    fn a_failed_write_gives_its_name_back_to_a_late_store() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        let hash = content_hash(b"old");
        let stale = cache
            .reserve(&hash, MediaKind::Photo, 3, 1)
            .expect("reserved");
        let path = stale.path.clone();
        write_reserved(&stale, &[Key::Message(7, 9)], b"old").expect("written");
        cache.clear();
        let failed = cache
            .reserve(&hash, MediaKind::Photo, 3, 1)
            .expect("reserved");
        cache.abandon(&failed);

        assert_eq!(cache.commit(stale, &[Key::Message(7, 9)]), None);
        assert!(!path.exists(), "and the late file is removed");
    }
}
