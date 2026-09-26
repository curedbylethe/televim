//! The `grammers`-backed client: [`ClientBuilder`] and [`Client`].

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use grammers_mtsender::SenderPool;

use crate::auth::{LoginToken, PasswordToken, SignInResult};
use crate::error::{AuthError, FrameworkError, RequestError};
use crate::session::{KeyringStore, SessionStore, StoreSession};
use crate::updates::UpdateRelay;

/// How long after a login-code request a second one starts looking suspicious.
///
/// Telegram does not publish its limit and applies it per phone number, based
/// on the account's history, so this is a "you are about to be throttled" nudge
/// rather than a rule the wrapper enforces.
const CODE_REQUEST_COOLDOWN: Duration = Duration::from_secs(60);

/// Remembers when a login code was last requested.
///
/// It lives in its own type so the cooldown decision can be tested without a
/// client or a datacenter: the `warn!` is one line, and the comparison is the
/// part that can be wrong.
#[derive(Debug, Default)]
struct CodeRequestLog {
    last: Mutex<Option<Instant>>,
}

impl CodeRequestLog {
    /// Records a request made at `now`, reporting whether it followed closely
    /// enough on the previous one to be worth warning about.
    fn record(&self, now: Instant) -> bool {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);

        let is_repeat = last.is_some_and(|previous| {
            now.saturating_duration_since(previous) < CODE_REQUEST_COOLDOWN
        });
        *last = Some(now);

        is_repeat
    }
}

/// Builds a [`Client`].
///
/// ```no_run
/// use telegram_framework::{ClientBuilder, KeyringStore};
///
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let client = ClientBuilder::new(1234, "api-hash")
///     .session_store(Box::new(KeyringStore::default()))
///     .build()
///     .await?;
///
/// if !client.is_authorized().await? {
///     // …run the login flow…
/// }
/// # Ok(())
/// # }
/// ```
pub struct ClientBuilder {
    api_id: i32,
    api_hash: String,
    session_store: Option<Box<dyn SessionStore>>,
}

impl ClientBuilder {
    /// Creates a builder for a pair of Telegram API credentials.
    ///
    /// `api_id` and `api_hash` identify the *application*, not the user, and
    /// come from <https://my.telegram.org>. Every user of the built client logs
    /// in under these credentials.
    #[must_use]
    pub fn new(api_id: i32, api_hash: impl Into<String>) -> Self {
        Self {
            api_id,
            api_hash: api_hash.into(),
            session_store: None,
        }
    }

    /// Overrides where the session is persisted.
    ///
    /// Defaults to [`KeyringStore::default`], which keeps the authorisation key
    /// in the OS credential store. Pass a [`FileStore`](crate::FileStore) on
    /// machines that have no keyring, or a
    /// [`MemoryStore`](crate::MemoryStore) for a session that must not outlive
    /// the process.
    #[must_use]
    pub fn session_store(mut self, store: Box<dyn SessionStore>) -> Self {
        self.session_store = Some(store);
        self
    }

    /// Loads the session, starts the network, and returns a [`Client`].
    ///
    /// This does not log in: it restores whatever session the store held. Call
    /// [`Client::is_authorized`] to find out whether a login is still needed.
    ///
    /// Must be called from within a `tokio` runtime, because the connection
    /// pool is spawned onto it. The signature is `async` even though nothing is
    /// awaited: awaiting the builder is what makes the runtime requirement
    /// impossible to miss.
    #[allow(clippy::unused_async, clippy::unused_async_trait_impl)]
    pub async fn build(self) -> Result<Client, FrameworkError> {
        let store: Arc<dyn SessionStore> = match self.session_store {
            Some(store) => Arc::from(store),
            None => Arc::new(KeyringStore::default()),
        };

        let session = Arc::new(StoreSession::new(store)?);
        let pool = SenderPool::new(Arc::clone(&session), self.api_id);
        let inner = grammers_client::Client::new(&pool);

        // `grammers` keeps the request half of the pool alive through a handle
        // it clones into the client, so what is left here is the runner itself
        // and the update channel. The runner owns the sockets: it opens a
        // connection on demand and has to keep running for as long as the
        // client does.
        let SenderPool {
            runner,
            handle: _,
            updates,
        } = pool;
        tokio::spawn(runner.run());

        Ok(Client {
            inner,
            api_hash: self.api_hash,
            session,
            code_requests: CodeRequestLog::default(),
            updates: UpdateRelay::start(updates),
        })
    }
}

