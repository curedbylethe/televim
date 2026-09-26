//! `ratatui` widgets + modal key handling for `televim`.
//!
//! Depends on `domain`, but **not** on `proto` or `telegram-framework`.

#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_sign_loss)]

pub mod app;
pub mod event;
pub mod theme;
pub mod widgets;

pub use app::{App, FetchDirection, Mode, PromptKind};
