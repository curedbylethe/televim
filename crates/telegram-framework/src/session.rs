//! Pluggable session persistence.
//!
//! Telegram authorises a client by handing it a permanent *authorisation key*
//! bound to one datacenter. Losing that key means signing in again, which is
//! both rate limited and disruptive, so it has to survive restarts.
//!
//! [`SessionData`] is the serialisable form of that state. It is built from
//! primitives, strings and byte vectors only — never from a `grammers` type —
//! so any backend can persist it without knowing anything about `MTProto`.
//! [`MemoryStore`], [`FileStore`] and [`KeyringStore`] are the three shipped
//! backends.
//!
//! A snapshot also names *who* is signed in, as an [`AccountIdentity`]. Without
//! it a stored session cannot tell a fresh machine from a signed-out one without
//! a round trip, and it cannot prefill a sign-in form with the number it is
//! being asked for.
//!
//! # Choosing a backend
//!
//! | Backend        | Use it for                                                        |
//! | :------------- | :---------------------------------------------------------------- |
//! | `KeyringStore` | Production. The session lives in the OS credential store.          |
//! | `FileStore`    | Fallback when there is no keyring. **Writes the key in plaintext.** |
//! | `MemoryStore`  | Tests and throwaway sessions that must not outlive the process.     |

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::SessionError;

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// A permanent, 256-byte authorisation key.
///
/// It serialises as a lowercase hex string: 512 bytes instead of the roughly
/// 1.2 kB a JSON array of numbers would take, which matters because some
/// credential stores cap how large an entry may be. Its [`Debug`]
/// implementation never prints key material.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AuthKey([u8; Self::LEN]);

impl AuthKey {
    /// Length of an authorisation key, in bytes.
    pub const LEN: usize = 256;

    /// Wraps raw key bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; Self::LEN]) -> Self {
        Self(bytes)
    }

    /// Borrows the raw key bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; Self::LEN] {
        &self.0
    }
}

impl fmt::Debug for AuthKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthKey(<redacted>)")
    }
}

impl Serialize for AuthKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&encode_hex(&self.0))
    }
}

impl<'de> Deserialize<'de> for AuthKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let hex = String::deserialize(deserializer)?;
        let bytes = decode_hex(&hex).map_err(D::Error::custom)?;
        let key = <[u8; Self::LEN]>::try_from(bytes).map_err(|_| {
            D::Error::custom(format!(
                "an authorisation key must be exactly {} bytes",
                Self::LEN
            ))
        })?;
        Ok(Self(key))
    }
}

/// A Telegram datacenter, with the permanent authorisation key bound to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DcOption {
    /// Datacenter identifier. The primary datacenters are `1..=5`.
    pub id: i32,

    /// `IPv4` endpoint, formatted as `host:port`.
    pub ipv4: String,

    /// `IPv6` endpoint, formatted as `[host]:port`.
    pub ipv6: String,

    /// Authorisation key, once one has been negotiated with this datacenter.
    pub auth_key: Option<AuthKey>,
}

/// The kind of a channel peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    /// A supergroup.
    Megagroup,
    /// A broadcast channel.
    Broadcast,
    /// A gigagroup.
    Gigagroup,
}

/// A peer the session has cached.
///
/// Telegram hands out an `access_hash` for every peer, and it is needed to
/// address that peer again. Losing it only costs a re-fetch, but caching it
/// keeps the first request after a restart cheap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Peer {
    /// A user or bot.
    User {
        /// Bare user identifier.
        id: i64,
        /// `access_hash`, if Telegram disclosed one.
        auth: Option<i64>,
        /// Whether the account is a bot.
        bot: Option<bool>,
        /// Whether this is the logged-in account itself.
        is_self: Option<bool>,
    },
    /// A small group chat.
    Chat {
        /// Bare chat identifier.
        id: i64,
    },
    /// A channel, megagroup or gigagroup.
    Channel {
        /// Bare channel identifier.
        id: i64,
        /// `access_hash`, if Telegram disclosed one.
        auth: Option<i64>,
        /// What kind of channel this is.
        kind: Option<ChannelKind>,
    },
}

/// Persistent timestamp of a single channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelState {
    /// Bare channel identifier.
    pub id: i64,
    /// Persistent timestamp value for that channel.
    pub pts: i32,
}

/// How far through the update stream the session has read.
///
/// Telegram numbers every update, so keeping these counters lets a client catch
/// up on exactly what it missed instead of refetching everything.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateState {
    /// Primary persistent timestamp.
    pub pts: i32,
    /// Secondary persistent timestamp.
    pub qts: i32,
    /// Auxiliary date value.
    pub date: i32,
    /// Auxiliary sequence value.
    pub seq: i32,
    /// Per-channel persistent timestamps.
    pub channels: Vec<ChannelState>,
}

/// A complete, serialisable snapshot of the state that has to survive a restart.
///
/// It carries no `grammers` type: every field is a primitive, a string or a
/// byte vector, so it can be handed to a file, an OS keyring, or anything else
/// without that backend having to understand `MTProto`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionData {
    /// Schema version of this snapshot.
    pub version: u32,

    /// Bare identifier of the account this session authorises, when the snapshot
    /// says.
    ///
    /// `None` on a machine that has never signed in, and on a snapshot written
    /// by a build older than this field — see [`SessionData::VERSION`]. Telegram
    /// discloses the account's own identifier in exactly one place, and a
    /// snapshot that does not carry it cannot tell a reader which account they
    /// are looking at without a round trip.
    pub user_id: Option<i64>,

    /// The phone number the account signed in with, when the snapshot says.
    ///
    /// The one field of the login flow that is not derivable from anything else
    /// and cannot be asked for: Telegram will not disclose a number the reader
    /// did not give it. So it is kept for the sign-in form to prefill with.
    pub phone: Option<String>,

    /// Datacenter that is home to the logged-in account, if it is known yet.
    ///
    /// `None` means "keep whatever default the transport layer picks", which is
    /// what a session that has never connected looks like.
    pub home_dc_id: Option<i32>,

    /// Datacenters that have been seen, along with their authorisation keys.
    pub dc_options: Vec<DcOption>,

    /// Peers the session has cached.
    ///
    /// The account itself is *not* needed here for the update feed to work, and
    /// the reason is worth writing down because the opposite is the kind of
    /// assumption that only fails when a real message arrives:
    ///
    /// * An update's conversation is resolved from the peer identifier encoded
    ///   in the message, through the peer map the update itself arrived with —
    ///   not out of this cache. See `grammers`' `Message::peer`.
    /// * Your own user only ever turns up as a *sender*, and there it is
    ///   synthesised from the message's outgoing flag: no lookup, no cache.
    /// * Even a lookup could not find it, because the account's own peer
    ///   identifier has no bare identifier and `StoreSession::cached_peer`
    ///   accepts nothing but one.
    ///
    /// So the self peer is not put here, and the chat list's own skip of a peer
    /// with no bare identifier is the same rule rather than a second one.
    pub peers: Vec<Peer>,

    /// How far through the update stream the session has read.
    pub update_state: UpdateState,
}

