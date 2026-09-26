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
/// It carries a single variant, and that is the point: the framework already
/// reports every way a request can fail as an error of its own, so this crate
/// has nothing to add to it. A variant with no producer would be a guess at
/// what a caller needs to match on, and a caller cannot act on a guess. The
/// enum is `#[non_exhaustive]` so that it grows when a real failure mode
/// appears rather than to reserve room for one.
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
}