impl fmt::Debug for ClientBuilder {
    /// Renders the builder without exposing the API hash.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("api_id", &self.api_id)
            .field("has_session_store", &self.session_store.is_some())
            .finish_non_exhaustive()
    }
}

/// A Telegram client, wrapping `grammers`' own.
///
/// Only this crate's types are exposed, with one deliberate exception:
/// [`Client::invoke`] takes a `grammers` request, because that is the whole
/// point of an escape hatch. Everything else — errors included — is defined
/// here, so callers never have to add `grammers` to their own manifest.
///
/// The session is persisted by the login methods, and flushed whenever a
/// request changes it — a datacenter migration, a newly cached peer, a moved
/// update counter. [`Client::persist_session`] forces a write on demand.
///
/// # Updates
///
/// The connection pool starts producing updates as soon as a connection exists,
/// but nothing reads them until [`Client::subscribe_updates`] is called. Until
/// then they are discarded rather than held, so a client that never subscribes
/// costs no memory and loses nothing: the session's update state only moves
/// once a feed is running, and `catch_up` replays from wherever it stopped.
/// There is no implicit drain, and no warning for the discarded events beyond
/// a periodic debug log.
pub struct Client {
    inner: grammers_client::Client,
    api_hash: String,
    session: Arc<StoreSession>,
    code_requests: CodeRequestLog,
    updates: UpdateRelay,
}

impl fmt::Debug for Client {
    /// Renders the client without exposing the API hash or the session.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client").finish_non_exhaustive()
    }
}

impl Client {
    /// Returns `true` when the stored session is authorised and usable.
    ///
    /// A `false` here is not an error: it means the login flow has to be run.
    pub async fn is_authorized(&self) -> Result<bool, FrameworkError> {
        let authorized = self
            .inner
            .is_authorized()
            .await
            .map_err(|error| RequestError::from_invocation(&error))
            .map_err(FrameworkError::from)?;

        // Asking costs a round trip that can negotiate a datacenter or cache a
        // peer, so the session may have moved even though nothing was sent.
        self.flush_session();

        Ok(authorized)
    }

    /// Asks Telegram to send a login code to `phone`.
    ///
    /// The phone number must be in international format, for example
    /// `+15551234567`. The code arrives over SMS or, when another Telegram
    /// client is already signed in, as a notification there.
    ///
    /// # Rate limits
    ///
    /// Telegram throttles this call per phone number, and the throttling gets
    /// worse the more it is retried. A second request within
    /// [`CODE_REQUEST_COOLDOWN`](self) is logged at `warn`. The returned
    /// [`LoginToken`] is single use — see its documentation.
    pub async fn request_login_code(&self, phone: &str) -> Result<LoginToken, AuthError> {
        self.note_code_request();
        // Deliberately logs nothing about the phone number.
        tracing::debug!("requesting a telegram login code");

        self.inner
            .request_login_code(phone, &self.api_hash)
            .await
            .map_err(|error| AuthError::from_invocation(&error))
            .map(LoginToken::new)
    }

    /// Submits the login code and completes the login.
    ///
    /// On [`SignInResult::Success`] the session has been persisted. On
    /// [`SignInResult::PasswordRequired`] nothing has been persisted yet,
    /// because the account is not signed in until the password step finishes.
    ///
    /// The token is consumed by the attempt, whether it succeeds or not; call
    /// [`Client::request_login_code`] again to retry.
    pub async fn sign_in(&self, token: &LoginToken, code: &str) -> Result<SignInResult, AuthError> {
        // Claims the token before anything else: a second attempt has to fail
        // here, where the reason is obvious, rather than against Telegram with
        // a hash it has already burned.
        token.claim()?;
        // Deliberately logs neither the phone number nor the code.
        tracing::debug!("submitting the telegram login code");

        match self.inner.sign_in(&token.inner, code).await {
            Ok(_user) => {
                self.persist_after_login();
                Ok(SignInResult::Success)
            }
            Err(grammers_client::SignInError::PasswordRequired(password_token)) => Ok(
                SignInResult::PasswordRequired(PasswordToken::new(password_token)),
            ),
            Err(error) => Err(AuthError::from_sign_in(error)),
        }
    }

