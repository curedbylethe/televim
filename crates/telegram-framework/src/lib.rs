//! First-party wrapper over `grammers`.
//!
//! This is the **only** crate permitted to depend on `grammers-*`. Everything
//! outside sees the types defined here.
//!
//! The `live` feature (off by default) will be
//! enabled when real `MTProto` calls land.

#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::module_name_repetitions)]

pub mod client;
pub mod error;
pub mod session;
pub mod updates;

pub use client::{Client, ClientBuilder};
pub use error::Error;
pub use session::{MemoryStore, SessionStore};
pub use updates::Updates;
