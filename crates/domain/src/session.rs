//! Session state machine.
//!

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    LoggedOut,
    AwaitingCode { phone: String },
    AwaitingPassword { phone: String },
    LoggedIn { user_id: i64 },
}

#[derive(Debug, Clone)]
pub struct Session {
    pub state: SessionState,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            state: SessionState::LoggedOut,
        }
    }
}

impl Session {
    #[must_use]
    pub fn is_authenticated(&self) -> bool {
        matches!(self.state, SessionState::LoggedIn { .. })
    }
}

/// Where the reader is in the sign-in flow, and what Telegram last refused.
///
/// The `refusal` is persistent state, not a flash: a flash is transient by
/// definition, and "that code was wrong" has to still be on screen when the
/// reader looks back after typing the next one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginState {
    pub step: SessionState,
    pub refusal: Option<String>,
}

impl Default for LoginState {
    fn default() -> Self {
        Self {
            step: SessionState::LoggedOut,
            refusal: None,
        }
    }
}

/// How many 2FA passwords the reader may get wrong before the flow is over.
pub const PASSWORD_ATTEMPTS: u8 = 3;

/// How many password attempts remain after `used` have been spent.
///
/// A free function over a constant, so changing the number is a diff in one
/// place. Clamped rather than a panic: `panic = "abort"`, and a count that went
/// below zero would be a value, not a crash.
#[must_use]
pub fn attempts_left(used: u8) -> u8 {
    PASSWORD_ATTEMPTS.saturating_sub(used)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_logged_out() {
        let s = Session::default();
        assert!(!s.is_authenticated());
    }

    #[test]
    fn logged_in_is_authenticated() {
        let s = Session {
            state: SessionState::LoggedIn { user_id: 42 },
        };
        assert!(s.is_authenticated());
    }

    #[test]
    fn attempts_left_counts_down_from_three() {
        assert_eq!(attempts_left(0), 3);
        assert_eq!(attempts_left(1), 2);
        assert_eq!(attempts_left(2), 1);
        assert_eq!(attempts_left(3), 0);
    }

    #[test]
    fn attempts_left_clamps_at_zero() {
        assert_eq!(attempts_left(4), 0);
    }
}
