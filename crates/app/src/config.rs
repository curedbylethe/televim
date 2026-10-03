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
//! says so; with credentials but no stored session the sign-in flow asks for a
//! phone number, and the three settings below are what it fills the fields in
//! with so that a reader does not have to type them.
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
use tui::bidi::BidiMode;

/// Who arranges a right-to-left row: the terminal, or this program.
///
/// The two the terminal itself will not do for us, named as the configuration
/// spells them.
const BIDI_TERMINAL: &str = "terminal";
const BIDI_VISUAL: &str = "visual";

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub log_level: String,
    pub theme: String,

    /// Who emits a right-to-left row: `terminal` or `visual`.
    ///
    /// `terminal` — the default — emits the row in logical order and leaves the
    /// reordering to whatever is drawing it, which is what a terminal that
    /// shapes Arabic or Hebrew needs. `visual` applies the permutation here
    /// instead, for a terminal that does neither (xterm, alacritty); it is a
    /// scramble on a shaping one. There is no detection, so this is a setting
    /// and not a fact this program can go and read off the far end.
    ///
    /// **Per machine, not per terminal:** an ssh hop keeps the value it was
    /// launched with, and `tmux` multiplexes one value over every pane. Whether
    /// the right value followed you is the reader's to know.
    pub bidi: String,

    /// Application identifier, from <https://my.telegram.org>.
    ///
    /// Identifies the *application*, not the account: every user of a build
    /// logs in under the same pair.
    pub api_id: Option<i32>,

    /// Application hash, matching [`Config::api_id`].
    pub api_hash: Option<String>,

    /// Phone number of the account, in international format (`+15551234567`).
    ///
    /// A **pre-fill** for the sign-in flow's phone row, not where the number
    /// comes from: the reader types or corrects it on the screen, and that is
    /// what is sent. It exists in the configuration so that a launch on a machine
    /// that is only ever one account does not ask again.
    pub phone: Option<String>,

    /// The login code Telegram delivered for [`Config::phone`].
    ///
    /// A **pre-fill** for the sign-in flow's code row, and Telegram expires the
    /// code — so this is only useful for a sign-in about to happen, and never
    /// read once the session is stored.
    pub code: Option<String>,

    /// The two-factor password, for an account that has one enabled.
    ///
    /// A **pre-fill** for the sign-in flow's password row, like the code: the
    /// flow is where it is asked for, and this only saves a reader typing it on a
    /// machine that is only ever one account.
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
            bidi: BIDI_TERMINAL.to_owned(),
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
            .set_default("theme", "default")?
            .set_default("bidi", BIDI_TERMINAL)?;

        if path.exists() {
            builder = builder.add_source(config::File::from(path));
        }

        // `TELEGRAM_*` first, `TELEVIM_*` second: the same key names from two
        // prefixes, merged in order, so the workspace's own prefix still wins.
        builder = builder
            .add_source(config::Environment::with_prefix("TELEGRAM"))
            .add_source(config::Environment::with_prefix("TELEVIM"));

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

    /// Who emits a right-to-left row, from [`Config::bidi`].
    ///
    /// Only the exact word `visual` asks for the permutation to be applied here.
    /// Everything else — including a spelling this program does not know — is
    /// [`BidiMode::Terminal`], because a value it cannot read that guessed
    /// `Visual` would scramble a run on precisely the terminals whose shaper is
    /// the reason `Terminal` is the default. The comparison is case-sensitive:
    /// the value is a word from the documentation, not a path, and `config`
    /// lowercases the *key* without touching the value.
    #[must_use]
    pub fn bidi_mode(&self) -> BidiMode {
        if self.bidi == BIDI_VISUAL {
            BidiMode::Visual
        } else {
            BidiMode::Terminal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A configuration with nothing set but the two required-by-nobody values.
    fn bare() -> Config {
        Config::default()
    }

    /// The environment is process-wide, so the two tests that write to it take
    /// turns. Everything else in this module builds a `Config` directly and
    /// touches no variable.
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn a_configuration_with_nothing_in_it_is_still_a_configuration() {
        let cfg = bare();

        assert_eq!(cfg.log_level, "info");
        assert_eq!(cfg.credentials(), None);
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
    }

    /// The right-to-left mode is the terminal's job unless a reader says
    /// otherwise, and saying otherwise is visible in the application it is built.
    #[test]
    fn the_terminal_permutes_right_to_left_rows_unless_asked_not_to() {
        assert_eq!(
            bare().bidi_mode(),
            BidiMode::Terminal,
            "and the default configuration says so"
        );

        let mut cfg = bare();
        cfg.bidi = "visual".to_owned();

        // Asserted through the application, which is where the value lands and
        // is the only reader of it — no terminal is involved either way.
        assert_eq!(
            tui::app::App::new().with_bidi(cfg.bidi_mode()).bidi(),
            BidiMode::Visual
        );
    }

    /// A spelling the configuration does not know leaves the reordering to the
    /// terminal, which is the safe answer on the terminals that shape.
    #[test]
    fn an_unknown_spelling_of_the_bidi_key_is_the_terminal_not_visual() {
        let mut cfg = bare();
        cfg.bidi = "Visual".to_owned();
        assert_eq!(cfg.bidi_mode(), BidiMode::Terminal);

        cfg.bidi = String::new();
        assert_eq!(cfg.bidi_mode(), BidiMode::Terminal);
    }

    /// The environment is read under the prefix the rest of the workspace
    /// already uses, and a missing file is not an error.
    #[test]
    fn the_environment_supplies_what_the_file_does_not() {
        let _turn = ENV.lock().expect("the environment lock is not poisoned");

        // SAFETY: the environment is process-wide and setting it races with any
        // other thread reading it. Nothing else in this crate reads these two
        // names — the other tests in this module build a `Config` directly —
        // and they are the same names the integration tests use, so a run that
        // has them set is already exercising this path.
        unsafe {
            std::env::set_var("TELEVIM_API_ID", "4242");
            std::env::set_var("TELEVIM_API_HASH", "from-the-environment");
            std::env::set_var("TELEVIM_BIDI", "visual");
        }

        let cfg = Config::load(Path::new("a-file-that-does-not-exist.toml"))
            .expect("a missing file is not an error");

        assert_eq!(cfg.credentials(), Some((4242, "from-the-environment")));
        assert_eq!(cfg.log_level, "info", "and the defaults still apply");
        assert_eq!(
            cfg.bidi_mode(),
            BidiMode::Visual,
            "so the escape hatch is reachable from the environment alone"
        );
    }

    /// The `TELEGRAM_*` prefix is read too, and the workspace's own prefix
    /// overrides it.
    #[test]
    fn the_telegram_prefix_is_read_and_televim_still_wins() {
        let _turn = ENV.lock().expect("the environment lock is not poisoned");

        // The other test in this module leaves `TELEVIM_*` set, so clear them
        // first: this one is about the `TELEGRAM_*` prefix on its own.
        for name in ["TELEVIM_API_ID", "TELEVIM_API_HASH"] {
            // SAFETY: as below — process-wide environment, written only by the
            // two tests in this module and only while holding `ENV`.
            unsafe { std::env::remove_var(name) };
        }

        // SAFETY: as below.
        unsafe {
            std::env::set_var("TELEGRAM_API_ID", "1111");
            std::env::set_var("TELEGRAM_API_HASH", "from-the-telegram-prefix");
        }

        let cfg = Config::load(Path::new("a-file-that-does-not-exist.toml"))
            .expect("a missing file is not an error");
        assert_eq!(cfg.credentials(), Some((1111, "from-the-telegram-prefix")));

        // SAFETY: process-wide environment, written only by the two tests in
        // this module and only while holding `ENV`.
        unsafe {
            std::env::set_var("TELEVIM_API_ID", "2222");
        }

        let cfg = Config::load(Path::new("a-file-that-does-not-exist.toml"))
            .expect("a missing file is not an error");
        assert_eq!(
            cfg.credentials(),
            Some((2222, "from-the-telegram-prefix")),
            "TELEVIM_API_ID overrides TELEGRAM_API_ID while the hash still falls through"
        );

        for name in [
            "TELEVIM_API_ID",
            "TELEVIM_API_HASH",
            "TELEVIM_BIDI",
            "TELEGRAM_API_ID",
            "TELEGRAM_API_HASH",
        ] {
            // SAFETY: as above — clearing names this module's tests set.
            unsafe { std::env::remove_var(name) };
        }
    }
}
