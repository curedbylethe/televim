//! Fictional demo data. No Telegram data, credentials or real conversations.

#![forbid(unsafe_code)]

mod effect;
mod fixtures;
mod search;
mod sim;
mod world;

pub use effect::Effect;
pub use sim::{DemoSimulator, Scheduled};
pub use world::{DemoAccount, DemoMedia, DemoWorld};
