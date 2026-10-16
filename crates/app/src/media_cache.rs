//! Downloaded media kept on disk, so a re-open does not download it again.
//!
//! Each distinct file is stored once, as a blob named `<sha256>.<suffix>` for
//! the hash of its bytes. Each key has a pointer file holding the hash of its
//! blob and a CRC32 of the same bytes, so a lookup verifies the file on disk and
//! evicts a corrupt or truncated one rather than serving it. A key is either the
//! `(chat, message)` a file was downloaded for,
//! `<chat>-<message>.ref`, the Telegram media id it carries, `m<id>.ref`, which
//! is the same for a file forwarded into any chat, or the `(chat, message)`
//! whose embedded thumbnail a blob is, `<chat>-<message>.thumb`. The pointers
//! are the index: a restart rebuilds it from their names and contents. The
//! directory is bounded by the bytes of its distinct blobs and by its pointer
//! count; past either, the least recently used pointer goes first, and its blob
//! with it once no other pointer names it.
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
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result};
use domain::message::MediaKind;
use sha2::{Digest, Sha256};

/// The most bytes the cache keeps on disk. This is the total, so a few hundred
/// large files cannot grow without bound. It is a disk figure and does not
/// count against the memory budget.
pub const MEDIA_CACHE_MAX_BYTES: u64 = 1024 * 1024 * 1024;

/// The most files the cache keeps, whatever their size.
pub const MEDIA_CACHE_MAX_ENTRIES: usize = 256;

/// The file beside the media that records which account wrote it.
const ACCOUNT_FILE: &str = "account";

/// A streamed download's temp file, before its hash names it: a recognizable
/// name, so a leftover from a crash is swept on the next open and never
/// mistaken for a blob.
const DOWNLOAD_TEMP_PREFIX: &str = ".download-";
const DOWNLOAD_TEMP_SUFFIX: &str = ".tmp";

/// Whether a name is a streamed download's temp file.
fn is_download_temp(name: &str) -> bool {
    name.starts_with(DOWNLOAD_TEMP_PREFIX) && name.ends_with(DOWNLOAD_TEMP_SUFFIX)
}

/// How old a download temp must be, by its last write, for `scan` to sweep it
/// when another process owns it. A live download writes a chunk at a time and
/// so keeps refreshing the mtime; only a dead or crashed writer leaves a temp
/// untouched for a day.
const DOWNLOAD_TEMP_MAX_AGE: Duration = Duration::from_hours(24);

/// The pid in a `.download-<pid>-<n>.tmp` name, or `None` when the name does
/// not have that shape.
fn download_temp_pid(name: &str) -> Option<u32> {
    let body = name
        .strip_prefix(DOWNLOAD_TEMP_PREFIX)?
        .strip_suffix(DOWNLOAD_TEMP_SUFFIX)?;
    let (pid, counter) = body.split_once('-')?;
    counter.parse::<u64>().ok()?;
    pid.parse().ok()
}

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

/// The thumbnail pointer suffix, kept apart from a file's `.ref` so the two
/// indexes cannot be confused.
const THUMBNAIL_SUFFIX: &str = ".thumb";

/// What a cached file is looked up by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Key {
    /// The message a file was downloaded for: `<chat>-<message>.ref`.
    Message(i64, i64),
    /// Telegram's stable media id, which a forwarded copy shares: `m<id>.ref`.
    Media(i64),
    /// The message whose embedded thumbnail a blob is:
    /// `<chat>-<message>.thumb`.
    Thumbnail(i64, i64),
}

impl Key {
    /// The pointer file's name. The `m` keeps a media id apart from a chat id,
    /// which may be negative, and the suffix keeps a thumbnail apart from the
    /// file it previews.
    fn pointer_name(self) -> String {
        match self {
            Self::Message(chat_id, message_id) => format!("{chat_id}-{message_id}.ref"),
            Self::Media(media_id) => format!("m{media_id}.ref"),
            Self::Thumbnail(chat_id, message_id) => {
                format!("{chat_id}-{message_id}{THUMBNAIL_SUFFIX}")
            }
        }
    }

    /// The thumbnail key that belongs to this parent key, when it names a
    /// message: the thumbnail is the parent's, so it is removed and evicted with
    /// it. A media id has no thumbnail of its own — the blob is captured for the
    /// message that opened it — so there is none to pair here.
    fn thumbnail(self) -> Option<Self> {
        match self {
            Self::Message(chat_id, message_id) => Some(Self::Thumbnail(chat_id, message_id)),
            Self::Media(_) | Self::Thumbnail(_, _) => None,
        }
    }
}

/// The key a pointer's name carries, or `None` for anything else.
fn parse_pointer(name: &str) -> Option<Key> {
    if let Some(stem) = name.strip_suffix(THUMBNAIL_SUFFIX) {
        let (chat_id, message_id) = parse_ids(stem)?;
        return Some(Key::Thumbnail(chat_id, message_id));
    }
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

/// A content hash computed over bytes as they arrive: the sha256 that names the
/// blob, the CRC32 that verifies it, and the byte count.
///
/// A streamed download knows none of the three until the last chunk, so the
/// caller feeds the chunks here as it writes them and gets the final values
/// back from [`ContentHasher::finish`].
pub(crate) struct ContentHasher {
    sha: Sha256,
    crc: crc32fast::Hasher,
    bytes: u64,
}

impl ContentHasher {
    pub(crate) fn new() -> Self {
        Self {
            sha: Sha256::new(),
            crc: crc32fast::Hasher::new(),
            bytes: 0,
        }
    }

    pub(crate) fn update(&mut self, chunk: &[u8]) {
        self.sha.update(chunk);
        self.crc.update(chunk);
        self.bytes = self.bytes.saturating_add(chunk.len() as u64);
    }

    /// The sha256 hex, the CRC32, and the total bytes.
    pub(crate) fn finish(self) -> (String, u32, u64) {
        use std::fmt::Write as _;

        let mut hash = String::with_capacity(64);
        for byte in self.sha.finalize() {
            write!(hash, "{byte:02x}").expect("writing to a String cannot fail");
        }
        (hash, self.crc.finalize(), self.bytes)
    }
}

/// The sha256 of `bytes`, as lowercase hex: the name a blob is stored under.
/// The whole-buffer twin of [`ContentHasher`], for the tests that store bytes
/// directly rather than streaming them.
#[cfg(test)]
fn content_hash(bytes: &[u8]) -> String {
    let mut hasher = ContentHasher::new();
    hasher.update(bytes);
    hasher.finish().0
}

/// The hash and checksum a pointer file records, or `None` for one that names no
/// blob. The first line is the blob's sha256; the second, when present, is its
/// CRC32 as lowercase hex. A pointer written before integrity was added has only
/// the first line, and its checksum reads as unknown.
fn pointer_body(text: &str) -> Option<(String, Option<u32>)> {
    let mut lines = text.lines();
    let hash = lines.next()?.trim();
    if hash.is_empty() {
        return None;
    }
    let crc = lines
        .next()
        .map(str::trim)
        .and_then(|hex| u32::from_str_radix(hex, 16).ok());
    Some((hash.to_owned(), crc))
}

/// The CRC32 of a file's bytes, read in chunks so a large blob is never held in
/// memory. `None` if the file cannot be read, which a lookup treats as a
/// mismatch and evicts.
fn crc32_of(path: &Path) -> Option<u32> {
    use std::io::Read as _;

    let mut file = fs::File::open(path).ok()?;
    let mut hasher = crc32fast::Hasher::new();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => hasher.update(&buffer[..read]),
            Err(_) => return None,
        }
    }
    Some(hasher.finalize())
}

