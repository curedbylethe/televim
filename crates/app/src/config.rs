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
//! The authorisation key is stored in an encrypted session file. Its file key
//! lives beside the session unless a passphrase or explicit key file is set.
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
//! key file beside the session. A legacy keychain key is copied there before
//! the session opens. Prefer the environment variable to the configuration
//! file: a file is a second place the secret rests.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;
use telegram_framework::KdfParams;
use tui::bidi::BidiMode;
use tui::jumplist::DEFAULT_DEPTH;
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

impl Drop for Secret {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.0);
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

/// Who arranges a right-to-left row: `auto`, `terminal`, `visual` or `words`.
/// `auto` is the default; it chooses `words` only when the launch environment
/// names kitty. Explicit values win, and the choice stays fixed for the launch.
///
/// The values, named as the configuration spells them.
const BIDI_AUTO: &str = "auto";
const BIDI_VISUAL: &str = "visual";
const BIDI_WORDS: &str = "words";

/// What a sticker message draws: its picture, or its token.
///
/// The two the `stickers` key knows, named as the configuration spells them.
const STICKERS_INLINE: &str = "inline";
const STICKERS_OFF: &str = "off";

/// How a sticker picture reaches the terminal, named as the `graphics` key
/// spells it. `auto` decides from the environment once, at launch.
const GRAPHICS_AUTO: &str = "auto";
const GRAPHICS_KITTY: &str = "kitty";

/// Bounds on the KDF keys: wide enough to tune, tight enough that a typo is
/// neither trivial nor unbounded.
const KDF_MEMORY_KIB_RANGE: std::ops::RangeInclusive<u32> = 8_192..=524_288;
const KDF_PASSES_RANGE: std::ops::RangeInclusive<u32> = 1..=8;
const KDF_LANES_RANGE: std::ops::RangeInclusive<u32> = 1..=4;

