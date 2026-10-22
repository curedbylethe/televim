//! Account-scoped paths, the accounts index, and the store the session goes in.
//!
//! One resolver, so the files a run writes, the keyring entry it uses and the
//! description the profile panel shows cannot disagree about which account they
//! belong to.
//!
//! The legacy slot is the layout every install has today — `session` in the
//! credential store, `<stem>.drafts.json` beside the configuration — and stays
//! the first account's. [`AccountSlot::Id`] is an account added later, and no
//! existing bytes move when one appears.
//!
//! [`AccountsIndex`] is a launch hint and nothing more: it says which slot to
//! open, never who is signed in. The session's own tag is authoritative, which
//! is why the index is written only after a `Ready` names an account.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use telegram_framework::{
    FileStore, KeyProvider, KeyringStore, SessionData, SessionError, SessionStore,
};

use crate::config::Config;

/// Which account's bytes a run uses.
///
/// `Legacy` is the slot every install has today; `Id` is an account added after
/// the index existed. `Default` is `Legacy` because that is the layout a launch
/// with no index and a test's state both resolve to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum AccountSlot {
    #[default]
    Legacy,
    Id(i64),
}

impl AccountSlot {
    /// The slot an account argument names.
    fn of(account: Option<i64>) -> Self {
        account.map_or(Self::Legacy, Self::Id)
    }

    /// The account, `None` for the legacy slot.
    pub(crate) fn account(self) -> Option<i64> {
        match self {
            Self::Legacy => None,
            Self::Id(id) => Some(id),
        }
    }

    /// The tag every artefact of this slot is written under: the legacy slot
    /// keeps the configured phone, an id slot is its decimal id.
    ///
    /// One rule for drafts, history and media, so an upgrade does not read the
    /// first account's bytes as another account's, and a later account cannot
    /// read or clear them.
    pub(crate) fn label(self, cfg: &Config) -> Option<String> {
        match self {
            Self::Legacy => cfg.phone.clone(),
            Self::Id(id) => Some(id.to_string()),
        }
    }
}

/// Every path one account owns.
pub(crate) struct SlotPaths {
    /// The account whose paths these are.
    pub(crate) slot: AccountSlot,

    pub(crate) drafts: PathBuf,
    pub(crate) history: PathBuf,
    pub(crate) media: PathBuf,
    pub(crate) session: SessionTarget,

    /// What the profile panel says the session is. Derived here, beside the
    /// target, so the panel and the store cannot name different things.
    pub(crate) description: tui::SessionStore,
}

impl SlotPaths {
    /// The tag this slot's drafts, history and media are written under:
    /// [`AccountSlot::label`], asked once by the launch and handed down.
    pub(crate) fn label(&self, cfg: &Config) -> Option<String> {
        self.slot.label(cfg)
    }
}

/// Where the session goes.
#[derive(Clone)]
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

    let session = match (&cfg.session_path, AccountSlot::of(account)) {
        (None, slot) => SessionTarget::Keyring(session_name(slot)),
        (Some(path), AccountSlot::Legacy) => SessionTarget::File(path.clone()),
        (Some(path), AccountSlot::Id(id)) => SessionTarget::File(suffixed_session(path, id)),
    };

    let description = match &session {
        SessionTarget::Keyring(_) => tui::SessionStore::Keyring,
        SessionTarget::File(path) => tui::SessionStore::EncryptedFile(path.clone()),
    };

    SlotPaths {
        slot: AccountSlot::of(account),
        drafts,
        history,
        media,
        session,
        description,
    }
}

