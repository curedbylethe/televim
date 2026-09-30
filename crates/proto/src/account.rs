//! Reading a profile: the account's own, and a contact's.
//!
//! Two operations, which need the framework's client, and one translation between
//! them. They are the same operation — `users.getFullUser` takes any `inputUser`
//! and returns the same shape either way — so they share `translate` rather than
//! each having one, because a field that survives the first and not the second is
//! a card that shows a bio and not a birthday with nothing to say which.
//!
//! Thin on purpose: a field-by-field translation and nothing else. Every
//! decision that could be wrong — what an empty username means, whether a
//! birthday is a date, what to do with a tombstone — is made in the framework,
//! which is the only place a `grammers` type exists. This module's whole job is
//! that no `telegram_framework` or `grammers` type reaches `domain` or `tui`.

use domain::account::{Account, Birthday};

use crate::ProtoError;

#[cfg(feature = "live")]
impl crate::ProtoClient {
    /// Reads the account's own profile.
    ///
    /// Fetched once, at start-up, rather than when the panel is opened: the
    /// profile does not change under a reader, and a panel that blanked and
    /// refilled every time it was opened would be a panel they could not trust.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Framework`] if Telegram rejects the request, if the
    /// connection fails, if the answer cannot be decoded, or if it did not carry
    /// the account's own user.
    pub async fn fetch_account(&self) -> Result<Account, ProtoError> {
        let account = self.inner().fetch_account().await?;

        tracing::debug!(
            user_id = account.user_id,
            has_username = account.username.is_some(),
            has_bio = account.bio.is_some(),
            "read the account's own profile"
        );

        Ok(translate(account))
    }

    /// Reads a contact's profile.
    ///
    /// Fetched when the reader opens somebody's card rather than at start-up,
    /// because there is one card the reader opens and not one profile — and
    /// because a field their privacy hides is *absent* rather than empty, which a
    /// card cannot report until it has asked.
    ///
    /// # Errors
    ///
    /// Returns [`ProtoError::Framework`] if the request fails, if the peer is not
    /// in the session's cache, or if the person has become a `userEmpty`. The
    /// second of those is a fact about this client rather than about the
    /// connection, and the card says which it is.
    pub async fn fetch_user(&self, peer_id: i64) -> Result<Account, ProtoError> {
        let account = self.inner().fetch_user(peer_id).await?;

        tracing::debug!(
            peer_id,
            user_id = account.user_id,
            has_username = account.username.is_some(),
            has_bio = account.bio.is_some(),
            "read a contact's profile"
        );

        Ok(translate(account))
    }
}

/// Translates the framework's account into the domain's.
///
/// A free function rather than a `From` impl: both types belong to other
/// crates, and the orphan rule does not allow a trait to be implemented between
/// them. The rule this module follows is the same one `types` follows — one
/// named function per translation, so what a value became is named at the call
/// site rather than implied.
fn translate(account: telegram_framework::Account) -> Account {
    Account {
        user_id: account.user_id,
        first_name: account.first_name,
        last_name: account.last_name,
        username: account.username,
        phone: account.phone,
        birthday: account.birthday.map(birthday),
        bio: account.bio,
    }
}

/// Translates one birthday.
fn birthday(birthday: telegram_framework::Birthday) -> Birthday {
    Birthday {
        day: birthday.day,
        month: birthday.month,
        year: birthday.year,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A framework account with only the fields under test filled in.
    fn framework(user_id: i64) -> telegram_framework::Account {
        telegram_framework::Account {
            user_id,
            first_name: "Ada".to_owned(),
            last_name: "Lovelace".to_owned(),
            username: None,
            phone: None,
            birthday: None,
            bio: None,
        }
    }

    /// The translation has to carry every field, because the panel's rows are
    /// built from it and a dropped one is a row that is silently missing.
    #[test]
    fn every_field_survives_the_translation() {
        let account = translate(telegram_framework::Account {
            username: Some("ada".to_owned()),
            phone: Some("+15551234567".to_owned()),
            birthday: Some(telegram_framework::Birthday {
                day: 10,
                month: 12,
                year: Some(1815),
            }),
            bio: Some("Analytical engines".to_owned()),
            ..framework(1_234_567)
        });

        assert_eq!(account.user_id, 1_234_567);
        assert_eq!(account.first_name, "Ada");
        assert_eq!(account.last_name, "Lovelace");
        assert_eq!(account.username.as_deref(), Some("ada"));
        assert_eq!(account.phone.as_deref(), Some("+15551234567"));
        assert_eq!(account.bio.as_deref(), Some("Analytical engines"));
        assert_eq!(
            account.birthday,
            Some(Birthday {
                day: 10,
                month: 12,
                year: Some(1815)
            })
        );
    }

    /// A contact whose privacy hides everything arrives sparse, and the card's
    /// whole rule is *a row exists only when the peer says something* — so a
    /// field that is absent has to stay absent rather than becoming an empty one.
    /// An empty value is a row reading "not set", which is precisely the thing the
    /// card is not supposed to draw.
    #[test]
    fn a_contact_who_says_nothing_translates_to_an_account_with_nothing_in_it() {
        let contact = translate(framework(42));

        assert_eq!(contact.user_id, 42);
        assert_eq!(contact.display_name(), "Ada Lovelace");
        assert_eq!(contact.username, None);
        assert_eq!(contact.phone, None);
        assert_eq!(contact.bio, None);
        assert_eq!(contact.birthday, None);
        assert!(!contact.has_bio(), "and an absent bio is not a bio");
    }

    /// A yearless birthday has to stay yearless through the translation: it is
    /// the case the field exists to represent.
    #[test]
    fn a_birthday_without_a_year_stays_without_one() {
        let account = translate(telegram_framework::Account {
            birthday: Some(telegram_framework::Birthday {
                day: 10,
                month: 12,
                year: None,
            }),
            ..framework(1)
        });

        assert_eq!(account.birthday.map(|b| b.year), Some(None));
    }
}
