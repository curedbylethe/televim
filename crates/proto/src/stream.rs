//! Update subscription. Wire this to `telegram_framework::Updates` later.

use domain::message::Message;

#[derive(Debug)]
pub struct UpdateStream {
    _private: (),
}

impl UpdateStream {
    #[must_use]
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// yields nothing yet.
    pub fn next_stream(&mut self) -> Option<Message> {
        None
    }
}

impl Default for UpdateStream {
    fn default() -> Self {
        Self::new()
    }
}