/// The credential store entry a slot's session is kept under.
///
/// The one place the name is spelled, so the resolver, the description and
/// [`SlotStore`] cannot disagree about which entry is an account's.
fn session_name(slot: AccountSlot) -> String {
    match slot {
        AccountSlot::Legacy => "session".to_owned(),
        AccountSlot::Id(id) => format!("session-{id}"),
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

/// Where the accounts index lives: beside the configuration, in its own file.
pub(crate) fn index_path(config_path: &Path) -> PathBuf {
    config_path.with_extension("accounts.json")
}

/// One account the index knows about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct KnownAccount {
    /// The Telegram user id the account is signed in as.
    pub(crate) id: i64,

    /// The number it was seen with, when one is known.
    #[serde(default)]
    pub(crate) phone: Option<String>,

    /// Which slot its bytes are in.
    pub(crate) slot: AccountSlot,

    /// Epoch seconds of the last launch whose account details changed.
    #[serde(default)]
    pub(crate) last_used: u64,
}

/// The accounts this configuration has seen, and the last one used.
///
/// A launch hint, not a record of who is signed in: an index says which slot to
/// open, and a slot with no session opens onto the sign-in screen like any
/// other. A missing or corrupt index therefore means "no hint", not "broken".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AccountsIndex {
    /// The shape of the file. An unknown version is read as no index.
    pub(crate) version: u32,

    /// The account the last `Ready` named.
    pub(crate) active: Option<i64>,

    #[serde(default)]
    pub(crate) accounts: Vec<KnownAccount>,
}

impl AccountsIndex {
    /// The only shape this build writes and reads.
    const VERSION: u32 = 1;

    /// Reads the index at `path`.
    ///
    /// Absent, unreadable or corrupt all mean "no index": the launch then uses
    /// the legacy slot, which is today's bytes, and never fails over a hint.
    pub(crate) fn load(path: &Path) -> Option<Self> {
        let bytes = fs::read(path).ok()?;
        let index: Self = serde_json::from_slice(&bytes).ok()?;
        (index.version == Self::VERSION).then_some(index)
    }

    /// Writes the index over whatever is at `path`.
    fn save(&self, path: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|error| error.to_string())?;
        write_atomically(path, &bytes)
    }

    /// The account a launch opens: the last one active, when the index still
    /// knows it.
    ///
    /// `None` is the legacy slot. An `active` the index has no entry for
    /// invents nothing — a slot is only ever one the index recorded.
    pub(crate) fn active_account(&self) -> Option<i64> {
        let active = self.active?;
        self.accounts
            .iter()
            .find(|known| known.id == active)?
            .slot
            .account()
    }
}

/// Records the account a `Ready` named, at the legacy slot, as the active one.
///
/// Called after `Ready` and never before: the index is a hint, and a launch
/// with no account named yet has nothing to hint at. Writes only when a field
/// changes — never per pass, and not on a repeat `Ready` for the same account —
/// so the file is written at most once per launch.
pub(crate) fn note_signed_in(path: &Path, id: i64, phone: Option<&str>) {
    let mut index = AccountsIndex::load(path).unwrap_or_else(|| AccountsIndex {
        version: AccountsIndex::VERSION,
        active: None,
        accounts: Vec::new(),
    });

    let at = index.accounts.iter().position(|known| known.id == id);
    let unchanged = index.active == Some(id)
        && at.is_some_and(|at| {
            let known = &index.accounts[at];
            known.slot == AccountSlot::Legacy && known.phone.as_deref() == phone
        });
    if unchanged {
        return;
    }

    let known = KnownAccount {
        id,
        phone: phone.map(str::to_owned),
        slot: AccountSlot::Legacy,
        last_used: now(),
    };
    index.active = Some(id);
    match at {
        Some(at) => index.accounts[at] = known,
        None => index.accounts.push(known),
    }

    if let Err(error) = index.save(path) {
        tracing::warn!(%error, "the accounts index could not be written");
    }
}

/// Epoch seconds, or zero for a clock before it — an index needs an order, not
/// a timestamp anyone reads.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Writes `bytes` so that the target only ever appears as a whole file.
///
/// The pattern [`crate::history_store`] and [`crate::draft_store`] both use: a
/// sibling temp in the same directory, restricted before a byte is in it,
/// renamed over the target — so an interrupted write leaves the previous index
/// intact, and the target inherits the temp's `0600`.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    let temp = path.with_file_name(name);

    let result = (|| -> io::Result<()> {
        fs::File::create(&temp)?;
        restrict_permissions(&temp);
        fs::write(&temp, bytes)?;
        fs::rename(&temp, path)
    })();

    result.map_err(|error| {
        // A half-written temp is not an index, and nothing would ever clean it up.
        let _ = fs::remove_file(&temp);
        format!("{error}")
    })
}

