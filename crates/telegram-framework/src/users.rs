//! Finding a person: an exact username, or a name among the account's
//! contacts.
//!
//! `grammers` stops at this module's private functions. [`ResolvedUser`] is
//! built from numbers, strings and a bool, so `proto` can turn a result into a
//! `domain` type without ever naming a `grammers` type — which is what `make
//! boundary` asserts for the whole crate.
//!
//! # Two calls, one question
//!
//! Telegram answers "who is this?" two different ways, and neither covers the
//! other. `contacts.ResolveUsername` finds *anyone* whose exact public handle
//! is given, including a stranger the account has never spoken to, but it needs
//! the handle and nothing else. `contacts.Search` searches the account's *own
//! contacts* by name — it will never surface a stranger — and `grammers-client`
//! does not wrap it at all, so it goes through [`Client::invoke`]. A query is
//! therefore routed by its shape rather than sent to both: a query that looks
//! like a username is resolved, and everything else is searched. That decision
//! is [Q5] in the plan, and it is the reason both methods exist rather than one.
//!
//! [Q5]: the `PR-CUR-18` plan.
//!
//! # The peer cache
//!
//! Every typed request path starts at [`Client::peer_ref`](crate::Client), which
//! reads a peer's `access_hash` out of the session cache — and only
//! [`Client::fetch_dialogs`](crate::Client::fetch_dialogs) used to write it
//! there. A person found by username or by name is not in the dialog list yet,
//! so both methods here put each result back into that cache before returning:
//! without it, the very next `send_message` or `fetch_history` would report
//! [`FrameworkError::UnknownPeer`](crate::FrameworkError::UnknownPeer) for the
//! peer this module just named.
//!
//! # The display name
//!
//! Telegram discloses a name in pieces and any of them can be missing, so
//! [`join_title`] repeats the chat list's fallback rather than inventing a
//! second rule: the two halves of the name, then the `@username`, then a
//! placeholder built from the identifier. A result is therefore never rendered
//! as an empty row.

use grammers_client::peer::Peer as ClientPeer;
use grammers_client::session::Session as _;
use grammers_client::session::types::PeerInfo;

use crate::client::Client;
use crate::error::{FrameworkError, RequestError};
use crate::history::clamp_limit;
use crate::tl;

/// A person Telegram named, described in this crate's own vocabulary.
///
/// The fields are primitives and strings so that a `grammers` type never
/// escapes the crate. `user_id` is the person's *bare* identifier — the same
/// number a message reports as its chat, so a result here can be matched against
/// the chat list without a lookup table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedUser {
    /// Bare identifier of the person.
    pub user_id: i64,

    /// Display name. Never empty: see [`join_title`].
    pub display_name: String,

    /// Public `@username`, if the person has one. Never empty, and carries no
    /// `@`.
    pub username: Option<String>,

    /// Whether the account is a bot rather than a person.
    ///
    /// A bot *is* a user as far as Telegram's peer identifiers are concerned;
    /// only the account flag tells the two apart. It is kept because a caller
    /// may want to mark a bot, and dropping it here would leave nothing left to
    /// mark it with.
    pub is_bot: bool,
}