/// Who a stored session belongs to.
///
/// What a snapshot can say about the account with no round trip at all: the
/// identifier, which Telegram discloses nowhere else, and the phone number,
/// which is the field a pre-filled sign-in form needs. It is deliberately not
/// an account — a snapshot has never been asked for a bio or a birthday, and a
/// type carrying two empty ones would be a card showing fields nobody filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountIdentity {
    /// Bare identifier of the account's own user.
    pub user_id: i64,

    /// The phone number the account signed in with, when the snapshot has it.
    pub phone: Option<String>,
}

impl SessionData {
    /// Schema version written by this build.
    ///
    /// The account identity was added *without* a bump, and that is the point:
    /// the field is optional and the struct is `#[serde(default)]`, so a
    /// snapshot written by an older build still parses, still carries a working
    /// authorisation key, and is still a session worth keeping. It simply does
    /// not name the account, and [`SessionData::account`] says so.
    ///
    /// The other answer — bump the version, so the old snapshot is refused — is
    /// the one that logs every existing reader out on upgrade, and an
    /// authorisation key that still works is not worth trading for a field that
    /// can be fetched. A version is only for a change that genuinely cannot be
    /// read back, and that one is refused with a sentence rather than silently
    /// started over.
    pub const VERSION: u32 = 1;

    /// Serialises the snapshot to JSON.
    ///
    /// Useful when implementing a [`SessionStore`] of your own.
    pub fn to_bytes(&self) -> Result<Vec<u8>, SessionError> {
        serde_json::to_vec(self).map_err(|error| SessionError::Save(error.to_string()))
    }

    /// Parses a snapshot produced by [`SessionData::to_bytes`].
    ///
    /// Returns [`SessionError::Corrupt`] if the bytes are not a snapshot, or if
    /// they were written by a schema version this build does not understand. An
    /// old version is *refused*, never treated as a fresh session: a snapshot
    /// that was silently reset is a reader who is asked to sign in again for a
    /// reason nothing on the screen can explain.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SessionError> {
        let data: Self = serde_json::from_slice(bytes)
            .map_err(|error| SessionError::Corrupt(error.to_string()))?;
        if data.version != Self::VERSION {
            return Err(SessionError::Corrupt(format!(
                "unsupported session version {} (this build writes {})",
                data.version,
                Self::VERSION
            )));
        }
        Ok(data)
    }

    /// The account this snapshot belongs to, if it says.
    ///
    /// `None` is a real answer rather than a missing one: it means the snapshot
    /// was written before anything recorded who signed in, which is what a
    /// machine that has never used the account and a machine running an older
    /// build both look like. Either way the authorisation key is what says
    /// whether the account is still signed in — this only says which one.
    #[must_use]
    pub fn account(&self) -> Option<AccountIdentity> {
        self.user_id.map(|user_id| AccountIdentity {
            user_id,
            phone: self.phone.clone(),
        })
    }
}

impl Default for SessionData {
    fn default() -> Self {
        Self {
            version: Self::VERSION,
            user_id: None,
            phone: None,
            home_dc_id: None,
            dc_options: Vec::new(),
            peers: Vec::new(),
            update_state: UpdateState::default(),
        }
    }
}

/// Persists [`SessionData`] between runs.
///
/// Implementations are used from the async event loop, so the methods are
/// synchronous and must not block for long. All three shipped backends are
/// cheap: the keyring call is the slowest of them, and it happens once per
/// login rather than once per request.
pub trait SessionStore: Send + Sync {
    /// Loads the stored session, or `None` when there is nothing stored yet.
    fn load(&self) -> Result<Option<SessionData>, SessionError>;

    /// Overwrites the stored session.
    fn save(&self, session: &SessionData) -> Result<(), SessionError>;

    /// Removes the stored session, if any.
    fn clear(&self) -> Result<(), SessionError>;
}

/// Keeps the session in memory.
///
/// Nothing is written anywhere, so the session is lost when the process exits.
/// This is the right choice for tests and for throwaway logins.
#[derive(Debug, Default)]
pub struct MemoryStore {
    inner: Mutex<Option<SessionData>>,
}

impl MemoryStore {
    /// Creates an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Locks the inner slot, recovering from a poisoned mutex.
    ///
    /// The guarded value is a plain `Option`, so a panic elsewhere cannot leave
    /// it inconsistent; refusing to use it would only turn one failure into two.
    fn lock(&self) -> MutexGuard<'_, Option<SessionData>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl SessionStore for MemoryStore {
    fn load(&self) -> Result<Option<SessionData>, SessionError> {
        Ok(self.lock().clone())
    }

    fn save(&self, session: &SessionData) -> Result<(), SessionError> {
        *self.lock() = Some(session.clone());
        Ok(())
    }

    fn clear(&self) -> Result<(), SessionError> {
        *self.lock() = None;
        Ok(())
    }
}

/// Persists the session as JSON in a file.
///
/// This is the fallback for machines with no usable OS keyring: headless Linux
/// servers, containers, CI. It is **not** the default, because the file holds a
/// permanent authorisation key in plaintext — anyone who can read the file can
/// impersonate the account until the session is revoked. On Unix the file is
/// created with `0600` permissions, which is a mitigation, not a solution.
///
/// Prefer [`KeyringStore`] wherever a credential store exists.
#[derive(Debug, Clone)]
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    /// Creates a store backed by the file at `path`.
    ///
    /// The file is not touched until [`SessionStore::save`] is called, and any
    /// missing parent directories are created then.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The file this store reads and writes.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl SessionStore for FileStore {
    fn load(&self) -> Result<Option<SessionData>, SessionError> {
        match fs::read(&self.path) {
            Ok(bytes) => SessionData::from_bytes(&bytes).map(Some),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(SessionError::Load(error.to_string())),
        }
    }

    fn save(&self, session: &SessionData) -> Result<(), SessionError> {
        let bytes = session.to_bytes()?;
        if let Some(parent) = self.path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|error| SessionError::Save(error.to_string()))?;
        }
        // Created empty and restricted *before* a byte is in it. A plain
        // `fs::write` leaves a window between creating the file and the
        // `chmod`, and what is in that window is a permanent authorisation key
        // at whatever the umask allowed.
        fs::File::create(&self.path).map_err(|error| SessionError::Save(error.to_string()))?;
        restrict_permissions(&self.path);
        fs::write(&self.path, &bytes).map_err(|error| SessionError::Save(error.to_string()))?;
        Ok(())
    }

    fn clear(&self) -> Result<(), SessionError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(SessionError::Clear(error.to_string())),
        }
    }
}