/// Restricts a freshly written index to its owner.
#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;

    if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        tracing::warn!(%error, "could not restrict the permissions of the accounts index");
    }
}

/// No-op on platforms whose ACLs `std` cannot express portably.
#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

/// Where a slot's session is kept.
///
/// Only [`Target::Existing`] in this stage: bytes that are already where the
/// resolver put them, so the target *is* the session target. Stage 02 adds a
/// `Fresh` target — an account being added, whose slot has to be minted and
/// whose bytes do not exist — which is why the store takes a target at all
/// rather than a slot.
pub(crate) enum Target {
    Existing(SessionTarget),
}

/// The session store of one slot.
///
/// One type in front of the two backends, so a slot's session can only be read,
/// written or cleared where [`slot_paths`] put it. It exists so stage 02 can add
/// a target without the client learning a second way to store a session.
pub(crate) struct SlotStore {
    target: Target,

    /// The file's key source. `None` for the credential store, which needs
    /// none from here.
    keys: Option<Arc<dyn KeyProvider>>,
}

impl SlotStore {
    /// The store `target` names, with `keys` for a session that is a file.
    ///
    /// `keys` is required for a file target and never read otherwise; the
    /// resolver only produces one when a key source exists.
    pub(crate) fn existing(target: Target, keys: Option<Arc<dyn KeyProvider>>) -> Self {
        Self { target, keys }
    }

    /// The file store for `path`, under the slot's key source.
    fn file(&self, path: &Path) -> FileStore {
        FileStore::with_key_provider(
            path,
            Arc::clone(
                self.keys
                    .as_ref()
                    .expect("a file session is resolved with its key source"),
            ),
        )
    }
}

impl SessionStore for SlotStore {
    fn load(&self) -> Result<Option<SessionData>, SessionError> {
        match &self.target {
            Target::Existing(SessionTarget::Keyring(account)) => {
                KeyringStore::new(KeyringStore::DEFAULT_SERVICE, account.clone()).load()
            }
            Target::Existing(SessionTarget::File(path)) => self.file(path).load(),
        }
    }

    fn save(&self, session: &SessionData) -> Result<(), SessionError> {
        match &self.target {
            Target::Existing(SessionTarget::Keyring(account)) => {
                KeyringStore::new(KeyringStore::DEFAULT_SERVICE, account.clone()).save(session)
            }
            Target::Existing(SessionTarget::File(path)) => self.file(path).save(session),
        }
    }

