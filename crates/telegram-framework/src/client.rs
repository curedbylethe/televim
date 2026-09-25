//! Ergonomic wrapper over `grammers_client::Client`.

use crate::error::Error;

#[derive(Debug, Default)]
pub struct ClientBuilder {
    _private: (),
}

impl ClientBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Build a connected client.
    ///
    /// Returns [`Error::NotImplemented`]. Later PRs will wire `MTProto`.
    pub fn build(self) -> Result<Client, Error> {
        Err(Error::NotImplemented)
    }
}

#[derive(Debug)]
pub struct Client {
    _private: (),
}

impl Client {
    /// Escape hatch into raw `grammers`.
    pub fn raw_invoke(&self) -> Result<(), Error> {
        Err(Error::NotImplemented)
    }
}
