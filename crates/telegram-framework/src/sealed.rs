//! Encryption of the session bytes at rest.
//!
//! [`seal`] wraps the output of [`SessionData::to_bytes`](crate::SessionData)
//! in an authenticated envelope; [`open`] undoes it. The envelope is what a
//! [`FileStore`](crate::FileStore) puts on disk, so the authorisation key never
//! sits in a file in the clear.
//!
//! # Envelope
//!
//! ```text
//! "TVIM1" | salt (16) | nonce (12) | ciphertext | tag (16)
//! ```
//!
//! * The cipher is AES-256-GCM-SIV (RFC 8452). It is nonce-misuse-resistant:
//!   a repeated random nonce leaks only that two plaintexts were equal, instead
//!   of the authentication key.
//! * The magic and the salt are bound as associated data, so editing either
//!   fails the tag.
//! * The salt feeds the passphrase KDF (Argon2id). A [`KeyringKeyProvider`]
//!   already holds a random 256-bit key and ignores it.
//!
//! The magic can never open a legacy plaintext session, which is JSON and so
//! begins with `{`. [`is_sealed`] tells the two apart before any parsing; the
//! schema [`SessionData::VERSION`](crate::SessionData) is untouched, because the
//! plaintext inside the envelope is byte-for-byte what it was.
//!
//! # Failure modes
//!
//! [`open`] separates three outcomes, because the caller must not treat them
//! alike:
//!
//! * `Ok(None)` — not an envelope. Legacy plaintext; parse it as before.
//! * `Ok(Some(_))` — the plaintext.
//! * `Err(`[`SessionError::Load`]`)` — an envelope that cannot be unsealed: the
//!   wrong key, a modified or truncated file. **Never** [`SessionError::Corrupt`],
//!   which a caller may answer by deleting the file; a typo in a passphrase
//!   must not cost the user their session.
//!
//! A key source that cannot be reached at all is [`SessionError::Unavailable`].
//!
//! # Keys
//!
//! A [`KeyProvider`] supplies the 256-bit [`FileKey`]. [`PassphraseProvider`]
//! stretches a passphrase; [`KeyringKeyProvider`] keeps a random key in the OS
//! credential store. Key material is wiped when dropped, its `Debug` is
//! redacted, and nothing here logs it.

use std::fmt;
use std::sync::{Mutex, MutexGuard, PoisonError};

use aes_gcm_siv::aead::{Aead, Payload};
use aes_gcm_siv::{Aes256GcmSiv, Key, KeyInit, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::Zeroizing;

use crate::error::SessionError;

/// The first bytes of every envelope.
pub const MAGIC: &[u8; 5] = b"TVIM1";

/// Length of a file key, in bytes.
pub const KEY_LEN: usize = 32;

/// Length of the KDF salt carried in the envelope, in bytes.
pub const SALT_LEN: usize = 16;

/// Length of the AEAD nonce carried in the envelope, in bytes.
const NONCE_LEN: usize = 12;

/// Length of the AEAD authentication tag, in bytes.
const TAG_LEN: usize = 16;

/// Bytes before the ciphertext: magic, salt and nonce.
const HEADER_LEN: usize = MAGIC.len() + SALT_LEN + NONCE_LEN;

/// Bytes authenticated but not encrypted: magic and salt.
const AAD_LEN: usize = MAGIC.len() + SALT_LEN;

/// Argon2id memory cost, in KiB (19 MiB, the OWASP minimum).
///
/// Fixed here rather than taken from the crate's defaults: these numbers define
/// what a passphrase derives to, so changing them orphans every file already
/// written. A different cost would need a new magic.
const KDF_MEMORY_KIB: u32 = 19 * 1024;
/// Argon2id passes over memory.
const KDF_PASSES: u32 = 2;
/// Argon2id lanes.
const KDF_LANES: u32 = 1;

/// The KDF salt of an envelope.
pub type Salt = [u8; SALT_LEN];

/// A 256-bit key that seals and unseals the session file.
///
/// Wiped when dropped. Its [`Debug`] implementation never prints key material.
#[derive(Clone)]
pub struct FileKey(Zeroizing<[u8; KEY_LEN]>);

impl FileKey {
    /// Wraps raw key bytes.
    #[must_use]
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    fn generate() -> Result<Self, SessionError> {
        let mut bytes = Zeroizing::new([0u8; KEY_LEN]);
        random(&mut *bytes)?;
        Ok(Self(bytes))
    }
}

impl fmt::Debug for FileKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FileKey(<redacted>)")
    }
}

