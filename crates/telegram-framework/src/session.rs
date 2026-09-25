//! Pluggable session storage.

use crate::error::Error;
use std::collections::HashMap;
use std::sync::Mutex;

pub trait SessionStore: Send + Sync {
    fn load(&self) -> Result<Option<String>, Error>;
    fn store(&self, session: &str) -> Result<(), Error>;
    fn clear(&self) -> Result<(), Error>;
}

#[derive(Debug, Default)]
pub struct MemoryStore {
    inner: Mutex<Option<String>>,
}

impl MemoryStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl SessionStore for MemoryStore {
    fn load(&self) -> Result<Option<String>, Error> {
        let guard = self
            .inner
            .lock()
            .map_err(|e| Error::Session(format!("mutex poisoned: {e}")))?;
        Ok(guard.clone())
    }

    fn store(&self, session: &str) -> Result<(), Error> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| Error::Session(format!("mutex poisoned: {e}")))?;
        *guard = Some(session.to_owned());
        Ok(())
    }

    fn clear(&self) -> Result<(), Error> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|e| Error::Session(format!("mutex poisoned: {e}")))?;
        *guard = None;
        Ok(())
    }
}

// Placeholder type reserved (file/keyring backends).
#[allow(dead_code)]
#[derive(Debug, Default)]
pub(crate) struct Reserved(HashMap<String, String>);