impl Client {
    /// Resolves a public `@username` to the person who owns it.
    ///
    /// The lookup is exact: `@ada` resolves only the account whose handle is
    /// `ada`, and a handle nobody owns is reported as `Ok(None)` rather than as
    /// an error — "there is nobody by that name" is an answer, not a failure.
    ///
    /// A query that is not shaped like a username — it is blank, or contains
    /// whitespace — is not resolved and is reported as `Ok(None)`; the caller
    /// sends such a query to [`Client::search_users`] instead. This is the
    /// routing rule [Q5] decides, made concrete: a name is only never lost to
    /// the username path because that path declines it first.
    ///
    /// # Errors
    ///
    /// Returns [`FrameworkError::Request`] when Telegram rejects the request,
    /// when the connection fails, or when the answer cannot be decoded. A
    /// username that resolves to a group or a channel is `Ok(None)`, because
    /// only a person has a [`ResolvedUser`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use telegram_framework::session::MemoryStore;
    /// use telegram_framework::ClientBuilder;
    ///
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = ClientBuilder::new(1234, "api-hash")
    ///     .session_store(Box::new(MemoryStore::new()))
    ///     .build()
    ///     .await?;
    ///
    /// if let Some(user) = client.resolve_username("@ada").await? {
    ///     println!("{} ({})", user.display_name, user.user_id);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn resolve_username(
        &self,
        username: &str,
    ) -> Result<Option<ResolvedUser>, FrameworkError> {
        let Some(username) = normalize_username(username) else {
            return Ok(None);
        };

        let peer = self
            .inner()
            .resolve_username(username)
            .await
            .map_err(|error| FrameworkError::from(RequestError::from_invocation(&error)))?;

        let Some(peer) = peer else {
            return Ok(None);
        };

        // `grammers`' own resolver runs the answer through a peer map that
        // caches it; this writes the peer out explicitly so the guarantee does
        // not rest on that default. Only a user has an `access_hash` worth
        // keeping, and a group's or a channel's is not this method's to cache.
        if let ClientPeer::User(user) = &peer {
            self.remember_peer(PeerInfo::from(user)).await;
        }
        // The cache change only reaches the store through this flush.
        self.flush_session();

        match peer {
            ClientPeer::User(user) => Ok(user_from_tl(&user.raw)),
            ClientPeer::Group(_) | ClientPeer::Channel(_) => {
                tracing::debug!("a username resolved to a group or a channel rather than a person");
                Ok(None)
            }
        }
    }

    /// Searches the account's own contacts for people matching `query`.
    ///
    /// The search is over the account's contacts, not over Telegram at large, so
    /// a person the account has never added will not appear here no matter how
    /// the query is spelled — they are reached by [`Client::resolve_username`]
    /// instead.
    ///
    /// `limit` is clamped into what Telegram accepts, so a caller may count in
    /// whatever is convenient. The returned list is in the order Telegram sent
    /// it; sorting is the caller's decision to make once rather than this
    /// module's on every call.
    ///
    /// # Errors
    ///
    /// Returns [`FrameworkError::Request`] when Telegram rejects the request,
    /// when the connection fails, or when the answer cannot be decoded.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use telegram_framework::session::MemoryStore;
    /// use telegram_framework::ClientBuilder;
    ///
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = ClientBuilder::new(1234, "api-hash")
    ///     .session_store(Box::new(MemoryStore::new()))
    ///     .build()
    ///     .await?;
    ///
    /// for user in client.search_users("ada", 20).await? {
    ///     println!("{} ({})", user.display_name, user.user_id);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn search_users(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ResolvedUser>, FrameworkError> {
        let request = tl::functions::contacts::Search {
            // Channels are not people, and this method is only about people.
            // Bots are kept: a bot is a contact the reader may be looking for.
            broadcasts: false,
            bots: true,
            q: query.to_owned(),
            limit: clamp_limit(limit),
        };

        let response = self.invoke(&request).await?;

        // `contacts.Search` is not wrapped by `grammers`, so no peer map caches
        // the answer's `access_hash` values; seed them here, or a candidate the
        // reader picks cannot be opened.
        let tl::enums::contacts::Found::Found(found) = &response;
        for user in &found.users {
            self.remember_peer(PeerInfo::from(user)).await;
        }
        self.flush_session();

        Ok(users_from(response))
    }

    /// Lists the people the account talks to most, highest-rated first.
    ///
    /// Telegram rates each peer by how often the account writes to them, so
    /// this is the account's own frequency signal rather than a guess from the
    /// chat list. Only correspondents are asked for: a contact the account has
    /// never written to is not a conversation it is in, and bots, groups and
    /// channels are not people.
    ///
    /// `limit` is clamped into what Telegram accepts, and the result is also
    /// cut to `limit`. A peer Telegram names without sending its user, or
    /// whose user carries no `access_hash`, is left out: it could not be opened.
    ///
    /// # Errors
    ///
    /// Returns [`FrameworkError::Request`] when Telegram rejects the request,
    /// when the connection fails, when the answer cannot be decoded, or when
    /// the account has top peers switched off on Telegram's side.
    pub async fn top_peers(&self, limit: usize) -> Result<Vec<ResolvedUser>, FrameworkError> {
        let request = tl::functions::contacts::GetTopPeers {
            correspondents: true,
            bots_pm: false,
            bots_inline: false,
            phone_calls: false,
            forward_users: false,
            forward_chats: false,
            groups: false,
            channels: false,
            bots_app: false,
            bots_guestchat: false,
            offset: 0,
            limit: clamp_limit(limit),
            // Always a full answer. A non-zero hash would let Telegram reply
            // `NotModified`, and remembering that hash is state this client
            // does not keep.
            hash: 0,
        };

        let response = self.invoke(&request).await?;
        let users = top_peers_from(response, limit)?;

        // The same seeding as `search_users`: a top peer found here has to be
        // addressable when the reader opens it.
        for user in &users {
            self.remember_peer(PeerInfo::from(user)).await;
        }
        self.flush_session();

        Ok(users.iter().filter_map(user_from_tl).collect())
    }

    /// Puts a peer where [`Client::peer_ref`](crate::Client) reads it.
    ///
    /// A peer with no `access_hash` cannot be addressed at all, so it is not
    /// worth storing: a `min` user, or a `userEmpty`, is reported that way, and
    /// caching one would only take a slot that says nothing.
    pub(crate) async fn remember_peer(&self, info: PeerInfo) {
        if info.auth().is_none() {
            return;
        }
        // `StoreSession` implements `grammers`' `Session`, whose `cache_peer`
        // inserts into the in-memory map. Its error type is `Infallible`: the
        // store is written separately, so this cannot fail.
        self.session_handle()
            .cache_peer(&info)
            .await
            .expect("the in-memory peer cache cannot fail");
    }
}

