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

/// A login code has been requested for a phone number.
///
/// It wraps Telegram's `phone_code_hash`, which may only be redeemed once. The
/// token therefore carries a flag that is set the first time it reaches
/// [`Client::sign_in`](crate::Client::sign_in); a second attempt fails with
/// [`AuthError::TokenAlreadyUsed`] instead of failing obscurely against
/// Telegram. If the code never arrives, or is mistyped, request a fresh one.
///
/// # Rate limits
///
/// Requesting a code is expensive: Telegram rate limits it per phone number and
/// delivers it over SMS or another logged-in client. Do not call
/// [`Client::request_login_code`](crate::Client::request_login_code) in a loop.
pub struct LoginToken {
    pub(crate) inner: grammers_client::types::LoginToken,
    used: AtomicBool,
}

impl LoginToken {
    /// Wraps a token handed out by `grammers`.
    pub(crate) fn new(inner: grammers_client::types::LoginToken) -> Self {
        Self {
            inner,
            used: AtomicBool::new(false),
        }
    }

    /// Claims the token, reporting whether it had already been claimed.
    ///
    /// This is deliberately a compare-and-swap rather than a read followed by a
    /// write, so that two concurrent sign-ins cannot both win.
    pub(crate) fn claim(&self) -> bool {
        self.used.swap(true, Ordering::AcqRel)
    }
}

impl fmt::Debug for LoginToken {
    /// Renders the token without exposing the phone number or code hash it
    /// holds.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoginToken")
            .field("used", &self.used.load(Ordering::Relaxed))
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
    inner: Box<grammers_client::types::PasswordToken>,
}

impl PasswordToken {
    /// Wraps a challenge handed out by `grammers`.
    pub(crate) fn new(inner: grammers_client::types::PasswordToken) -> Self {
        Self {
            inner: Box::new(inner),
        }
    }

    /// Unwraps the challenge for `grammers`.
    pub(crate) fn into_inner(self) -> grammers_client::types::PasswordToken {
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
