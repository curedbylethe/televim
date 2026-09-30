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
pub mod card;
pub mod emoji;
pub mod event;
mod grapheme;
pub mod line;
pub mod rows;
pub mod text_row;
pub mod theme;
pub mod widgets;
pub mod wrap;

pub use app::{
    AccountState, Action, App, ConfirmKind, FetchDirection, Focus, Jump, Mode, Pane, ProfileId,
    ProfileRow, PromptKind, Register, SessionStore,
};