/// Where the [`FileKey`] comes from.
///
/// Held by a store as an `Arc<dyn KeyProvider>`, so it is `Send + Sync`.
/// Implementations report a key source that cannot be reached as
/// [`SessionError::Unavailable`] and a key that exists but is wrong or
/// missing as [`SessionError::Load`]; neither is ever `Corrupt`.
pub trait KeyProvider: Send + Sync + fmt::Debug {
    /// The key to seal a new envelope with, and the salt to record in it.
    ///
    /// May create the key: the keyring provider generates one on first use.
    fn sealing_key(&self) -> Result<(Salt, FileKey), SessionError>;

    /// The key an envelope carrying `salt` was sealed with.
    ///
    /// Never creates a key: there is nothing to open with one that was just made.
    fn opening_key(&self, salt: &Salt) -> Result<FileKey, SessionError>;
}

/// Derives the file key from a passphrase with Argon2id.
///
/// The derivation costs tens of milliseconds and 19 MiB, so the key is kept for
/// the salt it was derived with: a store that loads and then saves pays for it
/// once.
pub struct PassphraseProvider {
    passphrase: Zeroizing<String>,
    derived: Mutex<Option<(Salt, FileKey)>>,
}

impl PassphraseProvider {
    /// A provider for `passphrase`.
    ///
    /// An empty passphrase is accepted here and refused when a key is asked for,
    /// so that a blank environment variable surfaces as
    /// [`SessionError::Unavailable`] at the point of use.
    #[must_use]
    pub fn new(passphrase: impl Into<String>) -> Self {
        Self {
            passphrase: Zeroizing::new(passphrase.into()),
            derived: Mutex::new(None),
        }
    }

    fn derive(&self, salt: &Salt) -> Result<FileKey, SessionError> {
        if self.passphrase.is_empty() {
            return Err(SessionError::Unavailable("the passphrase is empty".into()));
        }
        let params = Params::new(KDF_MEMORY_KIB, KDF_PASSES, KDF_LANES, Some(KEY_LEN))
            .map_err(|error| SessionError::Unavailable(format!("bad KDF parameters: {error}")))?;
        let mut out = Zeroizing::new([0u8; KEY_LEN]);
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into(self.passphrase.as_bytes(), salt, &mut *out)
            .map_err(|error| {
                SessionError::Unavailable(format!("key derivation failed: {error}"))
            })?;
        Ok(FileKey(out))
    }
}

impl KeyProvider for PassphraseProvider {
    fn sealing_key(&self) -> Result<(Salt, FileKey), SessionError> {
        let mut derived = lock(&self.derived);
        if let Some((salt, key)) = derived.as_ref() {
            return Ok((*salt, key.clone()));
        }
        let mut salt = [0u8; SALT_LEN];
        random(&mut salt)?;
        let key = self.derive(&salt)?;
        *derived = Some((salt, key.clone()));
        Ok((salt, key))
    }

    fn opening_key(&self, salt: &Salt) -> Result<FileKey, SessionError> {
        let mut derived = lock(&self.derived);
        if let Some((cached, key)) = derived.as_ref()
            && cached == salt
        {
            return Ok(key.clone());
        }
        let key = self.derive(salt)?;
        *derived = Some((*salt, key.clone()));
        Ok(key)
    }
}

impl fmt::Debug for PassphraseProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PassphraseProvider(<redacted>)")
    }
}

/// Keeps a random 256-bit file key in the OS credential store.
///
/// The key is created on the first [`sealing_key`](KeyProvider::sealing_key)
/// call, never on an open. On a machine with no credential store every call
/// fails with [`SessionError::Unavailable`]; give such a machine a
/// [`PassphraseProvider`].
pub struct KeyringKeyProvider {
    service: String,
    account: String,
    cached: Mutex<Option<FileKey>>,
}

impl KeyringKeyProvider {
    /// Service name used by [`KeyringKeyProvider::default`].
    pub const DEFAULT_SERVICE: &'static str = "televim";

    /// Account name used by [`KeyringKeyProvider::default`].
    pub const DEFAULT_ACCOUNT: &'static str = "file-key";

