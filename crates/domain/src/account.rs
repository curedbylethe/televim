//! The signed-in account, as this program describes it.
//!
//! One struct and the two questions a display asks of it. Everything Telegram
//! sends that this type does not keep is dropped at the protocol boundary, so
//! what a widget can read is what this module is willing to commit to — and the
//! two `Option`s in here are the reason a panel never has to ask whether a field
//! is missing or empty.

/// A calendar date, as Telegram reports one.
///
/// Not a timestamp and not a `NaiveDate`. Telegram sends a birthday as a day, a
/// month and *sometimes* a year, because a year is a disclosure a person is not
/// required to make, and a type that cannot hold the yearless form would be
/// lying about the field in the case that matters most to the reader. Turning
/// this into a date string is a display concern and lives with the display.
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
/// Read-only. Two of the fields can never be edited by the person they describe
/// — the identifier and the phone number — and both are here from the start so
/// that their presence is the type's own claim rather than a later patch's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Account {
    /// The account's own user identifier.
    pub user_id: i64,

    pub first_name: String,
    pub last_name: String,

    /// `None` when the account has none set.
    ///
    /// An `Option` rather than an empty string because the empty string is a
    /// thing that renders: it becomes a bare `@` on screen, which reads as a
    /// broken widget rather than as an account that has not set one.
    pub username: Option<String>,

    /// The account's own phone number, in the form Telegram returns it.
    pub phone: Option<String>,

    /// When the account was born, if it says.
    pub birthday: Option<Birthday>,

    /// The account's bio. `None` when unset. May be several lines.
    pub bio: Option<String>,
}

impl Account {
    /// The name to show where a chat list shows a title.
    ///
    /// First and last joined, the first alone when there is no last, and the
    /// identifier when there is neither. Blank and whitespace-only halves count
    /// as missing, because that is what a cleared-out account sends.
    ///
    /// The identifier is the last resort rather than a constant for the same
    /// reason the chat list skips a peer with none: a made-up name would be a
    /// name that addresses nothing. Here the panel is already on screen, so the
    /// number is better than a blank row.
    #[must_use]
    pub fn display_name(&self) -> String {
        let mut name = String::new();
        for part in [self.first_name.trim(), self.last_name.trim()] {
            if part.is_empty() {
                continue;
            }
            if !name.is_empty() {
                name.push(' ');
            }
            name.push_str(part);
        }
        if name.is_empty() {
            name.push_str(&self.user_id.to_string());
        }
        name
    }

    /// Whether the account has anything to say in the bio row.
    ///
    /// A `bool` at the call site would be the rule written twice, and a bio row
    /// drawn for a bio that is not there is a row of nothing.
    #[must_use]
    pub fn has_bio(&self) -> bool {
        self.bio
            .as_deref()
            .is_some_and(|bio| !bio.trim().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An account with only the fields under test set.
    fn account(first: &str, last: &str) -> Account {
        Account {
            user_id: 1_234_567,
            first_name: first.to_owned(),
            last_name: last.to_owned(),
            ..Account::default()
        }
    }

    #[test]
    fn a_name_is_the_two_halves_joined() {
        assert_eq!(account("Ada", "Lovelace").display_name(), "Ada Lovelace");
        assert_eq!(account("Ada", "").display_name(), "Ada");
        assert_eq!(account("Ada", "   ").display_name(), "Ada");
        assert_eq!(
            account("  Ada  ", "  Lovelace ").display_name(),
            "Ada Lovelace"
        );
    }

    /// Nothing to call them: the number is the only honest thing left to draw.
    #[test]
    fn a_nameless_account_is_its_identifier() {
        for (first, last) in [("", ""), ("  ", "\t"), ("", "  ")] {
            let account = account(first, last);
            assert_eq!(
                account.display_name(),
                "1234567",
                "first={first:?} last={last:?}"
            );
        }
    }

    /// `display_name` is a claim about every account, so a default one has to
    /// produce a row rather than an empty string.
    #[test]
    fn a_default_account_still_names_itself() {
        assert_eq!(Account::default().display_name(), "0");
    }

    #[test]
    fn a_bio_exists_only_when_it_says_something() {
        let mut account = account("Ada", "Lovelace");
        assert!(!account.has_bio(), "an unset bio is not a bio");

        account.bio = Some(String::new());
        assert!(!account.has_bio(), "nor is an empty one");

        account.bio = Some("   \n ".to_owned());
        assert!(!account.has_bio(), "nor is one that is only whitespace");

        account.bio = Some("Analytical engines".to_owned());
        assert!(account.has_bio());
    }

    /// The yearless case is the one that has to survive, so it has to be
    /// representable.
    #[test]
    fn a_birthday_can_be_held_without_a_year() {
        let account = Account {
            birthday: Some(Birthday {
                day: 10,
                month: 12,
                year: None,
            }),
            ..account("Ada", "Lovelace")
        };

        assert_eq!(
            account.birthday.map(|b| (b.day, b.month, b.year)),
            Some((10, 12, None))
        );
    }
}
