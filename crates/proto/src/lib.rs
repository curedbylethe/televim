//! Translation layer between `telegram-framework` and `domain`.
//!
//! Rules:
//! - Must not expose any `grammers` or `telegram-framework` types to `domain` or `tui`.
//! - Domain types only cross this boundary.
//!
//! # Where `tl` may be named
//!
//! `telegram-framework` re-exports the `grammers` request and response types as
//! `telegram_framework::tl`, and `Client::invoke` takes one of them. That escape
//! hatch is the framework's, not this crate's, and the rule is:
//!
//! - **`proto` does not name `telegram_framework::tl`.** Reaching Telegram from
//!   here goes through operations the framework exposes as its own types. When
//!   this crate needs a method the framework does not wrap yet, the wrapper is
//!   added *inside* `telegram-framework` — a typed operation, or a variant of a
//!   request enum the framework owns — and this crate calls that.
//! - **`app` may.** It is the composition root, it is allowed to enable `live`,
//!   and it already depends on every crate in the workspace.
//! - **If the rule ever has to be relaxed, it is relaxed deliberately**: behind a
//!   `live` feature on this crate, inside a module that names `tl`, translating
//!   into domain types before anything leaves. It is not a drive-by import.
//!
//! ## Why not a type-erased `invoke_raw(bytes)`
//!
//! The integration plan floated that as the alternative, and it was rejected.
//! `invoke`'s response type comes from the request's `RemoteCall::Return`, so
//! erasing it to bytes means the caller has to say what to decode back into —
//! which is either a registry of response types inside the framework (a
//! hand-written wrapper per method, just spelled differently) or downcasting
//! from `Any` to a `grammers` type, which leaks the type anyway. Neither is
//! worth trading the compile-time boundary for.
//!
//! ## How the boundary is checked
//!
//! The default build of every crate but the framework must contain no `grammers`
//! crate at all:
//!
//! ```console
//! $ make boundary
//! ```
//!
//! CI runs that target, so the rule fails the build rather than relying on a
//! reviewer noticing an import.

#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::module_name_repetitions)]

pub mod auth;
pub mod client;
pub mod error;
pub mod stream;
pub mod types;

pub use client::ProtoClient;
pub use error::ProtoError;
pub use types::{ProtoChat, ProtoMessage};
