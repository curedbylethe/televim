//! Every error type this crate produces.
//!
//! They are all defined here, rather than next to the code that raises them, so
//! that [`FrameworkError`] can aggregate them without the modules having to
//! know about each other. None of them carries a `grammers` type: the boundary
//! this crate maintains is about errors too.

use thiserror::Error;

#[cfg(feature = "live")]
use grammers_client::{InvocationError, SignInError};

/// Something went wrong while reading or writing a
/// [`SessionStore`](crate::SessionStore).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SessionError {
    /// The stored session could not be read back.
    #[error("could not read the stored session: {0}")]
    Load(String),

    /// The session could not be written.
    #[error("could not save the session: {0}")]
    Save(String),

    /// The stored session could not be removed.
    #[error("could not clear the stored session: {0}")]
    Clear(String),

    /// The stored bytes are not a session this version understands.
    ///
    /// This covers both malformed input and a snapshot written by a different
    /// schema version.
    #[error("the stored session is not usable: {0}")]
    Corrupt(String),

    /// The backing store itself could not be reached.
    ///
    /// For [`KeyringStore`](crate::KeyringStore) this usually means the machine
    /// has no credential store — a headless Linux box, for instance.
    #[error("the session store is unavailable: {0}")]
    Unavailable(String),
}

/// A request to Telegram failed, or the answer could not be understood.
///
/// This is the crate's `grammers`-free mirror of `grammers`' own invocation
/// error, kept structured so that callers can still react to, say, a flood wait
/// without depending on `grammers` themselves.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RequestError {
    /// Telegram answered the request with an RPC error.
    #[error("telegram returned rpc error {code} {name}")]
    Rpc {
        /// Numeric error code, similar to an HTTP status code.
        code: i32,
        /// ASCII error name, normally in screaming snake case.
        name: String,
        /// Trailing integer stripped out of the name, if Telegram sent one.
        ///
        /// For `FLOOD_WAIT_31` this is `Some(31)` and
        /// [`RequestError::Rpc::name`] is `FLOOD_WAIT`.
        value: Option<u32>,
    },

    /// The connection failed before Telegram could answer.
    #[error("network error: {0}")]
    Network(String),

    /// Telegram answered, but the response could not be deserialised.
    #[error("could not read telegram's response: {0}")]
    Deserialize(String),

    /// The request was cancelled because the client is shutting down.
    #[error("the request was dropped before telegram answered")]
    Dropped,

    /// The request named a datacenter the session does not know about.
    #[error("the requested datacenter is not known")]
    UnknownDatacenter,
}

/// The login flow could not be completed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AuthError {
    /// Telegram rejected the phone number outright.
    #[error("telegram rejected the phone number")]
    InvalidPhone,

    /// The login code was wrong, or it has expired.
    #[error("the login code is invalid or has expired")]
    InvalidCode,

    /// The account has two-factor authentication enabled.
    ///
    /// [`Client::sign_in`](crate::Client::sign_in) does not treat this as a
    /// failure: it reports it as
    /// [`SignInResult::PasswordRequired`](crate::SignInResult::PasswordRequired)
    /// so the caller can collect the password and continue.
    #[error("the account requires a two-factor password")]
    PasswordRequired,

    /// The two-factor password was wrong.
    #[error("the two-factor password is invalid")]
    InvalidPassword,

    /// The phone number has no Telegram account yet.
    ///
    /// Telegram only lets an official client create accounts, so the user has
    /// to sign up there first.
    #[error("the phone number has no telegram account yet; sign up in an official client first")]
    SignUpRequired,

    /// The [`LoginToken`](crate::LoginToken) had already been redeemed.
    #[error("this login token has already been used; request a new login code")]
    TokenAlreadyUsed,

    /// Telegram is throttling the request.
    #[error("telegram is rate limiting this request")]
    RateLimited {
        /// How many seconds Telegram asked the client to wait, when it said.
        retry_after: Option<u32>,
    },

    /// The request never reached Telegram, or the answer was unintelligible.
    #[error(transparent)]
    NetworkError(#[from] RequestError),

    /// The session could not be persisted after a successful login.
    #[error(transparent)]
    SessionError(#[from] SessionError),
}

/// The umbrella error for every fallible operation in this crate.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FrameworkError {
    /// The login flow failed.
    #[error(transparent)]
    Auth(#[from] AuthError),

    /// Session persistence failed.
    #[error(transparent)]
    Session(#[from] SessionError),

    /// A request to Telegram failed.
    #[error(transparent)]
    Request(#[from] RequestError),
}

#[cfg(feature = "live")]
impl RequestError {
    /// Mirrors a `grammers` invocation error into this crate's own type.
    ///
    /// The variant mapping is deliberately one-to-one so that no information a
    /// caller might branch on is thrown away.
    pub(crate) fn from_invocation(error: &InvocationError) -> Self {
        match error {
            InvocationError::Rpc(rpc) => Self::Rpc {
                code: rpc.code,
                name: rpc.name.clone(),
                value: rpc.value,
            },
            InvocationError::Io(error) => Self::Network(error.to_string()),
            InvocationError::Deserialize(error) => Self::Deserialize(error.to_string()),
            InvocationError::Transport(error) => Self::Network(error.to_string()),
            InvocationError::Dropped => Self::Dropped,
            InvocationError::InvalidDc => Self::UnknownDatacenter,
            InvocationError::Authentication(error) => Self::Network(error.to_string()),
        }
    }
}

#[cfg(feature = "live")]
impl AuthError {
    /// Maps a failed login request onto the closest authentication error.
    ///
    /// Anything Telegram answered with that is not specifically about the
    /// credentials, throttling or the phone number is reported as a network
    /// error, which keeps the original RPC code reachable through
    /// [`AuthError::NetworkError`].
    pub(crate) fn from_invocation(error: &InvocationError) -> Self {
        if let InvocationError::Rpc(rpc) = error {
            if rpc.code == 420 || rpc.name.contains("FLOOD") {
                return Self::RateLimited {
                    retry_after: rpc.value,
                };
            }
            if rpc.name.starts_with("PHONE_NUMBER_") {
                return Self::InvalidPhone;
            }
        }
        Self::NetworkError(RequestError::from_invocation(error))
    }

    /// Maps a `grammers` sign-in failure onto this crate's error type.
    pub(crate) fn from_sign_in(error: SignInError) -> Self {
        match error {
            SignInError::InvalidCode => Self::InvalidCode,
            SignInError::InvalidPassword => Self::InvalidPassword,
            SignInError::PasswordRequired(_) => Self::PasswordRequired,
            SignInError::SignUpRequired { .. } => Self::SignUpRequired,
            SignInError::Other(error) => Self::from_invocation(&error),
        }
    }
}