/// One key: the blob its pointer names, and when it was last used.
struct Entry {
    hash: String,
    /// The blob's CRC32, recorded at commit and checked on lookup. `None` for a
    /// pointer written before integrity existed, whose checksum is unknown.
    crc: Option<u32>,
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
    /// The CRC32 of the bytes this store writes, so its pointer records what a
    /// later lookup verifies.
    crc: u32,
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
    /// Paths a viewer may still be reading, with how many viewers hold each.
    /// Eviction and [`MediaCache::clear`] leave these files on disk.
    viewing: BTreeMap<PathBuf, usize>,
}

impl MediaCache {
    /// Opens the cache in `dir` for `account`, clearing it first if it was
    /// written for another account or for none. An unwritable `dir` opens a
    /// fallback under the platform cache directory instead, so cached media
    /// survives a restart; the system temp directory is the last resort, used
    /// only when no cache directory can be found. `max_bytes` is the byte cap,
    /// [`MEDIA_CACHE_MAX_BYTES`] when `None`.
    pub(crate) fn open(dir: PathBuf, account: Option<&str>, max_bytes: Option<u64>) -> Self {
        let fallback_root = match dirs::cache_dir() {
            Some(cache) => cache.join("televim"),
            None => std::env::temp_dir(),
        };
        Self::open_in(dir, &fallback_root, account, max_bytes)
    }

