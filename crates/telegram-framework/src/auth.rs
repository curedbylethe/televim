//! The user-account login flow: phone, then code, then — for accounts that
//! have it enabled — the two-factor password.
//!
//! ```text
//!   phone ──▶ request_login_code ──▶ LoginToken
//!                                      │
//!                                      ▼
//!                                    sign_in ◀── code
//!                                      │
//!                     ┌────────────────┴────────────────┐
//!                     ▼                                 ▼
//!          SignInResult::Success         SignInResult::PasswordRequired(PasswordToken)
//!          (session persisted)                        │
//!                                                     ▼
//!                                      check_password ◀── password
//!                                                     │
//!                                                     ▼
//!                                          session persisted
//! ```
//!
//! Both terminal steps persist the session through the
//! [`SessionStore`](crate::SessionStore) the client was built with. If that
//! write fails the login still succeeds — the failure is logged at `warn`, so a
//! user who will have to sign in again on the next launch can find out why.
//!
//! Both of Telegram's refusals and the user its sign-in hands back are named
//! here: [`Refusal`] and [`classify`] for the first, [`SignedInUser`] and
//! [`Client::signed_in_user`](crate::Client::signed_in_user) for the second.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use crate::tl;

pub use crate::error::AuthError;

/// A latch that can be claimed exactly once.
///
/// [`LoginToken`] is its only user. Telegram's `phone_code_hash` may be redeemed
/// once, so the wrapper has to remember that it has been handed over. Keeping
/// the latch in its own type keeps the part that is easy to get wrong — the
/// compare-and-swap, which has to hold under concurrent sign-ins — testable
/// without a token, and therefore without a datacenter.
#[derive(Debug, Default)]
struct SingleUse {
    claimed: AtomicBool,
}

impl SingleUse {
    /// Claims the latch, or reports that it was already claimed.
    ///
    /// A compare-and-swap rather than a read followed by a write, so that two
    /// concurrent sign-ins cannot both win.
    fn claim(&self) -> Result<(), AuthError> {
        if self.claimed.swap(true, Ordering::AcqRel) {
            Err(AuthError::TokenAlreadyUsed)
        } else {
            Ok(())
        }
    }

    /// Whether the latch has been claimed, for `Debug` only.
    fn is_claimed(&self) -> bool {
        self.claimed.load(Ordering::Relaxed)
    }
}

/// A login code has been requested for a phone number.
///
/// It wraps Telegram's `phone_code_hash`, which may only be redeemed once. The
/// token therefore carries a flag that is set the first time it reaches
/// [`Client::sign_in`](crate::Client::sign_in); a second attempt fails with
/// [`AuthError::TokenAlreadyUsed`] instead of failing obscurely against
/// Telegram. If the code never arrives, or is mistyped, request a fresh one.
///
/// It is deliberately not `Clone`: a copy would be a second handle to a hash
/// Telegram will only redeem once, and the single-use guarantee would stop
/// meaning anything. A compile-time assertion in this module's tests keeps it
/// that way.
///
/// # Rate limits
///
/// Requesting a code is expensive: Telegram rate limits it per phone number and
/// delivers it over SMS or another logged-in client. Do not call
/// [`Client::request_login_code`](crate::Client::request_login_code) in a loop.
pub struct LoginToken {
    pub(crate) inner: grammers_client::client::LoginToken,
    used: SingleUse,
}

impl LoginToken {
    /// Wraps a token handed out by `grammers`.
    pub(crate) fn new(inner: grammers_client::client::LoginToken) -> Self {
        Self {
            inner,
            used: SingleUse::default(),
        }
    }

    /// Claims the token for a single sign-in attempt.
    ///
    /// Returns [`AuthError::TokenAlreadyUsed`] when it has already been
    /// redeemed, which is what stops a second attempt from reaching Telegram
    /// with a hash it will reject anyway.
    pub(crate) fn claim(&self) -> Result<(), AuthError> {
        self.used.claim()
    }
}