    /// Submits the two-factor password and completes the login.
    ///
    /// On success the session is persisted. The [`PasswordToken`] comes from
    /// [`SignInResult::PasswordRequired`].
    pub async fn check_password(
        &self,
        token: PasswordToken,
        password: &str,
    ) -> Result<(), AuthError> {
        // Deliberately logs neither the password nor its hint.
        tracing::debug!("submitting the two-factor password");

        match self
            .inner
            .check_password(token.into_inner(), password.as_bytes())
            .await
        {
            Ok(_user) => {
                self.persist_after_login();
                Ok(())
            }
            Err(error) => Err(AuthError::from_sign_in(error)),
        }
    }

    /// Writes the current session state to the configured [`SessionStore`].
    ///
    /// Worth calling after anything that changes the session materially — a
    /// datacenter migration, for instance — so that the next launch does not
    /// have to renegotiate. The login methods already write on success, and the
    /// request paths flush automatically, so this is the escape hatch for
    /// callers that know something changed and want the write now.
    pub fn persist_session(&self) -> Result<(), FrameworkError> {
        self.session.persist().map_err(FrameworkError::from)
    }

    /// Borrows the wrapped client, for the escape hatch in [`crate::raw`].
    pub(crate) fn inner(&self) -> &grammers_client::Client {
        &self.inner
    }

    /// Borrows the update pipeline, for [`Client::subscribe_updates`].
    pub(crate) fn updates(&self) -> &UpdateRelay {
        &self.updates
    }

    /// Takes a share of the session, for the update feed.
    ///
    /// The feed outlives any single borrow of the client — it is held by
    /// whatever drives it, and it writes the session back when it is dropped —
    /// so it needs a handle of its own rather than a reference.
    pub(crate) fn session_handle(&self) -> Arc<StoreSession> {
        Arc::clone(&self.session)
    }

    /// Records a code request, warning when it follows closely on another.
    fn note_code_request(&self) {
        if self.code_requests.record(Instant::now()) {
            tracing::warn!(
                cooldown_secs = CODE_REQUEST_COOLDOWN.as_secs(),
                "a login code was requested very recently; telegram may throttle this one"
            );
        }
    }

    /// Writes the session back when a request changed it.
    ///
    /// A datacenter migration or a newly cached peer only reaches the store
    /// through this call. Without it the next launch would silently renegotiate
    /// — or, after a migration, find itself talking to a datacenter whose key
    /// it no longer has and log the user out. The write is skipped unless
    /// something actually changed, so a credential-store write stays off the
    /// hot path.
    pub(crate) fn flush_session(&self) {
        match self.session.persist_if_dirty() {
            Ok(false) => {}
            Ok(true) => tracing::debug!("wrote the session back after it changed"),
            Err(error) => tracing::warn!(
                %error,
                "the session changed but could not be saved; the next launch may renegotiate or need a new login"
            ),
        }
    }

    /// Persists the session after a successful login, reporting a failure
    /// without turning a completed login into an error.
    fn persist_after_login(&self) {
        if let Err(error) = self.session.persist() {
            tracing::warn!(
                %error,
                "signed in, but the session could not be saved; the next launch will need a new login"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_code_request_is_not_flagged() {
        let log = CodeRequestLog::default();
        assert!(
            !log.record(Instant::now()),
            "there is nothing to compare a first request against"
        );
    }

    #[test]
    fn a_quick_second_code_request_is_flagged() {
        let log = CodeRequestLog::default();
        let start = Instant::now();

        assert!(!log.record(start), "the first request is unremarkable");
        assert!(
            log.record(start + Duration::from_secs(1)),
            "a request one second later is the retry the warning is for"
        );
    }

    #[test]
    fn a_code_request_after_the_cooldown_is_not_flagged() {
        let log = CodeRequestLog::default();
        let start = Instant::now();

        assert!(!log.record(start));
        assert!(
            !log.record(start + CODE_REQUEST_COOLDOWN),
            "exactly one cooldown later is no longer a retry"
        );
    }
}
