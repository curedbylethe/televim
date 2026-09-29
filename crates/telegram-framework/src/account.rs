//! The signed-in account, described in this crate's own vocabulary.
//!
//! `grammers` stops at this module's private functions. Every public item is
//! built from numbers and strings, so `proto` can turn an account into a
//! `domain` type without ever naming a `grammers` type — which is what `make
//! boundary` asserts for the whole crate.
//!
//! # Why this call is not a builder
//!
//! The request names `inputUserSelf`, so nothing has to be resolved first: there
//! is no `access_hash` to look up and no peer to be in the session's cache. What
//! does have to happen is reading two different objects — the `userFull` holds
//! the bio and the birthday, and the `user` holds the name, the username and
//! the phone number — and joining them on an identifier, which is the decision
//! that can be wrong. It is therefore a free function over the wire value, and
//! it is tested on every CI job rather than against a datacenter.
//!
//! # Why it also names the account
//!
//! Telegram discloses the account's own user identifier in exactly one place,
//! and this is it: `grammers` reports a bare identifier of `None` for a peer
//! that is the account itself, because there is no number to report until the
//! account asks. So a client that has never made this call cannot name its own
//! user, which is why the chat list skips such a peer rather than filing it
//! under an identifier that would address nothing.

use crate::client::Client;
use crate::error::{FrameworkError, RequestError};
use crate::tl;

/// A calendar date, as Telegram reports one.
///
/// A year is a disclosure a person is not required to make, so it is optional
/// rather than defaulted: a type that could not hold the yearless case would be
/// lying about the field in the one case a reader most wants to see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Birthday {
    /// Day of the month.
    pub day: i32,

    /// Month of the year, where January is 1.
    pub month: i32,

    /// The year, when the account disclosed one.
    pub year: Option<i32>,
}

/// What the account's own profile says.
///
/// The fields are primitives and strings so that a `grammers` type never escapes
/// the crate. `user_id` is the account's *bare* identifier — the same number a
/// [`DialogInfo`](crate::DialogInfo) reports for a conversation with itself,
/// once one exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// Bare identifier of the account's own user.
    pub user_id: i64,

    pub first_name: String,
    pub last_name: String,

    /// `None` when the account has none set.
    pub username: Option<String>,

    /// The account's own phone number, in the form Telegram returns it.
    pub phone: Option<String>,

    /// When the account was born, if it says.
    pub birthday: Option<Birthday>,

    /// The account's bio. `None` when unset. May be several lines.
    pub bio: Option<String>,
}

impl Client {
    /// Reads the account's own profile.
    ///
    /// Fetched once, at start-up: the profile does not change under a reader,
    /// and a panel that blanked and refilled every time it was opened would be a
    /// panel they could not trust.
    ///
    /// # Errors
    ///
    /// Returns [`FrameworkError::Request`] if Telegram rejects the request, if
    /// the connection fails, or if the answer cannot be decoded, and
    /// [`FrameworkError::AccountMissing`] if the answer did not carry the
    /// account's own user. The second is not a decoding failure — the answer
    /// parsed — but it is just as unusable, and reporting it as a network
    /// failure would send a reader to check a connection that is fine.
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
    /// let account = client.fetch_account().await?;
    /// println!("{} (@{:?})", account.user_id, account.username);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn fetch_account(&self) -> Result<Account, FrameworkError> {
        let request = tl::functions::users::GetFullUser {
            id: tl::enums::InputUser::UserSelf,
        };

        let response = self
            .inner()
            .invoke(&request)
            .await
            .map_err(|error| FrameworkError::from(RequestError::from_invocation(&error)))?;

        let Some(account) = account_from(response) else {
            tracing::warn!("telegram answered users.getFullUser without the account's own user");
            return Err(FrameworkError::AccountMissing);
        };

        // A call like this can cache a peer or move the datacenter, and that only
        // reaches the store if it is written back here.
        self.flush_session();

        tracing::debug!(
            user_id = account.user_id,
            has_username = account.username.is_some(),
            has_bio = account.bio.is_some(),
            "read the account's own profile"
        );

        Ok(account)
    }
}

/// Joins a `userFull` and the `user` of the same identifier into one account.
///
/// `None` when the answer does not carry a user for the account, which is the
/// one case in which there is nothing to show and no way to say who it belongs
/// to. Everything else in here is a field copy, and everything that is *not* a
/// field copy is a free function below — which is also why the two objects the
/// answer is made of are named in the same breath: the bio and the birthday are
/// on one, the name, username and phone number are on the other, and the
/// compiler will not tell us that we read `about` off the wrong one.
fn account_from(response: tl::enums::users::UserFull) -> Option<Account> {
    let tl::enums::users::UserFull::Full(page) = response;
    let tl::enums::UserFull::Full(full) = page.full_user;

    let index = position_of(&page.users, full.id)?;
    let user = present_user(&page.users[index])?;

    Some(Account {
        user_id: full.id,
        first_name: user.first_name.clone().unwrap_or_default(),
        last_name: user.last_name.clone().unwrap_or_default(),
        username: username(user.username.as_deref().unwrap_or_default()).map(str::to_owned),
        phone: text(user.phone.as_deref().unwrap_or_default()),
        birthday: birthday(full.birthday.as_ref()),
        bio: text(full.about.as_deref().unwrap_or_default()),
    })
}