/// Restricts a freshly written session file to its owner.
#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;

    if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        tracing::warn!(%error, "could not restrict the permissions of the session file");
    }
}

/// No-op on platforms whose ACLs `std` cannot express portably.
#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

/// Persists the session in the OS credential store.
///
/// This is the production backend. It uses the macOS Keychain, the Windows
/// Credential Manager, or the Linux Secret Service, so the authorisation key is
/// protected by the platform rather than sitting in a file.
///
/// # Errors
///
/// On a machine with no credential store — a headless Linux container, for
/// instance — every operation fails with [`SessionError::Unavailable`]. Fall
/// back to [`FileStore`] there.
#[derive(Debug, Clone)]
pub struct KeyringStore {
    service: String,
    account: String,
}

impl KeyringStore {
    /// Service name used by [`KeyringStore::default`].
    pub const DEFAULT_SERVICE: &'static str = "televim";

    /// Account name used by [`KeyringStore::default`].
    pub const DEFAULT_ACCOUNT: &'static str = "session";

    /// Creates a store for the given `service`/`account` pair.
    #[must_use]
    pub fn new(service: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            account: account.into(),
        }
    }

    /// The service name this store writes under.
    #[must_use]
    pub fn service(&self) -> &str {
        &self.service
    }

    /// The account name this store writes under.
    #[must_use]
    pub fn account(&self) -> &str {
        &self.account
    }

    /// Opens the credential store entry.
    ///
    /// The entry is recreated for every operation rather than cached so that
    /// the store stays `Send + Sync` and free of platform handles.
    fn entry(&self) -> Result<keyring::Entry, SessionError> {
        keyring::Entry::new(&self.service, &self.account)
            .map_err(|error| SessionError::Unavailable(error.to_string()))
    }
}

impl Default for KeyringStore {
    fn default() -> Self {
        Self::new(Self::DEFAULT_SERVICE, Self::DEFAULT_ACCOUNT)
    }
}

impl SessionStore for KeyringStore {
    fn load(&self) -> Result<Option<SessionData>, SessionError> {
        match self.entry()?.get_secret() {
            Ok(bytes) => SessionData::from_bytes(&bytes).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(SessionError::Load(error.to_string())),
        }
    }

    fn save(&self, session: &SessionData) -> Result<(), SessionError> {
        let bytes = session.to_bytes()?;
        self.entry()?
            .set_secret(&bytes)
            .map_err(|error| SessionError::Save(error.to_string()))
    }

    fn clear(&self) -> Result<(), SessionError> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(SessionError::Clear(error.to_string())),
        }
    }
}

#[cfg(feature = "live")]
pub(crate) use bridge::StoreSession;

/// Bridges a [`SessionStore`] into the session interface `grammers` expects.
#[cfg(feature = "live")]
mod bridge {
    use std::fmt;
    use std::net::SocketAddrV4;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use grammers_client::session::types::{
        ChannelKind as TlChannelKind, ChannelState as TlChannelState, DcOption as TlDcOption,
        PeerAuth, PeerId, PeerInfo, UpdateState as TlUpdateState, UpdatesState as TlUpdatesState,
    };
    use grammers_client::session::{BoxFuture, Session, SessionData as TlSessionData};

    use super::{
        AccountIdentity, AuthKey, ChannelKind, ChannelState, DcOption, Peer, SessionData,
        SessionStore, UpdateState,
    };
    use crate::error::SessionError;

    /// Largest bare user identifier `grammers` accepts.
    const MAX_USER_ID: i64 = 0x0000_00ff_ffff_ffff;
    /// Largest bare group-chat identifier `grammers` accepts.
    const MAX_CHAT_ID: i64 = 999_999_999_999;
    /// Largest bare channel identifier `grammers` accepts.
    const MAX_CHANNEL_ID: i64 = 997_852_516_352;
    /// Smallest monoforum identifier `grammers` accepts.
    const MIN_MONOFORUM_ID: i64 = 1_002_147_483_649;
    /// Largest monoforum identifier `grammers` accepts.
    const MAX_MONOFORUM_ID: i64 = 3_000_000_000_000;

    /// Adapts a [`SessionStore`] to the interface `grammers` expects.
    ///
    /// `grammers` consults the session on every single request and its
    /// [`Session`] methods are infallible, so the decoded state is held in
    /// memory and mirrored back to the store on demand — via
    /// [`StoreSession::persist_if_dirty`] — rather than on every mutation. That
    /// keeps a credential-store write off the hot path.
    pub struct StoreSession {
        store: Arc<dyn SessionStore>,
        state: Mutex<TlSessionData>,

        /// Who this session is for, when something has said.
        ///
        /// Not part of the mirror, because `grammers` has no field for it: the
        /// account is disclosed by the login flow and by nothing else, so it is
        /// carried alongside the state rather than inside it.
        account: Mutex<Option<AccountIdentity>>,

        /// Set whenever the transport mutates the session.
        ///
        /// A handful of requests change the session without any of the login
        /// methods being involved: a datacenter migration, a peer learned from
        /// a response, a moved update counter. Those changes are worth keeping —
        /// losing a migration means the next launch cannot address the
        /// datacenter it was moved to — but writing the whole snapshot on every
        /// one of them would put a keyring write in the request path. So the
        /// mutation only raises this flag, and the write happens once the
        /// request that caused it is finished.
        dirty: AtomicBool,
    }

    impl StoreSession {
        /// Loads whatever the store holds, layered over `grammers`' defaults.
        ///
        /// `grammers` ships a statically-known set of datacenters, and the
        /// snapshot only carries the ones that have been connected to, so the
        /// defaults are the starting point and the snapshot is applied on top.
        pub(crate) fn new(store: Arc<dyn SessionStore>) -> Result<Self, SessionError> {
            let mut state = TlSessionData::default();
            let mut account = None;
            if let Some(snapshot) = store.load()? {
                apply_snapshot(&snapshot, &mut state);
                account = snapshot.account();
            }
            Ok(Self {
                store,
                state: Mutex::new(state),
                account: Mutex::new(account),
                dirty: AtomicBool::new(false),
            })
        }