/// Reads a query as a username, or reports that it is not one.
///
/// A leading `@` is what a person writes and what Telegram does not send, so it
/// is stripped. Any whitespace makes the query a name rather than a handle, and
/// a handle with nothing left in it is not one either. The answer borrows the
/// input so the common miss costs no allocation.
fn normalize_username(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    let name = trimmed.strip_prefix('@').unwrap_or(trimmed);
    (!name.is_empty() && !name.chars().any(char::is_whitespace)).then_some(name)
}

/// Maps a `contacts.Search` answer to the people it names.
///
/// Extracted so that the only real logic in name search is testable without a
/// client: the answer carries users and chats in the same breath, and skipping a
/// chat is what keeps a group out of a list of people.
fn users_from(response: tl::enums::contacts::Found) -> Vec<ResolvedUser> {
    let tl::enums::contacts::Found::Found(found) = response;
    found
        .users
        .into_iter()
        .filter_map(|user| user_from_tl(&user))
        .collect()
}

/// How many top peers a caller is shown: the empty search's whole list.
pub const TOP_PEERS_LIMIT: usize = 5;

/// Maps a `contacts.GetTopPeers` answer to the users it names, in rating order.
///
/// The answer carries peers and users apart: a peer names its user by identifier
/// only, and the `access_hash` that makes it addressable is in the top-level
/// `users` list, so the two are joined here. Only correspondent peers that are
/// users count, and a joined user that is `min` or has no `access_hash` is
/// dropped, the same rule `remember_peer` applies.
///
/// `NotModified` is empty: it is only sent against a non-zero hash, which this
/// client never sends. `Disabled` is an error, because the reader asked for a
/// list the account's settings do not give out.
fn top_peers_from(
    response: tl::enums::contacts::TopPeers,
    limit: usize,
) -> Result<Vec<tl::enums::User>, FrameworkError> {
    let peers = match response {
        tl::enums::contacts::TopPeers::Peers(peers) => peers,
        tl::enums::contacts::TopPeers::NotModified => return Ok(Vec::new()),
        // `RequestError` has no variant for a feature the server switched off,
        // and the caller needs only a reason to show. `Deserialize` is the
        // closest, and the reason names the actual fact.
        tl::enums::contacts::TopPeers::Disabled => {
            return Err(FrameworkError::Request(RequestError::Deserialize(
                "top peers are disabled for this account".to_owned(),
            )));
        }
    };

    Ok(peers
        .categories
        .iter()
        .filter(|category| {
            let tl::enums::TopPeerCategoryPeers::Peers(category) = category;
            matches!(category.category, tl::enums::TopPeerCategory::Correspondents)
        })
        .flat_map(|category| {
            let tl::enums::TopPeerCategoryPeers::Peers(category) = category;
            category.peers.iter()
        })
        .filter_map(|top| {
            let tl::enums::TopPeer::Peer(top) = top;
            match &top.peer {
                tl::enums::Peer::User(user) => Some(user.user_id),
                _ => None,
            }
        })
        .filter_map(|id| {
            peers
                .users
                .iter()
                .find(|user| match user {
                    tl::enums::User::User(user) => user.id == id,
                    tl::enums::User::Empty(user) => user.id == id,
                })
                .cloned()
        })
        .filter(|user| matches!(user, tl::enums::User::User(user) if !user.min && user.access_hash.is_some()))
        .take(limit)
        .collect())
}