/// Where in the list the user with this identifier is.
fn position_of(users: &[tl::enums::User], id: i64) -> Option<usize> {
    users.iter().position(|user| user.id() == id)
}

/// The user behind an entry, when the entry is a user rather than a tombstone.
///
/// Telegram answers with `userEmpty` for a user it has nothing to say about,
/// and one of those is not a profile: it has an identifier and no name, no
/// username and no phone number, so a panel built from one would show an
/// account with a name nobody set.
fn present_user(user: &tl::enums::User) -> Option<&tl::types::User> {
    match user {
        tl::enums::User::User(user) => Some(user),
        tl::enums::User::Empty(_) => None,
    }
}

/// The username to show, if the account has one.
///
/// `users.getFullUser` returns an empty string rather than an absent field, and
/// an empty username rendered as `@` reads as a broken screen rather than as an
/// account that has not set one. The rule belongs here rather than in the widget
/// because this is the only place a `grammers` type exists: a second copy of it
/// downstream would be a second thing to be wrong.
fn username(raw: &str) -> Option<&str> {
    (!raw.is_empty()).then_some(raw)
}

/// A field to show, when Telegram sent something in it.
fn text(raw: &str) -> Option<String> {
    (!raw.is_empty()).then(|| raw.to_owned())
}

/// The birthday to show, when Telegram sent a date rather than a number.
///
/// An out-of-range month or day is dropped rather than drawn: it is a field
/// somebody's client wrote by hand, and a row reading `born 0-99` is worse than
/// no row. A year of zero or less is dropped on its own, because Telegram
/// leaves it out and a placeholder would be indistinguishable from a real year.
fn birthday(raw: Option<&tl::enums::Birthday>) -> Option<Birthday> {
    let tl::enums::Birthday::Birthday(raw) = raw?;
    if !(1..=12).contains(&raw.month) || !(1..=31).contains(&raw.day) {
        tracing::warn!(
            day = raw.day,
            month = raw.month,
            "telegram sent a birthday that is not a date; it is not shown"
        );
        return None;
    }

    Some(Birthday {
        day: raw.day,
        month: raw.month,
        year: raw.year.filter(|year| *year > 0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A user Telegram has nothing to say about, which still has an identifier.
    fn empty(id: i64) -> tl::enums::User {
        tl::enums::User::Empty(tl::types::UserEmpty { id })
    }

    /// A birthday as the wire carries it.
    fn born(day: i32, month: i32, year: Option<i32>) -> tl::enums::Birthday {
        tl::enums::Birthday::Birthday(tl::types::Birthday { day, month, year })
    }

    #[test]
    fn a_username_telegram_left_blank_is_not_one() {
        assert_eq!(username(""), None, "an empty string is not a username");
        assert_eq!(username("ada"), Some("ada"));
    }

    /// A space is a character somebody typed, so it is content — the same rule
    /// the chat list's last-message preview follows.
    #[test]
    fn a_field_is_content_whenever_telegram_sent_anything() {
        assert_eq!(text(""), None);
        assert_eq!(text(" "), Some(" ".to_owned()));
        assert_eq!(text("+15551234567"), Some("+15551234567".to_owned()));
    }

    #[test]
    fn a_birthday_without_a_year_is_still_a_birthday() {
        assert_eq!(
            birthday(Some(&born(10, 12, None))),
            Some(Birthday {
                day: 10,
                month: 12,
                year: None
            })
        );
    }

    #[test]
    fn a_birthday_is_dropped_when_it_is_not_a_date() {
        for (day, month) in [(0, 1), (32, 1), (1, 0), (1, 13), (-1, 1), (1, -1)] {
            assert_eq!(
                birthday(Some(&born(day, month, Some(1815)))),
                None,
                "day={day} month={month}"
            );
        }
        assert_eq!(birthday(None), None, "an account need not have one");
    }

    /// A year of zero is what a client writing the field by hand leaves behind,
    /// and it must not be read as the year 0.
    #[test]
    fn a_birthday_with_an_impossible_year_keeps_the_date() {
        for year in [Some(0), Some(-1815), None] {
            let kept = birthday(Some(&born(1, 1, year))).expect("1 January is a date");
            assert_eq!(kept.year, None, "year={year:?} is not a year");
        }
    }

    #[test]
    fn the_account_is_found_by_its_own_identifier() {
        let users = vec![empty(1), empty(2)];
        assert_eq!(position_of(&users, 2), Some(1));
        assert_eq!(position_of(&users, 3), None);
        assert_eq!(position_of(&[], 1), None);
    }

    /// Telegram's `userEmpty` carries an identifier and nothing else, so a
    /// profile built from one would have a name nobody set.
    #[test]
    fn a_tombstone_is_not_a_profile() {
        assert!(present_user(&empty(7)).is_none());
    }
}