        /// Snapshots the live state into a value a [`SessionStore`] can persist.
        pub(crate) fn snapshot(&self) -> SessionData {
            let state = self.lock();
            let account = self
                .account
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            SessionData {
                version: SessionData::VERSION,
                user_id: account.as_ref().map(|account| account.user_id),
                phone: account.and_then(|account| account.phone),
                home_dc_id: Some(state.home_dc),
                dc_options: state.dc_options.values().map(dc_option_to_data).collect(),
                peers: state.peer_infos.values().map(peer_to_data).collect(),
                update_state: update_state_to_data(&state.updates_state),
            }
        }

        /// The cached peer with this bare identifier, if there is one.
        ///
        /// `grammers` keys its cache by [`PeerId`], which packs the peer's kind
        /// into the identifier, so a *bare* identifier — the only kind this
        /// crate hands out — is not enough to look one up directly. Scanning is
        /// what avoids reconstructing the kind, which would mean guessing
        /// between three constructors that panic on a value outside their range.
        /// The cache holds one entry per conversation the account has, so the
        /// scan is short and it only runs when a request is being built.
        ///
        /// A peer whose identifier is the account's own has no bare identifier at
        /// all, and so can never be the answer to a lookup by one. That is a miss
        /// rather than a match, which is what makes this safe to compare rather
        /// than a case that has to be handled: the question being asked is always
        /// "which peer *is* this number", and the self entry is not a number.
        pub(crate) fn cached_peer(&self, bare_id: i64) -> Option<PeerInfo> {
            self.lock()
                .peer_infos
                .values()
                .find(|info| info.id().bare_id() == Some(bare_id))
                .cloned()
        }

        /// Writes the current state to the backing store.
        pub(crate) fn persist(&self) -> Result<(), SessionError> {
            // Cleared *before* the snapshot, not after. A mutation that lands
            // while the write is in flight re-raises the flag, so the worst case
            // is one redundant write; clearing it afterwards would instead drop
            // that mutation, which is the failure this whole mechanism exists to
            // prevent.
            self.dirty.store(false, Ordering::Release);

            if let Err(error) = self.store.save(&self.snapshot()) {
                // Put the flag back, so the next flush retries the change
                // instead of giving up on it.
                self.mark_dirty();
                return Err(error);
            }

            Ok(())
        }

        /// Forgets the session entirely: the store is cleared, and the in-memory
        /// mirror is put back to what a client that has never signed in has.
        ///
        /// Both halves, because clearing only the store would leave a client
        /// holding an authorisation key that nothing on the next launch has —
        /// and clearing only the mirror would leave that key on disk, which is
        /// the half a reader asking to log out means.
        ///
        /// The mirror is reset rather than patched. What it drops besides the
        /// keys is this account's update counters, which belong to the account
        /// that is being logged out and must not be resumed by whoever signs in
        /// next.
        pub(crate) fn clear(&self) -> Result<(), SessionError> {
            self.store.clear()?;

            *self.lock() = TlSessionData::default();
            *self.account.lock().unwrap_or_else(PoisonError::into_inner) = None;
            // Nothing is left worth writing, so a mutation that lands after this
            // is what raises the flag again — not the reset itself.
            self.dirty.store(false, Ordering::Release);
            Ok(())
        }

        /// Writes the current state back, but only if something changed it.
        ///
        /// Returns whether a write happened. The check is a plain load, so two
        /// threads racing here can both decide to write; a snapshot is written
        /// whole, so the worst case is a duplicated write rather than a torn
        /// one.
        pub(crate) fn persist_if_dirty(&self) -> Result<bool, SessionError> {
            if !self.dirty.load(Ordering::Acquire) {
                return Ok(false);
            }
            self.persist()?;
            Ok(true)
        }

        /// Raises the flag [`StoreSession::persist_if_dirty`] reads.
        fn mark_dirty(&self) {
            self.dirty.store(true, Ordering::Release);
        }

        /// Locks the in-memory mirror, recovering from a poisoned mutex.
        fn lock(&self) -> MutexGuard<'_, TlSessionData> {
            self.state.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    impl fmt::Debug for StoreSession {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("StoreSession").finish_non_exhaustive()
        }
    }

    impl Session for StoreSession {
        /// Every method here touches the in-memory mirror, which cannot fail.
        ///
        /// The store is written separately, by
        /// [`StoreSession::persist_if_dirty`], precisely so that a credential
        /// write never lands in the request path. So the fallibility the trait
        /// requires is answered with the absence of it rather than with a
        /// swallowed error — a session that reports `Infallible` and then failed
        /// would be lying in a way nothing downstream could detect.
        type Error = std::convert::Infallible;

        fn home_dc_id(&self) -> Result<i32, Self::Error> {
            Ok(self.lock().home_dc)
        }

        fn set_home_dc_id(&self, dc_id: i32) -> BoxFuture<'_, Result<(), Self::Error>> {
            Box::pin(async move {
                self.lock().home_dc = dc_id;
                self.mark_dirty();
                Ok(())
            })
        }

        fn dc_option(&self, dc_id: i32) -> Result<Option<TlDcOption>, Self::Error> {
            Ok(self.lock().dc_options.get(&dc_id).cloned())
        }

