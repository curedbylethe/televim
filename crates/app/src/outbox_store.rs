//! Queued and failed text sends persisted beside the configuration.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tui::{App, OutboxSend};

use crate::draft_store::{drafts_acceptable, write_atomically};

const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OutboxEntry {
    pub(crate) chat_id: i64,
    pub(crate) temp_id: i64,
    pub(crate) text: String,
    pub(crate) reply_to: Option<i64>,
    pub(crate) failure: Option<String>,
}

impl From<OutboxSend> for OutboxEntry {
    fn from(send: OutboxSend) -> Self {
        Self {
            chat_id: send.chat_id,
            temp_id: send.temp_id,
            text: send.text,
            reply_to: send.reply_to,
            failure: send.failure,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LoadedOutbox {
    pub(crate) account: Option<String>,
    pub(crate) sends: Vec<OutboxEntry>,
}

#[derive(Debug)]
pub(crate) struct OutboxFile {
    path: PathBuf,
    foreign: Cell<bool>,
}

#[derive(Serialize)]
struct Payload<'a> {
    version: u32,
    account: Option<&'a str>,
    sends: &'a [OutboxEntry],
}

#[derive(Deserialize)]
struct RawPayload {
    version: u32,
    #[serde(default)]
    account: Option<String>,
    sends: Vec<Value>,
}

impl OutboxFile {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            foreign: Cell::new(false),
        }
    }

    pub(crate) fn foreign(&self) -> bool {
        self.foreign.get()
    }

    pub(crate) fn load(&self) -> LoadedOutbox {
        self.foreign.set(false);
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return LoadedOutbox::default();
            }
            Err(error) => {
                tracing::warn!(%error, "outbox file could not be read");
                return LoadedOutbox::default();
            }
        };
        if let Ok(value) = serde_json::from_slice::<Value>(&bytes)
            && let Some(version) = value.get("version").and_then(Value::as_u64)
            && version > u64::from(VERSION)
        {
            self.foreign.set(true);
            tracing::warn!(version, "outbox file was written by a newer version");
            return LoadedOutbox::default();
        }
        let payload = match serde_json::from_slice::<RawPayload>(&bytes) {
            Ok(payload) => payload,
            Err(error) => {
                tracing::warn!(%error, "discarding an unreadable outbox file");
                return LoadedOutbox::default();
            }
        };
        if payload.version != VERSION {
            tracing::warn!("discarding an outbox file with an unsupported version");
            return LoadedOutbox::default();
        }
        let mut seen = BTreeSet::new();
        let sends = payload
            .sends
            .into_iter()
            .filter_map(|value| {
                let entry = serde_json::from_value::<OutboxEntry>(value).ok();
                match entry {
                    Some(entry)
                        if entry.chat_id != 0
                            && entry.temp_id < 0
                            && entry.temp_id > i64::MIN + 1
                            && !entry.text.trim().is_empty()
                            && seen.insert((entry.chat_id, entry.temp_id)) =>
                    {
                        Some(entry)
                    }
                    _ => {
                        tracing::warn!("discarding an invalid or duplicate outbox entry");
                        None
                    }
                }
            })
            .collect();
        LoadedOutbox {
            account: payload.account,
            sends,
        }
    }

    /// Returns whether the snapshot reached disk, so a failed write can be retried.
    pub(crate) fn save(&self, sends: &[OutboxEntry], account: Option<&str>) -> bool {
        if self.foreign() {
            return false;
        }
        if sends.is_empty() {
            return self.clear();
        }
        let bytes = match serde_json::to_vec(&Payload {
            version: VERSION,
            account,
            sends,
        }) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(%error, "outbox could not be serialised");
                return false;
            }
        };
        if let Some(parent) = self.path.parent().filter(|dir| !dir.as_os_str().is_empty())
            && let Err(error) = fs::create_dir_all(parent)
        {
            tracing::warn!(%error, "outbox directory could not be created");
            return false;
        }
        write_atomically(&self.path, &bytes)
    }

    pub(crate) fn clear(&self) -> bool {
        if self.foreign() {
            return false;
        }
        match fs::remove_file(&self.path) {
            Ok(()) => true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => true,
            Err(error) => {
                tracing::warn!(%error, "outbox file could not be removed");
                false
            }
        }
    }
}

