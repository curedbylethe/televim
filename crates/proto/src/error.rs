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
    #[error("message {id} is outside the range telegram numbers messages in")]
    MessageIdOutOfRange {
        /// The identifier that could not be narrowed.
        id: i64,
    },
}
