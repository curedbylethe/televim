//! Fixtures shared by the crate's unit tests.
//!
//! These are the values `grammers` builds for itself when it talks to Telegram.
//! Rebuilding them by hand is what lets the wrapper's mapping and bookkeeping
//! code be exercised without a datacenter — and therefore on every CI run,
//! rather than only in the opt-in integration suite.
//!
//! Nothing here reaches a release build: the module is behind `cfg(test)` and,
//! because it names `grammers` types, behind `live` as well.

use grammers_client::client::PasswordToken;
use grammers_mtsender::{InvocationError, RpcError};

use crate::tl;

/// Builds the RPC error Telegram answers a misused request with.
///
/// `name` is what Telegram sends once the trailing digits have been stripped
/// out, which is why `FLOOD_WAIT_31` arrives as `name: "FLOOD_WAIT"` with
/// `value: Some(31)`.
pub(crate) fn rpc(code: i32, name: &str, value: Option<u32>) -> InvocationError {
    InvocationError::Rpc(RpcError {
        code,
        name: name.to_owned(),
        value,
        caused_by: None,
    })
}

/// Builds the two-factor challenge a password prompt carries.
pub(crate) fn password_token(hint: Option<&str>) -> PasswordToken {
    use tl::enums::{PasswordKdfAlgo, SecurePasswordKdfAlgo};
    use tl::types::account::Password;

    PasswordToken::new(Password {
        has_recovery: false,
        has_secure_values: false,
        has_password: true,
        current_algo: None,
        srp_b: None,
        srp_id: None,
        hint: hint.map(str::to_owned),
        email_unconfirmed_pattern: None,
        new_algo: PasswordKdfAlgo::Unknown,
        new_secure_algo: SecurePasswordKdfAlgo::Unknown,
        secure_random: Vec::new(),
        pending_reset_date: None,
        login_email_pattern: None,
    })
}
