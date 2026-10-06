//! Cross-cutting coordination: the mutations no single state type owns.
//!
//! Every other module in [`crate::state`] holds one piece of
//! [`crate::app::App`] and the mutations that touch only that piece. The
//! methods that mutate two or more pieces at once cannot live on any one of
//! them, so they move here as free functions taking exactly the substructs
//! they mutate — never `&mut App`.
//!
//! The three it will host (CUR-115; no behaviour lives here yet):
//!
//! - `select_chat_none`: clears the open conversation across `ui`, `outbox`,
//!   `pending`, `conversation`, and `input`.
//! - `handle_key`: the top-level dispatch across `ui`, `session`, `profile`,
//!   and `pending`.
//! - `handle_normal`: normal-mode keys across `ui`, `pending`, `conversation`,
//!   and `input`.
