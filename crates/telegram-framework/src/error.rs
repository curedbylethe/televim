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

    /// The session implementation failed while the request was being built.
    ///
    /// This crate's own session is infallible — it holds the decoded state in
    /// memory and writes the credential store separately — so nothing should
    /// reach this. It is mapped rather than dropped so that a session failure
    /// is never reported as a network failure, which is what a reader would
    /// otherwise be told to check their connection for.
    #[error("the session could not be read or written: {0}")]
    Session(String),
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

    /// The update feed had already been taken.
    ///
    /// An account's updates form one ordered sequence, so they are delivered to
    /// exactly one subscriber. A second one could not be given a consistent
    /// view without buffering for whichever consumer is slower.
    #[error("this client is already subscribed to its updates")]
    UpdatesAlreadySubscribed,

    /// The conversation is not in the session's peer cache.
    ///
    /// Addressing a peer takes the `access_hash` Telegram handed out for it, and
    /// only the chat list discloses those — see
    /// [`Client::fetch_dialogs`](crate::Client::fetch_dialogs). Asking for the
    /// history of a conversation this client has never fetched is therefore
    /// unanswerable rather than merely slow, and reporting it beats sending a
    /// request Telegram would reject for a peer this client cannot name.
    #[error("conversation {0} is not in the session's peer cache; fetch the chat list first")]
    UnknownPeer(i64),

    /// The message text is empty, or nothing but whitespace.
    ///
    /// Telegram does not accept a message with no content, and this is checked
    /// before the request rather than discovered as a rejection: it is a fact
    /// about the argument, not about the account or the connection.
    #[error("cannot send an empty message")]
    TextEmpty,

    /// The message text is longer than Telegram accepts.
    ///
    /// The count is in **characters**, which is the unit Telegram limits on, so
    /// a message of emoji is measured the same way the server measures it.
    #[error("the message is {chars} character(s) long; the limit is {limit}")]
    TextTooLong {
        /// How many characters the text has.
        chars: usize,

        /// The most characters a message may have.
        limit: usize,
    },
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
            InvocationError::Session(error) => Self::Session(error.to_string()),
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
            // `grammers` hands back a fresh SRP challenge alongside the failure,
            // so the reader could retry the password without authenticating
            // again. Carrying it would mean holding a token in the error type,
            // which is a change to the login flow rather than to this mapping.
            SignInError::InvalidPassword(_) => Self::InvalidPassword,
            SignInError::PasswordRequired(_) => Self::PasswordRequired,
            SignInError::SignUpRequired => Self::SignUpRequired,
            SignInError::Other(error) => Self::from_invocation(&error),
        }
    }
}

/// The mapping from `grammers` errors onto this crate's own.
///
/// These are the only paths that run exclusively against a live datacenter, so
/// without these tests the whole translation layer would be unverified on CI.
#[cfg(all(test, feature = "live"))]
mod tests {
    use std::io;

    use grammers_mtproto::authentication;
    use grammers_mtproto::mtp::DeserializeError;
    use grammers_mtproto::transport;

    use super::*;
    use crate::testing;

    #[test]
    fn an_rpc_error_keeps_its_code_name_and_value() {
        let error = RequestError::from_invocation(&testing::rpc(420, "FLOOD_WAIT", Some(31)));

        let RequestError::Rpc { code, name, value } = &error else {
            panic!("expected an rpc error, got {error:?}");
        };
        assert_eq!(*code, 420);
        assert_eq!(name, "FLOOD_WAIT");
        assert_eq!(*value, Some(31));
    }