/// Maps one wire user to a [`ResolvedUser`], when it is a user at all.
///
/// Telegram answers with `userEmpty` for an account it has nothing to say about,
/// and one of those is not a person a list can show: it has an identifier and no
/// name, no username and no `access_hash`. `None` is what keeps it out.
fn user_from_tl(user: &tl::enums::User) -> Option<ResolvedUser> {
    let tl::enums::User::User(user) = user else {
        return None;
    };

    Some(ResolvedUser {
        user_id: user.id,
        display_name: join_title(
            user.first_name.as_deref(),
            user.last_name.as_deref(),
            user.username.as_deref(),
            user.id,
        ),
        username: username_of(user.username.as_deref()),
        is_bot: user.bot,
    })
}

/// The username to show, if the person has one.
///
/// Telegram sends an empty string rather than an absent field, and an empty
/// username rendered as `@` reads as a broken row rather than as an account that
/// has not set one. Blank and whitespace-only values count as missing for the
/// same reason a name made of spaces is not a name.
fn username_of(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// Joins the pieces of a name, falling back to a username and then to a
/// placeholder built from the identifier.
///
/// The same chain the chat list's `join_title` walks, repeated rather than
/// shared so that this module owns no dependency on that one's private
/// functions. Blank and whitespace-only pieces count as missing, because
/// Telegram sends them for accounts that have been emptied out.
fn join_title(
    first: Option<&str>,
    last: Option<&str>,
    username: Option<&str>,
    user_id: i64,
) -> String {
    let parts = [first, last]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|part| !part.is_empty());

    let mut name = String::new();
    for part in parts {
        if !name.is_empty() {
            name.push(' ');
        }
        name.push_str(part);
    }
    if !name.is_empty() {
        return name;
    }

    match username.map(str::trim).filter(|name| !name.is_empty()) {
        Some(username) => format!("@{username}"),
        None => format!("chat {user_id}"),
    }
}

#[cfg(all(test, feature = "live"))]
mod tests {
    use super::*;

    /// A user Telegram has nothing to say about, which still has an identifier.
    fn empty(id: i64) -> tl::enums::User {
        tl::enums::User::Empty(tl::types::UserEmpty { id })
    }