impl fmt::Debug for LoginToken {
    /// Renders the token without exposing the phone number or code hash it
    /// holds.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginToken")
            .field("used", &self.used.is_claimed())
            .finish_non_exhaustive()
    }
}

/// The account has two-factor authentication enabled, and needs its password.
///
/// Obtained from [`SignInResult::PasswordRequired`]; hand it to
/// [`Client::check_password`](crate::Client::check_password) to finish logging
/// in. It wraps Telegram's SRP challenge, so it must be passed straight back —
/// the password itself is never stored.
pub struct PasswordToken {
    /// Boxed because the SRP challenge is large, and because it would otherwise
    /// inflate every [`SignInResult`] to several hundred bytes.
    inner: Box<grammers_client::client::PasswordToken>,
}

impl PasswordToken {
    /// Wraps a challenge handed out by `grammers`.
    pub(crate) fn new(inner: grammers_client::client::PasswordToken) -> Self {
        Self {
            inner: Box::new(inner),
        }
    }

    /// Unwraps the challenge for `grammers`.
    pub(crate) fn into_inner(self) -> grammers_client::client::PasswordToken {
        *self.inner
    }

    /// The password hint the account owner set, if they set one.
    ///
    /// Safe to show to the user: Telegram sends it precisely so that a client
    /// can prompt with it.
    #[must_use]
    pub fn hint(&self) -> Option<&str> {
        self.inner.hint()
    }
}

impl fmt::Debug for PasswordToken {
    /// Renders the token without exposing the SRP challenge it holds.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PasswordToken")
            .field("hint", &self.hint())
            .finish_non_exhaustive()
    }
}

/// The outcome of [`Client::sign_in`](crate::Client::sign_in).
#[derive(Debug)]
#[non_exhaustive]
pub enum SignInResult {
    /// The account is signed in, and the session has been persisted.
    Success,

    /// Telegram wants the two-factor password before it will sign the account
    /// in. Nothing has been persisted yet: call
    /// [`Client::check_password`](crate::Client::check_password) with the token.
    PasswordRequired(PasswordToken),
}

/// How many wrong two-factor passwords the account can take.
///
/// Telegram allows three and refuses the fourth. Exported because the panel
/// draws the count, and the panel may not name this crate.
pub const PASSWORD_ATTEMPTS: u8 = 3;

/// What Telegram said about a sign-in attempt.
///
/// Named rather than a `bool` and a message, because the refusals differ in what
/// the reader can do next: an expired code asks for a new one, a wrong code does
/// not, and a throttled account says to wait. Three refusals that read the same
/// are three refusals that say the wrong thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The code Telegram sent is not the one that arrived.
    CodeInvalid,

    /// Telegram's code has expired. Asking again is the answer.
    CodeExpired,

    /// Telegram will not accept the phone number as written.
    PhoneInvalid,

    /// The phone number is banned, and no retry will change that.
    PhoneBanned,

    /// Telegram is rate limiting this account or this number.
    Throttled,

    /// The two-factor password is not right. Carries how many tries are left,
    /// because the fourth is refused and a reader who does not know the count
    /// will use all of them.
    PasswordInvalid {
        /// How many wrong passwords the account can still take.
        ///
        /// [`classify`] has no way to know this — the error *name* carries no
        /// count — so it fills in [`PASSWORD_ATTEMPTS`], and the count that is
        /// actually right is the one
        /// [`Client::check_password`](crate::Client::check_password) reports
        /// through [`AuthError::InvalidPassword`].
        attempts_left: u8,
    },

    /// The account has no two-factor password, so the step cannot be completed.
    PasswordMissing,

    /// The stored session was revoked from elsewhere.
    SessionRevoked,

    /// The stored session's authorisation key is no longer registered.
    SessionUnregistered,

    /// Telegram said something this build does not have a sentence for.
    ///
    /// The text is kept verbatim rather than discarded: a reason we have not
    /// heard of is more useful than a shrug, and `AGENTS.md` says a refusal says
    /// why.
    Other(String),
}