    #[test]
    fn an_io_failure_is_reported_as_a_network_error() {
        let error = RequestError::from_invocation(&InvocationError::Io(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "reset by peer",
        )));
        assert!(matches!(error, RequestError::Network(_)), "got {error:?}");
    }

    #[test]
    fn a_deserialisation_failure_is_reported_as_such() {
        let error = RequestError::from_invocation(&InvocationError::Deserialize(
            DeserializeError::MessageBufferTooSmall,
        ));
        assert!(
            matches!(error, RequestError::Deserialize(_)),
            "got {error:?}"
        );
    }

    #[test]
    fn a_transport_failure_is_reported_as_a_network_error() {
        let error = RequestError::from_invocation(&InvocationError::Transport(
            transport::Error::MissingBytes,
        ));
        assert!(matches!(error, RequestError::Network(_)), "got {error:?}");
    }

    #[test]
    fn a_failed_key_exchange_is_reported_as_a_network_error() {
        let error = RequestError::from_invocation(&InvocationError::Authentication(
            authentication::Error::DhParamsFail,
        ));
        assert!(matches!(error, RequestError::Network(_)), "got {error:?}");
    }

    #[test]
    fn a_dropped_request_is_reported_as_dropped() {
        assert!(matches!(
            RequestError::from_invocation(&InvocationError::Dropped),
            RequestError::Dropped
        ));
    }

    #[test]
    fn an_unknown_datacenter_is_reported_as_such() {
        assert!(matches!(
            RequestError::from_invocation(&InvocationError::InvalidDc),
            RequestError::UnknownDatacenter
        ));
    }

    #[test]
    fn a_flood_wait_becomes_a_rate_limit_with_its_retry_delay() {
        let error = AuthError::from_invocation(&testing::rpc(420, "FLOOD_WAIT", Some(31)));
        assert!(
            matches!(
                error,
                AuthError::RateLimited {
                    retry_after: Some(31)
                }
            ),
            "got {error:?}"
        );
    }

    /// Telegram does not always use code 420; the name is the reliable signal.
    #[test]
    fn a_flood_named_error_is_rate_limited_without_code_420() {
        let error = AuthError::from_invocation(&testing::rpc(400, "FLOOD_PREMIUM_WAIT", None));
        assert!(
            matches!(error, AuthError::RateLimited { retry_after: None }),
            "got {error:?}"
        );
    }

    #[test]
    fn a_rejected_phone_number_is_reported_as_invalid() {
        for name in [
            "PHONE_NUMBER_INVALID",
            "PHONE_NUMBER_BANNED",
            "PHONE_NUMBER_UNOCCUPIED",
        ] {
            let error = AuthError::from_invocation(&testing::rpc(400, name, None));
            assert!(
                matches!(error, AuthError::InvalidPhone),
                "{name} gave {error:?}"
            );
        }
    }

    /// Anything that is not about credentials, throttling or the phone number
    /// stays a network error, with the RPC details still reachable.
    #[test]
    fn anything_else_keeps_its_rpc_details() {
        let error = AuthError::from_invocation(&testing::rpc(500, "INTERNAL", None));

        let AuthError::NetworkError(RequestError::Rpc { code, name, .. }) = &error else {
            panic!("expected the rpc error to pass through, got {error:?}");
        };
        assert_eq!(*code, 500);
        assert_eq!(name, "INTERNAL");
    }

    #[test]
    fn a_non_rpc_invocation_failure_still_becomes_a_network_error() {
        let error = AuthError::from_invocation(&InvocationError::Dropped);
        assert!(
            matches!(error, AuthError::NetworkError(RequestError::Dropped)),
            "got {error:?}"
        );
    }

    #[test]
    fn every_sign_in_failure_maps_onto_its_own_error() {
        assert!(matches!(
            AuthError::from_sign_in(SignInError::InvalidCode),
            AuthError::InvalidCode
        ));
        assert!(matches!(
            AuthError::from_sign_in(SignInError::InvalidPassword(testing::password_token(Some(
                "hint"
            )),)),
            AuthError::InvalidPassword
        ));
        assert!(matches!(
            AuthError::from_sign_in(SignInError::SignUpRequired),
            AuthError::SignUpRequired
        ));
    }

    #[test]
    fn a_password_challenge_maps_onto_password_required() {
        let error = AuthError::from_sign_in(SignInError::PasswordRequired(
            testing::password_token(Some("hint")),
        ));
        assert!(
            matches!(error, AuthError::PasswordRequired),
            "got {error:?}"
        );
    }

    #[test]
    fn a_sign_in_failure_wrapping_an_rpc_error_is_mapped_through() {
        let error =
            AuthError::from_sign_in(SignInError::Other(testing::rpc(420, "FLOOD_WAIT", Some(5))));
        assert!(
            matches!(
                error,
                AuthError::RateLimited {
                    retry_after: Some(5)
                }
            ),
            "got {error:?}"
        );
    }
}
