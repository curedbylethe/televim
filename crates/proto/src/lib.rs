//! Translation layer between `telegram-framework` and `domain`.
//!
//! Rules:
//! - Must not expose any `grammers` or `telegram-framework` types to `domain` or `tui`.
//! - Domain types only cross this boundary.

#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::module_name_repetitions)]

pub mod auth;
pub mod client;
pub mod stream;
pub mod types;

pub use client::ProtoClient;
pub use types::{ProtoChat, ProtoMessage};
