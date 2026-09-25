use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("not implemented yet")]
    NotImplemented,

    #[error("session store error: {0}")]
    Session(String),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("authentication error: {0}")]
    Auth(String),
}
