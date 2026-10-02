//! The sign-in surface: three methods and the sentences a refusal is spoken in.
//!
//! Two halves, and they are not the same shape. The login state a panel draws is
//! [`LoginFlow`], which is a `domain` type and needs no Telegram. The three
//! operations that talk to Telegram are on [`ProtoClient`](crate::ProtoClient)
//! and are gated on `live`, because the framework's tokens exist only there.
//!
//! Thin on purpose, the same rule `account` and `types` follow. Which error a
//! failure is is decided in the framework by [`classify`]; this module's own job
//! is that no `telegram_framework` or `grammers` type reaches `domain` or `tui`,
//! which is why [`LoginCode`] and [`PasswordChallenge`] are opaque newtypes
//! rather than the framework's tokens named directly. `tui` holds one across
//! keystrokes and never learns what is inside it.
//!
//! ```text
//!   request_login_code ──▶ LoginCode ──▶ sign_in(code) ──┬─▶ SignIn::Account
//!                                                             │
//!                                                             └─▶ SignIn::PasswordRequired
//!                                                                      │
//!                                              check_password(challenge) ┘
//! ```

use domain::session::{Session, SessionState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginStep {
    Phone,
    Code,
    Password,
    Done,
}

#[derive(Debug)]
pub struct LoginFlow {
    pub session: Session,
    pub step: LoginStep,
}

impl LoginFlow {
    #[must_use]
    pub fn new() -> Self {
        Self {
            session: Session::default(),
            step: LoginStep::Phone,
        }
    }

    pub fn begin(&mut self, phone: &str) {
        self.session.state = SessionState::AwaitingCode {
            phone: phone.to_owned(),
        };
        self.step = LoginStep::Code;
    }
}

impl Default for LoginFlow {
    fn default() -> Self {
        Self::new()
    }
}

// Everything below this line names a `live`-gated framework type, so it is
// behind the same feature: `make boundary` resolves this crate with default
// features, and nothing here may be in that graph.
#[cfg(feature = "live")]
mod sign_in {
    use domain::account::Account;

    use telegram_framework::{
        AuthError, LoginToken, PasswordToken, Refusal, RequestError, SignInResult, SignedInUser,
        classify,
    };

    use crate::ProtoClient;
    use crate::ProtoError;

    /// A login code has been requested and the hash that redeems it is waiting.
    ///
    /// Opaque to everything but this crate: the field is private, so a caller
    /// cannot reach the framework's token, and [`Debug`] renders it rather than
    /// the `phone_code_hash` it holds.
    ///
    /// Taken by value wherever it is spent — see
    /// [`ProtoClient::sign_in`](crate::ProtoClient::sign_in). The framework's
    /// token is single-use and deliberately not `Clone`, and a newtype that
    /// could be copied would turn that into a comment.
    #[derive(Debug)]
    pub struct LoginCode(LoginToken);

    /// A two-factor challenge, and the hint the account published.
    ///
    /// The hint is read off the token **once**, here, and owned from then on:
    /// [`PasswordToken::hint`] borrows from the challenge, and the panel wants
    /// a `String` it can hand to a line that outlives this value's place in the
    /// flow. Reading it at the moment of display would mean holding the token
    /// alive to ask, which is the thing this type exists to avoid.
    #[derive(Debug)]
    pub struct PasswordChallenge {
        token: PasswordToken,
        hint: Option<String>,
    }

    impl PasswordChallenge {
        /// Wraps a challenge, taking the hint with it.
        fn new(token: PasswordToken) -> Self {
            let hint = token.hint().map(str::to_owned);
            Self { token, hint }
        }

        /// The password hint the account owner set, if they set one.
        ///
        /// Telegram sends it precisely so a client can prompt with it.
        #[must_use]
        pub fn hint(&self) -> Option<&str> {
            self.hint.as_deref()
        }

        /// Unwraps the challenge for the framework, which consumes it.
        fn into_token(self) -> PasswordToken {
            self.token
        }
    }

    /// The outcome of a sign-in step.
    ///
    /// Two, because Telegram has two answers and one of them is not a failure:
    /// an account with two-factor authentication is not signed in until the
    /// password step, and the challenge that step needs travels back out.
    #[derive(Debug)]
    pub enum SignIn {
        /// The account is signed in and the session has been persisted.
        Account(Account),

        /// Telegram wants the two-factor password. Nothing has been persisted
        /// yet, and the session becomes real only once this challenge is spent.
        PasswordRequired(PasswordChallenge),
    }

    impl ProtoClient {
        /// Asks Telegram to send a login code to `phone`.
        ///
        /// The number must be in international format, for example
        /// `+15551234567`. The returned [`LoginCode`] is what
        /// [`ProtoClient::sign_in`] redeems it with, and Telegram will only
        /// redeem one hash once.
        ///
        /// # Errors
        ///
        /// Returns [`ProtoError::Auth`] if Telegram refuses the number, if the
        /// request is throttled, or if the connection fails.
        pub async fn request_login_code(&self, phone: &str) -> Result<LoginCode, ProtoError> {
            let token = self.inner().request_login_code(phone).await?;

            // Deliberately logs neither the number nor the hash.
            tracing::debug!("requested a telegram login code");

            Ok(LoginCode(token))
        }

        /// Redeems the code and completes the login.
        ///
        /// The [`LoginCode`] is taken by value and spent whether this succeeds
        /// or not, because the hash behind it may be redeemed once. A retry asks
        /// [`ProtoClient::request_login_code`] for a fresh one.
        ///
        /// # Errors
        ///
        /// Returns [`ProtoError::Auth`] if the code is wrong or has expired, if
        /// the request is throttled, or if the connection fails. The refusal is
        /// reachable as [`ProtoError::refusal`], and
        /// [`refusal_sentence`] turns it into what the panel says.
        pub async fn sign_in(&self, code: LoginCode, entered: &str) -> Result<SignIn, ProtoError> {
            let LoginCode(token) = code;

            // Deliberately logs neither the code nor the hash.
            tracing::debug!("submitting the telegram login code");

            match self.inner().sign_in(&token, entered).await? {
                SignInResult::Success => self.signed_in(),
                SignInResult::PasswordRequired(token) => {
                    Ok(SignIn::PasswordRequired(PasswordChallenge::new(token)))
                }
                // `SignInResult` is `#[non_exhaustive]`. A new outcome is a new
                // sentence to write, not a new thing to do silently.
                other => Err(ProtoError::unrecognised(format!("{other:?}"))),
            }
        }

        /// Submits the two-factor password and completes the login.
        ///
        /// The [`PasswordChallenge`] is taken by value because the framework
        /// consumes it, and returned as [`SignIn::PasswordRequired`] by
        /// [`ProtoClient::sign_in`] the one time it is needed.
        ///
        /// # Errors
        ///
        /// Returns [`ProtoError::Auth`] if the password is wrong — carrying how
        /// many attempts are left, which is the reader's own count and not
        /// something a retry can find out again — or if the request fails.
        pub async fn check_password(
            &self,
            challenge: PasswordChallenge,
            password: &str,
        ) -> Result<SignIn, ProtoError> {
            // Deliberately logs neither the password nor its hint.
            tracing::debug!("submitting the telegram two-factor password");

            self.inner()
                .check_password(challenge.into_token(), password)
                .await?;

            self.signed_in()
        }

        /// The account a completed step signed in, as `domain` already describes
        /// it.
        ///
        /// Read from what the login was handed back rather than fetched: the
        /// identifier of your own user is disclosed nowhere else, so a
        /// `users.getFullUser` here would be a second request for something
        /// Telegram has already said. The bio and the birthday stay empty,
        /// because those live on the *full* user and login never fetches it —
        /// the account's own card reads them when it is opened.
        fn signed_in(&self) -> Result<SignIn, ProtoError> {
            let user = self
                .inner()
                .signed_in_user()
                .ok_or(ProtoError::UnnamedAccount)?;

            Ok(SignIn::Account(account(user)))
        }
    }

    /// A refusal, in the words the panel uses.
    ///
    /// Every sentence is the design's own. Three of them are Telegram's doing or
    /// the account's rather than the reader's, and none of them says `you`: a
    /// reader who mistypes a code is told the code was wrong, and a reader whose
    /// session was revoked from another device is not told they did anything.
    ///
    /// An unrecognised refusal is spoken as Telegram's own words, verbatim,
    /// because a reason we have not heard of is more useful than a shrug.
    #[must_use]
    pub fn refusal_sentence(r: &Refusal) -> String {
        match r {
            Refusal::CodeInvalid => "that code is not the one Telegram sent".to_owned(),
            Refusal::CodeExpired => "that code has expired — ⏎ asks for a new one".to_owned(),
            Refusal::PhoneInvalid => "that is not a phone number Telegram will accept".to_owned(),
            Refusal::PhoneBanned => "Telegram has banned that number".to_owned(),
            Refusal::Throttled => "too many attempts — wait, then try again".to_owned(),
            // The count is worth a branch rather than a plural: a panel drawing
            // "1 attempts left" is a panel nobody believes.
            Refusal::PasswordInvalid { attempts_left: 1 } => {
                "that password is not right (1 attempt left)".to_owned()
            }
            Refusal::PasswordInvalid { attempts_left } => {
                format!("that password is not right ({attempts_left} attempts left)")
            }
            Refusal::PasswordMissing => "this account has no two-factor password".to_owned(),
            Refusal::SessionRevoked => "this session was revoked — sign in again".to_owned(),
            Refusal::SessionUnregistered => {
                "the stored session is no longer valid — sign in again".to_owned()
            }
            Refusal::Other(raw) => raw.clone(),
        }
    }

    /// Translates the user a sign-in was handed back into the domain's account.
    ///
    /// A free function rather than a `From` impl, for the reason `account`'s is:
    /// both types belong to other crates and the orphan rule is in the way. One
    /// named function per translation, so what a value became is named where it
    /// is used.
    fn account(user: SignedInUser) -> Account {
        Account {
            user_id: user.user_id,
            first_name: user.first_name,
            last_name: user.last_name,
            username: user.username,
            phone: user.phone,
            // Login is handed the short user, which carries neither. A `default`
            // would be a guess; this says they are absent, which is what they are.
            birthday: None,
            bio: None,
        }
    }

    /// Reads an authentication failure as a refusal.
    ///
    /// The framework has already collapsed `grammers`' errors into
    /// [`AuthError`], so the question here is what a reader is told, and the
    /// answer is the same vocabulary [`classify`] speaks — one error, one
    /// sentence, whichever end of the crate saw it.
    ///
    /// Where the framework has folded Telegram's name away — the number has no
    /// account, the token was already spent, the password turned out to be
    /// needed after all — there is no name left to quote, so the framework's
    /// own wording is used. Each of those is a condition of the flow rather
    /// than a refusal Telegram stated, and none of them is a code the panel
    /// would recognise.
    pub(crate) fn refusal_of(error: &AuthError) -> Refusal {
        match error {
            AuthError::InvalidPhone => Refusal::PhoneInvalid,
            // `classify` reads `PHONE_CODE_INVALID` here too. An expired code
            // is told apart by asking for a new one, and only Telegram's own
            // name on a later request would say so.
            AuthError::InvalidCode => Refusal::CodeInvalid,
            AuthError::PasswordRequired => {
                Refusal::Other("the account requires a two-factor password".to_owned())
            }
            AuthError::InvalidPassword { attempts_left } => Refusal::PasswordInvalid {
                attempts_left: *attempts_left,
            },
            AuthError::SignUpRequired => Refusal::Other(
                "the phone number has no telegram account yet; sign up in an official client first"
                    .to_owned(),
            ),
            AuthError::TokenAlreadyUsed => Refusal::Other(
                "this login token has already been used; request a new login code".to_owned(),
            ),
            AuthError::RateLimited { .. } => Refusal::Throttled,
            // A refused request that is not about the credentials is the one
            // place a name survives, so it is read exactly as `classify` would.
            AuthError::NetworkError(RequestError::Rpc { name, .. }) => classify(name),
            // `AuthError` is `#[non_exhaustive]`, and a connection that failed is
            // not a refusal at all — it is reported in the framework's words,
            // which say what actually happened.
            other => Refusal::Other(other.to_string()),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Every refusal, with the sentence it is spoken in.
        ///
        /// `Refusal` is `PartialEq` precisely so this is a `for` loop: a table
        /// written as assertions is a table that can be left half-updated when
        /// a variant is added.
        #[test]
        fn every_refusal_has_the_designs_sentence() {
            let cases = [
                (
                    Refusal::CodeInvalid,
                    "that code is not the one Telegram sent",
                ),
                (
                    Refusal::CodeExpired,
                    "that code has expired — ⏎ asks for a new one",
                ),
                (
                    Refusal::PhoneInvalid,
                    "that is not a phone number Telegram will accept",
                ),
                (Refusal::PhoneBanned, "Telegram has banned that number"),
                (
                    Refusal::Throttled,
                    "too many attempts — wait, then try again",
                ),
                (
                    Refusal::PasswordInvalid { attempts_left: 2 },
                    "that password is not right (2 attempts left)",
                ),
                (
                    Refusal::PasswordMissing,
                    "this account has no two-factor password",
                ),
                (
                    Refusal::SessionRevoked,
                    "this session was revoked — sign in again",
                ),
                (
                    Refusal::SessionUnregistered,
                    "the stored session is no longer valid — sign in again",
                ),
                (Refusal::Other("in the soup".to_owned()), "in the soup"),
            ];

            for (refusal, expected) in cases {
                assert_eq!(refusal_sentence(&refusal), expected, "{refusal:?}");
            }
        }

        /// The one pluralisation the screen can get wrong. A count of one is a
        /// reader's last try, and "1 attempts left" is a sentence that makes the
        /// number look like a bug.
        #[test]
        fn the_last_attempt_is_singular() {
            assert_eq!(
                refusal_sentence(&Refusal::PasswordInvalid { attempts_left: 1 }),
                "that password is not right (1 attempt left)"
            );
        }

        /// Three refusals are Telegram's doing or the account's, and a reader
        /// who is told `you` did something they did not do stops believing the
        /// other sentences. This is the test that says so, per arm.
        #[test]
        fn a_refusal_that_is_not_the_readers_fault_does_not_say_you() {
            for refusal in [
                Refusal::CodeExpired,
                Refusal::SessionRevoked,
                Refusal::SessionUnregistered,
            ] {
                let sentence = refusal_sentence(&refusal);
                assert!(
                    !sentence.contains("you"),
                    "{refusal:?} blames the reader: {sentence}"
                );
            }
        }

        /// The number of wrong passwords the account has left is the reader's
        /// own count. It cannot be recovered by asking again, so it has to
        /// survive the translation into the error a panel draws.
        #[test]
        fn a_refused_password_keeps_the_count_it_was_given() {
            let error = ProtoError::from(AuthError::InvalidPassword { attempts_left: 1 });

            assert_eq!(
                error.refusal(),
                Some(&Refusal::PasswordInvalid { attempts_left: 1 })
            );
        }

        /// Every authentication failure this crate can be handed, and the
        /// refusal a panel would speak. `AuthError` is `#[non_exhaustive]`, so
        /// this is also what keeps the catch-all honest: a failure with no arm of
        /// its own is spoken in the framework's words rather than dropped.
        #[test]
        fn every_auth_error_reads_as_a_refusal() {
            let cases = [
                (AuthError::InvalidPhone, Refusal::PhoneInvalid),
                (AuthError::InvalidCode, Refusal::CodeInvalid),
                (
                    AuthError::PasswordRequired,
                    Refusal::Other("the account requires a two-factor password".to_owned()),
                ),
                (
                    AuthError::SignUpRequired,
                    Refusal::Other(
                        "the phone number has no telegram account yet; sign up in an official client first"
                            .to_owned(),
                    ),
                ),
                (
                    AuthError::TokenAlreadyUsed,
                    Refusal::Other(
                        "this login token has already been used; request a new login code"
                            .to_owned(),
                    ),
                ),
                (
                    AuthError::RateLimited {
                        retry_after: Some(31),
                    },
                    Refusal::Throttled,
                ),
                (
                    AuthError::NetworkError(RequestError::Network("no route".to_owned())),
                    Refusal::Other("network error: no route".to_owned()),
                ),
            ];

            for (error, expected) in cases {
                assert_eq!(
                    refusal_of(&error),
                    expected,
                    "{error} was not read as expected"
                );
            }
        }

        /// A refused request that is not about the credentials keeps Telegram's
        /// name, so it reaches the panel as the same refusal `classify` would
        /// have produced for it. Two paths to one sentence is the point.
        #[test]
        fn a_refused_request_keeps_the_refusal_telegrams_name_reads_as() {
            for name in [
                "PHONE_NUMBER_BANNED",
                "PASSWORD_HASH_INVALID",
                "SESSION_REVOKED",
                "FLOOD_WAIT_31",
            ] {
                let error = AuthError::NetworkError(RequestError::Rpc {
                    code: 400,
                    name: name.to_owned(),
                    value: None,
                });

                assert_eq!(
                    refusal_of(&error),
                    classify(name),
                    "{name} did not read the same way twice"
                );
            }
        }

        /// The account a sign-in names has to reach `domain` whole, because the
        /// card is built from it and a dropped field is a row that is silently
        /// missing.
        #[test]
        fn the_signed_in_user_becomes_a_whole_account() {
            let signed_in = account(SignedInUser {
                user_id: 1_234_567,
                first_name: "Ada".to_owned(),
                last_name: "Lovelace".to_owned(),
                username: Some("ada".to_owned()),
                phone: Some("+15551234567".to_owned()),
            });

            assert_eq!(signed_in.user_id, 1_234_567);
            assert_eq!(signed_in.display_name(), "Ada Lovelace");
            assert_eq!(signed_in.username.as_deref(), Some("ada"));
            assert_eq!(signed_in.phone.as_deref(), Some("+15551234567"));
        }

        /// Login is handed the short user, which carries no bio and no birthday.
        /// Those are absent rather than empty, so the card draws no row for
        /// them instead of a row reading "not set".
        #[test]
        fn the_fields_login_was_not_handed_stay_absent() {
            let signed_in = account(SignedInUser {
                user_id: 42,
                first_name: "Ada".to_owned(),
                last_name: String::new(),
                username: None,
                phone: None,
            });

            assert_eq!(signed_in.user_id, 42);
            assert_eq!(signed_in.display_name(), "Ada");
            assert_eq!(signed_in.username, None);
            assert_eq!(signed_in.phone, None);
            assert_eq!(signed_in.birthday, None);
            assert_eq!(signed_in.bio, None);
            assert!(!signed_in.has_bio());
        }
    }
}

/// The reading of a failure into a refusal. `crate`'s, because `ProtoError` is
/// the thing that needs it and a panel is the thing that must not have it.
#[cfg(feature = "live")]
pub(crate) use sign_in::refusal_of;
#[cfg(feature = "live")]
pub use sign_in::{LoginCode, PasswordChallenge, SignIn, refusal_sentence};