pub(crate) fn outbox_acceptable(stored: Option<&str>, configured: Option<&str>) -> bool {
    drafts_acceptable(stored, configured)
}

pub(crate) fn restore_outbox(app: &mut App, file: &OutboxFile, label: Option<&str>) {
    let loaded = file.load();
    if file.foreign() {
        return;
    }
    if !outbox_acceptable(loaded.account.as_deref(), label) {
        file.foreign.set(true);
        tracing::warn!("outbox stored for another account was ignored");
        return;
    }
    app.install_sends(
        loaded
            .sends
            .into_iter()
            .map(|entry| OutboxSend {
                chat_id: entry.chat_id,
                temp_id: entry.temp_id,
                text: entry.text,
                reply_to: entry.reply_to,
                failure: entry.failure,
                sending: false,
            })
            .collect(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> (tempfile::TempDir, OutboxFile) {
        let dir = tempfile::tempdir().expect("scratch directory");
        let file = OutboxFile::new(dir.path().join("televim.outbox.json"));
        (dir, file)
    }

    fn entry(temp_id: i64) -> OutboxEntry {
        OutboxEntry {
            chat_id: 7,
            temp_id,
            text: format!("unsent {temp_id}"),
            reply_to: Some(12),
            failure: None,
        }
    }

    #[test]
    fn queued_and_failed_sends_round_trip_in_insertion_order() {
        let (dir, file) = scratch();
        let mut failed = entry(-2);
        failed.failure = Some("network down".to_owned());
        let sends = vec![entry(-1), failed];
        assert!(file.save(&sends, Some("account")));
        assert_eq!(
            file.load(),
            LoadedOutbox {
                account: Some("account".to_owned()),
                sends,
            }
        );
        assert_eq!(fs::read_dir(dir.path()).expect("directory").count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(&file.path)
                    .expect("stored file")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn missing_corrupt_and_empty_files_load_empty() {
        let (_dir, file) = scratch();
        assert_eq!(file.load(), LoadedOutbox::default());
        for bytes in [b"{".as_slice(), b"", br#"{"version":1,"sends":[]}"#] {
            fs::write(&file.path, bytes).expect("fixture");
            assert_eq!(file.load(), LoadedOutbox::default());
            assert!(!file.foreign());
        }
    }

    #[test]
    fn future_file_is_neither_seeded_overwritten_nor_removed() {
        let (_dir, file) = scratch();
        let bytes = br#"{"version":2,"sends":"new format"}"#;
        fs::write(&file.path, bytes).expect("future fixture");
        assert_eq!(file.load(), LoadedOutbox::default());
        assert!(file.foreign());
        assert!(!file.save(&[entry(-1)], None));
        assert!(!file.clear());
        assert_eq!(fs::read(&file.path).expect("future file remains"), bytes);
    }

    #[test]
    fn missing_zero_negative_and_malformed_versions_are_rejected() {
        let (_dir, file) = scratch();
        for version in [
            Value::Null,
            Value::from(0),
            Value::from(-1),
            Value::from("1"),
        ] {
            let mut payload = serde_json::to_value(Payload {
                version: VERSION,
                account: None,
                sends: &[entry(-1)],
            })
            .expect("fixture");
            if version.is_null() {
                payload.as_object_mut().expect("object").remove("version");
            } else {
                payload["version"] = version;
            }
            fs::write(&file.path, serde_json::to_vec(&payload).expect("fixture")).expect("fixture");
            assert_eq!(file.load(), LoadedOutbox::default());
        }
    }

    #[test]
    fn invalid_and_duplicate_entries_are_dropped_without_reordering_valid_sends() {
        let (_dir, file) = scratch();
        let mut invalid = entry(-2);
        invalid.chat_id = 0;
        let mut blank = entry(-3);
        blank.text = "  ".to_owned();
        let entries = vec![
            serde_json::to_value(entry(-1)).expect("entry"),
            serde_json::to_value(invalid).expect("entry"),
            serde_json::to_value(entry(i64::MIN)).expect("entry"),
            serde_json::to_value(entry(i64::MIN + 1)).expect("entry"),
            serde_json::to_value(entry(1)).expect("entry"),
            serde_json::to_value(blank).expect("entry"),
            serde_json::to_value(entry(-1)).expect("duplicate entry"),
            Value::String("invalid".to_owned()),
            serde_json::to_value(entry(-4)).expect("entry"),
        ];
        let payload = Value::Object(serde_json::Map::from_iter([
            ("version".to_owned(), Value::from(VERSION)),
            ("sends".to_owned(), Value::Array(entries)),
        ]));
        fs::write(&file.path, serde_json::to_vec(&payload).expect("fixture")).expect("fixture");
        assert_eq!(file.load().sends, vec![entry(-1), entry(-4)]);
    }

    #[test]
    fn empty_save_and_clear_remove_the_file() {
        let (_dir, file) = scratch();
        assert!(file.save(&[entry(-1)], None));
        assert!(file.save(&[], None));
        assert!(!file.path.exists());
        assert!(file.clear());
    }

    #[test]
    fn failed_write_preserves_previous_file_and_removes_temp() {
        let (dir, file) = scratch();
        assert!(file.save(&[entry(-1)], None));
        let original = fs::read(&file.path).expect("original");
        let mut temp_name = file.path.file_name().expect("name").to_os_string();
        temp_name.push(format!(".{}.tmp", std::process::id()));
        let temp_path = dir.path().join(temp_name);
        fs::create_dir(&temp_path).expect("block temp creation");
        assert!(!file.save(&[entry(-2)], None));
        assert_eq!(fs::read(&file.path).expect("original remains"), original);
        fs::remove_dir(&temp_path).expect("remove blocker");
        assert!(file.save(&[entry(-2)], None));
        assert!(!temp_path.exists());
    }

    #[test]
    fn failed_rename_removes_temporary_file() {
        let (dir, file) = scratch();
        fs::create_dir(&file.path).expect("block rename");
        assert!(!file.save(&[entry(-1)], None));
        assert_eq!(fs::read_dir(dir.path()).expect("directory").count(), 1);
    }

    #[test]
    fn account_acceptance_matches_the_drafts_policy() {
        assert!(outbox_acceptable(None, None));
        assert!(outbox_acceptable(Some("7"), None));
        assert!(outbox_acceptable(None, Some("7")));
        assert!(outbox_acceptable(Some("7"), Some("7")));
        assert!(!outbox_acceptable(Some("7"), Some("8")));
    }

    #[test]
    fn restore_ignores_and_preserves_another_accounts_file() {
        let (_dir, file) = scratch();
        assert!(file.save(&[entry(-1)], Some("7")));
        let original = fs::read(&file.path).expect("original");
        let mut app = App::new();
        restore_outbox(&mut app, &file, Some("8"));
        assert!(app.snapshot_sends().is_empty());
        assert!(file.foreign());
        assert!(!file.save(&[], Some("8")));
        assert_eq!(fs::read(&file.path).expect("original remains"), original);
    }

    #[test]
    fn restore_installs_queued_and_failed_entries_for_the_same_account() {
        let (_dir, file) = scratch();
        let mut failed = entry(-2);
        failed.failure = Some("network down".to_owned());
        assert!(file.save(&[entry(-1), failed], Some("7")));
        let mut app = App::new();
        restore_outbox(&mut app, &file, Some("7"));
        let sends = app.snapshot_sends();
        assert_eq!(sends.len(), 2);
        assert_eq!(sends[0].temp_id, -1);
        assert_eq!(sends[1].failure.as_deref(), Some("network down"));
        assert!(sends.iter().all(|send| !send.sending));
    }
}