    /// A full user with the given name, handle and bot flag.
    ///
    /// The field list is `tl`'s and it is long because the type is generated
    /// with no `Default` to lean on; only the fields a test reads are worth
    /// setting.
    fn user(
        id: i64,
        first: Option<&str>,
        last: Option<&str>,
        username: Option<&str>,
        bot: bool,
    ) -> tl::enums::User {
        tl::enums::User::User(tl::types::User {
            is_self: false,
            contact: false,
            mutual_contact: false,
            deleted: false,
            bot,
            bot_chat_history: false,
            bot_nochats: false,
            verified: false,
            restricted: false,
            min: false,
            bot_inline_geo: false,
            support: false,
            scam: false,
            apply_min_photo: false,
            fake: false,
            bot_attach_menu: false,
            premium: false,
            attach_menu_enabled: false,
            bot_can_edit: false,
            close_friend: false,
            stories_hidden: false,
            stories_unavailable: false,
            contact_require_premium: false,
            bot_business: false,
            bot_has_main_app: false,
            bot_forum_view: false,
            bot_forum_can_manage_topics: false,
            bot_can_manage_bots: false,
            bot_guestchat: false,
            bot_guard: false,
            id,
            access_hash: Some(7),
            first_name: first.map(str::to_owned),
            last_name: last.map(str::to_owned),
            username: username.map(str::to_owned),
            phone: None,
            photo: None,
            status: None,
            bot_info_version: None,
            restriction_reason: None,
            bot_inline_placeholder: None,
            lang_code: None,
            emoji_status: None,
            usernames: None,
            stories_max_id: None,
            color: None,
            profile_color: None,
            bot_active_users: None,
            bot_verification_icon: None,
            send_paid_messages_stars: None,
        })
    }

    /// A search answer carrying the given users and no chats.
    fn found(users: Vec<tl::enums::User>) -> tl::enums::contacts::Found {
        tl::enums::contacts::Found::Found(tl::types::contacts::Found {
            my_results: Vec::new(),
            results: Vec::new(),
            chats: Vec::new(),
            users,
        })
    }

    #[test]
    fn a_username_strips_a_leading_at() {
        assert_eq!(normalize_username("@ada"), Some("ada"));
        assert_eq!(
            normalize_username("ada"),
            Some("ada"),
            "the at is optional, and a handle is a handle either way"
        );
        assert_eq!(
            normalize_username("  @ada  "),
            Some("ada"),
            "the whitespace a prompt hands over is not part of the handle"
        );
    }

    #[test]
    fn a_query_with_whitespace_is_a_name_rather_than_a_username() {
        assert_eq!(normalize_username("ada lovelace"), None);
        assert_eq!(normalize_username("ada\tlovelace"), None);
        assert_eq!(
            normalize_username(""),
            None,
            "an empty query names nobody at all"
        );
        assert_eq!(normalize_username("@"), None, "a bare at is not a handle");
        assert_eq!(normalize_username("   "), None);
    }

    #[test]
    fn a_found_answer_maps_the_people_in_it() {
        let users = users_from(found(vec![user(
            42,
            Some("Ada"),
            Some("Lovelace"),
            Some("ada"),
            false,
        )]));

        assert_eq!(
            users,
            vec![ResolvedUser {
                user_id: 42,
                display_name: "Ada Lovelace".to_owned(),
                username: Some("ada".to_owned()),
                is_bot: false,
            }]
        );
    }

    /// The answer carries chats as well as users, and a chat is not a person a
    /// list of candidates can show.
    #[test]
    fn a_tombstone_is_not_a_person() {
        let users = users_from(found(vec![
            empty(7),
            user(42, Some("Ada"), None, None, false),
        ]));

        assert_eq!(users.len(), 1, "the tombstone was skipped");
        assert_eq!(users[0].user_id, 42);
    }

    #[test]
    fn a_bot_is_kept_and_marked() {
        let users = users_from(found(vec![user(9, Some("Helper"), None, None, true)]));
        assert!(users[0].is_bot);
    }

    /// The profile's status is read off the user Telegram sent: absent is no
    /// presence, and a sent status is kept, `Offline`'s timestamp included.
    #[test]
    fn a_profile_keeps_the_status_telegram_sent() {
        use crate::account::presence_of;
        use crate::updates::UserPresence;

        let tl::enums::User::User(mut ada) = user(42, Some("Ada"), None, None, false) else {
            panic!("the fixture is a full user");
        };
        assert_eq!(presence_of(&ada), None, "no status sent is no presence");

        ada.status = Some(tl::enums::UserStatus::Offline(
            tl::types::UserStatusOffline {
                was_online: 1_700_000_000,
            },
        ));
        assert_eq!(
            presence_of(&ada),
            Some(UserPresence::Offline {
                was_online: 1_700_000_000
            })
        );
    }

