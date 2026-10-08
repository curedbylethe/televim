//! Read messages on disk: the history file beside the configuration.
//!
//! The last [`HISTORY_CACHE_DEPTH`] messages of each private conversation, so a
//! launch can paint a conversation before the wire has answered. It lives here
//! for the drafts file's reason: `domain` and `tui` own no filesystem, and this
//! is the crate that owns the configuration path. What crosses out of it is
//! plain [`domain::message::Message`] values, never the file's own rows.
//!
//! **Bodies, not bytes.** A row is a message's text and the *kind* of
//! attachment it carries — the `[image]` token, not the image. Media bytes
//! belong to a media cache of their own, and a history file that held them
//! would be that cache under another name and a budget nobody measured.
//!
//! **Plaintext, like the drafts.** The file is restricted to its owner and
//! tagged with the account, but not encrypted; sealing it the way the session
//! is sealed is a recorded follow-up, not something this file half-does.
//!
//! Every operation is best-effort and launch-safe, the drafts file's
//! discipline: a missing file is an empty cache, an unreadable one is warned
//! about and forgotten (the next save overwrites it), and nothing here aborts
//! the run or fails a logout — diagnostics go to `tracing`, never the
//! terminal, which this program is drawing on.

// Built ahead of its callers: this stage constructs the file and clears it on
// sign-out, and the reads and writes arrive with the stages that put them in
// the render path and behind the fetches. The expectation fails the build once
// they do, so it cannot outlive its reason.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "loaded and saved by the history-cache stages that follow"
    )
)]

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use domain::history::CONVERSATION_WINDOW;
use domain::message::{MediaKind, Message, MessageStatus};
use serde::{Deserialize, Serialize};

/// How many messages of one conversation the file keeps.
///
/// Exactly one [`CONVERSATION_WINDOW`]: a cache hit fills the window the
/// conversation opens into with no wire follow-up, and keeping more would be
/// rows the window drops on arrival. Named apart from the window rather than
/// reusing it because the two are different promises — that one is memory,
/// this one is disk — and the assertion below is what keeps them equal until
/// someone decides they should not be.
pub(crate) const HISTORY_CACHE_DEPTH: usize = 200;

const _: () = assert!(HISTORY_CACHE_DEPTH == CONVERSATION_WINDOW);

/// The history file beside the configuration.
///
/// Owns its path, which is `config_path.with_extension("history.json")`
/// computed once in `run` and threaded to the loop — passed rather than
/// derived at each use, for the reason [`DraftFile`](crate::draft_store::DraftFile)
/// is: the load, the save and the logout clear cannot disagree about which
/// file they mean.
#[derive(Debug, Clone)]
pub(crate) struct HistoryFile {
    path: PathBuf,
}

/// What [`HistoryFile::load`] found: the stored account tag and the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadedHistory {
    pub account: Option<String>,
    pub cache: HistoryCache,
}

impl LoadedHistory {
    /// Nothing stored, and nothing stored under: a missing file, an
    /// unreadable one, or any other read failure.
    fn empty() -> Self {
        Self {
            account: None,
            cache: HistoryCache::default(),
        }
    }
}

/// The cached messages, per private peer, oldest first.
///
/// Keyed by the bare peer id, the way the drafts are: the product shows only
/// private conversations, and deciding which peers those are is the caller's
/// business — the chat list has already vetted them by the time a window is
/// open. Every peer holds at least one message and at most
/// [`HISTORY_CACHE_DEPTH`], and none of them is a local placeholder; an empty
/// peer is no entry rather than an empty one.
///
/// A `BTreeMap` for the drafts file's reason: the same cache always
/// serialises to the same bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HistoryCache {
    peers: BTreeMap<i64, Vec<CachedMessage>>,
}

impl HistoryCache {
    /// The messages cached for `peer`, oldest first, as the domain's own type.
    ///
    /// Empty for a peer with nothing cached.
    #[must_use]
    pub(crate) fn get(&self, peer: i64) -> Vec<Message> {
        self.peers.get(&peer).map_or_else(Vec::new, |rows| {
            rows.iter().map(|row| row.to_message(peer)).collect()
        })
    }

