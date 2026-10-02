//! The `grammers`-backed client: [`ClientBuilder`] and [`Client`].

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use grammers_client::session::types::PeerRef;
use grammers_mtsender::SenderPool;
use tokio::task::JoinHandle;

use crate::auth::{
    LoginToken, PasswordAttempts, PasswordToken, SignInResult, SignedInUser, signed_in_from,
};
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

        // The client takes the pool's request handle by value, so the pool is
        // split before the client is built. What is left here is the runner
        // itself and the update channel. The runner owns the sockets: it opens
        // a connection on demand and has to keep running for as long as the
        // client does.
        let SenderPool {
            runner,
            handle,
            updates,
        } = pool;
        let inner = grammers_client::Client::new(handle);

        // The runner is kept rather than detached. It owns the sockets, so a
        // client that has been dropped must be able to stop it; a detached one
        // would keep the connection open until the process ended.
        let runner = tokio::spawn(runner.run());

        Ok(Client {
            inner,
            api_hash: self.api_hash,
            session,
            code_requests: CodeRequestLog::default(),
            updates: UpdateRelay::start(updates),
            dialogs_fetched: AtomicBool::new(false),
            signed_in: Mutex::new(None),
            password_attempts: PasswordAttempts::default(),
            runner,
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
///
/// Resolving what was missed while offline reads peer access hashes back out of
/// the session, and only [`Client::fetch_dialogs`] writes them, so a feed
/// started before the first fetch has nothing to resolve against.
/// [`Client::has_fetched_dialogs`] reports which side of that line the client is
/// on, and [`Client::subscribe_updates`] logs the cold case at `debug`.
pub struct Client {
    inner: grammers_client::Client,
    api_hash: String,
    session: Arc<StoreSession>,
    code_requests: CodeRequestLog,
    updates: UpdateRelay,

    /// Whether [`Client::fetch_dialogs`] has run. A latch: it only ever goes
    /// one way, and relaxed ordering is enough for that.
    dialogs_fetched: AtomicBool,

    /// The account that signed in, as `sign_in` and `check_password` reported
    /// it. See [`Client::signed_in_user`].
    signed_in: Mutex<Option<SignedInUser>>,

    /// How many wrong two-factor passwords this sign-in has already had. See
    /// [`PasswordAttempts`].
    password_attempts: PasswordAttempts,

    /// The connection pool's task, stopped when the client is dropped.
    runner: JoinHandle<()>,
}

impl fmt::Debug for Client {
    /// Renders the client without exposing the API hash or the session.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field(
                "dialogs_fetched",
                &self.dialogs_fetched.load(Ordering::Relaxed),
            )
            .finish_non_exhaustive()
    }
}

