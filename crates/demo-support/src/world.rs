use std::collections::{BTreeMap, BTreeSet};

use domain::account::Account;
use domain::chat::Chat;
use domain::message::MediaKind;
use domain::user::UserCandidate;
use tui::KnownAccount;

/// The deterministic data for one fictional demo account.
#[derive(Debug, Clone)]
pub struct DemoAccount {
    pub id: i64,
    pub label: String,
    pub account: Account,
    pub known_accounts: Vec<KnownAccount>,
    pub chats: Vec<Chat>,
    pub archived: Vec<Chat>,
    pub messages: BTreeMap<i64, Vec<domain::message::Message>>,
    pub contacts: Vec<UserCandidate>,
    pub profiles: BTreeMap<i64, Account>,
    pub blocked: BTreeSet<i64>,
    pub local_names: BTreeMap<i64, String>,
    pub media: BTreeMap<(i64, i64), DemoMedia>,
    pub default_chat: i64,
}

/// Safe, generated media bytes used by the offline demo.
#[derive(Debug, Clone)]
pub struct DemoMedia {
    pub kind: MediaKind,
    pub filename: String,
    pub mime: &'static str,
    pub bytes: Vec<u8>,
    pub caption: String,
    pub downloadable: bool,
}

impl DemoAccount {
    /// Returns an active or archived private chat by its peer id.
    #[must_use]
    pub fn chat(&self, id: i64) -> Option<&Chat> {
        self.chats
            .iter()
            .chain(&self.archived)
            .find(|chat| chat.id == id)
    }

    /// Returns the messages for a chat, or an empty slice when it has none.
    #[must_use]
    pub fn messages(&self, chat_id: i64) -> &[domain::message::Message] {
        self.messages.get(&chat_id).map_or(&[], Vec::as_slice)
    }

    /// Returns the known profile for a peer.
    #[must_use]
    pub fn profile(&self, peer_id: i64) -> Option<&Account> {
        self.profiles.get(&peer_id)
    }
}

/// The fictional accounts and current selection for one demo session.
#[derive(Debug, Clone)]
pub struct DemoWorld {
    pub base_timestamp: i64,
    pub accounts: Vec<DemoAccount>,
    pub active: usize,
}

impl DemoWorld {
    /// Builds the same three-account world for the same caller-supplied time.
    #[must_use]
    pub fn new(base_timestamp: i64) -> Self {
        Self {
            base_timestamp,
            accounts: crate::fixtures::accounts(base_timestamp),
            active: 0,
        }
    }

    /// Returns the active fixture account.
    ///
    /// # Panics
    ///
    /// Panics if callers mutate the public fields to remove the active account.
    #[must_use]
    pub fn active(&self) -> &DemoAccount {
        self.accounts
            .get(self.active)
            .expect("demo world always has an active account")
    }

    /// Returns the active fixture account mutably.
    ///
    /// # Panics
    ///
    /// Panics if callers mutate the public fields to remove the active account.
    #[must_use]
    pub fn active_mut(&mut self) -> &mut DemoAccount {
        self.accounts
            .get_mut(self.active)
            .expect("demo world always has an active account")
    }

    /// Looks up an account by its fictional user id.
    #[must_use]
    pub fn account(&self, id: i64) -> Option<&DemoAccount> {
        self.accounts.iter().find(|account| account.id == id)
    }

    /// Selects a seeded account by id. Returns false when it is not in the cast.
    pub fn switch_to(&mut self, id: i64) -> bool {
        let Some(index) = self.accounts.iter().position(|account| account.id == id) else {
            return false;
        };
        self.active = index;
        true
    }
}