    /// Replaces what is cached for `peer` with `messages`, oldest first.
    ///
    /// Not a merge: whatever `peer` held before is gone. Three things are
    /// dropped on the way in. Placeholders — an identifier at or below zero is
    /// a send the server has not numbered, which a relaunch cannot confirm or
    /// retry, so caching one would resurrect a message that may never have
    /// gone. Messages naming another conversation, because a message names its
    /// own and the key must not disagree with it. And everything older than
    /// the newest [`HISTORY_CACHE_DEPTH`], so the oldest go first, the way the
    /// window drops its far end.
    pub(crate) fn put<'a>(&mut self, peer: i64, messages: impl IntoIterator<Item = &'a Message>) {
        let mut rows: Vec<CachedMessage> = messages
            .into_iter()
            .filter(|message| message.id > 0 && message.chat_id == peer)
            .map(CachedMessage::from_message)
            .collect();
        let excess = rows.len().saturating_sub(HISTORY_CACHE_DEPTH);
        rows.drain(..excess);

        if rows.is_empty() {
            self.peers.remove(&peer);
        } else {
            self.peers.insert(peer, rows);
        }
    }

    /// Forgets everything cached for `peer`.
    pub(crate) fn remove(&mut self, peer: i64) {
        self.peers.remove(&peer);
    }
}

/// The file payload:
/// `{ "account": <phone|null>, "peers": { "<peer_id>": [<row>, …] } }`.
#[derive(Debug, Serialize, Deserialize)]
struct Payload {
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    peers: BTreeMap<i64, Vec<CachedMessage>>,
}

/// One cached message: what the conversation view draws, and nothing else.
///
/// The peer is the map key rather than a field, so a row cannot name a
/// conversation other than the one it is filed under. The status is not
/// stored either: only numbered messages are cached, and a numbered message
/// is `Sent` or `Received` by its direction alone — the rule `proto` applies
/// to everything that arrives over the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CachedMessage {
    id: i64,
    text: String,
    timestamp: i64,
    is_outgoing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reply_to: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    media: Option<CachedMedia>,
}

impl CachedMessage {
    fn from_message(message: &Message) -> Self {
        Self {
            id: message.id,
            text: message.text.clone().into_owned(),
            timestamp: message.timestamp,
            is_outgoing: message.is_outgoing,
            reply_to: message.reply_to,
            media: message.media.map(CachedMedia::from),
        }
    }

    fn to_message(&self, peer: i64) -> Message {
        Message {
            id: self.id,
            chat_id: peer,
            text: Cow::Owned(self.text.clone()),
            timestamp: self.timestamp,
            status: if self.is_outgoing {
                MessageStatus::Sent
            } else {
                MessageStatus::Received
            },
            is_outgoing: self.is_outgoing,
            reply_to: self.reply_to,
            media: self.media.map(MediaKind::from),
        }
    }
}

/// [`MediaKind`] as the file spells it.
///
/// A mirror rather than a derive on the domain type: `domain` carries no
/// `serde`, and the file's spelling is this module's promise, not the model's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum CachedMedia {
    Photo,
    Video,
    Gif,
    Voice,
    Sticker,
    File,
}

impl From<MediaKind> for CachedMedia {
    fn from(kind: MediaKind) -> Self {
        match kind {
            MediaKind::Photo => Self::Photo,
            MediaKind::Video => Self::Video,
            MediaKind::Gif => Self::Gif,
            MediaKind::Voice => Self::Voice,
            MediaKind::Sticker => Self::Sticker,
            MediaKind::File => Self::File,
        }
    }
}

impl From<CachedMedia> for MediaKind {
    fn from(kind: CachedMedia) -> Self {
        match kind {
            CachedMedia::Photo => Self::Photo,
            CachedMedia::Video => Self::Video,
            CachedMedia::Gif => Self::Gif,
            CachedMedia::Voice => Self::Voice,
            CachedMedia::Sticker => Self::Sticker,
            CachedMedia::File => Self::File,
        }
    }
}

