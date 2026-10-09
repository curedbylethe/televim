//! Unsent words on disk: the drafts file beside the configuration.
//!
//! `tui` owns the drafts and their snapshot shape (`Vec<(i64, String)>`,
//! plain peer id plus text — no cursor, no undo, no purpose) and it owns no
//! filesystem, so the file lives here, in the crate that owns the
//! configuration path. The payload carries the account the words were written
//! under, so a launch for another account discards them rather than seeding
//! its bars with a stranger's sentences.
//!
//! Every operation is best-effort and launch-safe: a missing file is an empty
//! launch, an unreadable one is warned about and forgotten (the next sync
//! overwrites it), and nothing here aborts the run or fails a logout —
//! diagnostics go to `tracing`, never the terminal, which this program is
//! drawing on.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// The drafts file beside the configuration.
///
/// Owns its path, which is `config_path.with_extension("drafts.json")`
/// computed once in `run` and threaded to the loop — passed rather than
/// derived at each use, so the load, the sync and the logout clear cannot
/// disagree about which file they mean (the same reason the log path is
/// passed, not derived).
#[derive(Debug, Clone)]
pub(crate) struct DraftFile {
    path: PathBuf,
}

/// What [`DraftFile::load`] found: the stored account tag, the drafts and the
/// read marks.
///
/// The account is `cfg.phone`, the only account identity `app` owns at
/// launch; `None` is a file from before the tag existed, or a launch with no
/// phone configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadedDrafts {
    pub account: Option<String>,
    pub drafts: Vec<(i64, String)>,
    /// `(peer id, highest id the peer has read)`, oldest first: the order
    /// the file holds them, which is the recency order. A file in the older
    /// object form has no order, so its marks come back in map order.
    pub reads: Vec<(i64, i64)>,
}

impl LoadedDrafts {
    /// Nothing stored, and nothing stored under: a missing file, an
    /// unreadable one, or any other read failure.
    fn empty() -> Self {
        Self {
            account: None,
            drafts: Vec::new(),
            reads: Vec::new(),
        }
    }
}

/// The file payload:
/// `{ "account": <phone|null>, "drafts": { "<peer_id>": "<text>" }, "read": [[<peer_id>, <max_id>], ...] }`.
///
/// `read` lists the marks oldest first, so the recency order survives the
/// round trip into `DraftStore::restore_read_marks`. It is
/// omitted when empty, so a file with no read marks keeps the shape it had
/// before they were stored, and an older file without the key loads with none.
///
/// A file written before this array form stored `read` as an object
/// (`{ "<peer_id>": <max_id> }`); that still loads, in map order. The `read`
/// key is read leniently: anything that is not a pair of integers is dropped,
/// so a damaged marks key never costs the drafts beside it.
///
/// `drafts` stays a `BTreeMap`, so the same drafts always serialise to the
/// same bytes: iteration order is peer id order, and a sync that changed
/// nothing writes nothing different.
#[derive(Debug, Serialize, Deserialize)]
struct Payload {
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    drafts: BTreeMap<i64, String>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "lenient_reads"
    )]
    read: Vec<(i64, i64)>,
}

/// Reads the `read` key without letting it fail the whole payload.
fn lenient_reads<'de, D>(deserializer: D) -> Result<Vec<(i64, i64)>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(read_pairs(&value))
}

/// The marks in a `read` value: the array form in the order written, or the
/// older object form in map order. Any other shape, and any entry that is
/// not a pair of integers, contributes nothing.
fn read_pairs(value: &Value) -> Vec<(i64, i64)> {
    match value {
        Value::Array(items) => items
            .iter()
            .filter_map(|item| match item.as_array()?.as_slice() {
                [peer, max] => Some((peer.as_i64()?, max.as_i64()?)),
                _ => None,
            })
            .collect(),
        Value::Object(map) => map
            .iter()
            .filter_map(|(peer, max)| Some((peer.parse().ok()?, max.as_i64()?)))
            .collect(),
        _ => Vec::new(),
    }
}

impl DraftFile {
    /// A handle on the drafts file at `path`.
    #[must_use]
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Reads the stored drafts and the account tag they were saved under.
    ///
    /// A missing file is an empty launch, not an error. Corrupt JSON is
    /// warned about and forgotten — the file is left for the next sync to
    /// overwrite, the discard discipline `discard_corrupt_session` keeps for
    /// sessions. Any other read failure is warned about and empty too: a
    /// launch that cannot read drafts still launches.
    pub(crate) fn load(&self) -> LoadedDrafts {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return LoadedDrafts::empty(),
            Err(error) => {
                tracing::warn!(%error, "drafts file could not be read");
                return LoadedDrafts::empty();
            }
        };

