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
//!
//! # Built-in credentials
//!
//! A release build carries the application credentials it was built with, read
//! from `TELEVIM_API_ID` and `TELEVIM_API_HASH` at compile time. They are the
//! lowest layer: a file or environment setting overrides them, so a user can
//! sign in as their own application. A build with neither has none, and says so.
//!
//! # The key to that file
//!
//! The file holds the authorisation key encrypted. The key to it is, in order:
//! [`Config::session_passphrase`] (`TELEVIM_SESSION_PASSPHRASE`), else a random
//! key kept in the operating system's credential store. With neither the session
//! is not read or written at all, and the screen says which key is missing —
//! the file is never left in plaintext to get around it. Prefer the environment
//! variable to the configuration file: a file is a second place the secret rests.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;
use tui::bidi::BidiMode;
use tui::state::ui::{GraphicsMode, StickerMode};

/// A setting that must not reach a log or a panic message.
///
/// `Debug` is redacted, so `{cfg:?}` and a derived `Debug` on anything holding a
/// [`Config`] cannot print it. The text is only reachable through
/// [`Secret::expose`].
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// The secret itself. Only for handing to the thing that uses it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<String> for Secret {
    fn from(text: String) -> Self {
        Self(text)
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Who arranges a right-to-left row: the terminal, or this program.
///
/// The two the terminal itself will not do for us, named as the configuration
/// spells them.
const BIDI_TERMINAL: &str = "terminal";
const BIDI_VISUAL: &str = "visual";

/// What a sticker message draws: its picture, or its token.
///
/// The two the `stickers` key knows, named as the configuration spells them.
const STICKERS_INLINE: &str = "inline";
const STICKERS_OFF: &str = "off";

/// How a sticker picture reaches the terminal, named as the `graphics` key
/// spells it. `auto` decides from the environment once, at launch.
const GRAPHICS_AUTO: &str = "auto";
const GRAPHICS_KITTY: &str = "kitty";

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

    /// Whether a decoded sticker draws its picture: `inline` or `off`.
    ///
    /// `inline` — the default — draws the bounded block where bytes are
    /// cached, and `[sticker]` while they are missing. `off` draws `[sticker]`
    /// for every sticker message and fetches nothing: no decode, no download
    /// traffic.
    pub stickers: String,

    /// How a sticker picture reaches the terminal: `auto`, `kitty`, or `off`.
    ///
    /// `kitty` places the picture with the kitty graphics protocol, which
    /// the kitty, ghostty and wezterm terminals speak. `off` paints it as
    /// half-block cells on every terminal. `auto` — the default — is `kitty`
    /// where the environment names one of those terminals, and `off`
    /// everywhere else. The environment is read once, at launch, and never
    /// per frame: see [`Config::graphics_mode`].
    ///
    /// **Per machine, not per terminal**, like [`Config::bidi`]: an ssh hop
    /// keeps the value it was launched with.
    pub graphics: String,

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

    /// The passphrase the session file is encrypted under.
    ///
    /// Only read when [`Config::session_path`] is set. Absent or blank, the file
    /// key comes from the OS credential store instead. Prefer
    /// `TELEVIM_SESSION_PASSPHRASE`: this program never writes the configuration
    /// file, but a value put in one rests there in the clear.
    pub session_passphrase: Option<Secret>,

    /// The directory downloaded media is kept in, so a re-open is instant.
    ///
    /// Unset, the cache sits beside the configuration file, as the history and
    /// drafts files do. Files in it are plaintext, readable by the account that
    /// runs this program, and the directory is emptied when the account changes.
    pub media_cache_dir: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            log_level: "info".to_owned(),
            theme: "default".to_owned(),
            bidi: BIDI_TERMINAL.to_owned(),
            stickers: STICKERS_INLINE.to_owned(),
            graphics: GRAPHICS_AUTO.to_owned(),
            api_id: None,
            api_hash: None,
            phone: None,
            code: None,
            password: None,
            session_path: None,
            session_passphrase: None,
            media_cache_dir: None,
        }
    }
}