impl HistoryFile {
    /// A handle on the history file at `path`.
    #[must_use]
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Reads the stored cache and the account tag it was saved under.
    ///
    /// A missing file is an empty cache, not an error; corrupt JSON and any
    /// other read failure are warned about and empty too. What a readable
    /// file holds goes through [`HistoryCache::put`], so a hand-edited or
    /// older file is held to the same bounds as one this build wrote: no
    /// placeholders, no peer deeper than [`HISTORY_CACHE_DEPTH`].
    pub(crate) fn load(&self) -> LoadedHistory {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return LoadedHistory::empty();
            }
            Err(error) => {
                tracing::warn!(%error, "history file could not be read");
                return LoadedHistory::empty();
            }
        };

        match serde_json::from_slice::<Payload>(&bytes) {
            Ok(payload) => {
                let mut cache = HistoryCache::default();
                for (peer, rows) in payload.peers {
                    let messages: Vec<Message> =
                        rows.iter().map(|row| row.to_message(peer)).collect();
                    cache.put(peer, &messages);
                }
                LoadedHistory {
                    account: payload.account,
                    cache,
                }
            }
            Err(error) => {
                tracing::warn!(%error, "discarding an unreadable history file");
                LoadedHistory::empty()
            }
        }
    }

    /// Stores `cache` under `account`, atomically.
    ///
    /// Best-effort: a failure is warned about and the run carries on with the
    /// messages still in memory — the file is a head start for the next
    /// launch, and the wire can always refill it.
    pub(crate) fn save(&self, cache: &HistoryCache, account: Option<&str>) {
        let payload = Payload {
            account: account.map(str::to_owned),
            peers: cache.peers.clone(),
        };
        let bytes = match serde_json::to_vec(&payload) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(%error, "history could not be serialised");
                return;
            }
        };

        if let Some(parent) = self.path.parent().filter(|dir| !dir.as_os_str().is_empty())
            && let Err(error) = fs::create_dir_all(parent)
        {
            tracing::warn!(%error, "history directory could not be created");
            return;
        }
        write_atomically(&self.path, &bytes);
    }

    /// Removes the file; a missing file is success.
    ///
    /// Logout calls this, and a failed delete must not fail the logout, so
    /// the failure is a warning and the return is nothing.
    pub(crate) fn clear(&self) {
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(%error, "history file could not be removed"),
        }
    }
}

/// Whether a stored file may seed this launch.
///
/// The drafts rule turned around. For drafts a false clear loses the reader's
/// words, so only a named mismatch discards. Here a false clear costs one
/// fetch the wire was going to make anyway, while a false accept paints
/// another account's conversation under a peer id that happens to be reused —
/// so the file is accepted only when it was saved under exactly the account
/// configured now, unnamed matching unnamed.
pub(crate) fn history_acceptable(stored: Option<&str>, configured: Option<&str>) -> bool {
    stored == configured
}

/// Writes `bytes` so that the target only ever appears as a whole file.
///
/// The drafts file's `write_atomically`, copied with its reasoning: a sibling
/// temp in the same directory, restricted before a byte is in it, renamed over
/// the target — so an interrupted write leaves the previous history intact
/// rather than a truncated file, and the target inherits the temp's `0600`.
fn write_atomically(path: &Path, bytes: &[u8]) {
    let temp = temp_sibling(path);

    let result = (|| -> Result<()> {
        fs::File::create(&temp).context("creating the history temp file")?;
        restrict_permissions(&temp);
        fs::write(&temp, bytes).context("writing the history temp file")?;
        fs::rename(&temp, path).context("replacing the history file")?;
        Ok(())
    })();

    if let Err(error) = result {
        // A half-written temp is not a history file, and nothing would ever
        // clean it up.
        let _ = fs::remove_file(&temp);
        tracing::warn!(%error, "history file could not be written");
    }
}

/// The temp file [`write_atomically`] writes through, tagged with the process
/// id so two processes sharing a history path do not write the same temp.
fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// Restricts a freshly written history file to its owner.
#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;

    if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        tracing::warn!(%error, "could not restrict the permissions of the history file");
    }
}

