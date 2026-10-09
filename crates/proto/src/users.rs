//! Finding a person, as a `domain` candidate.
//!
//! Telegram answers "who is this?" two ways, and the framework exposes both:
//! an exact `@handle` resolves to one person, and a name is searched over the
//! account's own contacts. Each answer already comes back in the framework's
//! own vocabulary — numbers, strings and a bool — so all that is left here is
//! to put each result into the [`UserCandidate`](domain::user::UserCandidate)
//! the domain applies, the same seam `history` and `search` use. No `grammers`
//! type is named, and none needs to be: the framework stopped at its boundary.
//!
//! # Why the cap is here
//!
//! [`SEARCH_MATCHES`] lives in `domain`, and this crate clamps a requested page
//! to it so that the one number appears in one place — the same rule, and the
//! same constant, as `search`'s `limit_of`. A limit written down in two crates
//! is a limit that will be changed in one of them.
//!
//! # The bot flag
//!
//! A framework result says whether the account is a bot; a [`UserCandidate`]
//! has no field for it, so the flag is dropped at this seam. The candidate is
//! kept either way: the reader may be looking for a bot, and the surface can
//! mark one later — dropping the result here would leave nothing to mark.
//!
//! [`UserCandidate`]: domain::user::UserCandidate

#[cfg(any(feature = "live", test))]
use domain::search::SEARCH_MATCHES;

/// The page size a user search asks for, capped at what the domain will hold.
#[cfg(any(feature = "live", test))]
fn limit_of(limit: usize) -> usize {
    limit.min(SEARCH_MATCHES)
}

/// Turns a framework result into the domain's candidate.
///
/// Field for field but for `is_bot`, which the candidate has nowhere to put.
#[cfg(feature = "live")]
fn to_candidate(user: telegram_framework::users::ResolvedUser) -> domain::user::UserCandidate {
    domain::user::UserCandidate {
        user_id: user.user_id,
        display_name: user.display_name,
        username: user.username,
    }
}

/// The user operations, which need the framework's client.
///
/// Both return `domain` values or [`ProtoError`](crate::ProtoError), so a caller
/// needs no knowledge of Telegram's request shapes. A username that resolves to
/// nobody is `Ok(None)`, not an error: "there is nobody by that name" is an
/// answer.
#[cfg(feature = "live")]
impl crate::ProtoClient {
    /// Resolves an exact `@username` to the person who owns it.
    ///
    /// A handle nobody owns, a query that is not shaped like a handle, and a
    /// handle naming a group or a channel all come back as `Ok(None)`, because
    /// only a person has a [`UserCandidate`](domain::user::UserCandidate). A
    /// caller sends a name-shaped query to
    /// [`ProtoClient::search_users`](crate::ProtoClient::search_users) instead.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Framework`](crate::ProtoError::Framework) if
    /// Telegram rejects the request, if the connection fails, or if the answer
    /// cannot be decoded.
    pub async fn resolve_user(
        &self,
        query: &str,
    ) -> Result<Option<domain::user::UserCandidate>, crate::ProtoError> {
        let resolved = self.inner().resolve_username(query).await?;

        Ok(resolved.map(to_candidate))
    }

    /// Searches the account's own contacts for people matching `query`.
    ///
    /// The search does not reach strangers; they are found by
    /// [`ProtoClient::resolve_user`](crate::ProtoClient::resolve_user) instead.
    /// The result is in the order Telegram ranked it, one page bounded by
    /// [`SEARCH_MATCHES`].
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Framework`](crate::ProtoError::Framework) on the
    /// same failures as `resolve_user`.
    pub async fn search_users(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<domain::user::UserCandidate>, crate::ProtoError> {
        let users = self.inner().search_users(query, limit_of(limit)).await?;

        tracing::debug!(
            returned = users.len(),
            "searched the account's contacts for a person"
        );

        Ok(users.into_iter().map(to_candidate).collect())
    }

    /// Lists the people the account talks to most, highest-rated first.
    ///
    /// The list is one page of [`TOP_PEERS_LIMIT`] people, and Telegram's rating
    /// is the order: a caller does not sort it.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Framework`](crate::ProtoError::Framework) on the
    /// same failures as `resolve_user`, and when the account has top peers
    /// switched off.
    pub async fn top_peers(&self) -> Result<Vec<domain::user::UserCandidate>, crate::ProtoError> {
        let users = self
            .inner()
            .top_peers(telegram_framework::users::TOP_PEERS_LIMIT)
            .await?;

        Ok(users.into_iter().map(to_candidate).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_user_search_never_asks_for_more_than_the_domain_holds() {
        assert_eq!(limit_of(10), 10);
        assert_eq!(limit_of(SEARCH_MATCHES), SEARCH_MATCHES);
        assert_eq!(
            limit_of(SEARCH_MATCHES + 1),
            SEARCH_MATCHES,
            "asking for more than the list holds only delays the answer"
        );
        assert_eq!(limit_of(usize::MAX), SEARCH_MATCHES);
    }
}

/// The translation from the framework's result.
///
/// Gated with the type it converts from. The cap rule needs neither a feature
/// nor a datacenter and runs on every job; these run whenever the client does.
#[cfg(all(test, feature = "live"))]
mod live_tests {
    use super::*;

    /// A person as the framework describes one.
    fn resolved(
        user_id: i64,
        display_name: &str,
        username: Option<&str>,
        is_bot: bool,
    ) -> telegram_framework::users::ResolvedUser {
        telegram_framework::users::ResolvedUser {
            user_id,
            display_name: display_name.to_owned(),
            username: username.map(str::to_owned),
            is_bot,
        }
    }

    #[test]
    fn a_resolved_user_becomes_a_candidate_field_for_field() {
        assert_eq!(
            to_candidate(resolved(42, "Ada Lovelace", Some("ada"), false)),
            domain::user::UserCandidate {
                user_id: 42,
                display_name: "Ada Lovelace".to_owned(),
                username: Some("ada".to_owned()),
            }
        );
    }

    /// A person who has not set a handle is still a candidate; the field stays
    /// absent rather than becoming an empty string.
    #[test]
    fn a_missing_username_stays_missing() {
        assert_eq!(
            to_candidate(resolved(7, "chat 7", None, false)).username,
            None
        );
    }

    /// The bot flag is the one field with nowhere to go, and the result must
    /// survive its loss: a bot is a person the reader may be looking for.
    #[test]
    fn the_bot_flag_is_dropped_and_the_candidate_is_kept() {
        let candidate = to_candidate(resolved(9, "Helper", Some("helper_bot"), true));

        assert_eq!(candidate.user_id, 9);
        assert_eq!(candidate.display_name, "Helper");
        assert_eq!(
            candidate.username.as_deref(),
            Some("helper_bot"),
            "a bot is a candidate like any other"
        );
    }
}