/// Telegram's error name, and what it means to a reader.
///
/// A match on the *name* of a known variant, not on the whole message: the
/// surrounding prose is `grammers`' and may change, and a mapping that breaks
/// when it does takes every refusal with it. An unrecognised error becomes
/// [`Refusal::Other`] and is reported verbatim rather than guessed at.
///
/// The input is the name Telegram sends — [`RequestError::Rpc::name`](crate::RequestError::Rpc::name),
/// which is ASCII in screaming snake case — rather than a formatted message, so
/// that `PHONE_CODE_INVALID` means the same thing however it is written up.
#[must_use]
pub fn classify(raw: &str) -> Refusal {
    match raw {
        "PHONE_CODE_INVALID" => Refusal::CodeInvalid,
        "PHONE_CODE_EXPIRED" => Refusal::CodeExpired,
        "PHONE_NUMBER_INVALID" => Refusal::PhoneInvalid,
        "PHONE_NUMBER_BANNED" => Refusal::PhoneBanned,
        // Telegram names the per-number rate limit differently from the
        // per-account one it signals with a trailing wait time, so both are
        // spelled out.
        "PHONE_NUMBER_FLOOD" => Refusal::Throttled,
        "PASSWORD_HASH_INVALID" => Refusal::PasswordInvalid {
            // The name carries no count; see the field's own documentation.
            attempts_left: PASSWORD_ATTEMPTS,
        },
        // The name Telegram actually sends, and the shorter form it has also
        // used. Either way the account has no password to give.
        "PASSWORD_MISSING" | "SESSION_PASSWORD_MISSING" => Refusal::PasswordMissing,
        "SESSION_REVOKED" => Refusal::SessionRevoked,
        "AUTH_KEY_UNREGISTERED" => Refusal::SessionUnregistered,
        // A throttle under any name: `FLOOD_WAIT_31` is a delay appended to the
        // name, and `FLOOD_PREMIUM_WAIT_7` is a different limit spelled the
        // same way. A *contains* rather than a prefix, so that it agrees with
        // `AuthError::from_invocation` — one flood, one answer. The exact
        // `PHONE_NUMBER_FLOOD` above is already matched by the time this arm is
        // reached, and is named rather than folded in because Telegram does send
        // it under that name.
        other if other.contains("FLOOD") => Refusal::Throttled,
        other => Refusal::Other(other.to_owned()),
    }
}

/// The account that just signed in, as `grammers` reported it.
///
/// Handed back by `sign_in` and `check_password` and kept by the client, because
/// it is the only object that identifies the account without a round trip: a
/// profile read is a second `users.getFullUser` for something Telegram has
/// already said, and this is what a `domain::Account` is built from first.
///
/// The fields are primitives and strings, as everywhere else in this crate, so
/// that no `grammers` type escapes it. There is no bio and no birthday here:
/// those are on the *full* user, and login is handed the short one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedInUser {
    /// Bare identifier of the account's own user.
    pub user_id: i64,

    pub first_name: String,
    pub last_name: String,

    /// `None` when the account has none set.
    pub username: Option<String>,

    /// The account's own phone number, as Telegram returned it.
    pub phone: Option<String>,
}

/// The user `grammers` handed back, in this crate's own vocabulary.
///
/// `None` for a `userEmpty`: a user Telegram has nothing to say about is not
/// somebody who has just signed in, and a name from one is a name nobody set.
/// A free function over the wire value, so the rule is tested without a
/// datacenter.
pub(crate) fn signed_in_from(raw: &tl::enums::User) -> Option<SignedInUser> {
    let tl::enums::User::User(user) = raw else {
        return None;
    };

    Some(SignedInUser {
        user_id: user.id,
        first_name: user.first_name.clone().unwrap_or_default(),
        last_name: user.last_name.clone().unwrap_or_default(),
        username: text(user.username.as_deref()),
        phone: text(user.phone.as_deref()),
    })
}

