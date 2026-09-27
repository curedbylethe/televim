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

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

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
}