    #[test]
    fn a_name_made_of_blanks_falls_back_to_the_username() {
        let users = users_from(found(vec![user(42, Some("   "), None, Some("ada"), false)]));

        assert_eq!(users[0].display_name, "@ada");
        assert_eq!(
            users[0].username.as_deref(),
            Some("ada"),
            "and the handle itself is kept for the caller"
        );
    }

    #[test]
    fn a_user_with_no_name_or_username_falls_back_to_its_identifier() {
        let users = users_from(found(vec![user(42, None, None, None, false)]));

        assert_eq!(users[0].display_name, "chat 42");
        assert_eq!(users[0].username, None);
    }

    /// A resolved user is addressable only because its `access_hash` reaches the
    /// cache, so the value [`Client::remember_peer`] is handed has to carry one.
    #[test]
    fn a_full_user_carries_the_access_hash_the_cache_needs() {
        let info = PeerInfo::from(&user(42, Some("Ada"), None, Some("ada"), false));

        assert_eq!(info.id().bare_id(), Some(42));
        assert!(
            info.auth().is_some(),
            "an addressable user must carry an access hash"
        );
    }

    /// A global search answer names the conversation of each hit, so the peer it
    /// names must be addressable once the answer is in. The user helper lives in
    /// this module, which is why the test does too; the seeding is the one the
    /// search runs before it returns.
    #[tokio::test]
    async fn a_search_answer_leaves_its_peers_in_the_cache() {
        let client = crate::client::ClientBuilder::new(1, "hash")
            .session_store(Box::new(crate::session::MemoryStore::new()))
            .build()
            .await
            .expect("a client builds without a network");
        assert!(client.peer_ref(42).is_none(), "nothing is cached yet");

        let response = tl::enums::messages::Messages::Messages(tl::types::messages::Messages {
            messages: Vec::new(),
            topics: Vec::new(),
            chats: Vec::new(),
            users: vec![user(42, Some("Ada"), None, Some("ada"), false)],
        });
        client.remember_answer_peers(&response).await;

        assert!(
            client.peer_ref(42).is_some(),
            "a peer the answer named can be opened"
        );
    }

    /// A correspondent peer naming the user with the given identifier.
    fn top_peer(id: i64) -> tl::enums::TopPeer {
        tl::enums::TopPeer::Peer(tl::types::TopPeer {
            peer: tl::enums::Peer::User(tl::types::PeerUser { user_id: id }),
            rating: 1.0,
        })
    }

    /// One category of a top-peers answer. The count is not read by the mapping,
    /// so it is left at zero.
    fn category(
        category: tl::enums::TopPeerCategory,
        peers: Vec<tl::enums::TopPeer>,
    ) -> tl::enums::TopPeerCategoryPeers {
        tl::enums::TopPeerCategoryPeers::Peers(tl::types::TopPeerCategoryPeers {
            category,
            count: 0,
            peers,
        })
    }

    /// A top-peers answer with the given categories and users.
    fn top_peers_answer(
        categories: Vec<tl::enums::TopPeerCategoryPeers>,
        users: Vec<tl::enums::User>,
    ) -> tl::enums::contacts::TopPeers {
        tl::enums::contacts::TopPeers::Peers(tl::types::contacts::TopPeers {
            categories,
            chats: Vec::new(),
            users,
        })
    }

    /// `user` with one of its flags or its access hash changed by `edit`.
    fn edited(user: tl::enums::User, edit: impl FnOnce(&mut tl::types::User)) -> tl::enums::User {
        let tl::enums::User::User(mut user) = user else {
            panic!("the fixture is a full user");
        };
        edit(&mut user);
        tl::enums::User::User(user)
    }

    fn id_of(user: &tl::enums::User) -> i64 {
        match user {
            tl::enums::User::User(user) => user.id,
            tl::enums::User::Empty(user) => user.id,
        }
    }