/// A field to show, when Telegram sent something in it.
///
/// Telegram answers an absent field as `None` and an unset one as an empty
/// string, and neither is a value: `@` with nothing behind it reads as a broken
/// screen rather than as an account that has not set a username.
fn text(raw: Option<&str>) -> Option<String> {
    raw.filter(|value| !value.is_empty()).map(str::to_owned)
}

/// How many wrong passwords this sign-in has already had.
///
/// Telegram does not report the count — the client counts its own refusals in
/// this flow — so the bookkeeping is a plain counter rather than anything read
/// out of an answer. It lives in its own type so the arithmetic is testable
/// without a client, and it is reset by a successful step so that a second
/// login flow on the same client starts from a full set of attempts.
#[derive(Debug, Default)]
pub(crate) struct PasswordAttempts {
    refused: AtomicU8,
}

impl PasswordAttempts {
    /// Records one refused password, and reports how many are left.
    pub(crate) fn refuse(&self) -> u8 {
        let refused = self
            .refused
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        PASSWORD_ATTEMPTS.saturating_sub(refused)
    }

    /// Forgets the refusals of a flow that has finished.
    pub(crate) fn reset(&self) {
        self.refused.store(0, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::thread;

    use super::*;
    use crate::testing;

    // A `LoginToken` must stay single use. A `Clone` impl would hand out a
    // second handle to a `phone_code_hash` Telegram only redeems once, which
    // would turn the guarantee into a comment instead of a rule.
    static_assertions::assert_not_impl_any!(LoginToken: Clone);

    #[test]
    fn a_single_use_latch_is_claimed_exactly_once() {
        let latch = SingleUse::default();

        assert!(!latch.is_claimed(), "a fresh latch starts unclaimed");
        assert!(latch.claim().is_ok(), "the first claim wins");
        assert!(latch.is_claimed(), "claiming sets the latch");

        assert!(
            matches!(latch.claim(), Err(AuthError::TokenAlreadyUsed)),
            "the second claim has to be refused, not silently accepted"
        );
    }

    #[test]
    fn concurrent_claims_have_exactly_one_winner() {
        const THREADS: usize = 8;

        let latch = Arc::new(SingleUse::default());
        let winners = Arc::new(AtomicUsize::new(0));

        let threads: Vec<_> = (0..THREADS)
            .map(|_| {
                let latch = Arc::clone(&latch);
                let winners = Arc::clone(&winners);
                thread::spawn(move || {
                    if latch.claim().is_ok() {
                        winners.fetch_add(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();

        for thread in threads {
            thread.join().expect("a claiming thread does not panic");
        }

        assert_eq!(
            winners.load(Ordering::SeqCst),
            1,
            "a compare-and-swap must let exactly one of {THREADS} claims through"
        );
    }

    #[test]
    fn a_password_token_exposes_the_hint_telegram_sent() {
        let token = PasswordToken::new(testing::password_token(Some("the pet's name")));
        assert_eq!(token.hint(), Some("the pet's name"));

        let token = PasswordToken::new(testing::password_token(None));
        assert_eq!(token.hint(), None, "an account without a hint reports none");
    }

    #[test]
    fn password_token_debug_shows_the_hint_but_not_the_challenge() {
        let token = PasswordToken::new(testing::password_token(Some("hint")));
        let rendered = format!("{token:?}");

        assert!(rendered.contains("Some(\"hint\")"), "got {rendered}");
        assert!(
            rendered.ends_with(".. }"),
            "the SRP challenge must not be rendered: {rendered}"
        );
    }

    /// Every name this build knows, and the one sentence that fits it.
    ///
    /// `PartialEq` is derived on `Refusal` precisely so this is a `for` loop:
    /// a table written as assertions is a table that can be left half-updated
    /// when a variant is added.
    #[test]
    fn every_known_error_name_is_its_own_refusal() {
        let cases = [
            ("PHONE_CODE_INVALID", Refusal::CodeInvalid),
            ("PHONE_CODE_EXPIRED", Refusal::CodeExpired),
            ("PHONE_NUMBER_INVALID", Refusal::PhoneInvalid),
            ("PHONE_NUMBER_BANNED", Refusal::PhoneBanned),
            ("PHONE_NUMBER_FLOOD", Refusal::Throttled),
            ("FLOOD_WAIT", Refusal::Throttled),
            // The delay follows the name, and is not this function's business.
            ("FLOOD_WAIT_31", Refusal::Throttled),
            // A throttle that is not the wait-at-the-front one.
            ("FLOOD_PREMIUM_WAIT_7", Refusal::Throttled),
            (
                "PASSWORD_HASH_INVALID",
                Refusal::PasswordInvalid {
                    attempts_left: PASSWORD_ATTEMPTS,
                },
            ),
            ("PASSWORD_MISSING", Refusal::PasswordMissing),
            ("SESSION_PASSWORD_MISSING", Refusal::PasswordMissing),
            ("SESSION_REVOKED", Refusal::SessionRevoked),
            ("AUTH_KEY_UNREGISTERED", Refusal::SessionUnregistered),
        ];

        for (name, expected) in cases {
            assert_eq!(classify(name), expected, "{name} was not read as expected");
        }
    }

    /// A message rather than a name is not something to guess at. `grammers`
    /// wraps its prose however it likes, and a mapping that reads the wrong half
    /// of a sentence takes every refusal with it.
    #[test]
    fn a_message_is_reported_verbatim_rather_than_matched() {
        let message = "rpc error 400 PHONE_CODE_INVALID: the code you sent is not the one we sent";
        assert_eq!(
            classify(message),
            Refusal::Other(message.to_owned()),
            "a message must not be read as a name"
        );
    }

    /// A refusal says why. An unknown name keeps its text so a reader can be told
    /// what actually happened.
    #[test]
    fn an_unknown_name_is_kept_whole() {
        for raw in ["INTERNAL", "", "phone_code_invalid", "🙂"] {
            assert_eq!(
                classify(raw),
                Refusal::Other(raw.to_owned()),
                "{raw:?} should have been kept verbatim"
            );
        }
    }

    /// Telegram allows three wrong passwords and refuses the fourth, so the
    /// count a reader is shown has to reach zero rather than wrap.
    #[test]
    fn each_refused_password_costs_one_of_three() {
        let attempts = PasswordAttempts::default();
        assert_eq!(attempts.refuse(), 2, "the first is refused, two are left");
        assert_eq!(attempts.refuse(), 1);
        assert_eq!(attempts.refuse(), 0, "the third is the last one");
        assert_eq!(attempts.refuse(), 0, "and a fourth cannot go negative");
    }

    /// A sign-in that finished starts over: the count is this flow's, not the
    /// client's lifetime.
    #[test]
    fn a_finished_flow_starts_with_a_full_set_of_attempts() {
        let attempts = PasswordAttempts::default();
        attempts.refuse();
        attempts.refuse();

        attempts.reset();

        assert_eq!(attempts.refuse(), 2);
    }

    /// A field somebody did not set is absent, not empty. An `@` with nothing
    /// behind it reads as a broken screen.
    #[test]
    fn an_unset_field_is_absent_rather_than_empty() {
        assert_eq!(text(None), None, "telegram may leave a field out");
        assert_eq!(text(Some("")), None, "and send it empty");
        assert_eq!(text(Some(" ")), Some(" ".to_owned()));
        assert_eq!(text(Some("ada")), Some("ada".to_owned()));
    }

    /// Telegram answers a `userEmpty` for a user it has nothing to say about, and
    /// a profile built from one would be a name nobody set.
    #[test]
    fn a_tombstone_is_not_the_account_that_just_signed_in() {
        let empty = tl::enums::User::Empty(tl::types::UserEmpty { id: 7 });
        assert_eq!(signed_in_from(&empty), None);
    }
}