    /// A provider for the given `service`/`account` pair.
    #[must_use]
    pub fn new(service: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            account: account.into(),
            cached: Mutex::new(None),
        }
    }

    /// The entry is recreated for every lookup, as [`KeyringStore`](crate::KeyringStore)
    /// does, so the provider stays free of platform handles.
    fn entry(&self) -> Result<keyring::Entry, SessionError> {
        keyring::Entry::new(&self.service, &self.account)
            .map_err(|error| SessionError::Unavailable(error.to_string()))
    }

    fn resolve(&self, create: bool) -> Result<FileKey, SessionError> {
        let mut cached = lock(&self.cached);
        if let Some(key) = cached.as_ref() {
            return Ok(key.clone());
        }
        let key = key_from_entry(&self.entry()?, create)?;
        *cached = Some(key.clone());
        Ok(key)
    }
}

impl Default for KeyringKeyProvider {
    fn default() -> Self {
        Self::new(Self::DEFAULT_SERVICE, Self::DEFAULT_ACCOUNT)
    }
}

impl KeyProvider for KeyringKeyProvider {
    fn sealing_key(&self) -> Result<(Salt, FileKey), SessionError> {
        Ok(([0; SALT_LEN], self.resolve(true)?))
    }

    fn opening_key(&self, _salt: &Salt) -> Result<FileKey, SessionError> {
        self.resolve(false)
    }
}

impl fmt::Debug for KeyringKeyProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyringKeyProvider")
            .field("service", &self.service)
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

/// Reads the key held by `entry`, generating and storing one if `create` and
/// there is none.
fn key_from_entry(entry: &keyring::Entry, create: bool) -> Result<FileKey, SessionError> {
    match entry.get_secret() {
        Ok(secret) => {
            let secret = Zeroizing::new(secret);
            <[u8; KEY_LEN]>::try_from(secret.as_slice())
                .map(FileKey::from_bytes)
                .map_err(|_| {
                    SessionError::Load("the file key in the OS keyring is malformed".into())
                })
        }
        Err(keyring::Error::NoEntry) if create => {
            let key = FileKey::generate()?;
            entry
                .set_secret(key.0.as_slice())
                .map_err(|error| SessionError::Unavailable(error.to_string()))?;
            Ok(key)
        }
        Err(keyring::Error::NoEntry) => Err(SessionError::Load(
            "the session is encrypted but the OS keyring holds no file key for it".into(),
        )),
        Err(error) => Err(SessionError::Unavailable(error.to_string())),
    }
}

