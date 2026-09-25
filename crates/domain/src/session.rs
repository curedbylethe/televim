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
}