    fn clear(&self) -> Result<(), SessionError> {
        match &self.target {
            Target::Existing(SessionTarget::Keyring(account)) => {
                KeyringStore::new(KeyringStore::DEFAULT_SERVICE, account.clone()).clear()
            }
            Target::Existing(SessionTarget::File(path)) => self.file(path).clear(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The index beside a scratch configuration.
    fn scratch() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let config_path = dir.path().join("televim.toml");
        let index = index_path(&config_path);
        (dir, config_path, index)
    }

    /// Which session a slot resolves to, as a name a test can compare.
    fn session_name_of(config_path: &Path, account: Option<i64>) -> String {
        match slot_paths(config_path, &Config::default(), account).session {
            SessionTarget::Keyring(name) => name,
            SessionTarget::File(path) => path.display().to_string(),
        }
    }

    fn known(id: i64, slot: AccountSlot) -> KnownAccount {
        KnownAccount {
            id,
            phone: None,
            slot,
            last_used: 1,
        }
    }

    #[test]
    fn the_legacy_slot_reproduces_todays_paths() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let config_path = dir.path().join("televim.toml");
        let paths = slot_paths(&config_path, &Config::default(), None);

        assert_eq!(paths.slot, AccountSlot::Legacy);
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

        assert_eq!(paths.slot, AccountSlot::Id(7));
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

    /// The tag rule in one place: the legacy slot keeps the configured phone,
    /// an id slot is its decimal id — the difference that stops an upgrade
    /// reading the first account's bytes as a stranger's.
    #[test]
    fn the_slot_label_is_the_configured_phone_or_the_id() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let config_path = dir.path().join("televim.toml");
        let cfg = Config {
            phone: Some("+15551234567".to_owned()),
            ..Config::default()
        };

        assert_eq!(
            slot_paths(&config_path, &cfg, None).label(&cfg).as_deref(),
            Some("+15551234567"),
            "the legacy slot's tag is what the install already writes"
        );
        assert_eq!(
            slot_paths(&config_path, &cfg, Some(7))
                .label(&cfg)
                .as_deref(),
            Some("7"),
            "an added account is its id, never the phone"
        );
        assert_eq!(
            slot_paths(&config_path, &Config::default(), None).label(&Config::default()),
            None,
            "no configured phone leaves the legacy slot untagged"
        );
    }

    #[test]
    fn the_index_round_trips() {
        let (_dir, config_path, index) = scratch();
        assert_eq!(index, config_path.with_extension("accounts.json"));

        let written = AccountsIndex {
            version: AccountsIndex::VERSION,
            active: Some(7),
            accounts: vec![known(42, AccountSlot::Legacy), known(7, AccountSlot::Id(7))],
        };
        written.save(&index).expect("the index is written");

        assert_eq!(AccountsIndex::load(&index), Some(written));
    }

    #[test]
    fn an_absent_index_is_no_index() {
        let (_dir, _config_path, index) = scratch();

        assert_eq!(AccountsIndex::load(&index), None);
    }

    /// The next write replaces a corrupt index rather than leaving the launch
    /// with no hint for ever.
    #[test]
    fn a_corrupt_index_is_no_index_and_is_replaced_by_the_next_write() {
        let (_dir, _config_path, index) = scratch();
        fs::write(&index, b"{ this is not an index").expect("the fixture is written");

        assert_eq!(AccountsIndex::load(&index), None);

        note_signed_in(&index, 7, Some("+1555"));

        let stored = AccountsIndex::load(&index).expect("the next write replaces it");
        assert_eq!(stored.active, Some(7));
        assert_eq!(stored.accounts.len(), 1);
        assert_eq!(stored.accounts[0].id, 7);
        assert_eq!(stored.accounts[0].slot, AccountSlot::Legacy);
    }

    #[cfg(unix)]
    #[test]
    fn the_index_is_written_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt as _;

        let (_dir, _config_path, index) = scratch();
        AccountsIndex {
            version: AccountsIndex::VERSION,
            active: Some(7),
            accounts: vec![known(7, AccountSlot::Legacy)],
        }
        .save(&index)
        .expect("the index is written");

        let mode = fs::metadata(&index)
            .expect("the index exists")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "got {mode:o}");
    }

    #[test]
    fn no_index_selects_the_legacy_slot() {
        let (_dir, config_path, index) = scratch();

        let selected = AccountsIndex::load(&index).and_then(|index| index.active_account());

        assert_eq!(
            selected, None,
            "no index is the legacy layout, not an error"
        );
        assert_eq!(session_name_of(&config_path, selected), "session");
    }

    #[test]
    fn an_active_id_slot_selects_that_account() {
        let (_dir, config_path, index) = scratch();
        AccountsIndex {
            version: AccountsIndex::VERSION,
            active: Some(7),
            accounts: vec![known(7, AccountSlot::Id(7))],
        }
        .save(&index)
        .expect("the index is written");

        let selected = AccountsIndex::load(&index).and_then(|index| index.active_account());

        assert_eq!(selected, Some(7));
        let paths = slot_paths(&config_path, &Config::default(), selected);
        assert_eq!(paths.slot, AccountSlot::Id(7));
        assert_eq!(paths.drafts, config_path.with_extension("7.drafts.json"));
        assert_eq!(session_name_of(&config_path, selected), "session-7");
    }

    /// An active id the index has no entry for is not a slot: nothing is
    /// invented from a pointer alone.
    #[test]
    fn an_active_the_index_does_not_know_invents_nothing() {
        let (_dir, _config_path, index) = scratch();
        AccountsIndex {
            version: AccountsIndex::VERSION,
            active: Some(9),
            accounts: vec![known(7, AccountSlot::Id(7))],
        }
        .save(&index)
        .expect("the index is written");

        let selected = AccountsIndex::load(&index).and_then(|index| index.active_account());

        assert_eq!(selected, None);
    }

    /// The first account keeps the legacy slot, so a `Ready` for it must not
    /// move the launch onto suffixed paths.
    #[test]
    fn a_legacy_entry_resolves_to_the_legacy_slot() {
        let (_dir, _config_path, index) = scratch();
        AccountsIndex {
            version: AccountsIndex::VERSION,
            active: Some(42),
            accounts: vec![known(42, AccountSlot::Legacy)],
        }
        .save(&index)
        .expect("the index is written");

        let selected = AccountsIndex::load(&index).and_then(|index| index.active_account());

        assert_eq!(selected, None);
    }

    /// A `Ready` for the same account with the same details is not a write, so
    /// the recorded `last_used` — here a value no clock would produce — stands.
    #[test]
    fn a_second_ready_for_the_same_account_is_not_a_write() {
        let (_dir, _config_path, index) = scratch();
        AccountsIndex {
            version: AccountsIndex::VERSION,
            active: Some(7),
            accounts: vec![KnownAccount {
                id: 7,
                phone: Some("+1555".to_owned()),
                slot: AccountSlot::Legacy,
                last_used: u64::MAX,
            }],
        }
        .save(&index)
        .expect("the index is written");

        note_signed_in(&index, 7, Some("+1555"));

        let stored = AccountsIndex::load(&index).expect("the index reads");
        assert_eq!(
            stored.accounts[0].last_used,
            u64::MAX,
            "nothing was written"
        );
    }

    /// The store writes and clears only the slot it names.
    #[test]
    fn the_slot_store_writes_only_the_slot_it_names() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let keys: Arc<dyn KeyProvider> =
            Arc::new(telegram_framework::PassphraseProvider::new("fixture"));
        let legacy_path = dir.path().join("session.bin");
        let account_path = dir.path().join("session-7.bin");
        let legacy = SlotStore::existing(
            Target::Existing(SessionTarget::File(legacy_path.clone())),
            Some(Arc::clone(&keys)),
        );
        let account = SlotStore::existing(
            Target::Existing(SessionTarget::File(account_path.clone())),
            Some(Arc::clone(&keys)),
        );

        legacy.save(&snapshot(1)).expect("the legacy slot stores");
        account
            .save(&snapshot(7))
            .expect("the account's slot stores");

        let user = |store: &SlotStore| {
            store
                .load()
                .expect("the slot reads")
                .and_then(|it| it.user_id)
        };
        assert_eq!(user(&legacy), Some(1));
        assert_eq!(user(&account), Some(7));
        assert!(legacy_path.exists() && account_path.exists());

        account.clear().expect("the account's slot is cleared");

        assert_eq!(user(&account), None, "the account's slot is empty");
        assert_eq!(user(&legacy), Some(1), "the legacy slot is untouched");
    }

    /// The migration that matters: a first account stays where its bytes are,
    /// and the index written at `Ready` sends the next launch to the same place.
    #[test]
    fn a_ready_names_the_account_and_the_next_launch_still_opens_the_legacy_slot() {
        let (_dir, config_path, index) = scratch();

        // No index, so this launch is the layout the install already has.
        assert_eq!(AccountsIndex::load(&index), None);
        assert_eq!(session_name_of(&config_path, None), "session");

        // The first `Ready` naming an account, account 42.
        note_signed_in(&index, 42, Some("+1555"));

        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(&index).expect("the index is written"))
                .expect("the index is json");
        assert_eq!(stored["version"], 1);
        assert_eq!(stored["active"], 42);
        assert_eq!(stored["accounts"][0]["id"], 42);
        assert_eq!(stored["accounts"][0]["slot"], "Legacy");

        // The second launch reads that index and resolves the same bytes.
        let selected = AccountsIndex::load(&index).and_then(|index| index.active_account());
        assert_eq!(session_name_of(&config_path, selected), "session");
        assert!(
            !config_path.with_extension("42.drafts.json").exists(),
            "account 42's slot was never opened, so nothing of it was created"
        );
        assert!(!config_path.with_extension("42.history.json").exists());
    }

    /// A session for `user_id`, so a store can be given one.
    fn snapshot(user_id: i64) -> SessionData {
        SessionData {
            user_id: Some(user_id),
            ..SessionData::default()
        }
    }
}