        match serde_json::from_slice::<Payload>(&bytes) {
            Ok(payload) => LoadedDrafts {
                account: payload.account,
                drafts: payload.drafts.into_iter().collect(),
                reads: payload.read,
            },
            Err(error) => {
                tracing::warn!(%error, "discarding an unreadable drafts file");
                LoadedDrafts::empty()
            }
        }
    }

    /// Stores `drafts` and `reads` under `account`, atomically.
    ///
    /// Best-effort: a serialisation or write failure is warned about and the
    /// loop carries on with the words still in memory — losing the file is
    /// not losing the drafts, and must not take the run down with it.
    pub(crate) fn save(
        &self,
        drafts: &[(i64, String)],
        reads: &[(i64, i64)],
        account: Option<&str>,
    ) {
        let payload = Payload {
            account: account.map(str::to_owned),
            drafts: drafts
                .iter()
                .map(|(peer_id, text)| (*peer_id, text.clone()))
                .collect(),
            read: reads.to_vec(),
        };
        let bytes = match serde_json::to_vec(&payload) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(%error, "drafts could not be serialised");
                return;
            }
        };

        if let Some(parent) = self.path.parent().filter(|dir| !dir.as_os_str().is_empty())
            && let Err(error) = fs::create_dir_all(parent)
        {
            tracing::warn!(%error, "drafts directory could not be created");
            return;
        }
        write_atomically(&self.path, &bytes);
    }

    /// Removes the file; a missing file is success.
    ///
    /// Best-effort like everything else here: logout calls this and a failed
    /// delete must not fail the logout, so the failure is a warning and the
    /// return is nothing.
    pub(crate) fn clear(&self) {
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(%error, "drafts file could not be removed"),
        }
    }
}

/// Whether a stored file may seed this launch.
///
/// Discard only when both sides name an account and they differ: a false
/// clear loses the reader's words, the harm this file exists to prevent,
/// while a false accept shows stale text from the reader's own other
/// account — corrected by the ordinary park/resume flow on the next switch.
pub(crate) fn drafts_acceptable(stored: Option<&str>, configured: Option<&str>) -> bool {
    match (stored, configured) {
        (Some(stored), Some(configured)) => stored == configured,
        _ => true,
    }
}

/// Writes `bytes` so that the target only ever appears as a whole file.
///
/// The session store's `write_atomically` shape, copied rather than reused:
/// that helper is private to its module and `FileStore::save` takes a
/// `&SessionData`, so sharing it would mean widening a security-sensitive
/// API for drafts — and this crate is the one allowed to see everything, so
/// the ~30-line duplication lands here.
///
/// The bytes go to a sibling temp file in the *same* directory and the temp
/// is then renamed over the target. An interrupted write — a kill, a full
/// disk, a permission refusal — therefore leaves the previous drafts intact
/// instead of a truncated file, because the target is only ever replaced by
/// the finished file. The temp has to be a rename rather than a copy for
/// that, and it has to be in the same directory because a cross-filesystem
/// rename is not atomic and would degrade to a non-atomic copy.
fn write_atomically(path: &Path, bytes: &[u8]) {
    let temp = temp_sibling(path);

    let result = (|| -> Result<()> {
        // Created empty and restricted *before* a byte is in it. A plain
        // `fs::write` leaves a window between creating the file and the
        // `chmod`, and what is in that window is the reader's unsent words
        // at whatever the umask allowed.
        fs::File::create(&temp).context("creating the drafts temp file")?;
        restrict_permissions(&temp);
        fs::write(&temp, bytes).context("writing the drafts temp file")?;
        // A rename keeps the source inode, so the target inherits the mode
        // the temp was restricted to rather than being a fresh, umask-behind
        // file.
        fs::rename(&temp, path).context("replacing the drafts file")?;
        Ok(())
    })();

    if result.is_err() {
        // A half-written temp is not a drafts file, and nothing would ever
        // clean it up.
        let _ = fs::remove_file(&temp);
    }
    if let Err(error) = result {
        tracing::warn!(%error, "drafts file could not be written");
    }
}