/// Bounds on the jumplist depth: wide enough to tune, tight enough that a typo
/// is neither useless nor unbounded.
const JUMPLIST_DEPTH_RANGE: std::ops::RangeInclusive<usize> = 1..=1_000;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub log_level: String,
    pub theme: String,

    /// Who emits a right-to-left row: `auto`, `terminal`, `visual` or `words`.
    ///
    /// `auto` — the default — chooses `words` when `KITTY_WINDOW_ID` is set or
    /// `TERM=xterm-kitty`; otherwise it chooses `terminal`. Explicit values win.
    /// `terminal` emits the row in logical order for a terminal that shapes it.
    /// `visual` permutes here for a terminal that does neither (xterm, alacritty).
    /// `words` reverses the words, each whole, for kitty's per-word reversal.
    ///
    /// The default is resolved once from the launch environment. An explicit
    /// value stays per machine, so an ssh hop keeps it. `tmux` multiplexes one
    /// value over every pane.
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

    /// Whether desktop notifications are enabled: `on` (default) or `off`.
    pub notifications: String,

    /// Whether to check for stable releases: `auto` or `off`.
    ///
    /// The exact word `off` disables checks; unknown values keep the default.
    pub updates: Option<String>,

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

    /// A file holding the 32-byte key the session file is encrypted under.
    ///
    /// Used when [`Config::session_path`] is set; otherwise only by the history
    /// cache. Do not combine it with [`Config::session_passphrase`]: one file
    /// has one key. The app creates the key file with mode `0600` on first use
    /// and never overwrites it. A relative path resolves against the
    /// configuration file's directory. `TELEVIM_SESSION_KEY_FILE` sets it.
    pub session_key_file: Option<PathBuf>,

    #[serde(skip)]
    pub(crate) session_key_file_derived: bool,

    /// Argon2id memory cost of the passphrase-derived file key, in KiB.
    ///
    /// Unset, 19 MiB. Allowed 8192 to 524288. The envelope does not record it:
    /// changing it makes a file sealed under the old value unopenable (the
    /// same refusal as a wrong passphrase), so it is a seal-time choice.
    pub session_kdf_memory_kib: Option<u32>,

    /// Argon2id passes of the passphrase-derived file key. Unset, 2; allowed 1 to 8.
    pub session_kdf_passes: Option<u32>,

    /// Argon2id lanes of the passphrase-derived file key. Unset, 1; allowed 1 to 4.
    pub session_kdf_lanes: Option<u32>,

    /// The directory downloaded media is kept in, so a re-open is instant.
    ///
    /// Unset, the cache sits beside the configuration file, as the history and
    /// drafts files do. Files in it are plaintext, readable by the account that
    /// runs this program, and the directory is emptied when the account changes.
    pub media_cache_dir: Option<PathBuf>,

    /// The most bytes the media cache keeps on disk.
    ///
    /// Unset, the cap is 1 GiB. The cache also keeps at most 256 files, whatever
    /// their size; this is the byte half of that bound.
    pub media_cache_max_bytes: Option<u64>,

    /// How many back/forward marks each conversation's jumplist keeps.
    ///
    /// Unset, the default is [`DEFAULT_DEPTH`] (100). Allowed 1 to 1000. The
    /// cap applies to **both** stacks of every conversation, and is fixed for
    /// the life of the run: it is handed to `tui` once, before the first frame,
    /// so it can only be changed by a relaunch.
    pub jumplist_depth: Option<usize>,

    /// The command that opens any downloaded media, when a kind-specific key
    /// below does not name one.
    ///
    /// A command line split on whitespace: the first token is the program, the
    /// rest are its arguments, and the file path is appended last. There is no
    /// shell, so quoting and expansion do not apply. Blank is unset. Unset, the
    /// platform opener (`open -W`, `xdg-open`, `cmd /C start`) is used. A
    /// configured command is run directly and waited on, so the status line
    /// reports the viewer's own exit.
    pub viewer: Option<String>,

    /// The command for photos and stickers; falls back to `viewer`.
    pub viewer_image: Option<String>,

    /// The command for videos and GIFs; falls back to `viewer`.
    pub viewer_video: Option<String>,

    /// The command for voice messages; falls back to `viewer`.
    pub viewer_audio: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            log_level: "info".to_owned(),
            theme: "default".to_owned(),
            bidi: BIDI_AUTO.to_owned(),
            stickers: STICKERS_INLINE.to_owned(),
            graphics: GRAPHICS_AUTO.to_owned(),
            notifications: "on".to_owned(),
            updates: None,
            api_id: None,
            api_hash: None,
            phone: None,
            code: None,
            password: None,
            session_path: None,
            session_passphrase: None,
            session_key_file: None,
            session_key_file_derived: false,
            session_kdf_memory_kib: None,
            session_kdf_passes: None,
            session_kdf_lanes: None,
            media_cache_dir: None,
            media_cache_max_bytes: None,
            jumplist_depth: None,
            viewer: None,
            viewer_image: None,
            viewer_video: None,
            viewer_audio: None,
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
            .set_default("bidi", BIDI_AUTO)?
            .set_default("stickers", STICKERS_INLINE)?
            .set_default("graphics", GRAPHICS_AUTO)?
            .set_default("notifications", "on")?;

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

        let mut cfg: Config = builder
            .build()
            .context("building config")?
            .try_deserialize()
            .context("deserialising config")?;
        if cfg.notifications.trim().is_empty() {
            "on".clone_into(&mut cfg.notifications);
        }
        cfg.validate_session_key()?;
        cfg.default_session_files(path);
        let parent = path.parent().unwrap_or_else(|| Path::new(""));
        for value in [&mut cfg.session_path, &mut cfg.session_key_file]
            .into_iter()
            .flatten()
        {
            if value.is_relative() {
                *value = parent.join(&*value);
            }
        }
        cfg.validate_kdf()?;
        cfg.validate_jumplist_depth()?;
        Ok(cfg)
    }

    /// Keeps the session out of the OS credential store unless asked otherwise.
    ///
    /// With no `session_path` the session is an encrypted file beside the
    /// configuration, and unless a passphrase or key file is set its key is a
    /// random 32-byte file beside that, created on first use. The credential
    /// store prompts for a password on an unsigned build, so it is no longer the
    /// default.
    fn default_session_files(&mut self, config_path: &Path) {
        if self.session_path.is_none() {
            let dir = config_path.parent().unwrap_or_else(|| Path::new(""));
            self.session_path = Some(dir.join("session"));
        }
        if self.passphrase().is_none() && self.session_key_file.is_none() {
            self.session_key_file = Some(
                self.session_path
                    .as_ref()
                    .expect("session path defaulted")
                    .with_extension("key"),
            );
            self.session_key_file_derived = true;
        }
    }

    /// Refuses a passphrase and a key file at once: one file, one key.
    fn validate_session_key(&self) -> Result<()> {
        if self.passphrase().is_some() && self.session_key_file.is_some() {
            anyhow::bail!(
                "session_passphrase and session_key_file are both set: choose one, because \
                 the session file can have only one key"
            );
        }
        Ok(())
    }

    /// Refuses a jumplist depth a typo could make useless or unbounded, naming
    /// the key.
    fn validate_jumplist_depth(&self) -> Result<()> {
        if let Some(value) = self.jumplist_depth
            && !JUMPLIST_DEPTH_RANGE.contains(&value)
        {
            anyhow::bail!(
                "jumplist_depth = {value} is out of range: it must be {} to {}",
                JUMPLIST_DEPTH_RANGE.start(),
                JUMPLIST_DEPTH_RANGE.end()
            );
        }
        Ok(())
    }

    /// Refuses a KDF cost a typo could make trivial or unbounded, naming the key.
    fn validate_kdf(&self) -> Result<()> {
        for (key, value, range) in [
            (
                "session_kdf_memory_kib",
                self.session_kdf_memory_kib,
                KDF_MEMORY_KIB_RANGE,
            ),
            (
                "session_kdf_passes",
                self.session_kdf_passes,
                KDF_PASSES_RANGE,
            ),
            ("session_kdf_lanes", self.session_kdf_lanes, KDF_LANES_RANGE),
        ] {
            if let Some(value) = value
                && !range.contains(&value)
            {
                anyhow::bail!(
                    "{key} = {value} is out of range: it must be {} to {}",
                    range.start(),
                    range.end()
                );
            }
        }
        Ok(())
    }

    /// The Argon2id cost for the session key: the keys set, the defaults for the rest.
    #[must_use]
    pub fn kdf_params(&self) -> KdfParams {
        let default = KdfParams::default();
        KdfParams {
            memory_kib: self.session_kdf_memory_kib.unwrap_or(default.memory_kib),
            passes: self.session_kdf_passes.unwrap_or(default.passes),
            lanes: self.session_kdf_lanes.unwrap_or(default.lanes),
        }
    }

    /// The number of back/forward marks each conversation's jumplist keeps.
    ///
    /// The key set, or [`DEFAULT_DEPTH`] (100) when it is not — the same
    /// default the `tui` type carries, so the two cannot drift.
    #[must_use]
    pub fn jumplist_depth(&self) -> usize {
        self.jumplist_depth.unwrap_or(DEFAULT_DEPTH)
    }

    /// Whether the background stable-release check is enabled.
    #[must_use]
    pub fn updates_enabled(&self) -> bool {
        self.updates.as_deref() != Some("off")
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
    /// Only the exact words `visual` and `words` ask for the row to be permuted here.
    /// Everything else — including a spelling this program does not know — is
    /// [`BidiMode::Terminal`], because a value it cannot read that guessed
    /// `Visual` would scramble a run on precisely the terminals whose shaper is
    /// the reason `Terminal` is the default. The comparison is case-sensitive:
    /// the value is a word from the documentation, not a path, and `config`
    /// lowercases the *key* without touching the value.
    #[must_use]
    pub fn bidi_mode(&self, lookup: impl Fn(&str) -> Option<String>) -> BidiMode {
        match self.bidi.as_str() {
            BIDI_VISUAL => BidiMode::Visual,
            BIDI_WORDS => BidiMode::Words,
            BIDI_AUTO if kitty_words_terminal(lookup) => BidiMode::Words,
            _ => BidiMode::Terminal,
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

/// Whether the environment names kitty for bidi's per-word default.
fn kitty_words_terminal(lookup: impl Fn(&str) -> Option<String>) -> bool {
    lookup("KITTY_WINDOW_ID").is_some() || lookup("TERM").as_deref() == Some("xterm-kitty")
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

    fn load_toml(name: &str, body: &str) -> Result<Config> {
        let _turn = ENV.lock().expect("the environment lock is not poisoned");
        let path =
            std::env::temp_dir().join(format!("televim-cfg-{name}-{}.toml", std::process::id()));
        std::fs::write(&path, body).expect("a temp file can be written");
        let loaded = Config::load(&path);
        std::fs::remove_file(&path).expect("the temp file can be removed");
        loaded
    }

    #[test]
    fn the_kdf_keys_are_read_and_default_to_the_shipped_cost() {
        assert_eq!(bare().kdf_params(), KdfParams::default());

        let cfg = load_toml(
            "kdf-ok",
            "session_kdf_memory_kib = 65536\nsession_kdf_passes = 3\nsession_kdf_lanes = 2\n",
        )
        .expect("in-range keys load");
        let params = cfg.kdf_params();
        assert_eq!(
            (params.memory_kib, params.passes, params.lanes),
            (65_536, 3, 2)
        );

        let cfg = load_toml("kdf-part", "session_kdf_passes = 4\n").expect("one key loads");
        let params = cfg.kdf_params();
        assert_eq!(
            (params.memory_kib, params.passes, params.lanes),
            (19 * 1024, 4, 1)
        );
    }

    #[test]
    fn a_passphrase_and_a_key_file_together_fail_the_launch_naming_both() {
        let error = load_toml(
            "both-keys",
            "session_passphrase = \"hunter2\"\nsession_key_file = \"/tmp/session.key\"\n",
        )
        .expect_err("one file has one key");

        let sentence = error.to_string();
        assert!(sentence.contains("session_passphrase"), "{sentence}");
        assert!(sentence.contains("session_key_file"), "{sentence}");
    }

    #[test]
    fn a_blank_passphrase_beside_a_key_file_is_allowed() {
        let cfg = load_toml(
            "blank-pass-key-file",
            "session_passphrase = \"  \"\nsession_key_file = \"/tmp/session.key\"\n",
        )
        .expect("a blank passphrase is absent");

        assert_eq!(
            cfg.session_key_file,
            Some(PathBuf::from("/tmp/session.key"))
        );
        assert_eq!(cfg.passphrase(), None);
    }

    #[test]
    fn the_environment_supplies_the_session_key_file() {
        let _turn = ENV.lock().expect("the environment lock is not poisoned");
        // SAFETY: process-wide environment, written only by tests in this module under ENV.
        unsafe { std::env::set_var("TELEVIM_SESSION_KEY_FILE", "/tmp/env-session.key") };
        let cfg = Config::load(Path::new("a-file-that-does-not-exist.toml"))
            .expect("a missing file is not an error");
        // SAFETY: as above — clearing the variable this test set.
        unsafe { std::env::remove_var("TELEVIM_SESSION_KEY_FILE") };

        assert_eq!(
            cfg.session_key_file,
            Some(PathBuf::from("/tmp/env-session.key"))
        );
    }

    #[test]
    fn an_out_of_range_kdf_key_fails_the_launch_naming_the_key() {
        for (key, value) in [
            ("session_kdf_memory_kib", 0),
            ("session_kdf_memory_kib", 1024),
            ("session_kdf_memory_kib", 524_289),
            ("session_kdf_passes", 0),
            ("session_kdf_passes", 9),
            ("session_kdf_lanes", 0),
            ("session_kdf_lanes", 5),
        ] {
            let error = load_toml("kdf-bad", &format!("{key} = {value}\n"))
                .expect_err("an out-of-range value is refused");
            assert!(error.to_string().contains(key), "{error} names {key}");
        }
    }

    /// With no key the depth is the `tui` type's own default, so the two cannot
    /// drift.
    #[test]
    fn the_jumplist_depth_defaults_to_a_hundred() {
        assert_eq!(bare().jumplist_depth(), DEFAULT_DEPTH);
        assert_eq!(bare().jumplist_depth(), 100);
    }

    #[test]
    fn updates_default_on_and_only_exact_off_disables_them() {
        assert!(bare().updates_enabled());
        assert!(
            load_toml("updates-unknown", "updates = \"OFF\"\n")
                .expect("unknown spelling loads")
                .updates_enabled()
        );
        assert!(
            !load_toml("updates-off", "updates = \"off\"\n")
                .expect("off loads")
                .updates_enabled()
        );
    }

    #[test]
    fn the_environment_supplies_the_updates_key() {
        let _turn = ENV.lock().expect("the environment lock is not poisoned");
        // SAFETY: process-wide environment, written only by tests in this module under ENV.
        unsafe { std::env::set_var("TELEVIM_UPDATES", "off") };
        let cfg = Config::load(Path::new("a-file-that-does-not-exist.toml"))
            .expect("a missing file is not an error");
        // SAFETY: as above — clearing the variable this test set.
        unsafe { std::env::remove_var("TELEVIM_UPDATES") };
        assert!(!cfg.updates_enabled());
    }

    /// The depth is read from the file under its own key.
    #[test]
    fn the_jumplist_depth_is_read_from_the_file() {
        let cfg =
            load_toml("jumplist-depth-ok", "jumplist_depth = 42\n").expect("an in-range key loads");

        assert_eq!(cfg.jumplist_depth, Some(42));
        assert_eq!(cfg.jumplist_depth(), 42);
    }

    /// `TELEVIM_JUMPLIST_DEPTH` sets the depth with no file at all.
    #[test]
    fn the_environment_supplies_the_jumplist_depth() {
        let _turn = ENV.lock().expect("the environment lock is not poisoned");

        // SAFETY: process-wide environment, written only by the tests in this
        // module and only while holding `ENV`.
        unsafe {
            std::env::set_var("TELEVIM_JUMPLIST_DEPTH", "250");
        }

        let cfg = Config::load(Path::new("a-file-that-does-not-exist.toml"))
            .expect("a missing file is not an error");

        // SAFETY: as above — clearing a name this module's tests set.
        unsafe { std::env::remove_var("TELEVIM_JUMPLIST_DEPTH") };

        assert_eq!(cfg.jumplist_depth(), 250);
    }

    #[test]
    fn an_out_of_range_jumplist_depth_fails_the_launch_naming_the_key() {
        for value in [0, 1_001] {
            let error = load_toml("jumplist-depth-bad", &format!("jumplist_depth = {value}\n"))
                .expect_err("an out-of-range value is refused");
            assert!(
                error.to_string().contains("jumplist_depth"),
                "{error} names jumplist_depth"
            );
        }
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
    fn the_default_session_is_a_file_with_its_own_key_file() {
        let cfg = load_toml("default-session", "").expect("loads");
        assert!(cfg.session_path.is_some(), "no credential-store session");
        assert!(cfg.session_key_file.is_some(), "no credential-store key");

        let cfg = load_toml("own-passphrase", "session_passphrase = \"x\"\n").expect("loads");
        assert!(cfg.session_path.is_some());
        assert_eq!(cfg.session_key_file, None, "a passphrase is the key");

        let cfg = load_toml("own-path", "session_path = \"/tmp/s\"\n").expect("loads");
        assert_eq!(
            cfg.session_key_file.as_deref(),
            Some(Path::new("/tmp/s.key"))
        );
        assert!(cfg.session_key_file_derived);
    }

    #[test]
    fn a_configuration_with_nothing_in_it_is_still_a_configuration() {
        let cfg = bare();

        assert_eq!(cfg.log_level, "info");
        assert_eq!(cfg.notifications, "on");
        assert_eq!(cfg.credentials(), None);
        assert_eq!(
            cfg.session_path, None,
            "`load` fills this in, not `default`"
        );
        assert_eq!(
            cfg.media_cache_dir, None,
            "beside the config file is the default"
        );
        assert_eq!(
            cfg.media_cache_max_bytes, None,
            "the cache's own 1 GiB is the default"
        );
        assert_eq!(
            cfg.jumplist_depth, None,
            "the jumplist's own depth is the default"
        );
    }

    #[test]
    fn notifications_are_read_from_the_file() {
        assert_eq!(
            load_toml("notifications-off", "notifications = \"off\"\n")
                .expect("the key loads")
                .notifications,
            "off"
        );
    }

    #[test]
    fn the_environment_supplies_notifications_and_blank_uses_the_default() {
        let _turn = ENV.lock().expect("the environment lock is not poisoned");
        // SAFETY: process-wide environment, written only by tests in this module under ENV.
        unsafe { std::env::set_var("TELEVIM_NOTIFICATIONS", "off") };
        let cfg = Config::load(Path::new("a-file-that-does-not-exist.toml"))
            .expect("a missing file is not an error");
        // SAFETY: as above — clearing the variable this test set.
        unsafe { std::env::set_var("TELEVIM_NOTIFICATIONS", "") };
        let blank = Config::load(Path::new("a-file-that-does-not-exist.toml"))
            .expect("a blank environment value is ignored");
        // SAFETY: as above — clearing the variable this test set.
        unsafe { std::env::remove_var("TELEVIM_NOTIFICATIONS") };

        assert_eq!(cfg.notifications, "off");
        assert_eq!(blank.notifications, "on");
    }

    /// Each viewer key is read from the file under its own name.
    #[test]
    fn the_viewer_keys_are_read_from_the_file() {
        let _turn = ENV.lock().expect("the environment lock is not poisoned");

        let path =
            std::env::temp_dir().join(format!("televim-cfg-viewer-{}.toml", std::process::id()));
        std::fs::write(
            &path,
            "viewer = \"a\"\nviewer_image = \"b -x\"\nviewer_video = \"c\"\nviewer_audio = \"d\"\n",
        )
        .expect("a temp file can be written");

        let cfg = Config::load(&path).expect("a file with the keys loads");
        std::fs::remove_file(&path).expect("the temp file can be removed");

        assert_eq!(cfg.viewer.as_deref(), Some("a"));
        assert_eq!(cfg.viewer_image.as_deref(), Some("b -x"));
        assert_eq!(cfg.viewer_video.as_deref(), Some("c"));
        assert_eq!(cfg.viewer_audio.as_deref(), Some("d"));
    }

    /// Each viewer key has its `TELEVIM_` variable.
    #[test]
    fn the_environment_supplies_the_viewer_keys() {
        let _turn = ENV.lock().expect("the environment lock is not poisoned");
        let names = [
            "TELEVIM_VIEWER",
            "TELEVIM_VIEWER_IMAGE",
            "TELEVIM_VIEWER_VIDEO",
            "TELEVIM_VIEWER_AUDIO",
        ];

        // SAFETY: process-wide environment, written only by the tests in this
        // module and only while holding `ENV`.
        unsafe {
            for name in names {
                std::env::set_var(name, format!("{name}-cmd"));
            }
        }

        let cfg = Config::load(Path::new("a-file-that-does-not-exist.toml"))
            .expect("a missing file is not an error");

        // SAFETY: as above — clearing names this test set.
        unsafe {
            for name in names {
                std::env::remove_var(name);
            }
        }

        assert_eq!(cfg.viewer.as_deref(), Some("TELEVIM_VIEWER-cmd"));
        assert_eq!(
            cfg.viewer_image.as_deref(),
            Some("TELEVIM_VIEWER_IMAGE-cmd")
        );
        assert_eq!(
            cfg.viewer_video.as_deref(),
            Some("TELEVIM_VIEWER_VIDEO-cmd")
        );
        assert_eq!(
            cfg.viewer_audio.as_deref(),
            Some("TELEVIM_VIEWER_AUDIO-cmd")
        );
    }

    /// The byte cap is read from the file under its own key.
    #[test]
    fn the_media_cache_max_bytes_is_read_from_the_file() {
        let _turn = ENV.lock().expect("the environment lock is not poisoned");

        let path =
            std::env::temp_dir().join(format!("televim-cfg-media-cap-{}.toml", std::process::id()));
        std::fs::write(&path, "media_cache_max_bytes = 2147483648\n")
            .expect("a temp file can be written");

        let cfg = Config::load(&path).expect("a file with the key loads");
        std::fs::remove_file(&path).expect("the temp file can be removed");

        assert_eq!(cfg.media_cache_max_bytes, Some(2_147_483_648));
    }

    /// `TELEVIM_MEDIA_CACHE_MAX_BYTES` sets the cap with no file at all.
    #[test]
    fn the_environment_supplies_the_media_cache_max_bytes() {
        let _turn = ENV.lock().expect("the environment lock is not poisoned");

        // SAFETY: process-wide environment, written only by the tests in this
        // module and only while holding `ENV`.
        unsafe {
            std::env::set_var("TELEVIM_MEDIA_CACHE_MAX_BYTES", "536870912");
        }

        let cfg = Config::load(Path::new("a-file-that-does-not-exist.toml"))
            .expect("a missing file is not an error");

        // SAFETY: as above — clearing a name this module's tests set.
        unsafe { std::env::remove_var("TELEVIM_MEDIA_CACHE_MAX_BYTES") };

        assert_eq!(cfg.media_cache_max_bytes, Some(536_870_912));
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
            bare().bidi_mode(env(&[])),
            BidiMode::Terminal,
            "auto resolves to terminal without a kitty signal"
        );

        let mut cfg = bare();
        cfg.bidi = "visual".to_owned();

        // Asserted through the application, which is where the value lands and
        // is the only reader of it — no terminal is involved either way.
        assert_eq!(
            tui::app::App::new()
                .with_bidi(cfg.bidi_mode(env(&[])))
                .bidi(),
            BidiMode::Visual
        );
    }

    /// `words` is its own mode: the words reversed, the letters left to the
    /// terminal. Only the exact word selects it, like `visual`.
    #[test]
    fn the_words_value_selects_the_word_order_mode() {
        let mut cfg = bare();
        cfg.bidi = "words".to_owned();
        assert_eq!(cfg.bidi_mode(env(&[])), BidiMode::Words);

        cfg.bidi = "Words".to_owned();
        assert_eq!(cfg.bidi_mode(env(&[])), BidiMode::Terminal);
    }

    /// A spelling the configuration does not know leaves the reordering to the
    /// terminal, which is the safe answer on the terminals that shape.
    #[test]
    fn an_unknown_spelling_of_the_bidi_key_is_the_terminal_not_visual() {
        let mut cfg = bare();
        cfg.bidi = "Visual".to_owned();
        assert_eq!(
            cfg.bidi_mode(env(&[("KITTY_WINDOW_ID", "3")])),
            BidiMode::Terminal
        );

        cfg.bidi = String::new();
        assert_eq!(cfg.bidi_mode(env(&[])), BidiMode::Terminal);
    }

    #[test]
    fn auto_orders_words_on_kitty() {
        let cfg = bare();
        assert_eq!(
            cfg.bidi_mode(env(&[("KITTY_WINDOW_ID", "3")])),
            BidiMode::Words
        );
        assert_eq!(
            cfg.bidi_mode(env(&[("TERM", "xterm-kitty")])),
            BidiMode::Words
        );
    }

    #[test]
    fn auto_leaves_other_terminals_in_terminal_order() {
        let cfg = bare();
        assert_eq!(
            cfg.bidi_mode(env(&[("TERM_PROGRAM", "ghostty")])),
            BidiMode::Terminal
        );
        assert_eq!(
            cfg.bidi_mode(env(&[("TERM_PROGRAM", "WezTerm")])),
            BidiMode::Terminal
        );
        assert_eq!(
            cfg.bidi_mode(env(&[("TERM", "xterm-256color")])),
            BidiMode::Terminal
        );
        assert_eq!(cfg.bidi_mode(env(&[])), BidiMode::Terminal);
    }

    #[test]
    fn explicit_bidi_values_win_over_kitty() {
        let kitty = env(&[("KITTY_WINDOW_ID", "3")]);
        let mut cfg = bare();
        cfg.bidi = "terminal".to_owned();
        assert_eq!(cfg.bidi_mode(kitty), BidiMode::Terminal);

        cfg.bidi = BIDI_VISUAL.to_owned();
        assert_eq!(
            cfg.bidi_mode(env(&[("KITTY_WINDOW_ID", "3")])),
            BidiMode::Visual
        );

        cfg.bidi = BIDI_WORDS.to_owned();
        assert_eq!(cfg.bidi_mode(env(&[])), BidiMode::Words);
    }

    #[test]
    fn both_bidi_default_sites_use_auto() {
        assert_eq!(Config::default().bidi, BIDI_AUTO);

        let _turn = ENV.lock().expect("the environment lock is not poisoned");
        let names = ["TELEVIM_BIDI", "TELEGRAM_BIDI"];
        let saved = names.map(|name| (name, std::env::var_os(name)));
        // SAFETY: the environment is process-wide and these test mutations are
        // serialized with the module's other environment tests.
        unsafe {
            for (name, _) in &saved {
                std::env::remove_var(name);
            }
        }

        let loaded = Config::load(Path::new("a-file-that-does-not-exist.toml"));

        // SAFETY: restore the process environment values saved above.
        unsafe {
            for (name, value) in saved {
                if let Some(value) = value {
                    std::env::set_var(name, value);
                }
            }
        }
        let loaded = loaded.expect("a missing config file is not an error");
        assert_eq!(loaded.bidi, BIDI_AUTO);
        assert_eq!(loaded.bidi_mode(env(&[])), BidiMode::Terminal);
        assert_eq!(
            loaded.bidi_mode(env(&[("KITTY_WINDOW_ID", "3")])),
            BidiMode::Words
        );
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
            cfg.bidi_mode(|key| std::env::var(key).ok()),
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
