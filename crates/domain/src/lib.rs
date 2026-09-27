//! Pure business logic for `televim`.
//!
//! This crate has **no** dependency on `tokio`, `ratatui`, `grammers`, or
//! `tui`. It models chats, messages, session state, Vim motions, and what the
//! live update feed does to them.

#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::module_name_repetitions)]

pub mod chat;
pub mod history;
pub mod message;
pub mod search;
pub mod selection;
pub mod session;
pub mod updates;
pub mod vim;