/// Layers the credentials a build compiled in under every other source.
///
/// Both or neither: a half pair cannot identify an application, so a lone
/// value is dropped here rather than layered, the same rule
/// [`Config::credentials`] applies to the merged result.
fn with_compiled_credentials(
    builder: config::ConfigBuilder<config::builder::DefaultState>,
    api_id: Option<&str>,
    api_hash: Option<&str>,
) -> Result<config::ConfigBuilder<config::builder::DefaultState>> {
    match (api_id, api_hash) {
        (Some(id), Some(hash)) => Ok(builder
            .set_default("api_id", id)?
            .set_default("api_hash", hash)?),
        _ => Ok(builder),
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let mut builder = config::Config::builder()
            .set_default("log_level", "info")?
            .set_default("theme", "default")?
            .set_default("bidi", BIDI_TERMINAL)?
            .set_default("stickers", STICKERS_INLINE)?
            .set_default("graphics", GRAPHICS_AUTO)?;

        builder = with_compiled_credentials(
            builder,
            option_env!("TELEVIM_API_ID"),
            option_env!("TELEVIM_API_HASH"),
        )?;

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

    /// The session passphrase, if one was given and is not blank.
    ///
    /// A blank value is no passphrase: an empty `TELEVIM_SESSION_PASSPHRASE=` is
    /// what an unset variable in a template looks like, and deriving a key from
    /// it would encrypt the session under nothing.
    #[must_use]
    pub fn passphrase(&self) -> Option<&str> {
        self.session_passphrase
            .as_ref()
            .map(Secret::expose)
            .filter(|passphrase| !passphrase.trim().is_empty())
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

    /// Whether a decoded sticker draws its picture, from [`Config::stickers`].
    ///
    /// Only the exact word `off` asks for the token everywhere. Everything
    /// else — including a spelling this program does not know — is
    /// [`StickerMode::Inline`], because a value it cannot read that guessed
    /// `off` would take the pictures away from precisely the reader who never
    /// asked for that. The comparison is case-sensitive, like
    /// [`Config::bidi_mode`]: the value is a word from the documentation.
    #[must_use]
    pub fn sticker_mode(&self) -> StickerMode {
        if self.stickers == STICKERS_OFF {
            StickerMode::Token
        } else {
            StickerMode::Inline
        }
    }

    /// How a sticker picture reaches the terminal, from [`Config::graphics`].
    ///
    /// Only `kitty` names a mode outright; `off` and anything unknown are
    /// half-blocks. `auto` consults the
    /// environment through `lookup`, and anything else is
    /// [`GraphicsMode::Halfblocks`]: the same rule as [`Config::sticker_mode`],
    /// since a value this program cannot read must not switch a protocol on.
    /// Called once, where the application is built, and never per frame.
    #[must_use]
    pub fn graphics_mode(&self, lookup: impl Fn(&str) -> Option<String>) -> GraphicsMode {
        match self.graphics.as_str() {
            GRAPHICS_KITTY => GraphicsMode::Kitty,
            GRAPHICS_AUTO if kitty_terminal(lookup) => GraphicsMode::Kitty,
            _ => GraphicsMode::Halfblocks,
        }
    }
}

/// Whether the environment names a terminal that speaks the kitty graphics
/// protocol.
///
/// `KITTY_WINDOW_ID` is set by kitty in every window it hosts, `TERM` is
/// `xterm-kitty` there, and `TERM_PROGRAM` names ghostty and wezterm. A
/// multiplexer hides these, so inside `tmux` the answer is `off` unless the
/// configuration says `kitty`.
fn kitty_terminal(lookup: impl Fn(&str) -> Option<String>) -> bool {
    lookup("KITTY_WINDOW_ID").is_some()
        || lookup("TERM").as_deref() == Some("xterm-kitty")
        || matches!(
            lookup("TERM_PROGRAM").as_deref(),
            Some("ghostty" | "WezTerm")
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    /// The default asks the environment, and a bare environment is half-blocks.
    #[test]
    fn graphics_is_auto_and_half_blocks_where_nothing_is_named() {
        assert_eq!(bare().graphics, "auto");
        assert_eq!(bare().graphics_mode(env(&[])), GraphicsMode::Halfblocks);
    }

    #[test]
    fn auto_chooses_kitty_where_the_environment_names_one() {
        let cfg = bare();
        assert_eq!(
            cfg.graphics_mode(env(&[("KITTY_WINDOW_ID", "3")])),
            GraphicsMode::Kitty
        );
        assert_eq!(
            cfg.graphics_mode(env(&[("TERM", "xterm-kitty")])),
            GraphicsMode::Kitty
        );
        assert_eq!(
            cfg.graphics_mode(env(&[("TERM_PROGRAM", "ghostty")])),
            GraphicsMode::Kitty
        );
        assert_eq!(
            cfg.graphics_mode(env(&[("TERM_PROGRAM", "WezTerm")])),
            GraphicsMode::Kitty
        );
        assert_eq!(
            cfg.graphics_mode(env(&[("TERM", "xterm-256color")])),
            GraphicsMode::Halfblocks,
            "an ordinary terminal is not guessed kitty"
        );
    }

    /// An explicit `kitty` is honoured without the environment; `off` wins over
    /// a kitty environment.
    #[test]
    fn an_explicit_value_overrides_the_environment() {
        let mut cfg = bare();
        cfg.graphics = "kitty".to_owned();
        assert_eq!(cfg.graphics_mode(env(&[])), GraphicsMode::Kitty);

        cfg.graphics = "off".to_owned();
        assert_eq!(
            cfg.graphics_mode(env(&[("KITTY_WINDOW_ID", "3")])),
            GraphicsMode::Halfblocks
        );
    }

    /// The same rule as `stickers`: a spelling this program does not know must
    /// not switch a protocol on, even in a kitty terminal.
    #[test]
    fn an_unknown_graphics_spelling_is_half_blocks() {
        let mut cfg = bare();
        cfg.graphics = "Kitty".to_owned();
        assert_eq!(
            cfg.graphics_mode(env(&[("KITTY_WINDOW_ID", "3")])),
            GraphicsMode::Halfblocks
        );
    }

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
        assert_eq!(
            cfg.media_cache_dir, None,
            "beside the config file is the default"
        );
    }

    /// The cache directory is read from the file under its own key.
    #[test]
    fn the_media_cache_dir_is_read_from_the_file() {
        // The environment wins over the file, and the env test sets the same
        // key under `ENV`, so this one takes its turn too.
        let _turn = ENV.lock().expect("the environment lock is not poisoned");

        let path =
            std::env::temp_dir().join(format!("televim-cfg-media-{}.toml", std::process::id()));
        std::fs::write(&path, "media_cache_dir = \"/var/tmp/televim-media\"\n")
            .expect("a temp file can be written");

        let cfg = Config::load(&path).expect("a file with the key loads");
        std::fs::remove_file(&path).expect("the temp file can be removed");

        assert_eq!(
            cfg.media_cache_dir,
            Some(PathBuf::from("/var/tmp/televim-media"))
        );
    }

    /// `TELEVIM_MEDIA_CACHE_DIR` sets the directory with no file at all.
    #[test]
    fn the_environment_supplies_the_media_cache_dir() {
        let _turn = ENV.lock().expect("the environment lock is not poisoned");

        // SAFETY: process-wide environment, written only by the tests in this
        // module and only while holding `ENV`.
        unsafe {
            std::env::set_var("TELEVIM_MEDIA_CACHE_DIR", "/var/tmp/from-env");
        }

        let cfg = Config::load(Path::new("a-file-that-does-not-exist.toml"))
            .expect("a missing file is not an error");

        // SAFETY: as above — clearing a name this module's tests set.
        unsafe { std::env::remove_var("TELEVIM_MEDIA_CACHE_DIR") };

        assert_eq!(
            cfg.media_cache_dir,
            Some(PathBuf::from("/var/tmp/from-env"))
        );
    }

    /// The passphrase is read from the environment like the rest, wins over the
    /// file's, is never printed, and a blank one is no passphrase.
    #[test]
    fn the_session_passphrase_comes_from_the_environment_and_is_never_printed() {
        let _turn = ENV.lock().expect("the environment lock is not poisoned");

        let dir = tempfile::tempdir().expect("a scratch directory");
        let file = dir.path().join("televim.toml");
        std::fs::write(&file, "session_passphrase = \"from-the-file\"\n")
            .expect("the configuration is written");

        // SAFETY: process-wide environment, written only by the tests in this
        // module and only while holding `ENV`.
        unsafe { std::env::remove_var("TELEVIM_SESSION_PASSPHRASE") };
        let cfg = Config::load(&file).expect("the file loads");
        assert_eq!(cfg.passphrase(), Some("from-the-file"));

        // SAFETY: as above.
        unsafe { std::env::set_var("TELEVIM_SESSION_PASSPHRASE", "s3cret-pass") };
        let cfg = Config::load(&file).expect("the file loads");
        assert_eq!(cfg.passphrase(), Some("s3cret-pass"), "TELEVIM_* wins");

        let printed = format!("{cfg:?}");
        assert!(
            !printed.contains("s3cret-pass"),
            "Debug redacts it: {printed}"
        );

        // SAFETY: as above.
        unsafe { std::env::set_var("TELEVIM_SESSION_PASSPHRASE", "  ") };
        let cfg = Config::load(&file).expect("the file loads");
        assert_eq!(cfg.passphrase(), None, "a blank value is no passphrase");

        // SAFETY: as above — clearing a name this test set.
        unsafe { std::env::remove_var("TELEVIM_SESSION_PASSPHRASE") };
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

    /// Stickers draw inline unless the reader says `off`, and saying `off` is
    /// visible in the mode the configuration names.
    #[test]
    fn stickers_are_inline_unless_turned_off() {
        assert_eq!(
            bare().sticker_mode(),
            StickerMode::Inline,
            "and the default configuration says so"
        );

        let mut cfg = bare();
        cfg.stickers = "off".to_owned();
        assert_eq!(cfg.sticker_mode(), StickerMode::Token);
    }

    /// A spelling the configuration does not know keeps the pictures: taking
    /// them away on an unreadable value would punish precisely the reader who
    /// never asked for that.
    #[test]
    fn an_unknown_spelling_of_the_stickers_key_is_inline_not_off() {
        let mut cfg = bare();
        cfg.stickers = "Off".to_owned();
        assert_eq!(cfg.sticker_mode(), StickerMode::Inline);

        cfg.stickers = String::new();
        assert_eq!(cfg.sticker_mode(), StickerMode::Inline);
    }

    /// The flag is reachable from the environment under the workspace prefix,
    /// like the rest of the configuration.
    #[test]
    fn the_environment_supplies_the_stickers_key_too() {
        let _turn = ENV.lock().expect("the environment lock is not poisoned");

        // SAFETY: as above — process-wide environment, written only by the
        // tests in this module and only while holding `ENV`.
        unsafe {
            std::env::set_var("TELEVIM_STICKERS", "off");
        }

        let cfg = Config::load(Path::new("a-file-that-does-not-exist.toml"))
            .expect("a missing file is not an error");
        assert_eq!(
            cfg.sticker_mode(),
            StickerMode::Token,
            "so the escape hatch is reachable from the environment alone"
        );

        // SAFETY: as above — clearing a name this module's tests set.
        unsafe { std::env::remove_var("TELEVIM_STICKERS") };
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

    /// Built-in credentials are the lowest layer, and a half pair is no pair.
    /// Set through the builder directly so the test does not depend on what
    /// this build was compiled with.
    #[test]
    fn built_in_credentials_are_overridden_and_need_a_full_pair() {
        let load = |builder: config::ConfigBuilder<config::builder::DefaultState>, file: &str| {
            builder
                .add_source(config::File::from_str(file, config::FileFormat::Toml))
                .build()
                .expect("a literal source builds")
                .try_deserialize::<Config>()
                .expect("a literal source deserialises")
        };
        let base = || config::Config::builder();

        let built_in = with_compiled_credentials(base(), Some("77"), Some("built-in"))
            .expect("defaults layer");
        assert_eq!(
            load(built_in, "").credentials(),
            Some((77, "built-in")),
            "with nothing else set, the built-in pair is used"
        );

        let built_in = with_compiled_credentials(base(), Some("77"), Some("built-in"))
            .expect("defaults layer");
        assert_eq!(
            load(built_in, "api_hash = \"from-the-file\"").credentials(),
            Some((77, "from-the-file")),
            "a file setting overrides the built-in value it names"
        );

        let half = with_compiled_credentials(base(), Some("77"), None).expect("defaults layer");
        assert_eq!(
            load(half, "").credentials(),
            None,
            "a lone built-in id is dropped, so there is no pair"
        );
    }
}