/// Whether `bytes` begin with the envelope magic.
///
/// A cheap prefix test, made before any parsing: `true` means an envelope,
/// valid or not; `false` means legacy plaintext.
#[must_use]
pub fn is_sealed(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

/// Seals `plaintext` under the key `provider` supplies.
///
/// A fresh random nonce is used for every call, so sealing the same bytes twice
/// gives different output.
pub fn seal(plaintext: &[u8], provider: &dyn KeyProvider) -> Result<Vec<u8>, SessionError> {
    let (salt, key) = provider.sealing_key()?;
    let mut nonce = [0u8; NONCE_LEN];
    random(&mut nonce)?;

    let mut out = Vec::with_capacity(HEADER_LEN + plaintext.len() + TAG_LEN);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&salt);
    let ciphertext = cipher(&key)
        .encrypt(
            &Nonce::from(nonce),
            Payload {
                msg: plaintext,
                aad: &out,
            },
        )
        .map_err(|_| SessionError::Save("could not encrypt the session".into()))?;
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Opens an envelope with the key `provider` supplies.
///
/// Returns `Ok(None)` when `bytes` are not an envelope (see [`is_sealed`]),
/// leaving the caller to treat them as legacy plaintext.
///
/// # Errors
///
/// [`SessionError::Load`] when `bytes` are an envelope that does not open: the
/// wrong key, or a file that was truncated or modified. The two cannot be told
/// apart, and neither is a reason to discard the file. Whatever the provider
/// reports, such as [`SessionError::Unavailable`], passes through.
pub fn open(
    bytes: &[u8],
    provider: &dyn KeyProvider,
) -> Result<Option<Zeroizing<Vec<u8>>>, SessionError> {
    if !is_sealed(bytes) {
        return Ok(None);
    }
    if bytes.len() < HEADER_LEN + TAG_LEN {
        return Err(SessionError::Load(
            "the encrypted session is truncated".into(),
        ));
    }
    let (aad, rest) = bytes.split_at(AAD_LEN);
    let (nonce, ciphertext) = rest.split_at(NONCE_LEN);
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&aad[MAGIC.len()..]);
    let mut nonce_bytes = [0u8; NONCE_LEN];
    nonce_bytes.copy_from_slice(nonce);

    let key = provider.opening_key(&salt)?;
    cipher(&key)
        .decrypt(
            &Nonce::from(nonce_bytes),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map(|plaintext| Some(Zeroizing::new(plaintext)))
        .map_err(|_| {
            SessionError::Load(
                "could not decrypt the session: wrong passphrase or key, or the file was modified"
                    .into(),
            )
        })
}

fn cipher(key: &FileKey) -> Aes256GcmSiv {
    Aes256GcmSiv::new(&Key::<Aes256GcmSiv>::from(*key.0))
}

fn random(buffer: &mut [u8]) -> Result<(), SessionError> {
    getrandom::fill(buffer)
        .map_err(|error| SessionError::Unavailable(format!("no source of randomness: {error}")))
}

/// Locks `mutex`, ignoring poison: the cached value is replaced whole, so a
/// panic elsewhere cannot leave it half-written.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use keyring::Entry;
    use keyring::mock::MockCredential;

    use super::*;

    const PLAINTEXT: &[u8] = br#"{"version":1,"auth_key":"00"}"#;

    /// A provider with a fixed key and no KDF, so most tests stay fast.
    #[derive(Debug)]
    struct FixedProvider(u8);

    impl KeyProvider for FixedProvider {
        fn sealing_key(&self) -> Result<(Salt, FileKey), SessionError> {
            Ok(([self.0; SALT_LEN], FileKey::from_bytes([self.0; KEY_LEN])))
        }

        fn opening_key(&self, _salt: &Salt) -> Result<FileKey, SessionError> {
            Ok(FileKey::from_bytes([self.0; KEY_LEN]))
        }
    }

    fn mock_entry() -> Entry {
        Entry::new_with_credential(Box::new(MockCredential::default()))
    }

    #[test]
    fn round_trips() {
        let sealed = seal(PLAINTEXT, &FixedProvider(1)).expect("seal");
        let opened = open(&sealed, &FixedProvider(1)).expect("open");
        assert_eq!(opened.as_deref().map(Vec::as_slice), Some(PLAINTEXT));
    }

    #[test]
    fn round_trips_empty_plaintext() {
        let sealed = seal(b"", &FixedProvider(1)).expect("seal");
        let opened = open(&sealed, &FixedProvider(1)).expect("open");
        assert_eq!(opened.as_deref().map(Vec::as_slice), Some(&b""[..]));
    }

    #[test]
    fn sealed_bytes_carry_the_magic_and_hide_the_plaintext() {
        let sealed = seal(PLAINTEXT, &FixedProvider(1)).expect("seal");
        assert!(sealed.starts_with(b"TVIM1"));
        assert!(is_sealed(&sealed));
        assert!(serde_json::from_slice::<serde_json::Value>(&sealed).is_err());
        assert!(!sealed.windows(PLAINTEXT.len()).any(|w| w == PLAINTEXT));
        assert_eq!(sealed.len(), HEADER_LEN + PLAINTEXT.len() + TAG_LEN);
    }

    #[test]
    fn every_seal_uses_a_fresh_nonce() {
        let a = seal(PLAINTEXT, &FixedProvider(1)).expect("seal");
        let b = seal(PLAINTEXT, &FixedProvider(1)).expect("seal");
        assert_ne!(a, b);
    }

    #[test]
    fn plaintext_is_not_an_envelope() {
        assert!(!is_sealed(PLAINTEXT));
        assert!(!is_sealed(b""));
        assert!(!is_sealed(b"TVIM"));
        assert!(
            open(PLAINTEXT, &FixedProvider(1))
                .expect("not an error")
                .is_none()
        );
        assert!(open(b"", &FixedProvider(1)).expect("empty").is_none());
    }

    #[test]
    fn a_flipped_bit_anywhere_fails_closed() {
        let sealed = seal(PLAINTEXT, &FixedProvider(1)).expect("seal");
        for index in 0..sealed.len() {
            let mut tampered = sealed.clone();
            tampered[index] ^= 0x01;
            // Flipping a magic byte makes it legacy plaintext, which is the
            // caller's to reject; every other byte must be an error.
            match open(&tampered, &FixedProvider(1)) {
                Ok(None) => assert!(index < MAGIC.len(), "byte {index} opened"),
                Ok(Some(_)) => panic!("byte {index} opened"),
                Err(SessionError::Load(_)) => {}
                Err(other) => panic!("byte {index}: unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn wrong_key_is_load_not_corrupt() {
        let sealed = seal(PLAINTEXT, &FixedProvider(1)).expect("seal");
        assert!(matches!(
            open(&sealed, &FixedProvider(2)),
            Err(SessionError::Load(_))
        ));
    }

    #[test]
    fn truncation_is_load_not_corrupt() {
        let sealed = seal(PLAINTEXT, &FixedProvider(1)).expect("seal");
        for len in MAGIC.len()..sealed.len() {
            assert!(
                matches!(
                    open(&sealed[..len], &FixedProvider(1)),
                    Err(SessionError::Load(_))
                ),
                "length {len}"
            );
        }
        assert!(matches!(
            open(MAGIC, &FixedProvider(1)),
            Err(SessionError::Load(_))
        ));
    }

    #[test]
    fn passphrase_round_trips_across_providers() {
        let sealed = seal(PLAINTEXT, &PassphraseProvider::new("correct horse")).expect("seal");
        // A new provider has no cache, so this derives from the stored salt.
        let opened = open(&sealed, &PassphraseProvider::new("correct horse")).expect("open");
        assert_eq!(opened.as_deref().map(Vec::as_slice), Some(PLAINTEXT));
    }

    #[test]
    fn wrong_passphrase_is_load_not_corrupt() {
        let sealed = seal(PLAINTEXT, &PassphraseProvider::new("correct horse")).expect("seal");
        assert!(matches!(
            open(&sealed, &PassphraseProvider::new("battery staple")),
            Err(SessionError::Load(_))
        ));
    }

    #[test]
    fn passphrase_provider_derives_once_per_salt() {
        let provider = PassphraseProvider::new("correct horse");
        let (salt, first) = provider.sealing_key().expect("key");
        let (again, second) = provider.sealing_key().expect("key");
        assert_eq!(salt, again);
        assert_eq!(first.0.as_slice(), second.0.as_slice());
        let opened = provider.opening_key(&salt).expect("key");
        assert_eq!(first.0.as_slice(), opened.0.as_slice());
    }

    #[test]
    fn different_salts_give_different_keys() {
        let provider = PassphraseProvider::new("correct horse");
        let a = provider.opening_key(&[1; SALT_LEN]).expect("key");
        let b = provider.opening_key(&[2; SALT_LEN]).expect("key");
        assert_ne!(a.0.as_slice(), b.0.as_slice());
    }

    #[test]
    fn empty_passphrase_is_unavailable() {
        assert!(matches!(
            seal(PLAINTEXT, &PassphraseProvider::new("")),
            Err(SessionError::Unavailable(_))
        ));
    }

    #[test]
    fn keyring_key_is_created_on_seal_and_reused() {
        let entry = mock_entry();
        let first = key_from_entry(&entry, true).expect("created");
        let second = key_from_entry(&entry, true).expect("reused");
        assert_eq!(first.0.as_slice(), second.0.as_slice());
        let opened = key_from_entry(&entry, false).expect("read");
        assert_eq!(first.0.as_slice(), opened.0.as_slice());
    }

    #[test]
    fn keyring_open_never_creates_a_key() {
        let entry = mock_entry();
        assert!(matches!(
            key_from_entry(&entry, false),
            Err(SessionError::Load(_))
        ));
        assert!(matches!(entry.get_secret(), Err(keyring::Error::NoEntry)));
    }

    #[test]
    fn malformed_keyring_key_is_load_and_left_alone() {
        let entry = mock_entry();
        entry.set_secret(b"short").expect("set");
        for create in [true, false] {
            assert!(matches!(
                key_from_entry(&entry, create),
                Err(SessionError::Load(_))
            ));
        }
        assert_eq!(entry.get_secret().expect("get"), b"short");
    }

    #[test]
    fn debug_never_prints_key_material() {
        let key = FileKey::from_bytes([0xAB; KEY_LEN]);
        assert_eq!(format!("{key:?}"), "FileKey(<redacted>)");
        let provider = PassphraseProvider::new("hunter2");
        assert!(!format!("{provider:?}").contains("hunter2"));
    }

    #[test]
    fn providers_are_usable_behind_an_arc() {
        let provider: std::sync::Arc<dyn KeyProvider> = std::sync::Arc::new(FixedProvider(3));
        let sealed = seal(PLAINTEXT, provider.as_ref()).expect("seal");
        assert!(open(&sealed, provider.as_ref()).expect("open").is_some());
        static_assertions::assert_impl_all!(PassphraseProvider: Send, Sync);
        static_assertions::assert_impl_all!(KeyringKeyProvider: Send, Sync);
    }
}
