//! Pure business logic for `televim`.
//!
//! This crate has **no** dependency on `tokio`, `ratatui`, `grammers`, or
//! `tui`. It models chats, messages, session state, and Vim motions.

#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::module_name_repetitions)]

pub mod chat;
pub mod message;
pub mod session;
pub mod vim;
