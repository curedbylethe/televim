//! First-party wrapper over `grammers`.
//!
//! This is the **only** crate in the workspace permitted to depend on
//! `grammers-*`, and it exists to keep that boundary enforceable rather than
//! merely conventional. What it exposes today is **session storage**:
//! [`SessionData`], the [`SessionStore`] trait, and its [`MemoryStore`],
//! [`FileStore`] and [`KeyringStore`] backends. None of them mentions
//! `grammers`, so they are always compiled.
//!
//! # Why `live` is off by default
//!
//! `proto`, `domain` and `tui` must never see a `grammers` type. Making the
//! dependency optional turns that rule into something a machine can check:
//!
//! ```console
//! $ cargo tree -p proto  | grep grammers   # no output
//! $ cargo tree -p domain | grep grammers   # no output
//! ```
//!
//! `--features live` compiles the `grammers`-backed session adapter that sits
//! on top of a [`SessionStore`]. CI uses `--all-features`, so it is built and
//! tested on every change.
//!
//! # Logging
//!
//! The crate logs through [`tracing`]. No log line ever contains a phone
//! number, a login code, a password or an authorisation key.

#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::module_name_repetitions)]

pub mod error;
pub mod session;

pub use error::{AuthError, FrameworkError, RequestError, SessionError};
pub use session::{
    AuthKey, ChannelKind, ChannelState, DcOption, FileStore, KeyringStore, MemoryStore, Peer,
    SessionData, SessionStore, UpdateState,
};
