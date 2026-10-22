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
            self.invoke(&block_request(input)).await?;
        } else {
            self.invoke(&unblock_request(input)).await?;
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

/// The request that blocks a person: the person, not just their stories.
fn block_request(id: tl::enums::InputPeer) -> tl::functions::contacts::Block {
    tl::functions::contacts::Block {
        my_stories_from: false,
        id,
    }
}

/// The request that lifts a block, with the same scope as [`block_request`].
fn unblock_request(id: tl::enums::InputPeer) -> tl::functions::contacts::Unblock {
    tl::functions::contacts::Unblock {
        my_stories_from: false,
        id,
    }
}

/// The raw users, in server order.
fn raw_users(response: tl::enums::contacts::Contacts) -> Vec<tl::enums::User> {
    match response {
        tl::enums::contacts::Contacts::Contacts(list) => list.users,
        tl::enums::contacts::Contacts::NotModified => Vec::new(),
    }
}

#[cfg(all(test, feature = "live"))]
mod tests {
    use super::*;
    use crate::testing::user;
    use grammers_client::session::types::{PeerAuth, PeerId, PeerRef};

    fn person() -> tl::enums::InputPeer {
        PeerRef {
            id: PeerId::user(42).expect("42 is in the user range"),
            auth: PeerAuth::from_hash(7),
        }
        .into()
    }

    /// Swapping the two requests would make `:block` unblock, and a story-only
    /// scope would file the reader's block as something smaller.
    #[test]
    fn block_and_unblock_name_the_person_and_not_only_their_stories() {
        let block = block_request(person());
        let unblock = unblock_request(person());

        assert!(!block.my_stories_from && !unblock.my_stories_from);
        for id in [block.id, unblock.id] {
            let tl::enums::InputPeer::User(user) = id else {
                panic!("a person addresses as an input user peer");
            };
            assert_eq!((user.user_id, user.access_hash), (42, 7));
        }
    }

    #[test]
    fn contacts_keep_the_servers_order() {
        let users = vec![
            user(3, Some("Cy"), None, None, false),
            user(1, Some("Ada"), None, None, false),
            user(2, Some("Bo"), None, None, false),
        ];
        let page = tl::enums::contacts::Contacts::Contacts(tl::types::contacts::Contacts {
            contacts: Vec::new(),
            saved_count: 0,
            users,
        });

        let ids: Vec<i64> = raw_users(page).iter().map(tl::enums::User::id).collect();

        assert_eq!(ids, vec![3, 1, 2]);
    }

    #[test]
    fn an_unmodified_contact_list_is_empty() {
        assert!(raw_users(tl::enums::contacts::Contacts::NotModified).is_empty());
    }
}
