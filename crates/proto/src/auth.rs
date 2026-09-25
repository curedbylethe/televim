//! Login state machine.

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