        fn set_dc_option(&self, dc_option: &TlDcOption) -> BoxFuture<'_, Result<(), Self::Error>> {
            // Taken by value before the future is built, so that the future
            // borrows only `self`. The trait's output lifetime is tied to `&self`
            // alone, so a future that also captured the argument would not
            // satisfy it. Cloning once here is cheaper than the clone the body
            // would otherwise make anyway.
            let dc_option = dc_option.clone();
            Box::pin(async move {
                self.lock().dc_options.insert(dc_option.id, dc_option);
                self.mark_dirty();
                Ok(())
            })
        }

        fn peer(&self, peer: PeerId) -> BoxFuture<'_, Result<Option<PeerInfo>, Self::Error>> {
            Box::pin(async move { Ok(self.lock().peer_infos.get(&peer).cloned()) })
        }

        fn cache_peer(&self, peer: &PeerInfo) -> BoxFuture<'_, Result<(), Self::Error>> {
            // Taken by value for the same reason as `set_dc_option`.
            let peer = peer.clone();
            Box::pin(async move {
                let id = peer.id();
                self.lock().peer_infos.insert(id, peer);
                self.mark_dirty();
                Ok(())
            })
        }

        fn updates_state(&self) -> BoxFuture<'_, Result<TlUpdatesState, Self::Error>> {
            Box::pin(async move { Ok(self.lock().updates_state.clone()) })
        }

        fn set_update_state(
            &self,
            update: TlUpdateState,
        ) -> BoxFuture<'_, Result<(), Self::Error>> {
            Box::pin(async move {
                let mut state = self.lock();
                match update {
                    TlUpdateState::All(updates) => state.updates_state = updates,
                    TlUpdateState::Primary { pts, date, seq } => {
                        state.updates_state.pts = pts;
                        state.updates_state.date = date;
                        state.updates_state.seq = seq;
                    }
                    TlUpdateState::Secondary { qts } => state.updates_state.qts = qts,
                    TlUpdateState::Channel { id, pts } => {
                        state
                            .updates_state
                            .channels
                            .retain(|channel| channel.id != id);
                        state
                            .updates_state
                            .channels
                            .push(TlChannelState { id, pts });
                    }
                }
                drop(state);
                self.mark_dirty();
                Ok(())
            })
        }
    }

    /// Layers a stored snapshot over `grammers`' default session state.
    ///
    /// Anything the snapshot does not carry is left at its default, and any
    /// entry that cannot be converted back is skipped with a warning rather
    /// than aborting the load: a half-readable session that still has its
    /// authorisation key is worth far more than none at all.
    fn apply_snapshot(snapshot: &SessionData, state: &mut TlSessionData) {
        if let Some(home_dc_id) = snapshot.home_dc_id {
            state.home_dc = home_dc_id;
        }

        for option in &snapshot.dc_options {
            if let Some(converted) = dc_option_from_data(option) {
                state.dc_options.insert(converted.id, converted);
            } else {
                tracing::warn!(
                    dc_id = option.id,
                    "ignoring a stored datacenter whose address could not be parsed"
                );
            }
        }

        for peer in &snapshot.peers {
            if let Some(info) = peer_from_data(peer) {
                let id = info.id();
                state.peer_infos.insert(id, info);
            } else {
                tracing::warn!("ignoring a stored peer with an out-of-range identifier");
            }
        }

        state.updates_state = updates_state_from_data(&snapshot.update_state);
    }

    fn dc_option_to_data(option: &TlDcOption) -> DcOption {
        DcOption {
            id: option.id,
            ipv4: option.ipv4.to_string(),
            ipv6: option.ipv6.to_string(),
            auth_key: option.auth_key.map(AuthKey::from_bytes),
        }
    }

    fn dc_option_from_data(option: &DcOption) -> Option<TlDcOption> {
        Some(TlDcOption {
            id: option.id,
            ipv4: option.ipv4.parse::<SocketAddrV4>().ok()?,
            ipv6: option.ipv6.parse().ok()?,
            auth_key: option.auth_key.map(|key| *key.as_bytes()),
        })
    }

    fn peer_to_data(peer: &PeerInfo) -> Peer {
        match peer {
            PeerInfo::User {
                id,
                auth,
                bot,
                is_self,
            } => Peer::User {
                id: *id,
                auth: auth.map(PeerAuth::hash),
                bot: *bot,
                is_self: *is_self,
            },
            PeerInfo::Chat { id } => Peer::Chat { id: *id },
            PeerInfo::Channel { id, auth, kind } => Peer::Channel {
                id: *id,
                auth: auth.map(PeerAuth::hash),
                kind: kind.map(channel_kind_to_data),
            },
        }
    }

    fn peer_from_data(peer: &Peer) -> Option<PeerInfo> {
        match peer {
            Peer::User {
                id,
                auth,
                bot,
                is_self,
            } if user_id_in_range(*id) => Some(PeerInfo::User {
                id: *id,
                auth: auth.map(PeerAuth::from_hash),
                bot: *bot,
                is_self: *is_self,
            }),
            Peer::Chat { id } if chat_id_in_range(*id) => Some(PeerInfo::Chat { id: *id }),
            Peer::Channel { id, auth, kind } if channel_id_in_range(*id) => {
                Some(PeerInfo::Channel {
                    id: *id,
                    auth: auth.map(PeerAuth::from_hash),
                    kind: kind.map(channel_kind_from_data),
                })
            }
            _ => None,
        }
    }

    fn channel_kind_to_data(kind: TlChannelKind) -> ChannelKind {
        match kind {
            TlChannelKind::Megagroup => ChannelKind::Megagroup,
            TlChannelKind::Broadcast => ChannelKind::Broadcast,
            TlChannelKind::Gigagroup => ChannelKind::Gigagroup,
        }
    }

    fn channel_kind_from_data(kind: ChannelKind) -> TlChannelKind {
        match kind {
            ChannelKind::Megagroup => TlChannelKind::Megagroup,
            ChannelKind::Broadcast => TlChannelKind::Broadcast,
            ChannelKind::Gigagroup => TlChannelKind::Gigagroup,
        }
    }

    fn update_state_to_data(state: &TlUpdatesState) -> UpdateState {
        UpdateState {
            pts: state.pts,
            qts: state.qts,
            date: state.date,
            seq: state.seq,
            channels: state
                .channels
                .iter()
                .map(|channel| ChannelState {
                    id: channel.id,
                    pts: channel.pts,
                })
                .collect(),
        }
    }

    fn updates_state_from_data(state: &UpdateState) -> TlUpdatesState {
        TlUpdatesState {
            pts: state.pts,
            qts: state.qts,
            date: state.date,
            seq: state.seq,
            channels: state
                .channels
                .iter()
                .map(|channel| TlChannelState {
                    id: channel.id,
                    pts: channel.pts,
                })
                .collect(),
        }
    }

    /// `grammers` panics on an out-of-range peer identifier, so anything read
    /// back from a store is checked before it is handed over.
    fn user_id_in_range(id: i64) -> bool {
        (1..=MAX_USER_ID).contains(&id)
    }

    fn chat_id_in_range(id: i64) -> bool {
        (1..=MAX_CHAT_ID).contains(&id)
    }

    fn channel_id_in_range(id: i64) -> bool {
        (1..=MAX_CHANNEL_ID).contains(&id) || (MIN_MONOFORUM_ID..=MAX_MONOFORUM_ID).contains(&id)
    }
}

/// Encodes bytes as lowercase hex.
fn encode_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        hex.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        hex.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    hex
}