impl Drop for Client {
    /// Stops the connection pool.
    ///
    /// The pool's runner holds the sockets and its relay holds the channel the
    /// pool feeds. Neither has anything left to do once the client is gone —
    /// the runner would keep a connection open, and the relay would keep
    /// draining a feed no one can read — so both are stopped here rather than
    /// left to the process exiting. The relay stops itself; see its own `Drop`.
    fn drop(&mut self) {
        self.runner.abort();
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
            Ok(user) => {
                self.remember_signed_in(&user);
                self.persist_after_login();
                Ok(SignInResult::Success)
            }
            Err(grammers_client::SignInError::PasswordRequired(password_token)) => {
                // A new code is a new flow, so the previous one's spent
                // attempts are not this one's.
                self.password_attempts.reset();
                Ok(SignInResult::PasswordRequired(PasswordToken::new(
                    password_token,
                )))
            }
            Err(error) => Err(AuthError::from_sign_in(error)),
        }
    }

    /// Submits the two-factor password and completes the login.
    ///
    /// On success the session is persisted. The [`PasswordToken`] comes from
    /// [`SignInResult::PasswordRequired`].
    ///
    /// # Refusals
    ///
    /// A wrong password is [`AuthError::InvalidPassword`] carrying how many
    /// tries are left, counted by the client rather than read out of Telegram's
    /// answer — so [`classify`](crate::classify) on the same error's name
    /// reports [`PASSWORD_ATTEMPTS`](crate::PASSWORD_ATTEMPTS) and this is the
    /// only reading that is right once one has been spent.
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
            Ok(user) => {
                self.remember_signed_in(&user);
                self.persist_after_login();
                Ok(())
            }
            // Counted here rather than in `from_sign_in`, which is reached from
            // paths that have no flow to count against.
            Err(grammers_client::SignInError::InvalidPassword(_)) => {
                Err(AuthError::InvalidPassword {
                    attempts_left: self.password_attempts.refuse(),
                })
            }
            Err(error) => Err(AuthError::from_sign_in(error)),
        }
    }

    /// The account that signed in, as Telegram described it during the flow.
    ///
    /// `grammers` hands the account back on the step that authenticates it, and
    /// it is the only place a client that has just logged in can learn which
    /// account that was without asking: the identifier of your own user is
    /// disclosed nowhere else. So it is kept rather than dropped, and this reads
    /// it back — no `users.getFullUser` round trip for something the login
    /// already answered.
    ///
    /// `None` before a sign-in has completed, and on a client restored from a
    /// stored session: nothing in the store names the account, so a client built
    /// that way has not been told and must still ask
    /// [`Client::fetch_account`]. What this returns is a `SignedInUser` and not
    /// an [`Account`](crate::Account) because it carries no bio and no birthday:
    /// those live on the *full* user, which login never fetches.
    #[must_use]
    pub fn signed_in_user(&self) -> Option<SignedInUser> {
        self.signed_in
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Keeps the account a completed sign-in identified.
    ///
    /// A flow that starts again forgets what the last one was, so a client that
    /// signs in twice as two people does not keep answering for the first.
    fn remember_signed_in(&self, user: &grammers_client::peer::User) {
        if let Some(user) = signed_in_from(&user.raw) {
            *self
                .signed_in
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(user);
        }
        self.password_attempts.reset();
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

    /// Signs the account out and forgets the persisted session.
    ///
    /// Three things happen, in this order, and only the last two are allowed to
    /// fail:
    ///
    /// 1. Telegram is asked to revoke the key (`auth.logOut`). **Best effort.**
    ///    It is the only step that needs the network, and a reader who has asked
    ///    to log out must end up logged out whether or not the request got
    ///    through — so a failure is logged rather than returned. A key that is
    ///    still registered on Telegram's side is a revocation this build cannot
    ///    make; what it can make is a machine that no longer holds it.
    /// 2. The stored session is cleared, so the next launch has nothing to
    ///    restore — see [`SessionStore::clear`].
    /// 3. The in-memory mirror is reset, so *this* client is unauthorised too
    ///    and no further request can use the key it just discarded.
    ///
    /// The client remains usable afterwards: [`Client::is_authorized`] reports
    /// `false` and the login flow can be run again on it. What a fresh sign-in
    /// produces is a different account's session and overwrites this one.
    ///
    /// This is the framework half only — what a caller shows the reader before
    /// calling it, and what it does with the result, is not decided here.
    pub async fn logout(&self) -> Result<(), FrameworkError> {
        if let Err(error) = self.inner.sign_out().await {
            tracing::warn!(
                %error,
                "telegram did not confirm the sign-out; the stored key is still being discarded"
            );
        }

        self.session.clear()?;
        *self
            .signed_in
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        Ok(())
    }

    /// Borrows the wrapped client, for the escape hatch in [`crate::raw`].
    pub(crate) fn inner(&self) -> &grammers_client::Client {
        &self.inner
    }

    /// Borrows the update pipeline, for [`Client::subscribe_updates`].
    pub(crate) fn updates(&self) -> &UpdateRelay {
        &self.updates
    }

    /// The cached peer for a conversation, if the session has one.
    ///
    /// Telegram hands out an `access_hash` for every peer and addressing one
    /// takes it, so a conversation this client has never fetched cannot be
    /// named at all — only [`Client::fetch_dialogs`] puts it in the cache.
    /// `None` here is what a caller turns into
    /// [`FrameworkError::UnknownPeer`] rather than a request Telegram would
    /// reject.
    pub(crate) fn peer_ref(&self, peer_id: i64) -> Option<PeerRef> {
        // A peer with no `access_hash` cannot be addressed at all, so it is not
        // a `PeerRef` — which is why this is a filter rather than a map.
        self.session.cached_peer(peer_id).and_then(|info| {
            let id = info.id();
            info.auth().map(|auth| PeerRef { id, auth })
        })
    }

    /// Takes a share of the session, for the update feed.
    ///
    /// The feed outlives any single borrow of the client — it is held by
    /// whatever drives it, and it writes the session back when it is dropped —
    /// so it needs a handle of its own rather than a reference.
    pub(crate) fn session_handle(&self) -> Arc<StoreSession> {
        Arc::clone(&self.session)
    }

    /// Whether the chat list has been fetched since this client was built.
    ///
    /// Iterating the dialog list is what makes Telegram disclose a peer's
    /// `access_hash`, and resolving a gap in the update feed reads it back out
    /// of the session — so a feed started before the first fetch has nothing to
    /// resolve against. See [`Client::subscribe_updates`]. The answer is kept
    /// here rather than by each caller so that every caller gets the same one
    /// for the same client.
    ///
    /// A client resumed from a session store that already holds peers is
    /// reported as cold until it fetches again: the flag records what this
    /// client has done, not what the store holds, because the store cannot say
    /// which peers a gap would need.
    #[must_use]
    pub fn has_fetched_dialogs(&self) -> bool {
        self.dialogs_fetched.load(Ordering::Relaxed)
    }

    /// Records that the chat list was fetched.
    pub(crate) fn note_dialogs_fetched(&self) {
        self.dialogs_fetched.store(true, Ordering::Relaxed);
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
