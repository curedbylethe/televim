//! Update stream filtered to private messages only.

use crate::error::Error;

/// A stream of updates from Telegram, already filtered to private chats.
#[derive(Debug)]
pub struct Updates {
    _private: (),
}

impl Updates {
    pub fn next_messages(&mut self) -> Result<(), Error> {
        Err(Error::NotImplemented)
    }
}
