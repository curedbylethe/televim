//! First-party wrapper over `grammers`.
//!
//! This is the **only** crate in the workspace permitted to depend on
//! `grammers-*`, and it exists to keep that boundary enforceable rather than
//! merely conventional. It exposes two things:
//!
//! * **Session storage** — [`SessionData`], the [`SessionStore`] trait, and its
//!   [`MemoryStore`], [`FileStore`] and [`KeyringStore`] backends. None of them
//!   mentions `grammers`, so they are always compiled.
//! * **The client** — [`ClientBuilder`], the phone → code → 2FA login flow, the
//!   [`Client::fetch_dialogs`] chat list, the `fetch_history` message history,
//!   [`Client::fetch_account`] for the account's own profile,
//!   [`Client::search_messages`], [`Client::send_message`],
//!   [`Client::edit_message`] and [`Client::delete_messages`], the
//!   [`Client::subscribe_updates`] feed, and [`Client::invoke`] as a raw escape
//!   hatch. These need `grammers`, so they sit behind the `live` feature.
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
//! Build with `--features live` to get the client; CI uses `--all-features`, so
//! it is compiled and tested on every change.
//!
//! # Logging
//!
//! The crate logs through [`tracing`]. Auth steps are logged at `debug`, and a
//! session that could not be persisted after a successful login is logged at
//! `warn` so that an unexplained logout is never silent. No log line ever
//! contains a phone number, a login code, a password or an authorisation key.
//!
//! # Example
//!
//! ```ignore
//! use telegram_framework::session::KeyringStore;
//! use telegram_framework::{ClientBuilder, SignInResult};
//!
//! # async fn run(api_id: i32, api_hash: &str) -> Result<(), Box<dyn std::error::Error>> {
//! let client = ClientBuilder::new(api_id, api_hash)
//!     .session_store(Box::new(KeyringStore::default()))
//!     .build()
//!     .await?;
//!
//! if !client.is_authorized().await? {
//!     let token = client.request_login_code("+15551234567").await?;
//!     let code = ask_the_user_for_the_code();
//!
//!     match client.sign_in(&token, &code).await? {
//!         SignInResult::Success => {}
//!         SignInResult::PasswordRequired(password_token) => {
//!             let password = ask_the_user_for_the_password(password_token.hint());
//!             client.check_password(password_token, &password).await?;
//!         }
//!     }
//! }
//! # Ok(())
//! # }
//! ```

#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::module_name_repetitions)]

pub mod error;
pub mod session;

#[cfg(feature = "live")]
pub mod account;
#[cfg(feature = "live")]
pub mod auth;
#[cfg(feature = "live")]
pub mod client;
#[cfg(feature = "live")]
pub mod dialogs;
#[cfg(feature = "live")]
pub mod history;
#[cfg(feature = "live")]
pub mod media;
#[cfg(feature = "live")]
pub mod messages;
#[cfg(feature = "live")]
pub mod raw;
#[cfg(feature = "live")]
pub mod search;
#[cfg(feature = "live")]
pub mod updates;

// Fixtures shared by the unit tests. Compiled only under `cargo test`.
#[cfg(all(test, feature = "live"))]
mod testing;

pub use error::{AuthError, FrameworkError, RequestError, SessionError};
pub use session::{
    AccountIdentity, AuthKey, ChannelKind, ChannelState, DcOption, FileStore, KeyringStore,
    MemoryStore, Peer, SessionData, SessionStore, UpdateState,
};

#[cfg(feature = "live")]
pub use account::{Account, Birthday};
#[cfg(feature = "live")]
pub use auth::{
    LoginToken, PASSWORD_ATTEMPTS, PasswordToken, Refusal, SignInResult, SignedInUser, classify,
};
#[cfg(feature = "live")]
pub use client::{Client, ClientBuilder};
#[cfg(feature = "live")]
pub use dialogs::{DialogInfo, DialogKind};
#[cfg(feature = "live")]
pub use history::{HISTORY_LIMIT, HistoryArgs};
#[cfg(feature = "live")]
pub use media::MediaKind;
#[cfg(feature = "live")]
pub use messages::{TEXT_LIMIT, validate_text};
#[cfg(feature = "live")]
pub use search::{SEARCH_LIMIT, SearchArgs, SearchResults};
#[cfg(feature = "live")]
pub use updates::{MessageInfo, UpdateKind, UpdateSubscription};

/// The `grammers` request and response types accepted by [`Client::invoke`].
///
/// Re-exported from the `grammers` build this crate was compiled against, so
/// that callers can build requests without adding `grammers` to their own
/// manifest and without risking a version mismatch.
#[cfg(feature = "live")]
pub use grammers_tl_types as tl;
