//! The account's contact list, and the block flag on a person.
//!
//! Both calls are raw `tl` requests: `grammers` has no wrapper for
//! `contacts.getContacts`, and the block pair is a single field request.
//! Everything crosses this module's boundary as [`ResolvedUser`] or `()`, so a
//! `grammers` type never reaches `proto`.

use grammers_client::session::types::PeerInfo;

use crate::client::Client;
use crate::error::FrameworkError;
use crate::tl;
use crate::users::{ResolvedUser, user_from_tl};

impl Client {
    /// Blocks or unblocks a person.
    ///
    /// # Errors
    ///
    /// Returns [`FrameworkError::UnknownPeer`] when the person is not in the
    /// session's cache, [`FrameworkError::NotAUser`] for a group or channel, and
    /// the request's error when Telegram refuses.
    pub async fn set_blocked(&self, peer_id: i64, blocked: bool) -> Result<(), FrameworkError> {
        let peer = self
            .peer_ref(peer_id)
            .ok_or(FrameworkError::UnknownPeer(peer_id))?;
        let input: tl::enums::InputPeer = peer.into();
        if !matches!(input, tl::enums::InputPeer::User(_)) {
            return Err(FrameworkError::NotAUser(peer_id));
        }

        if blocked {
            self.invoke(&tl::functions::contacts::Block {
                my_stories_from: false,
                id: input,
            })
            .await?;
        } else {
            self.invoke(&tl::functions::contacts::Unblock {
                my_stories_from: false,
                id: input,
            })
            .await?;
        }

        Ok(())
    }

    /// Reads the account's full contact list.
    ///
    /// `hash: 0` asks for the whole list; this client keeps no hash, so
    /// `contactsNotModified` cannot be the answer. The order is Telegram's, and
    /// it is preserved.
    ///
    /// # Errors
    ///
    /// Returns [`FrameworkError::Request`] when Telegram refuses or the answer
    /// cannot be decoded.
    pub async fn fetch_contacts(&self) -> Result<Vec<ResolvedUser>, FrameworkError> {
        let response = self
            .invoke(&tl::functions::contacts::GetContacts { hash: 0 })
            .await?;

        let raw = raw_users(response);
        let users = raw.iter().filter_map(user_from_tl).collect();

        // A contact the reader has never opened is not in the dialog list, so
        // this call is the only place its access_hash can enter the cache.
        for user in raw {
            self.remember_peer(PeerInfo::from(user)).await;
        }
        self.flush_session();

        Ok(users)
    }
}

/// The raw users, in server order.
fn raw_users(response: tl::enums::contacts::Contacts) -> Vec<tl::enums::User> {
    match response {
        tl::enums::contacts::Contacts::Contacts(list) => list.users,
        tl::enums::contacts::Contacts::NotModified => Vec::new(),
    }
}
