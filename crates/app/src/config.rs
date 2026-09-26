//! Layered TOML + env configuration.
//!
//! Every setting is overridable from the environment under the `TELEVIM`
//! prefix, which is what makes the application runnable without a config file:
//! `TELEVIM_API_ID` is the same key the file would carry as `api_id`.
//!
//! # The account
//!
//! Four settings describe the account, and none of them is required. With no
//! application credentials the client has nothing to connect as and the screen
//! stays empty, which is a legitimate way to run it; with credentials but no
//! stored session it signs in, and that step needs a phone number and the code
//! Telegram sends to it.
//!
//! # Where the session is kept
//!
//! The authorisation key is the one secret this program holds, so by default it
//! goes to the operating system's credential store — the same default the
//! framework's builder has. [`Config::session_path`] is the way out for a
//! machine that has no such store, and the way to keep a run out of it
//! entirely.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub log_level: String,
    pub theme: String,

    /// Application identifier, from <https://my.telegram.org>.
    ///
    /// Identifies the *application*, not the account: every user of a build
    /// logs in under the same pair.
    pub api_id: Option<i32>,

    /// Application hash, matching [`Config::api_id`].
    pub api_hash: Option<String>,

    /// Phone number of the account, in international format (`+15551234567`).
    ///
    /// Only read when the stored session is not signed in.
    pub phone: Option<String>,

    /// The login code Telegram delivered for [`Config::phone`].
    ///
    /// Telegram expires it, so it is only useful for the sign-in that is about
    /// to happen: once the session is stored, this is not read again.
    pub code: Option<String>,

    /// The two-factor password, for an account that has one enabled.
    pub password: Option<String>,

    /// A file to keep the session in, instead of the OS credential store.
    ///
    /// Set this on a machine with no credential store — a headless one, for
    /// instance — or to keep a run from touching the real one.
    pub session_path: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            log_level: "info".to_owned(),
            theme: "default".to_owned(),
            api_id: None,
            api_hash: None,
            phone: None,
            code: None,
            password: None,
            session_path: None,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let mut builder = config::Config::builder()
            .set_default("log_level", "info")?
            .set_default("theme", "default")?;

        if path.exists() {
            builder = builder.add_source(config::File::from(path));
        }

        builder = builder.add_source(config::Environment::with_prefix("TELEVIM"));

        let cfg: Config = builder
            .build()
            .context("building config")?
            .try_deserialize()
            .context("deserialising config")?;
        Ok(cfg)
    }

    /// The application credentials, if both were given.
    ///
    /// Both or neither: half a pair cannot identify an application, so a
    /// missing half is the same as having none — the client is not built, and
    /// the screen says why.
    #[must_use]
    pub fn credentials(&self) -> Option<(i32, &str)> {
        Some((self.api_id?, self.api_hash.as_deref()?))
    }

    /// The phone number and login code, if both were given.
    ///
    /// The same rule as [`Config::credentials`], for the same reason: a phone
    /// number with no code cannot complete a sign-in, and asking Telegram for a
    /// code that will never be used is a request it throttles.
    #[must_use]
    pub fn login_credentials(&self) -> Option<(&str, &str)> {
        Some((self.phone.as_deref()?, self.code.as_deref()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A configuration with nothing set but the two required-by-nobody values.
    fn bare() -> Config {
        Config::default()
    }

    #[test]
    fn a_configuration_with_nothing_in_it_is_still_a_configuration() {
        let cfg = bare();

        assert_eq!(cfg.log_level, "info");
        assert_eq!(cfg.credentials(), None);
        assert_eq!(cfg.login_credentials(), None);
        assert_eq!(
            cfg.session_path, None,
            "the credential store is the default"
        );
    }

    #[test]
    fn both_halves_of_a_pair_are_needed_and_either_half_alone_is_not() {
        let mut cfg = bare();
        cfg.api_id = Some(1234);
        assert_eq!(cfg.credentials(), None, "an identifier with no hash");

        cfg.api_hash = Some("hash".to_owned());
        assert_eq!(cfg.credentials(), Some((1234, "hash")));

        let mut cfg = bare();
        cfg.phone = Some("+15551234567".to_owned());
        assert_eq!(cfg.login_credentials(), None, "a phone number with no code");

        cfg.code = Some("00000".to_owned());
        assert_eq!(cfg.login_credentials(), Some(("+15551234567", "00000")));
    }

    /// The environment is read under the prefix the rest of the workspace
    /// already uses, and a missing file is not an error.
    #[test]
    fn the_environment_supplies_what_the_file_does_not() {
        // SAFETY: the environment is process-wide and setting it races with any
        // other thread reading it. Nothing else in this crate reads these two
        // names — the other tests in this module build a `Config` directly —
        // and they are the same names the integration tests use, so a run that
        // has them set is already exercising this path.
        unsafe {
            std::env::set_var("TELEVIM_API_ID", "4242");
            std::env::set_var("TELEVIM_API_HASH", "from-the-environment");
        }

        let cfg = Config::load(Path::new("a-file-that-does-not-exist.toml"))
            .expect("a missing file is not an error");

        assert_eq!(cfg.credentials(), Some((4242, "from-the-environment")));
        assert_eq!(cfg.log_level, "info", "and the defaults still apply");
    }
}
