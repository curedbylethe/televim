//! Every error type this crate produces.
//!
//! It lives here rather than next to the code that raises it, so that every
//! operation on `ProtoClient` can share one type without the modules having to
//! know about each other. Nothing in it carries a `grammers` type: the boundary
//! this crate maintains covers errors too.
//!
//! `ProtoClient` is named rather than linked because it only exists under the
//! `live` feature, and a link to it would dangle in a default-feature build.

use thiserror::Error;

#[cfg(feature = "live")]
use telegram_framework::{AuthError, Refusal};

#[cfg(feature = "live")]
use crate::auth::refusal_of;

/// Something went wrong while talking to Telegram, or while translating what it
/// answered into a `domain` type.
///
/// Every fallible operation in this crate returns this type, so a caller has
/// one thing to handle rather than one per method.
///
/// It adds almost nothing of its own, and that is the point: the framework
/// already reports every way a request can fail as an error of its own, so a
/// request failure is passed through as [`ProtoError::Framework`]. The one
/// thing defined here is a fact about the two number spaces meeting in this
/// crate — an identifier Telegram could not have numbered — rather than a
/// request failure at all. The enum is `#[non_exhaustive]` so that it grows when
/// a real failure mode appears rather than to reserve room for one.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProtoError {
    /// The framework could not complete a request, or the update feed failed.
    ///
    /// The cause is passed through unchanged, so a caller that wants to react
    /// to, say, a flood wait can still reach
    /// [`RequestError::Rpc`](telegram_framework::RequestError::Rpc) without
    /// depending on `grammers`.
    #[error(transparent)]
    Framework(#[from] telegram_framework::FrameworkError),

    /// A message identifier is outside the range Telegram numbers messages in.
    ///
    /// Telegram numbers messages with an `i32`, and the workspace counts in
    /// `i64`, so a value outside that range cannot name a message. The framework
    /// reports its own identifiers without a width, and this crate is where the
    /// two spaces meet — which is why the failure is defined here rather than
    /// there.
    ///
    /// Both the conversation and the identifier travel with the error: an
    /// identifier that cannot exist is only actionable together with the peer it
    /// was addressed to, which is the same pairing
    /// [`FrameworkError::UnknownPeer`](telegram_framework::FrameworkError::UnknownPeer)
    /// makes.
    #[error("message {id} in conversation {peer_id} is outside telegram's range")]
    MessageIdOutOfRange {
        /// Bare identifier of the conversation the message was addressed to.
        peer_id: i64,

        /// The identifier that could not be narrowed.
        id: i64,
    },

    /// A sign-in step was refused, or could not reach Telegram.
    ///
    /// Carries the refusal rather than the error, because the thing a caller
    /// has to do with a refusal is say it: `classify` already read Telegram's
    /// error name, and [`refusal_sentence`](crate::refusal_sentence) turns the
    /// result into the sentence the panel draws. The error itself is kept in
    /// `detail` so the detail `grammers` carries stays reachable — a flood
    /// wait's length, a request's name — without a `grammers` dependency.
    #[cfg(feature = "live")]
    #[error("{}", crate::refusal_sentence(refusal))]
    Auth {
        /// What Telegram said, in the vocabulary the panel speaks.
        refusal: Refusal,

        /// The framework's own error, when the refusal came from one. `None`
        /// only where there was no error to begin with — an outcome
        /// `SignInResult` gained that this build does not model yet, which is
        /// reported rather than ignored.
        detail: Option<AuthError>,
    },

    /// A sign-in completed, but the answer did not name an account.
    ///
    /// The framework keeps the user login hands back, and that is the only
    /// object that identifies your own user without a round trip. Telegram can
    /// answer a sign-in with a `userEmpty`, in which case there is no name to
    /// hand on — and a `domain::Account` built out of one would be a name
    /// nobody set, which is the same reason the chat list skips a peer with no
    /// identifier. Reported rather than papered over with a fetch that could
    /// not answer it either.
    #[cfg(feature = "live")]
    #[error("telegram signed the account in without saying which account it was")]
    UnnamedAccount,
}

#[cfg(feature = "live")]
impl ProtoError {
    /// The refusal behind this error, if it is one.
    ///
    /// The one thing a sign-in panel asks of a failure, and asking for the
    /// variant instead would mean a `match` over an error it cannot otherwise
    /// interpret.
    #[must_use]
    pub fn refusal(&self) -> Option<&Refusal> {
        match self {
            Self::Auth { refusal, .. } => Some(refusal),
            _ => None,
        }
    }

    /// An answer this build has no reading for, reported in the same voice as
    /// a refusal rather than dropped.
    pub(crate) fn unrecognised(answer: String) -> Self {
        Self::Auth {
            refusal: Refusal::Other(answer),
            detail: None,
        }
    }
}

#[cfg(feature = "live")]
impl From<AuthError> for ProtoError {
    fn from(error: AuthError) -> Self {
        let refusal = refusal_of(&error);
        Self::Auth {
            refusal,
            detail: Some(error),
        }
    }
}