    /// [`MediaCache::open`] with the fallback root passed in, so tests can
    /// point it at a temp dir of their own.
    fn open_in(
        dir: PathBuf,
        fallback_root: &Path,
        account: Option<&str>,
        max_bytes: Option<u64>,
    ) -> Self {
        let dir = usable_dir(dir, fallback_root, account);
        Self::with_limits(
            dir,
            max_bytes.unwrap_or(MEDIA_CACHE_MAX_BYTES),
            MEDIA_CACHE_MAX_ENTRIES,
            account,
        )
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
            viewing: BTreeMap::new(),
        };
        cache.claim(account);
        cache.scan();
        cache
    }

    /// The directory the cache writes in. A streamed download's temp file lives
    /// here too, so the rename into place is on one filesystem.
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// The byte cap: the most the cache's distinct blobs may take on disk.
    pub(crate) fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    /// A unique temp path in [`MediaCache::dir`] for a streamed download.
    ///
    /// Named for the process and a counter so two stores in one run do not
    /// collide. The file is renamed into its content-named place once the stream
    /// has been hashed; a leftover from a crash is swept by [`MediaCache::scan`].
    pub(crate) fn download_temp_path(&self) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        self.dir.join(format!(
            "{DOWNLOAD_TEMP_PREFIX}{}-{n}{DOWNLOAD_TEMP_SUFFIX}",
            std::process::id()
        ))
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

    /// The file a message's embedded thumbnail is cached in, if it is.
    ///
    /// Consulted before the thumbnail is read again: a second open of the same
    /// message is served from disk. Verified on the way in exactly as a file is
    /// — a corrupt thumbnail is evicted and the lookup misses.
    pub(crate) fn lookup_thumbnail(&mut self, chat_id: i64, message_id: i64) -> Option<PathBuf> {
        self.hit(Key::Thumbnail(chat_id, message_id))
    }

    /// A lookup is a use: the key's recency moves to now, and so does its
    /// pointer's mtime, which is what carries the order across a restart.
    ///
    /// The blob is verified here, lazily: its bytes are read and their CRC32
    /// compared with the one its pointer recorded. A mismatch — a corrupt or
    /// truncated file — evicts the file and this key's pointer and returns a
    /// miss, so the caller re-downloads it. `scan` never re-hashes, so a restart
    /// stays cheap.
    fn hit(&mut self, key: Key) -> Option<PathBuf> {
        let pointer = self.pointer_path(key);
        let now = SystemTime::now();
        let (hash, crc) = {
            let entry = self.entries.get_mut(&key)?;
            entry.modified = now;
            (entry.hash.clone(), entry.crc)
        };
        let path = self.blobs.get(&hash)?.path.clone();
        if let Some(crc) = crc
            && crc32_of(&path) != Some(crc)
        {
            tracing::warn!(path = %path.display(), "a cached media file failed its checksum; evicting it");
            self.evict_corrupt(key, &hash);
            return None;
        }
        // ponytail: the pointer's mtime is set under the cache lock, one syscall
        // per hit. Ceiling: a slow filesystem stalls the lock, so a concurrent
        // store waits, not the UI loop. Upgrade path: move this touch off-lock
        // with spawn_blocking, as the store's write already is.
        if let Err(error) = set_mtime(&pointer, now) {
            tracing::warn!(%error, "a media pointer's recency could not be recorded");
        }
        Some(path)
    }

    /// Evicts a blob whose bytes failed their checksum: this key's entry and
    /// pointer go, and the file with them. Other keys that name the same blob are
    /// left as they are — the file is already gone, so their own lookup misses.
    fn evict_corrupt(&mut self, key: Key, hash: &str) {
        self.entries.remove(&key);
        if let Err(error) = fs::remove_file(self.pointer_path(key)) {
            tracing::warn!(%error, "a corrupt media pointer could not be removed");
        }
        if let Some(blob) = self.blobs.remove(hash)
            && let Err(error) = fs::remove_file(&blob.path)
        {
            tracing::warn!(%error, "a corrupt media file could not be removed");
        }
    }

    /// Caches `bytes` under each of `keys` and returns the file they landed in.
    ///
    /// The whole-buffer store, kept for tests: the production path streams a
    /// download to a temp file and commits it with
    /// [`MediaCache::store_from_path`], so a large attachment is never held whole.
    #[cfg(test)]
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
            .reserve(&hash, kind, bytes, keys.len())?;
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

    /// Commits a blob already written to `temp`: renames it into its
    /// content-named place, writes a pointer for each key, and indexes them.
    ///
    /// The streamed twin of [`MediaCache::store_shared`]. The rename is on one
    /// filesystem — the temp file was created in [`MediaCache::dir`] — so it is
    /// atomic, and it runs between the two lock acquisitions the same way the
    /// buffered store's write did. When the blob is already cached the temp is
    /// discarded and only the pointers are added.
    pub(crate) fn store_from_path(
        cache: &Mutex<Self>,
        keys: &[Key],
        kind: MediaKind,
        temp: &Path,
        hash: &str,
        crc: u32,
        size: u64,
    ) -> Option<PathBuf> {
        let reservation = cache
            .lock()
            .expect("the media cache lock is not poisoned")
            .reserve_hashed(hash, kind, size, crc, keys.len())?;

        if reservation.write {
            if let Err(error) = fs::rename(temp, &reservation.path) {
                tracing::warn!(%error, "a streamed media file could not be moved into the cache");
                cache
                    .lock()
                    .expect("the media cache lock is not poisoned")
                    .abandon(&reservation);
                let _ = fs::remove_file(temp);
                return None;
            }
        } else if let Err(error) = fs::remove_file(temp) {
            tracing::warn!(%error, "a redundant streamed media file could not be removed");
        }

        if write_pointers(&reservation.dir, &reservation.hash, reservation.crc, keys).is_none() {
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

    /// Stores a message's embedded thumbnail as a blob of its own, keyed to the
    /// message it belongs to, and returns the file it landed in.
    ///
    /// The bytes are already in memory — the accessor read them off the message
    /// — so they are written to a temp in the cache directory and committed the
    /// way a streamed download is: one store path, one checksum, the same
    /// content-addressing and dedup. The blob is a `bin` because its format is
    /// Telegram's and nothing here decodes it. An unwritable temp or a store
    /// that will not fit is warned about and answered with `None`.
    pub(crate) fn store_thumbnail(
        cache: &Mutex<Self>,
        chat_id: i64,
        message_id: i64,
        bytes: &[u8],
    ) -> Option<PathBuf> {
        let mut hasher = ContentHasher::new();
        hasher.update(bytes);
        let (hash, crc, size) = hasher.finish();

        let temp = cache
            .lock()
            .expect("the media cache lock is not poisoned")
            .download_temp_path();
        if let Err(error) = write_owned(&temp, bytes) {
            tracing::warn!(%error, "a message's thumbnail could not be written");
            let _ = fs::remove_file(&temp);
            return None;
        }

        Self::store_from_path(
            cache,
            &[Key::Thumbnail(chat_id, message_id)],
            MediaKind::File,
            &temp,
            &hash,
            crc,
            size,
        )
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
        let reservation = self.reserve(&content_hash(bytes), kind, bytes, keys.len())?;
        if write_reserved(&reservation, keys, bytes).is_none() {
            self.abandon(&reservation);
            return None;
        }
        self.commit(reservation, keys)
    }

    /// Claims the blob for `hash`, a store of `bytes` that will name it under
    /// `refs` pointers. The whole-buffer twin of [`MediaCache::reserve_hashed`],
    /// for the tests that store bytes directly.
    #[cfg(test)]
    fn reserve(
        &mut self,
        hash: &str,
        kind: MediaKind,
        bytes: &[u8],
        refs: usize,
    ) -> Option<Reservation> {
        let mut hasher = ContentHasher::new();
        hasher.update(bytes);
        let (_, crc, size) = hasher.finish();
        self.reserve_hashed(hash, kind, size, crc, refs)
    }

    /// Claims the blob named by `hash` of `size` bytes with checksum `crc`, that
    /// will name it under `refs` pointers. Refuses what could never fit: bytes
    /// over the byte cap. A blob already here is held rather than rewritten. No
    /// I/O.
    ///
    /// The streamed store knows its size and checksum before this runs — it has
    /// written the bytes to a temp file and hashed them — so they arrive as
    /// values rather than as a buffer.
    fn reserve_hashed(
        &mut self,
        hash: &str,
        kind: MediaKind,
        size: u64,
        crc: u32,
        refs: usize,
    ) -> Option<Reservation> {
        if size > self.max_bytes {
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
            crc,
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
            // A held file is the viewer's: `release_viewer` removes it, so the
            // hold outranks this removal.
            if !self.owns(&reservation.path, &reservation.hash)
                && !self.viewing.contains_key(&reservation.path)
            {
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
            && !self.viewing.contains_key(&blob.path)
            && let Err(error) = fs::remove_file(&blob.path)
        {
            tracing::warn!(%error, "an unreferenced media file could not be removed");
        }
    }

    /// Keeps `path` on disk until [`MediaCache::release_viewer`], because a
    /// viewer may be reading it. A held blob that loses its last reference is
    /// unindexed but not unlinked, and a clear leaves it too. Counted, so two
    /// holds of one path need two releases.
    pub(crate) fn hold_for_viewer(&mut self, path: &Path) {
        *self.viewing.entry(path.to_owned()).or_insert(0) += 1;
    }

    /// Gives back one [`MediaCache::hold_for_viewer`]. At the last, a blob that
    /// eviction or a clear left unindexed in the meantime is removed now; one
    /// still indexed or being written is left to the cache.
    pub(crate) fn release_viewer(&mut self, path: &Path) {
        let Some(count) = self.viewing.get_mut(path) else {
            return;
        };
        *count -= 1;
        if *count > 0 {
            return;
        }
        self.viewing.remove(path);
        let hash = (path.parent() == Some(self.dir.as_path()))
            .then(|| path.file_name()?.to_str().and_then(parse_blob))
            .flatten();
        if let Some(hash) = hash
            && !self.owns(path, &hash)
            && let Err(error) = fs::remove_file(path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, "a released media file could not be removed");
        }
    }

    fn pointer_path(&self, key: Key) -> PathBuf {
        self.dir.join(key.pointer_name())
    }

    /// Drops a message's own key, after an edit to it: its pointer goes, and its
    /// blob goes only with its last reference. A media id's pointer is content
    /// identity and is left alone.
    pub(crate) fn remove_message(&mut self, chat_id: i64, message_id: i64) {
        self.remove_keys(&[Key::Message(chat_id, message_id)]);
    }

    /// Drops every message key whose id is one of `message_ids`, in any chat.
    ///
    /// A deletion from the feed names no chat, so this matches the history
    /// file's rule. A message id is only unique within its chat, so a kept file
    /// for another chat's message with the same id goes too: that costs a
    /// download, never a wrong file.
    pub(crate) fn remove_messages(&mut self, message_ids: &[i64]) {
        let keys: Vec<Key> = self
            .entries
            .keys()
            .copied()
            .filter(|key| matches!(key, Key::Message(_, id) if message_ids.contains(id)))
            .collect();
        self.remove_keys(&keys);
    }

    /// Unindexes each key that is held, removes its pointer, and gives back its
    /// reference to its blob.
    ///
    /// A message's thumbnail is its parent's, so it goes with it; a blob shared
    /// with another key survives on that key's reference, the same rule the
    /// whole cache turns on.
    fn remove_keys(&mut self, keys: &[Key]) {
        for key in keys {
            self.forget_key(*key, "a removed media pointer could not be removed");
            if let Some(thumbnail) = key.thumbnail() {
                self.forget_key(thumbnail, "a removed media pointer could not be removed");
            }
        }
    }

    /// Unindexes one key that is held, removes its pointer, and gives back its
    /// reference to its blob. `what` names the pointer in the log when its
    /// removal fails, so the two callers — a removal and an eviction — say which
    /// of the two happened.
    fn forget_key(&mut self, key: Key, what: &str) {
        if let Some(entry) = self.entries.remove(&key) {
            if let Err(error) = fs::remove_file(self.pointer_path(key)) {
                tracing::warn!(%error, what);
            }
            self.drop_ref(&entry.hash);
        }
    }

    /// Indexes a written store under each of `keys`, unless a clear ran since it
    /// was reserved. A late store's files are removed unless a newer reservation
    /// owns them.
    fn commit(&mut self, reservation: Reservation, keys: &[Key]) -> Option<PathBuf> {
        let Reservation {
            hash,
            crc,
            path,
            size,
            generation,
            write,
            ..
        } = reservation;
        if generation != self.generation {
            // The clear already swept the cache; the late files must not survive
            // it. A newer store may own them by now, so leave what it owns. A
            // viewer's hold outranks the generation check, as it does in
            // `clear`: `release_viewer` removes the file then.
            if !self.owns(&path, &hash) && !self.viewing.contains_key(&path) {
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
                crc: Some(crc),
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
            let ours = name == ACCOUNT_FILE
                || name
                    .to_str()
                    .is_some_and(|name| is_cache_file(name) || is_download_temp(name));
            if ours
                && !self.viewing.contains_key(&entry.path())
                && let Err(error) = fs::remove_file(entry.path())
            {
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
            } else if is_download_temp(name) {
                // A streamed download's temp file. It is a crash's leftover only
                // when this process wrote it or its last write is long past; a
                // fresh temp of another pid belongs to a live instance.
                let stale = meta
                    .modified()
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .map(|age| age > DOWNLOAD_TEMP_MAX_AGE);
                let sweep = match (download_temp_pid(name), stale) {
                    (Some(pid), _) if pid == std::process::id() => true,
                    (Some(_), Some(stale)) => stale,
                    _ => {
                        tracing::warn!("a media download temp of unknown ownership was left alone");
                        false
                    }
                };
                if sweep && let Err(error) = fs::remove_file(entry.path()) {
                    tracing::warn!(%error, "an interrupted media download's temp file could not be removed");
                }
            }
        }
        // The pointers are read after the blobs, so each can be checked against
        // the blob it names. A pointer naming no blob is dangling and goes.
        for (key, path, modified) in pointers {
            let body = fs::read_to_string(&path)
                .ok()
                .and_then(|text| pointer_body(&text))
                .filter(|(hash, _)| self.blobs.contains_key(hash));
            match body {
                Some((hash, crc)) => {
                    if let Some(blob) = self.blobs.get_mut(&hash) {
                        blob.refs += 1;
                    }
                    self.entries.insert(
                        key,
                        Entry {
                            hash,
                            crc,
                            modified,
                        },
                    );
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
            self.forget_key(key, "an evicted media pointer could not be removed");
            // The thumbnail is the parent's and goes with it — unless this same
            // pass is storing it, which is the one `keep` protects against.
            if let Some(thumbnail) = key.thumbnail()
                && !keep.contains(&thumbnail)
            {
                self.forget_key(thumbnail, "an evicted media pointer could not be removed");
            }
        }
    }
}

/// `dir` if it can be written, else a fallback under `fallback_root` for `account`.
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
    // The caller passes the system temp dir as the root only when no platform
    // cache directory was found, so the two cases read differently: a cache-dir
    // fallback survives a restart, the temp last resort does not.
    if fallback_root == std::env::temp_dir().as_path() {
        tracing::warn!(
            configured = %dir.display(),
            fallback = %fallback.display(),
            "the media cache directory is not writable and no platform cache directory was \
             found; using the system temp directory, where cached media will not survive a \
             restart"
        );
    } else {
        tracing::warn!(
            configured = %dir.display(),
            fallback = %fallback.display(),
            "the media cache directory is not writable; using a fallback under the platform \
             cache directory"
        );
    }
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

/// Writes what a reserved buffered store owes the disk: the blob, when this
/// store is the one writing it, then a pointer for each key. A failed write is
/// warned about and returns `None`. The streamed store renames its blob into
/// place instead and calls [`write_pointers`] directly.
#[cfg(test)]
fn write_reserved(reservation: &Reservation, keys: &[Key], bytes: &[u8]) -> Option<()> {
    let written = fs::create_dir_all(&reservation.dir)
        .context("creating the media cache directory")
        .and_then(|()| {
            if reservation.write {
                write_atomically(&reservation.path, bytes)?;
            }
            Ok(())
        });
    if let Err(error) = written {
        tracing::warn!(%error, "media could not be cached");
        return None;
    }
    write_pointers(&reservation.dir, &reservation.hash, reservation.crc, keys)
}

/// Writes a pointer file for each key, naming `hash` and carrying `crc`.
///
/// The pointer body is the hash on the first line and the CRC32 as lowercase hex
/// on the second, which is what a restart reads back. A failed write is warned
/// about and returns `None`.
fn write_pointers(dir: &Path, hash: &str, crc: u32, keys: &[Key]) -> Option<()> {
    let pointer = format!("{hash}\n{crc:08x}\n");
    let written = keys
        .iter()
        .try_for_each(|key| write_atomically(&dir.join(key.pointer_name()), pointer.as_bytes()));
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

/// Creates `path` owner-only and writes `bytes` into it.
///
/// The discipline every cached file is made under, for the one store whose bytes
/// arrive whole rather than as a stream: a thumbnail is in memory before it is
/// written, so [`MediaCache::store_thumbnail`] hands it to the same
/// [`MediaCache::store_from_path`] a streamed download uses.
#[cfg(unix)]
fn write_owned(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?
        .write_all(bytes)
}

#[cfg(not(unix))]
fn write_owned(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    fs::write(path, bytes)
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
        MediaCache::open(dir.to_path_buf(), account, None)
    }

    /// Sets a key's pointer mtime, to make one key older than another on disk.
    fn set_pointer_time(dir: &Path, chat_id: i64, message_id: i64, when: SystemTime) {
        let pointer = Key::Message(chat_id, message_id).pointer_name();
        set_mtime(&dir.join(pointer), when).expect("the mtime is set");
    }

    #[test]
    fn a_fresh_temp_from_another_pid_survives_the_next_open() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let temp = dir
            .path()
            .join(format!(".download-{}-0.tmp", std::process::id() + 1));
        fs::write(&temp, b"partial").expect("the temp is written");
        drop(cache_in(dir.path(), None));
        assert!(temp.exists());
    }

    #[test]
    fn a_stale_temp_from_another_pid_is_swept() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let temp = dir
            .path()
            .join(format!(".download-{}-0.tmp", std::process::id() + 1));
        fs::write(&temp, b"partial").expect("the temp is written");
        let old = SystemTime::now() - DOWNLOAD_TEMP_MAX_AGE - Duration::from_hours(1);
        set_mtime(&temp, old).expect("the mtime is set");
        drop(cache_in(dir.path(), None));
        assert!(!temp.exists());
    }

    #[test]
    fn our_own_temp_is_swept_even_when_fresh() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let temp = dir
            .path()
            .join(format!(".download-{}-0.tmp", std::process::id()));
        fs::write(&temp, b"partial").expect("the temp is written");
        drop(cache_in(dir.path(), None));
        assert!(!temp.exists());
    }

    #[test]
    fn an_unparsable_download_temp_is_left_alone() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let temp = dir.path().join(".download-x-0.tmp");
        fs::write(&temp, b"partial").expect("the temp is written");
        let old = SystemTime::now() - DOWNLOAD_TEMP_MAX_AGE - Duration::from_hours(1);
        set_mtime(&temp, old).expect("the mtime is set");
        drop(cache_in(dir.path(), None));
        assert!(temp.exists());
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
    fn a_stored_file_passes_its_checksum_on_lookup() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        let path = cache
            .store(7, 9, MediaKind::Photo, b"picture")
            .expect("cached");

        assert_eq!(cache.lookup(7, 9), Some(path.clone()));
        assert_eq!(
            cache.entries[&Key::Message(7, 9)].crc,
            Some(crc32fast::hash(b"picture")),
            "the pointer records the blob's checksum"
        );
        assert_eq!(fs::read(&path).expect("the file is there"), b"picture");
    }

    #[test]
    fn a_valid_hit_returns_the_file_unchanged() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        let path = cache
            .store(1, 2, MediaKind::File, b"bytes")
            .expect("cached");

        assert_eq!(cache.lookup(1, 2), Some(path.clone()));
        assert_eq!(
            cache.lookup(1, 2),
            Some(path.clone()),
            "a second hit is the same"
        );
        assert_eq!(fs::read(&path).expect("the file is there"), b"bytes");
        assert!(cache.entries.contains_key(&Key::Message(1, 2)));
    }

    #[test]
    fn a_corrupt_blob_is_evicted_and_the_lookup_misses() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        let path = cache
            .store(7, 9, MediaKind::Photo, b"picture")
            .expect("cached");
        fs::write(&path, b"corrupted").expect("the blob can be corrupted");

        assert_eq!(cache.lookup(7, 9), None, "the corrupt file is a miss");
        assert!(!path.exists(), "the corrupt file is removed");
        assert!(
            !dir.path().join("7-9.ref").exists(),
            "and its pointer with it"
        );
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn a_truncated_blob_is_evicted_and_the_lookup_misses() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        let path = cache
            .store(7, 9, MediaKind::Photo, b"picture")
            .expect("cached");
        fs::write(&path, b"pic").expect("the blob can be truncated");

        assert_eq!(cache.lookup(7, 9), None, "the truncated file is a miss");
        assert!(!path.exists(), "the truncated file is removed");
    }

    #[test]
    fn evicting_a_corrupt_blob_leaves_a_shared_key_intact() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        let path = cache
            .store_keys(
                &[Key::Message(1, 1), Key::Media(77)],
                MediaKind::Video,
                b"clip",
            )
            .expect("cached");
        fs::write(&path, b"XXXX").expect("the shared blob can be corrupted");

        assert_eq!(cache.lookup(1, 1), None, "the corrupt file is a miss");
        assert!(!path.exists(), "the shared blob is removed");
        assert!(
            cache.entries.contains_key(&Key::Media(77)),
            "the other key is left alone"
        );
        assert!(dir.path().join("m77.ref").exists(), "and so is its pointer");
        assert_eq!(
            cache.lookup_media(77),
            None,
            "its blob is gone, so it misses too"
        );
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
    fn a_small_configured_cap_evicts_at_that_size() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = MediaCache::open(dir.path().to_path_buf(), None, Some(1024));
        let first = cache
            .store(1, 1, MediaKind::File, &[1; 600])
            .expect("cached");
        cache
            .store(1, 2, MediaKind::File, &[2; 600])
            .expect("cached");

        assert!(cache.total() <= 1024, "the configured cap holds");
        assert_eq!(cache.lookup(1, 1), None, "the older file goes");
        assert!(!first.exists(), "and is removed from disk");
        assert!(cache.lookup(1, 2).is_some());
    }

    #[test]
    fn a_deleted_message_drops_its_cache_entry() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        let gone = cache.store(1, 5, MediaKind::File, b"gone").expect("cached");
        let elsewhere = cache
            .store(2, 5, MediaKind::File, b"elsewhere")
            .expect("cached");
        let kept = cache.store(1, 6, MediaKind::File, b"kept").expect("cached");

        // The feed names the id with no chat: both chats' copies of 5 go.
        cache.remove_messages(&[5]);

        assert_eq!(cache.lookup(1, 5), None);
        assert_eq!(
            cache.lookup(2, 5),
            None,
            "the same id in another chat goes too"
        );
        assert!(
            !gone.exists() && !elsewhere.exists(),
            "and their blobs with them"
        );
        assert_eq!(
            cache.lookup(1, 6),
            Some(kept.clone()),
            "other messages stay"
        );
        assert!(kept.exists());
        assert!(
            !dir.path().join("1-5.ref").exists(),
            "the pointer is removed"
        );
    }

    #[test]
    fn a_blob_shared_with_another_key_survives_a_message_delete() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        let shared = cache.store(1, 5, MediaKind::File, b"same").expect("cached");
        cache.store(1, 6, MediaKind::File, b"same").expect("cached");

        cache.remove_message(1, 5);

        assert_eq!(cache.lookup(1, 5), None);
        assert!(shared.exists(), "message 6 still names the blob");
        assert_eq!(cache.lookup(1, 6), Some(shared));
    }

    #[test]
    fn a_media_id_is_kept_when_its_message_is_deleted() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        let path = cache
            .store_keys(
                &[Key::Message(1, 5), Key::Media(77)],
                MediaKind::Video,
                b"clip",
            )
            .expect("cached");

        cache.remove_messages(&[5]);

        assert_eq!(
            cache.lookup_media(77),
            Some(path.clone()),
            "the id is content identity"
        );
        assert!(path.exists());
    }

    #[test]
    fn the_production_caps_are_a_gibibyte_and_256_files() {
        assert_eq!(MEDIA_CACHE_MAX_BYTES, 1024 * 1024 * 1024);
        assert_eq!(MEDIA_CACHE_MAX_ENTRIES, 256);
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
    fn a_held_blob_survives_losing_its_last_reference_and_goes_on_release() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        let path = cache.store(1, 2, MediaKind::Gif, b"gif").expect("cached");
        cache.hold_for_viewer(&path);

        cache.remove_message(1, 2);
        assert_eq!(cache.lookup(1, 2), None);
        assert!(path.exists(), "a viewer may be reading it");

        cache.release_viewer(&path);
        assert!(!path.exists(), "released, it is reaped");
    }

    #[test]
    fn a_held_blob_survives_clear_and_goes_on_release() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), Some("+15550001"));
        let path = cache.store(1, 2, MediaKind::Gif, b"gif").expect("cached");
        cache.hold_for_viewer(&path);
        cache.hold_for_viewer(&path);

        cache.clear();
        assert!(path.exists());
        assert_eq!(cache.lookup(1, 2), None);
        assert!(!dir.path().join(ACCOUNT_FILE).exists());

        cache.release_viewer(&path);
        assert!(path.exists(), "one of two holds remains");
        cache.release_viewer(&path);
        assert!(!path.exists());
    }

    #[test]
    fn a_held_then_cleared_blob_is_swept_by_the_next_open() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        let path = cache.store(1, 2, MediaKind::Gif, b"gif").expect("cached");
        cache.hold_for_viewer(&path);
        cache.clear();
        assert!(path.exists());
        drop(cache);

        let _reopened = cache_in(dir.path(), None);
        assert!(
            !path.exists(),
            "no pointer names it, so the scan removes it"
        );
    }

    #[test]
    fn releasing_a_blob_that_is_still_cached_leaves_it() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        let path = cache.store(1, 2, MediaKind::Gif, b"gif").expect("cached");
        cache.hold_for_viewer(&path);
        cache.release_viewer(&path);

        assert!(path.exists());
        assert_eq!(cache.lookup(1, 2), Some(path));
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
            .reserve(&content_hash(b"late!!"), MediaKind::Photo, b"late!!", 1)
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

    /// Reserves a hashed store, holds its path for a viewer, and clears; the
    /// store's file then materialises, as a store that had already renamed.
    fn late_store_under_a_clear(cache: &mut MediaCache, held: bool) -> (Reservation, PathBuf) {
        let reservation = cache
            .reserve_hashed(&content_hash(b"late"), MediaKind::Photo, 4, 1, 1)
            .expect("the file is reserved");
        let path = reservation.path.clone();
        if held {
            cache.hold_for_viewer(&path);
        }
        cache.clear();
        fs::write(&path, b"late").expect("the file materialises");
        (reservation, path)
    }

    #[test]
    fn a_late_commit_leaves_a_held_file_for_its_viewer() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), Some("+15550001"));
        let (reservation, path) = late_store_under_a_clear(&mut cache, true);

        assert_eq!(cache.commit(reservation, &[Key::Message(7, 9)]), None);
        assert!(path.exists(), "a viewer holds it");

        cache.release_viewer(&path);
        assert!(!path.exists(), "released, it is removed");
    }

    #[test]
    fn a_late_commit_still_removes_an_unheld_file() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), Some("+15550001"));
        let (reservation, path) = late_store_under_a_clear(&mut cache, false);

        assert_eq!(cache.commit(reservation, &[Key::Message(7, 9)]), None);
        assert!(!path.exists());
    }

    #[test]
    fn an_abandoned_store_leaves_a_held_file() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), Some("+15550001"));
        let reservation = cache
            .reserve_hashed(&content_hash(b"late"), MediaKind::Photo, 4, 1, 1)
            .expect("the file is reserved");
        let path = reservation.path.clone();
        cache.hold_for_viewer(&path);
        fs::write(&path, b"part").expect("the file is there");

        cache.abandon(&reservation);
        assert!(path.exists(), "a viewer holds it");

        cache.release_viewer(&path);
        assert!(!path.exists(), "released, it is removed");
    }

    #[test]
    fn a_refused_store_leaves_a_file_a_later_store_committed_under_the_same_name() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        let stale = cache
            .reserve(&content_hash(b"new"), MediaKind::Photo, b"new", 1)
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
    fn an_unwritable_configured_dir_opens_a_working_fallback_under_the_cache_root() {
        let root = tempfile::tempdir().expect("a temp dir");
        let cache_root = root.path().join("cache");
        fs::create_dir(&cache_root).expect("the cache root is made");

        let mut cache = MediaCache::open_in(
            unwritable_dir(root.path()),
            &cache_root,
            Some("+15550001"),
            None,
        );
        let path = cache
            .store(7, 9, MediaKind::Photo, b"picture")
            .expect("the fallback caches it");

        assert!(path.starts_with(cache_root.join("televim.media-15550001")));
        assert_eq!(cache.lookup(7, 9), Some(path.clone()));
        assert_eq!(fs::read(&path).expect("the file is there"), b"picture");
        assert_eq!(cache.max_bytes, MEDIA_CACHE_MAX_BYTES);
        assert_eq!(cache.max_entries, MEDIA_CACHE_MAX_ENTRIES);
    }

    #[test]
    fn a_fallback_file_survives_a_restart() {
        let root = tempfile::tempdir().expect("a temp dir");
        let cache_root = root.path().join("cache");
        fs::create_dir(&cache_root).expect("the cache root is made");

        let mut cache = MediaCache::open_in(
            unwritable_dir(root.path()),
            &cache_root,
            Some("+15550001"),
            None,
        );
        let path = cache
            .store(7, 9, MediaKind::Photo, b"picture")
            .expect("the fallback caches it");
        drop(cache);

        // The restart analogue: a second open of the same configured dir finds
        // the file the first run cached under the persistent cache root.
        let mut reopened = MediaCache::open_in(
            unwritable_dir(root.path()),
            &cache_root,
            Some("+15550001"),
            None,
        );

        assert_eq!(reopened.lookup(7, 9), Some(path.clone()));
        assert_eq!(
            fs::read(&path).expect("the file survived the restart"),
            b"picture"
        );
    }

    #[test]
    fn nothing_is_created_under_the_unwritable_configured_path() {
        let root = tempfile::tempdir().expect("a temp dir");
        let fallback_root = root.path().join("tmp");
        fs::create_dir(&fallback_root).expect("the fallback root is made");
        let configured = unwritable_dir(root.path());

        let cache = MediaCache::open_in(configured.clone(), &fallback_root, None, None);

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
        let mut cache =
            MediaCache::open_in(unwritable_dir(root.path()), &fallback_root, None, None);
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

        let mut cache = MediaCache::open_in(configured, &fallback_root, None, None);

        assert_eq!(cache.store(1, 2, MediaKind::Gif, b"gif"), None);
        assert_eq!(cache.lookup(1, 2), None);
    }

    #[test]
    fn the_temp_dir_is_the_last_resort_when_no_cache_dir_can_be_found() {
        let root = tempfile::tempdir().expect("a temp dir");

        // The system temp dir is the root the caller passes only when
        // `dirs::cache_dir` is `None`: the last resort warns and lands here.
        let fallback = usable_dir(unwritable_dir(root.path()), &std::env::temp_dir(), None);

        assert_eq!(fallback, std::env::temp_dir().join("televim.media-none"));
    }

    #[test]
    fn a_late_store_leaves_the_file_a_newer_reservation_is_about_to_index() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut cache = cache_in(dir.path(), None);
        let stale = cache
            .reserve(&content_hash(b"new"), MediaKind::Photo, b"new", 1)
            .expect("reserved");
        write_reserved(&stale, &[Key::Message(7, 9)], b"new").expect("written");
        cache.clear();
        let fresh = cache
            .reserve(&content_hash(b"new"), MediaKind::Photo, b"new", 1)
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
            .reserve(&hash, MediaKind::Photo, b"old", 1)
            .expect("reserved");
        let path = stale.path.clone();
        write_reserved(&stale, &[Key::Message(7, 9)], b"old").expect("written");
        cache.clear();
        let failed = cache
            .reserve(&hash, MediaKind::Photo, b"old", 1)
            .expect("reserved");
        cache.abandon(&failed);

        assert_eq!(cache.commit(stale, &[Key::Message(7, 9)]), None);
        assert!(!path.exists(), "and the late file is removed");
    }

    /// The thumbnail a message already carries is stored as its own blob — a
    /// content-addressed one, under the message's own `.thumb` pointer — and
    /// found again after a restart, when `scan` rebuilds the index from it.
    #[test]
    fn a_thumbnail_is_found_again_after_a_restart() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let cache = Mutex::new(cache_in(dir.path(), Some("+15550001")));
        let path =
            MediaCache::store_thumbnail(&cache, 7, 9, b"preview").expect("a thumbnail is cached");
        let name = format!("{}.bin", content_hash(b"preview"));
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some(name.as_str()),
            "the preview is a content-addressed blob of its own"
        );
        drop(cache);

        let mut reopened = cache_in(dir.path(), Some("+15550001"));
        assert_eq!(reopened.lookup_thumbnail(7, 9), Some(path.clone()));
        assert_eq!(fs::read(&path).expect("the file is there"), b"preview");
        assert!(
            dir.path().join("7-9.thumb").exists(),
            "and its pointer is rebuilt"
        );
    }

    /// A lookup is a use: a stored thumbnail is handed back before it is ever
    /// read again.
    #[test]
    fn a_stored_thumbnail_is_handed_back_on_lookup() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let cache = Mutex::new(cache_in(dir.path(), None));
        let path = MediaCache::store_thumbnail(&cache, 1, 2, b"preview").expect("cached");

        assert_eq!(
            cache
                .lock()
                .expect("the media cache lock is not poisoned")
                .lookup_thumbnail(1, 2),
            Some(path)
        );
    }

    /// A thumbnail is its parent's: removing the message removes its pointer
    /// and its thumbnail's, and both blobs with them.
    #[test]
    fn a_thumbnail_is_dropped_with_its_parent_message() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let cache = Mutex::new(cache_in(dir.path(), None));
        let file = cache
            .lock()
            .expect("the media cache lock is not poisoned")
            .store(1, 5, MediaKind::File, b"clip")
            .expect("cached");
        let thumb = MediaCache::store_thumbnail(&cache, 1, 5, b"preview").expect("cached");

        cache
            .lock()
            .expect("the media cache lock is not poisoned")
            .remove_message(1, 5);

        assert!(
            cache
                .lock()
                .expect("the media cache lock is not poisoned")
                .lookup_thumbnail(1, 5)
                .is_none()
        );
        assert!(!thumb.exists(), "the thumbnail blob goes with its parent");
        assert!(!file.exists(), "and so does the parent's own blob");
        assert!(
            !dir.path().join("1-5.thumb").exists() && !dir.path().join("1-5.ref").exists(),
            "both pointers are removed"
        );
    }

    /// Two messages that carry the same preview share one thumbnail blob, and
    /// it goes only with the last of them — the blob-refcount rule the whole
    /// cache turns on, applied to a thumbnail.
    #[test]
    fn a_thumbnail_is_kept_while_a_shared_parent_remains() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let cache = Mutex::new(cache_in(dir.path(), None));
        let file = cache
            .lock()
            .expect("the media cache lock is not poisoned")
            .store(1, 1, MediaKind::File, b"clip")
            .expect("cached");
        cache
            .lock()
            .expect("the media cache lock is not poisoned")
            .store(1, 2, MediaKind::File, b"clip")
            .expect("cached");
        let thumb = MediaCache::store_thumbnail(&cache, 1, 1, b"preview").expect("cached");
        let again = MediaCache::store_thumbnail(&cache, 1, 2, b"preview").expect("cached");
        assert_eq!(thumb, again, "one blob holds both previews");

        cache
            .lock()
            .expect("the media cache lock is not poisoned")
            .remove_message(1, 1);

        assert!(
            cache
                .lock()
                .expect("the media cache lock is not poisoned")
                .lookup_thumbnail(1, 1)
                .is_none()
        );
        assert_eq!(
            cache
                .lock()
                .expect("the media cache lock is not poisoned")
                .lookup_thumbnail(1, 2),
            Some(thumb.clone()),
            "the other message still names the preview"
        );
        assert!(thumb.exists(), "so the shared thumbnail blob survives");
        assert!(file.exists(), "and the file the two messages share");
    }

    /// A corrupt thumbnail is evicted on lookup exactly as a file is: the
    /// pointer and the blob go, and the miss says to read it again.
    #[test]
    fn a_corrupt_thumbnail_is_evicted_and_the_lookup_misses() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let cache = Mutex::new(cache_in(dir.path(), None));
        let thumb = MediaCache::store_thumbnail(&cache, 1, 5, b"preview").expect("cached");
        fs::write(&thumb, b"corrupted").expect("the blob can be corrupted");

        assert!(
            cache
                .lock()
                .expect("the media cache lock is not poisoned")
                .lookup_thumbnail(1, 5)
                .is_none(),
            "the corrupt preview is a miss"
        );
        assert!(!thumb.exists(), "the corrupt blob is removed");
        assert!(
            !dir.path().join("1-5.thumb").exists(),
            "and its pointer with it"
        );
    }

    /// Eviction takes a thumbnail with its parent: when the parent is the
    /// oldest entry, its preview goes with it.
    #[test]
    fn an_evicted_parent_takes_its_thumbnail_with_it() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let cache = Mutex::new(MediaCache::with_limits(
            dir.path().to_path_buf(),
            1024,
            4,
            None,
        ));
        let file = cache
            .lock()
            .expect("the media cache lock is not poisoned")
            .store(1, 1, MediaKind::File, b"aaaa")
            .expect("cached");
        let thumb = MediaCache::store_thumbnail(&cache, 1, 1, b"preview").expect("cached");
        cache
            .lock()
            .expect("the media cache lock is not poisoned")
            .entries
            .get_mut(&Key::Message(1, 1))
            .expect("the parent is indexed")
            .modified = SystemTime::UNIX_EPOCH;
        for (message, bytes) in [(2, b"bbbb"), (3, b"cccc"), (4, b"dddd")] {
            cache
                .lock()
                .expect("the media cache lock is not poisoned")
                .store(1, message, MediaKind::File, bytes)
                .expect("cached");
        }

        assert!(
            cache
                .lock()
                .expect("the media cache lock is not poisoned")
                .lookup(1, 1)
                .is_none(),
            "the oldest parent is evicted"
        );
        assert!(
            cache
                .lock()
                .expect("the media cache lock is not poisoned")
                .lookup_thumbnail(1, 1)
                .is_none(),
            "and its preview with it"
        );
        assert!(!file.exists() && !thumb.exists());
    }
}