/// Decodes lowercase or uppercase hex, rejecting odd lengths and non-hex digits.
fn decode_hex(hex: &str) -> Result<Vec<u8>, String> {
    let digits = hex.as_bytes();
    if !digits.len().is_multiple_of(2) {
        return Err("a hex string must have an even number of digits".to_owned());
    }
    digits
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let text = std::str::from_utf8(pair)
                .map_err(|_| "a hex string must be valid utf-8".to_owned())?;
            // Deliberately does not quote the offending digits: this error can
            // end up in a log, and the digits belong to a key.
            u8::from_str_radix(text, 16)
                .map_err(|_| "a hex string contains a non-hex digit".to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{AuthError, FrameworkError, RequestError};

    /// Builds a snapshot with every field populated.
    fn sample_session() -> SessionData {
        SessionData {
            version: SessionData::VERSION,
            user_id: Some(42),
            phone: Some("+15551234567".to_owned()),
            home_dc_id: Some(2),
            dc_options: vec![DcOption {
                id: 2,
                ipv4: "149.154.167.51:443".to_owned(),
                ipv6: "[2001:67c:4e8:f002::a]:443".to_owned(),
                auth_key: Some(AuthKey::from_bytes([0xab; AuthKey::LEN])),
            }],
            peers: vec![
                Peer::User {
                    id: 42,
                    auth: Some(-1_234_567_890),
                    bot: Some(false),
                    is_self: Some(true),
                },
                Peer::Chat { id: 7 },
                Peer::Channel {
                    id: 9_000_000_000,
                    auth: Some(42),
                    kind: Some(ChannelKind::Megagroup),
                },
            ],
            update_state: UpdateState {
                pts: 12,
                qts: 3,
                date: 1_700_000_000,
                seq: 4,
                channels: vec![ChannelState {
                    id: 9_000_000_000,
                    pts: 5,
                }],
            },
        }
    }

    fn assert_round_trip(store: &dyn SessionStore) {
        assert_eq!(store.load().expect("empty store loads"), None);

        let session = sample_session();
        store.save(&session).expect("save succeeds");
        assert_eq!(store.load().expect("stored session loads"), Some(session));

        store.clear().expect("clear succeeds");
        assert_eq!(store.load().expect("cleared store loads"), None);
    }

    #[test]
    fn memory_store_round_trips() {
        assert_round_trip(&MemoryStore::new());
    }

    #[test]
    fn memory_store_clear_is_idempotent() {
        let store = MemoryStore::new();
        store.clear().expect("first clear succeeds");
        store.clear().expect("second clear succeeds");
    }

    #[test]
    fn file_store_round_trips() {
        let dir = tempfile::tempdir().expect("temp dir is created");
        assert_round_trip(&FileStore::new(dir.path().join("session.json")));
    }

    #[test]
    fn file_store_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().expect("temp dir is created");
        let path = dir.path().join("nested/deeper/session.json");
        let store = FileStore::new(&path);
        store.save(&sample_session()).expect("save succeeds");
        assert!(path.exists());
    }

    #[test]
    fn file_store_rejects_a_corrupt_file() {
        let dir = tempfile::tempdir().expect("temp dir is created");
        let path = dir.path().join("session.json");
        fs::write(&path, b"not json").expect("fixture is written");

        let error = FileStore::new(&path)
            .load()
            .expect_err("corrupt data is rejected");
        assert!(matches!(error, SessionError::Corrupt(_)), "got {error:?}");
    }

    #[test]
    fn file_store_reports_its_path() {
        let store = FileStore::new("/tmp/televim-session.json");
        assert_eq!(store.path(), Path::new("/tmp/televim-session.json"));
    }

    /// The file holds a permanent authorisation key in plaintext, so it is
    /// created for its owner alone. This is a mitigation and not a solution —
    /// [`FileStore`]'s own docs say so — but the mode is the whole of it, so it
    /// is asserted rather than assumed.
    #[cfg(unix)]
    #[test]
    fn file_store_creates_a_file_only_its_owner_can_read() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().expect("temp dir is created");
        let path = dir.path().join("session.json");
        FileStore::new(&path)
            .save(&sample_session())
            .expect("save succeeds");

        let mode = fs::metadata(&path)
            .expect("the file is there")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "the file was created with mode {mode:o}"
        );
    }

    /// A snapshot written before this build named the account still loads, key
    /// and all. The fields are `#[serde(default)]` precisely so that adding
    /// them did not have to cost every existing reader a login.
    #[test]
    fn a_snapshot_written_before_it_named_the_account_still_loads() {
        let key = encode_hex(&[0xab; AuthKey::LEN]);
        let old = format!(
            r#"{{"version":1,"home_dc_id":2,"dc_options":[{{"id":2,
            "ipv4":"149.154.167.51:443","ipv6":"[2001:67c:4e8:f002::a]:443",
            "auth_key":"{key}"}}],"peers":[],"update_state":{{"pts":12,"qts":3,
            "date":1700000000,"seq":4,"channels":[]}}}}"#
        );

        let session =
            SessionData::from_bytes(old.as_bytes()).expect("an old snapshot is still a snapshot");

        assert_eq!(session.account(), None, "it simply does not say who it is");
        assert_eq!(session.user_id, None);
        assert_eq!(session.phone, None);
        assert_eq!(
            session
                .dc_options
                .first()
                .and_then(|dc| dc.auth_key)
                .map(|key| key.as_bytes().to_vec()),
            Some(vec![0xab; AuthKey::LEN]),
            "the authorisation key is the reason the version was not bumped"
        );
        assert_eq!(session.update_state.pts, 12);
    }

    /// The identity is what a caller reads a snapshot without a round trip, so
    /// it has to come back through a backend rather than only off a clone.
    #[test]
    fn the_account_identity_survives_the_stores() {
        let dir = tempfile::tempdir().expect("temp dir is created");
        let session = sample_session();
        let identity = session.account().expect("the sample names an account");
        assert_eq!(identity.user_id, 42);
        assert_eq!(identity.phone.as_deref(), Some("+15551234567"));

        let memory = MemoryStore::new();
        let file = FileStore::new(dir.path().join("session.json"));
        for (name, store) in [
            ("memory", &memory as &dyn SessionStore),
            ("file", &file as &dyn SessionStore),
        ] {
            store.save(&session).expect("save succeeds");
            assert_eq!(
                store
                    .load()
                    .expect("load succeeds")
                    .and_then(|read| read.account()),
                Some(identity.clone()),
                "the {name} store lost the account identity"
            );
            store.clear().expect("clear succeeds");
        }
    }

    /// The credential store is not available on a headless CI machine, so this
    /// is opt-in like the round trip above it.
    #[test]
    #[ignore = "requires a working OS credential store"]
    fn the_account_identity_survives_the_keyring() {
        let store = KeyringStore::new("televim-tests-identity", "session");
        store.clear().expect("clear succeeds");

        let session = sample_session();
        store.save(&session).expect("save succeeds");
        assert_eq!(
            store
                .load()
                .expect("load succeeds")
                .and_then(|read| read.account()),
            session.account()
        );

        store.clear().expect("final clear succeeds");
    }

    /// Half a snapshot is not a session to fall back on, and silently starting
    /// over is the one answer that cannot be explained on a screen. Both halves
    /// of the file's bytes are refused rather than read as far as they go.
    #[test]
    fn a_truncated_snapshot_is_reported_rather_than_reset() {
        let bytes = sample_session().to_bytes().expect("serialises");
        let truncated = &bytes[..bytes.len() / 2];

        let error = SessionData::from_bytes(truncated).expect_err("half a snapshot is refused");
        assert!(matches!(error, SessionError::Corrupt(_)), "got {error:?}");

        let dir = tempfile::tempdir().expect("temp dir is created");
        let path = dir.path().join("session.json");
        fs::write(&path, truncated).expect("fixture is written");

        let error = FileStore::new(&path)
            .load()
            .expect_err("a truncated file is refused");
        assert!(matches!(error, SessionError::Corrupt(_)), "got {error:?}");
    }

    /// Clearing has to leave nothing behind that a later launch would restore.
    #[cfg(feature = "live")]
    #[test]
    fn clearing_the_session_forgets_the_key_and_the_account() {
        use std::sync::Arc;

        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let session =
            super::bridge::StoreSession::new(Arc::clone(&store)).expect("the stored session loads");

        session.clear().expect("clearing succeeds");

        assert!(
            store.load().expect("the store is readable").is_none(),
            "the stored key must be gone, not merely unused"
        );
        let left = session.snapshot();
        assert!(
            left.dc_options
                .iter()
                .all(|option| option.auth_key.is_none()),
            "no authorisation key may be left in memory: {:?}",
            left.dc_options
        );
        assert_eq!(left.user_id, None, "nor may the account be");
        assert_eq!(
            session.cached_peer(42),
            None,
            "the mirror is the state a client that has never signed in has"
        );
    }

    /// The update feed never reads this cache: a conversation comes from the
    /// peer encoded in the message, and your own user only ever turns up as a
    /// *sender*, which is synthesised from the outgoing flag. So the account's
    /// own peer does not have to be cached for the feed to work after a login,
    /// and the client needs nothing extra before it can subscribe.
    #[cfg(feature = "live")]
    #[test]
    fn the_accounts_own_peer_is_not_needed_by_the_session() {
        let fresh = super::bridge::StoreSession::new(std::sync::Arc::new(MemoryStore::new()))
            .expect("a fresh session loads");

        assert!(
            fresh.snapshot().peers.is_empty(),
            "nothing has been fetched yet, so there is no peer to cache"
        );
        assert_eq!(
            fresh.cached_peer(42),
            None,
            "the account's own peer is not here and the feed does not look for it"
        );
    }

    #[test]
    fn session_data_round_trips_through_json() {
        let session = sample_session();
        let bytes = session.to_bytes().expect("serialises");
        assert_eq!(
            SessionData::from_bytes(&bytes).expect("deserialises"),
            session
        );
    }

    #[test]
    fn session_data_rejects_an_unknown_version() {
        let mut session = sample_session();
        session.version = SessionData::VERSION + 1;
        let bytes = session.to_bytes().expect("serialises");

        let error = SessionData::from_bytes(&bytes).expect_err("version is rejected");
        assert!(matches!(error, SessionError::Corrupt(_)), "got {error:?}");
    }

    #[test]
    fn session_data_tolerates_missing_fields() {
        let session =
            SessionData::from_bytes(br#"{"version":1}"#).expect("minimal snapshot parses");
        assert_eq!(session.home_dc_id, None);
        assert!(session.dc_options.is_empty());
        assert!(session.peers.is_empty());
    }

    #[test]
    fn auth_key_debug_never_leaks_key_material() {
        let key = AuthKey::from_bytes([0xab; AuthKey::LEN]);
        assert_eq!(format!("{key:?}"), "AuthKey(<redacted>)");
    }

    #[test]
    fn auth_key_rejects_a_short_key() {
        let error = serde_json::from_str::<AuthKey>("\"00ff\"").expect_err("short key is rejected");
        assert!(error.to_string().contains("256 bytes"), "got {error}");
    }

    #[test]
    fn hex_codec_round_trips() {
        let bytes: Vec<u8> = (0..=255).collect();
        assert_eq!(decode_hex(&encode_hex(&bytes)).expect("decodes"), bytes);
    }

    #[test]
    fn hex_codec_accepts_uppercase() {
        assert_eq!(decode_hex("AB").expect("decodes"), vec![0xab]);
    }

    #[test]
    fn hex_codec_rejects_bad_input() {
        assert!(decode_hex("abc").is_err(), "odd length");
        assert!(decode_hex("zz").is_err(), "non-hex digits");
    }

    #[test]
    fn session_errors_render_a_useful_message() {
        let error = SessionError::Unavailable("no keyring".to_owned());
        assert_eq!(
            error.to_string(),
            "the session store is unavailable: no keyring"
        );
    }

    #[test]
    fn auth_errors_render_a_useful_message() {
        assert_eq!(
            AuthError::TokenAlreadyUsed.to_string(),
            "this login token has already been used; request a new login code"
        );
        assert_eq!(
            AuthError::RateLimited {
                retry_after: Some(30)
            }
            .to_string(),
            "telegram is rate limiting this request"
        );
    }

    #[test]
    fn framework_error_wraps_auth_and_session_errors() {
        let error = FrameworkError::from(AuthError::InvalidCode);
        assert_eq!(
            error.to_string(),
            "the login code is invalid or has expired"
        );

        let error = FrameworkError::from(SessionError::Clear("denied".to_owned()));
        assert_eq!(
            error.to_string(),
            "could not clear the stored session: denied"
        );
    }

    #[test]
    fn request_errors_render_a_useful_message() {
        let error = RequestError::Rpc {
            code: 420,
            name: "FLOOD_WAIT".to_owned(),
            value: Some(31),
        };
        assert_eq!(
            error.to_string(),
            "telegram returned rpc error 420 FLOOD_WAIT"
        );
    }

    /// The keyring is not available on a headless CI machine, so this is opt-in.
    #[test]
    #[ignore = "requires a working OS credential store"]
    fn keyring_store_round_trips() {
        let store = KeyringStore::new("televim-tests", "session");
        store.clear().expect("clear succeeds");
        assert_round_trip(&store);
        store.clear().expect("final clear succeeds");
    }

    /// Builds a `grammers`-backed session on top of an in-memory store.
    #[cfg(feature = "live")]
    fn bridge_session(snapshot: &SessionData) -> super::bridge::StoreSession {
        use std::sync::Arc;

        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        store.save(snapshot).expect("the snapshot is stored");
        super::bridge::StoreSession::new(store).expect("the snapshot loads")
    }

    /// The whole persistence path, without touching the network: a snapshot goes
    /// into a `grammers` session and comes back out again unchanged.
    #[cfg(feature = "live")]
    #[test]
    fn bridge_round_trips_a_snapshot() {
        let original = sample_session();
        let restored = bridge_session(&original).snapshot();

        assert_eq!(restored.home_dc_id, original.home_dc_id);
        assert_eq!(restored.update_state, original.update_state);
        assert_eq!(
            restored.peers.len(),
            original.peers.len(),
            "a peer was lost: {:?}",
            restored.peers
        );
        for peer in &original.peers {
            assert!(restored.peers.contains(peer), "lost {peer:?}");
        }

        let dc = restored
            .dc_options
            .iter()
            .find(|option| option.id == 2)
            .expect("datacenter 2 survives the round trip");
        assert_eq!(dc, &original.dc_options[0]);
    }

    /// `grammers` panics on an out-of-range peer identifier, so a corrupt
    /// snapshot has to be filtered rather than handed over.
    #[cfg(feature = "live")]
    #[test]
    fn bridge_skips_out_of_range_peers() {
        let snapshot = SessionData {
            peers: vec![
                Peer::User {
                    id: -1,
                    auth: None,
                    bot: None,
                    is_self: None,
                },
                Peer::Chat { id: 0 },
                Peer::User {
                    id: 42,
                    auth: None,
                    bot: None,
                    is_self: None,
                },
            ],
            ..SessionData::default()
        };

        let restored = bridge_session(&snapshot).snapshot();
        assert_eq!(
            restored.peers,
            vec![Peer::User {
                id: 42,
                auth: None,
                bot: None,
                is_self: None,
            }],
            "only the in-range peer should survive"
        );
    }

    /// A peer is looked up by the bare identifier this crate hands out, which is
    /// not the identifier `grammers` keys its cache by — the cache packs the
    /// peer's kind into the same integer.
    #[cfg(feature = "live")]
    #[test]
    fn bridge_finds_a_cached_peer_by_its_bare_identifier() {
        use grammers_client::session::types::{PeerAuth, PeerInfo};

        let session = bridge_session(&sample_session());

        let peer = session
            .cached_peer(42)
            .expect("user 42 is part of the sample session");
        let PeerInfo::User { id, auth, .. } = peer else {
            panic!("42 is a user, got {peer:?}");
        };
        assert_eq!(id, 42);
        assert_eq!(
            auth.map(PeerAuth::hash),
            Some(-1_234_567_890),
            "the access hash is what addressing the peer needs"
        );

        assert!(
            matches!(session.cached_peer(7), Some(PeerInfo::Chat { id: 7 })),
            "a small group is found by the same rule"
        );
        assert!(
            session.cached_peer(404).is_none(),
            "a peer the session has never seen has no access hash to give"
        );
    }

    /// The point of the dirty flag: a mutation the transport makes between
    /// logins has to survive a restart, and the store must not be written when
    /// nothing changed.
    #[cfg(feature = "live")]
    #[tokio::test]
    async fn bridge_writes_a_datacenter_migration_back_to_the_store() {
        use std::sync::Arc;

        use grammers_client::session::Session as _;

        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let session =
            super::bridge::StoreSession::new(Arc::clone(&store)).expect("a fresh session loads");

        assert!(
            !session.persist_if_dirty().expect("the check succeeds"),
            "a session nothing has touched is not worth writing"
        );

        session
            .set_home_dc_id(4)
            .await
            .expect("the mirror cannot fail");
        assert!(
            session.persist_if_dirty().expect("the write succeeds"),
            "a migration has to be written back"
        );
        assert!(
            !session.persist_if_dirty().expect("the check succeeds"),
            "the flag has to clear once the write succeeded"
        );

        drop(session);

        let restored = super::bridge::StoreSession::new(store).expect("the stored session loads");
        assert_eq!(
            restored.home_dc_id().expect("the mirror cannot fail"),
            4,
            "a rebuilt session must come back on the datacenter it migrated to"
        );
    }

    /// A cached peer and a moved update counter are the other two mutations that
    /// happen outside the login flow, and they have to raise the flag too.
    #[cfg(feature = "live")]
    #[tokio::test]
    async fn bridge_marks_a_cached_peer_and_a_moved_counter_dirty() {
        use std::sync::Arc;

        use grammers_client::session::Session as _;
        use grammers_client::session::types::{PeerInfo, UpdateState as TlUpdateState};

        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let session = super::bridge::StoreSession::new(store).expect("a fresh session loads");

        session
            .cache_peer(&PeerInfo::User {
                id: 42,
                auth: None,
                bot: None,
                is_self: None,
            })
            .await
            .expect("the mirror cannot fail");
        assert!(
            session.persist_if_dirty().expect("the write succeeds"),
            "a cached peer is a change worth keeping"
        );

        session
            .set_update_state(TlUpdateState::Primary {
                pts: 7,
                date: 0,
                seq: 1,
            })
            .await
            .expect("the mirror cannot fail");
        assert!(
            session.persist_if_dirty().expect("the write succeeds"),
            "a moved update counter is a change worth keeping"
        );
    }

    /// A store that refuses every write, so the retry path can be exercised.
    #[cfg(feature = "live")]
    #[derive(Default)]
    struct FailingStore {
        attempts: std::sync::atomic::AtomicUsize,
    }

    #[cfg(feature = "live")]
    impl SessionStore for FailingStore {
        fn load(&self) -> Result<Option<SessionData>, SessionError> {
            Ok(None)
        }

        fn save(&self, _session: &SessionData) -> Result<(), SessionError> {
            self.attempts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(SessionError::Save(
                "the credential store is locked".to_owned(),
            ))
        }

        fn clear(&self) -> Result<(), SessionError> {
            Ok(())
        }
    }

    /// A write that fails has to leave the flag raised, or the change would be
    /// dropped as soon as the store has a bad day.
    #[cfg(feature = "live")]
    #[tokio::test]
    async fn bridge_retries_a_write_the_store_refused() {
        use std::sync::Arc;
        use std::sync::atomic::Ordering;

        use grammers_client::session::Session as _;

        let store = Arc::new(FailingStore::default());
        let session = super::bridge::StoreSession::new(Arc::clone(&store) as Arc<dyn SessionStore>)
            .expect("a fresh session loads");

        session
            .set_home_dc_id(4)
            .await
            .expect("the mirror cannot fail");

        assert!(
            session.persist_if_dirty().is_err(),
            "the store is supposed to refuse this write"
        );
        assert!(
            session.persist_if_dirty().is_err(),
            "a refused write has to be retried rather than quietly forgotten"
        );
        assert_eq!(
            store.attempts.load(Ordering::SeqCst),
            2,
            "the second flush should have tried again"
        );
    }
}