/// The temp file [`write_atomically`] writes through.
///
/// A sibling, tagged with the process id so two processes sharing a drafts
/// path do not write the same temp.
fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// Restricts a freshly written drafts file to its owner.
#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;

    if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        tracing::warn!(%error, "could not restrict the permissions of the drafts file");
    }
}

/// No-op on platforms whose ACLs `std` cannot express portably.
#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A drafts file in a scratch directory, so a run never touches the
    /// developer's own — the `FileStore` tests' `tempdir().join` pattern.
    fn scratch(name: &str) -> (tempfile::TempDir, DraftFile) {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let file = DraftFile::new(dir.path().join(name));
        (dir, file)
    }

    #[test]
    fn a_save_round_trips_drafts_and_the_account() {
        let (_dir, file) = scratch("drafts.json");
        let drafts = vec![(30, "thirty".to_owned()), (7, "seven".to_owned())];

        file.save(&drafts, &[], Some("+15551234567"));

        let loaded = file.load();
        assert_eq!(loaded.account.as_deref(), Some("+15551234567"));
        assert_eq!(
            loaded.drafts,
            vec![(7, "seven".to_owned()), (30, "thirty".to_owned())],
            "and the pairs come back sorted by peer id"
        );
    }

    #[test]
    fn a_missing_file_is_an_empty_launch() {
        let (_dir, file) = scratch("drafts.json");

        assert_eq!(file.load(), LoadedDrafts::empty());
    }

    #[test]
    fn a_corrupt_file_is_forgotten_and_the_next_save_overwrites_it() {
        let (_dir, file) = scratch("drafts.json");
        fs::write(&file.path, "{ not json").expect("the corrupt file");

        assert_eq!(
            file.load(),
            LoadedDrafts::empty(),
            "corrupt JSON loads as nothing, warned about, launch carries on"
        );

        file.save(&[(7, "seven".to_owned())], &[], None);
        assert_eq!(
            file.load().drafts,
            vec![(7, "seven".to_owned())],
            "the next sync overwrites the corrupt file in place"
        );
    }

    #[test]
    fn a_save_leaves_a_whole_file_and_no_temp_behind() {
        let (_dir, file) = scratch("drafts.json");

        file.save(&[(7, "seven".to_owned())], &[], Some("+15551234567"));

        let raw = fs::read_to_string(&file.path).expect("the file the save wrote");
        let parsed: serde_json::Value =
            serde_json::from_str(&raw).expect("the target is only ever the finished file");
        let expected: serde_json::Value =
            serde_json::from_str(r#"{"account": "+15551234567", "drafts": {"7": "seven"}}"#)
                .expect("the expected payload is JSON");
        assert_eq!(parsed, expected, "the payload shape STAGE-02 promises");
        assert!(
            temp_gone(&file),
            "no half-written temp is left beside the target"
        );
    }

    /// No sibling temp survives a save: the rename consumed it, and a failed
    /// write removes it.
    fn temp_gone(file: &DraftFile) -> bool {
        let temp = temp_sibling(&file.path);
        !temp.exists()
    }

    #[test]
    fn clearing_a_missing_file_succeeds_and_clearing_removes() {
        let (_dir, file) = scratch("drafts.json");

        file.clear();

        file.save(&[(7, "seven".to_owned())], &[], None);
        assert!(file.path.exists());
        file.clear();
        assert!(!file.path.exists());
    }

    #[test]
    fn the_account_rule_discards_only_a_named_mismatch() {
        assert!(
            !drafts_acceptable(Some("+15550000001"), Some("+15550000002")),
            "both sides name an account and they differ: discard"
        );
        assert!(
            drafts_acceptable(Some("+1555"), Some("+1555")),
            "same: accept"
        );
        assert!(
            drafts_acceptable(None, Some("+1555")),
            "no stored tag: accept — a false clear loses words"
        );
        assert!(
            drafts_acceptable(Some("+1555"), None),
            "no configured phone: accept — the keyring path carries none"
        );
        assert!(drafts_acceptable(None, None), "neither: accept");
    }

    // ---- the seam: STAGE-01 snapshot plus this store, no terminal ---------

    use std::borrow::Cow;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use domain::chat::{Chat, ChatKind};
    use domain::message::{Message, MessageStatus};
    use tui::app::{App, Focus, PromptKind};

    /// An application with one conversation open and two messages loaded, the
    /// shape `net`'s own tests build — no terminal, no network.
    fn app_with_a_conversation(chat_id: i64) -> App {
        let mut app = App::new();
        app.set_chats(vec![Chat {
            read_outbox_max_id: None,
            id: chat_id,
            title: format!("chat-{chat_id}"),
            kind: ChatKind::Private,
            last_message: None,
            unread_count: 0,
            last_message_id: None,
            last_timestamp: None,
            pinned: false,
            presence: None,
            deleted: false,
        }]);
        app.select_chat(0);
        app.apply_latest(vec![
            Message {
                id: 1,
                chat_id,
                text: Cow::Borrowed("first"),
                timestamp: 1,
                status: MessageStatus::Received,
                is_outgoing: false,
                reply_to: None,
                media: None,
                media_id: None,
            },
            Message {
                id: 2,
                chat_id,
                text: Cow::Borrowed("second"),
                timestamp: 2,
                status: MessageStatus::Received,
                is_outgoing: false,
                reply_to: None,
                media: None,
                media_id: None,
            },
        ]);
        app
    }

    /// A hand-written file loads through `load` and `restore`, and the map
    /// holds the words: the relaunch half of the kill test, without the kill.
    #[test]
    fn a_hand_written_file_seeds_the_store_through_load_and_restore() {
        let (_dir, file) = scratch("drafts.json");
        fs::write(
            &file.path,
            r#"{"account": "+1555", "drafts": {"7": "mid-sentence"}}"#,
        )
        .expect("the hand-written file");

        let loaded = file.load();
        assert!(drafts_acceptable(loaded.account.as_deref(), Some("+1555")));

        let mut app = app_with_a_conversation(7);
        app.drafts.restore(loaded.drafts);
        assert_eq!(
            app.drafts.snapshot(None),
            vec![(7, "mid-sentence".to_owned())],
            "the words are back in the map after the relaunch"
        );
    }

    /// Snapshot, save, submit, re-snapshot, save, re-load: the entry is gone
    /// from the re-read file. STAGE-01's queue-time removal plus this store's
    /// next-tick sync, driven through the public key path.
    #[test]
    fn a_sent_line_leaves_the_file_on_the_next_sync() {
        let (_dir, file) = scratch("drafts.json");
        let mut app = app_with_a_conversation(7);
        app.drafts.restore(vec![(7, "parked".to_owned())]);
        app.input
            .line
            .open_with(PromptKind::Message, "sending this".to_owned());
        app.ui.focus = Focus::Input;

        let snapshot = app.drafts.snapshot(Some((7, &app.input.line)));
        file.save(&snapshot, &[], None);
        assert_eq!(
            file.load().drafts,
            vec![(7, "sending this".to_owned())],
            "the live line is what the tick before the send wrote"
        );

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let snapshot = app.drafts.snapshot(Some((7, &app.input.line)));
        file.save(&snapshot, &[], None);
        assert_eq!(
            file.load(),
            LoadedDrafts::empty(),
            "the submit cleared the map entry, so the next sync wrote nothing left"
        );
        assert!(
            app.take_action().is_some(),
            "and the send itself was queued, not swallowed by the test"
        );
    }

    #[test]
    fn a_mismatched_file_does_not_seed_the_store() {
        let (_dir, file) = scratch("drafts.json");
        file.save(
            &[(7, "someone else's".to_owned())],
            &[],
            Some("+10000000001"),
        );

        let loaded = file.load();
        let mut app = app_with_a_conversation(7);
        if drafts_acceptable(loaded.account.as_deref(), Some("+19999999999")) {
            app.drafts.restore(loaded.drafts);
        }

        assert_eq!(
            app.drafts.snapshot(None),
            Vec::new(),
            "words written under another account never reach the bars"
        );
    }

    #[test]
    fn read_marks_round_trip_beside_the_drafts() {
        let (_dir, file) = scratch("drafts.json");

        file.save(
            &[(7, "seven".to_owned())],
            &[(30, 4), (7, 9)],
            Some("+15551234567"),
        );

        let loaded = file.load();
        assert_eq!(loaded.drafts, vec![(7, "seven".to_owned())]);
        assert_eq!(
            loaded.reads,
            vec![(30, 4), (7, 9)],
            "the marks come back in the order they were saved, oldest first, \
             not sorted by peer id: the order is the recency"
        );
    }

    #[test]
    fn read_marks_are_written_as_an_ordered_array() {
        let (_dir, file) = scratch("drafts.json");

        file.save(&[], &[(9, 5), (4, 7), (2, 3)], None);

        let raw: Value = serde_json::from_str(&fs::read_to_string(&file.path).expect("the file"))
            .expect("the payload is JSON");
        assert_eq!(
            raw["read"],
            serde_json::from_str::<Value>("[[9, 5], [4, 7], [2, 3]]")
                .expect("the expected payload is JSON"),
            "the persisted form is an array of pairs, oldest first"
        );

        let loaded = file.load();
        assert_eq!(
            loaded.reads,
            vec![(9, 5), (4, 7), (2, 3)],
            "and the order survives the load, which an object's would not"
        );
    }

    #[test]
    fn an_object_form_file_still_loads_its_marks_and_drafts() {
        let (_dir, file) = scratch("drafts.json");
        fs::write(
            &file.path,
            r#"{"account": "+1555", "drafts": {"7": "seven"}, "read": {"30": 4, "7": 9}}"#,
        )
        .expect("the object-form file");

        let loaded = file.load();
        assert_eq!(loaded.account.as_deref(), Some("+1555"));
        assert_eq!(loaded.drafts, vec![(7, "seven".to_owned())]);
        let mut reads = loaded.reads.clone();
        reads.sort_unstable();
        assert_eq!(
            reads,
            vec![(7, 9), (30, 4)],
            "the older object form loads both marks; its order is map order, not stored"
        );
    }

    #[test]
    fn a_damaged_read_key_never_costs_the_drafts() {
        let (_dir, file) = scratch("drafts.json");
        fs::write(
            &file.path,
            r#"{"drafts": {"7": "seven"}, "read": [[30, 4], "junk", [1], [2, "x"], [5, 6, 7]]}"#,
        )
        .expect("the damaged file");

        let loaded = file.load();
        assert_eq!(
            loaded.drafts,
            vec![(7, "seven".to_owned())],
            "the drafts survive a read key that is not a list of pairs"
        );
        assert_eq!(
            loaded.reads,
            vec![(30, 4)],
            "and only the well-formed pair is kept"
        );

        fs::write(
            &file.path,
            r#"{"drafts": {"7": "seven"}, "read": "not a map or list"}"#,
        )
        .expect("the wrong-shape file");
        let loaded = file.load();
        assert_eq!(loaded.drafts, vec![(7, "seven".to_owned())]);
        assert!(
            loaded.reads.is_empty(),
            "a read key of the wrong shape is no marks"
        );
    }

    #[test]
    fn a_file_from_before_the_marks_loads_with_none() {
        let (_dir, file) = scratch("drafts.json");
        fs::write(
            &file.path,
            r#"{"account": "+1555", "drafts": {"7": "seven"}}"#,
        )
        .expect("the older file");

        assert!(file.load().reads.is_empty(), "no read key, no marks");
    }

    #[cfg(unix)]
    #[test]
    fn the_drafts_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let (_dir, file) = scratch("drafts.json");
        file.save(&[(7, "seven".to_owned())], &[(7, 9)], Some("+1555"));

        let mode = fs::metadata(&file.path)
            .expect("the file the save wrote")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "words and marks are the reader's own");
    }

    #[test]
    fn a_failed_write_leaves_the_previous_file_whole() {
        let (_dir, file) = scratch("drafts.json");
        file.save(&[(7, "first".to_owned())], &[(7, 4)], None);

        // A directory where the temp file would go makes the write fail before
        // the target is touched.
        fs::create_dir(temp_sibling(&file.path)).expect("a blocking directory");
        file.save(&[(7, "second".to_owned())], &[(7, 9)], None);

        let loaded = file.load();
        assert_eq!(
            loaded.drafts,
            vec![(7, "first".to_owned())],
            "the previous drafts stand, not a truncated file"
        );
        assert_eq!(
            loaded.reads,
            vec![(7, 4)],
            "and the previous marks with them"
        );
    }

    #[test]
    fn a_mark_stored_for_another_account_never_seeds_the_store() {
        let (_dir, file) = scratch("drafts.json");
        file.save(&[], &[(7, 9)], Some("+10000000001"));

        let loaded = file.load();
        let mut app = App::new();
        if drafts_acceptable(loaded.account.as_deref(), Some("+19999999999")) {
            app.drafts.restore_read_marks(loaded.reads);
        }

        assert!(
            app.drafts.recent_read_marks(32).is_empty(),
            "another account's read positions never reach the store"
        );
    }
}