    #[test]
    fn top_peers_keep_telegrams_rating_order() {
        let answer = top_peers_answer(
            vec![category(
                tl::enums::TopPeerCategory::Correspondents,
                vec![top_peer(42), top_peer(7)],
            )],
            vec![
                user(7, Some("Grace"), None, None, false),
                user(42, Some("Ada"), None, None, false),
            ],
        );

        let users = top_peers_from(answer, 5).expect("a list is not an error");
        let ids: Vec<i64> = users.iter().map(id_of).collect();

        assert_eq!(
            ids,
            vec![42, 7],
            "the rating order, not the order of the users list"
        );
    }

    #[test]
    fn only_the_correspondents_category_counts() {
        let answer = top_peers_answer(
            vec![category(
                tl::enums::TopPeerCategory::BotsPm,
                vec![top_peer(9)],
            )],
            vec![user(9, Some("Helper"), None, None, true)],
        );

        assert!(
            top_peers_from(answer, 5).expect("a list").is_empty(),
            "a bot the account messages is not a correspondent"
        );
    }

    #[test]
    fn a_peer_that_is_not_a_user_is_skipped() {
        let chat = tl::enums::TopPeer::Peer(tl::types::TopPeer {
            peer: tl::enums::Peer::Chat(tl::types::PeerChat { chat_id: 42 }),
            rating: 2.0,
        });
        let answer = top_peers_answer(
            vec![category(
                tl::enums::TopPeerCategory::Correspondents,
                vec![chat, top_peer(7)],
            )],
            vec![
                user(42, Some("Group"), None, None, false),
                user(7, Some("Grace"), None, None, false),
            ],
        );

        let ids: Vec<i64> = top_peers_from(answer, 5)
            .expect("a list")
            .iter()
            .map(id_of)
            .collect();
        assert_eq!(ids, vec![7], "a chat is not a person, even with a user id");
    }

    #[test]
    fn a_peer_with_no_user_entry_is_skipped() {
        let answer = top_peers_answer(
            vec![category(
                tl::enums::TopPeerCategory::Correspondents,
                vec![top_peer(5)],
            )],
            Vec::new(),
        );

        assert!(
            top_peers_from(answer, 5).expect("a list").is_empty(),
            "without a user there is no access hash, so nothing to open"
        );
    }

    #[test]
    fn a_min_or_hashless_user_is_skipped() {
        let answer = top_peers_answer(
            vec![category(
                tl::enums::TopPeerCategory::Correspondents,
                vec![top_peer(1), top_peer(2), top_peer(3)],
            )],
            vec![
                edited(user(1, Some("Min"), None, None, false), |u| u.min = true),
                edited(user(2, Some("Hashless"), None, None, false), |u| {
                    u.access_hash = None;
                }),
                user(3, Some("Ada"), None, None, false),
            ],
        );

        let ids: Vec<i64> = top_peers_from(answer, 5)
            .expect("a list")
            .iter()
            .map(id_of)
            .collect();
        assert_eq!(ids, vec![3], "only the addressable user is kept");
    }

    #[test]
    fn the_answer_is_cut_to_the_limit() {
        let answer = top_peers_answer(
            vec![category(
                tl::enums::TopPeerCategory::Correspondents,
                vec![top_peer(1), top_peer(2), top_peer(3)],
            )],
            vec![
                user(1, Some("One"), None, None, false),
                user(2, Some("Two"), None, None, false),
                user(3, Some("Three"), None, None, false),
            ],
        );

        let ids: Vec<i64> = top_peers_from(answer, 2)
            .expect("a list")
            .iter()
            .map(id_of)
            .collect();
        assert_eq!(ids, vec![1, 2], "the first two in rating order");
    }

    #[test]
    fn not_modified_is_an_empty_list_and_disabled_is_an_error() {
        assert!(
            top_peers_from(tl::enums::contacts::TopPeers::NotModified, 5)
                .expect("not modified is not an error")
                .is_empty()
        );
        assert!(
            top_peers_from(tl::enums::contacts::TopPeers::Disabled, 5).is_err(),
            "a list the account does not have is reported, not hidden as empty"
        );
    }
}