/// No-op on platforms whose ACLs `std` cannot express portably.
#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    const PEER: i64 = 7;

    /// A history file in a scratch directory, so a run never touches the
    /// developer's own.
    fn scratch(name: &str) -> (tempfile::TempDir, HistoryFile) {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let file = HistoryFile::new(dir.path().join(name));
        (dir, file)
    }

    fn message(id: i64, chat_id: i64, text: &str) -> Message {
        Message {
            id,
            chat_id,
            text: Cow::Owned(text.to_owned()),
            timestamp: id * 10,
            status: MessageStatus::Received,
            is_outgoing: false,
            reply_to: None,
            media: None,
        }
    }

    /// The fields a round trip has to keep, as one comparable value:
    /// `Message` itself has no `PartialEq`.
    fn fields(message: &Message) -> (i64, i64, &str, i64, MessageStatus, bool, Option<i64>) {
        (
            message.id,
            message.chat_id,
            &*message.text,
            message.timestamp,
            message.status,
            message.is_outgoing,
            message.reply_to,
        )
    }

    fn ids(messages: &[Message]) -> Vec<i64> {
        messages.iter().map(|message| message.id).collect()
    }

    #[test]
    fn a_save_round_trips_messages_and_the_account() {
        let (_dir, file) = scratch("history.json");
        let mut sent = message(2, PEER, "");
        sent.is_outgoing = true;
        sent.status = MessageStatus::Sent;
        sent.reply_to = Some(1);
        sent.media = Some(MediaKind::Photo);
        let written = vec![message(1, PEER, "first"), sent];
        let mut cache = HistoryCache::default();
        cache.put(PEER, &written);
        cache.put(30, &[message(9, 30, "elsewhere")]);

        file.save(&cache, Some("+15551234567"));

        let loaded = file.load();
        assert_eq!(loaded.account.as_deref(), Some("+15551234567"));
        assert_eq!(loaded.cache, cache);
        let read = loaded.cache.get(PEER);
        assert_eq!(
            read.iter().map(fields).collect::<Vec<_>>(),
            written.iter().map(fields).collect::<Vec<_>>(),
            "every field the view draws comes back, oldest first"
        );
        assert_eq!(read[1].media, Some(MediaKind::Photo));
        assert_eq!(ids(&loaded.cache.get(30)), vec![9]);
    }

    #[test]
    fn a_missing_file_is_an_empty_cache() {
        let (_dir, file) = scratch("history.json");

        assert_eq!(file.load(), LoadedHistory::empty());
    }

    #[test]
    fn a_corrupt_file_is_forgotten_and_the_next_save_overwrites_it() {
        let (_dir, file) = scratch("history.json");
        fs::write(&file.path, "{ not json").expect("the corrupt file");

        assert_eq!(
            file.load(),
            LoadedHistory::empty(),
            "corrupt JSON loads as nothing, warned about, launch carries on"
        );

        let mut cache = HistoryCache::default();
        cache.put(PEER, &[message(1, PEER, "one")]);
        file.save(&cache, None);
        assert_eq!(
            ids(&file.load().cache.get(PEER)),
            vec![1],
            "the next save overwrites the corrupt file in place"
        );
    }

    #[test]
    fn a_save_leaves_a_whole_file_and_no_temp_behind() {
        let (_dir, file) = scratch("history.json");
        let mut photo = message(1, PEER, "");
        photo.media = Some(MediaKind::Photo);
        let mut cache = HistoryCache::default();
        cache.put(PEER, &[photo]);

        file.save(&cache, Some("+15551234567"));

        let raw = fs::read_to_string(&file.path).expect("the file the save wrote");
        let parsed: serde_json::Value =
            serde_json::from_str(&raw).expect("the target is only ever the finished file");
        let expected: serde_json::Value = serde_json::from_str(
            r#"{"account": "+15551234567", "peers": {"7": [
                {"id": 1, "text": "", "timestamp": 10, "is_outgoing": false, "media": "photo"}
            ]}}"#,
        )
        .expect("the expected payload is JSON");
        assert_eq!(parsed, expected, "the payload shape, media as a kind only");
        assert!(
            !temp_sibling(&file.path).exists(),
            "no half-written temp is left beside the target"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_saved_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt as _;

        let (_dir, file) = scratch("history.json");
        file.save(&HistoryCache::default(), None);

        let mode = fs::metadata(&file.path)
            .expect("the file the save wrote")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn clearing_a_missing_file_succeeds_and_clearing_removes() {
        let (_dir, file) = scratch("history.json");

        file.clear();

        file.save(&HistoryCache::default(), None);
        assert!(file.path.exists());
        file.clear();
        assert!(!file.path.exists());
    }

    #[test]
    fn the_account_rule_accepts_only_the_same_account() {
        assert!(history_acceptable(Some("+1555"), Some("+1555")), "same");
        assert!(history_acceptable(None, None), "unnamed matches unnamed");
        assert!(
            !history_acceptable(Some("+15550000001"), Some("+15550000002")),
            "a named mismatch is someone else's conversations"
        );
        assert!(
            !history_acceptable(None, Some("+1555")),
            "an untagged file may be anyone's: a false clear costs a fetch"
        );
        assert!(
            !history_acceptable(Some("+1555"), None),
            "a tagged file under an unnamed launch may be someone else's"
        );
    }

    #[test]
    fn a_mismatched_file_does_not_seed_the_cache() {
        let (_dir, file) = scratch("history.json");
        let mut cache = HistoryCache::default();
        cache.put(PEER, &[message(1, PEER, "someone else's")]);
        file.save(&cache, Some("+10000000001"));

        let loaded = file.load();
        let seeded = if history_acceptable(loaded.account.as_deref(), Some("+19999999999")) {
            loaded.cache
        } else {
            HistoryCache::default()
        };

        assert!(
            seeded.get(PEER).is_empty(),
            "messages saved under another account never reach the window"
        );
    }

    #[test]
    fn the_depth_cap_evicts_the_oldest() {
        let depth = i64::try_from(HISTORY_CACHE_DEPTH).expect("the depth fits an id");
        let page: Vec<Message> = (1..=depth + 5).map(|id| message(id, PEER, "row")).collect();
        let mut cache = HistoryCache::default();

        cache.put(PEER, &page);

        let kept = ids(&cache.get(PEER));
        assert_eq!(kept.len(), HISTORY_CACHE_DEPTH);
        assert_eq!(
            kept.first(),
            Some(&6),
            "the five oldest went, not the newest"
        );
        assert_eq!(kept.last(), Some(&(depth + 5)));
    }

    #[test]
    fn placeholders_and_other_conversations_never_enter_the_cache() {
        let mut cache = HistoryCache::default();

        cache.put(
            PEER,
            &[
                message(1, PEER, "numbered"),
                message(0, PEER, "zero"),
                message(-3, PEER, "a send in flight"),
                message(2, 99, "another conversation"),
            ],
        );

        assert_eq!(ids(&cache.get(PEER)), vec![1]);
        assert!(cache.get(99).is_empty());
    }

    #[test]
    fn a_put_replaces_and_an_empty_put_or_remove_forgets() {
        let mut cache = HistoryCache::default();
        cache.put(PEER, &[message(1, PEER, "old")]);

        cache.put(PEER, &[message(2, PEER, "new")]);
        assert_eq!(ids(&cache.get(PEER)), vec![2], "replaced, not merged");

        cache.put(PEER, &[message(-1, PEER, "placeholder only")]);
        assert_eq!(
            cache,
            HistoryCache::default(),
            "nothing cacheable is no entry"
        );

        cache.put(PEER, &[message(3, PEER, "again")]);
        cache.remove(PEER);
        assert_eq!(cache, HistoryCache::default());
    }

    /// A hand-written file loads, and is held to the bounds a saved one is:
    /// the placeholder goes, the status comes from the direction, and the
    /// peer from the key.
    #[test]
    fn a_hand_written_file_seeds_the_cache_within_its_bounds() {
        let (_dir, file) = scratch("history.json");
        fs::write(
            &file.path,
            r#"{"account": "+1555", "peers": {"7": [
                {"id": -1, "text": "never sent", "timestamp": 1, "is_outgoing": true},
                {"id": 4, "text": "hello", "timestamp": 40, "is_outgoing": true, "reply_to": 3},
                {"id": 5, "text": "", "timestamp": 50, "is_outgoing": false, "media": "voice"}
            ]}}"#,
        )
        .expect("the hand-written file");

        let loaded = file.load();
        assert!(history_acceptable(loaded.account.as_deref(), Some("+1555")));

        let read = loaded.cache.get(PEER);
        assert_eq!(ids(&read), vec![4, 5]);
        assert_eq!(
            fields(&read[0]),
            (4, PEER, "hello", 40, MessageStatus::Sent, true, Some(3))
        );
        assert_eq!(read[1].status, MessageStatus::Received);
        assert_eq!(read[1].media, Some(MediaKind::Voice));
        assert_eq!(read[1].display_body(), "[voice]");
    }

    /// A hand-written peer deeper than the cap is cut on load, oldest first.
    #[test]
    fn an_over_deep_file_is_cut_to_the_cap_on_load() {
        let (_dir, file) = scratch("history.json");
        let depth = i64::try_from(HISTORY_CACHE_DEPTH).expect("the depth fits an id");
        let rows: Vec<CachedMessage> = (1..=depth + 1)
            .map(|id| CachedMessage::from_message(&message(id, PEER, "row")))
            .collect();
        let payload = Payload {
            account: None,
            peers: BTreeMap::from([(PEER, rows)]),
        };
        fs::write(
            &file.path,
            serde_json::to_vec(&payload).expect("the payload serialises"),
        )
        .expect("the over-deep file");

        let kept = ids(&file.load().cache.get(PEER));
        assert_eq!(kept.len(), HISTORY_CACHE_DEPTH);
        assert_eq!(kept.first(), Some(&2));
    }
}
