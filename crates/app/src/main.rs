//! `televim` binary entry point.

#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::module_name_repetitions)]

mod config;
mod draft_store;
mod history_store;
mod net;
mod runtime;

use anyhow::Result;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "televim", version, about = "A Vim-style Telegram client")]
struct Cli {
    /// Path to a TOML config file. Missing file -> defaults.
    ///
    /// The log goes beside it, under the same name with a `.log` extension.
    #[arg(long, default_value = "televim.toml")]
    config: std::path::PathBuf,

    /// Open this chat id on launch. Carried into launch state; STAGE-02 selects it.
    #[arg(long)]
    chat: Option<i64>,
}

fn main() -> Result<()> {
    // Before the configuration is read, so a measured launch includes reading
    // it. Off unless `TELEVIM_MEASURE` is set; see `runtime::note_launch`.
    runtime::note_launch();

    let cli = Cli::parse();

    // `.env` is a convenience, not a requirement: a run with no such file is a
    // run that takes its configuration from the file, the environment, or the
    // defaults, and that is legitimate.
    let _ = dotenvy::dotenv();

    let cfg = config::Config::load(&cli.config)?;

    runtime::run(&cfg, &cli.config, cli.chat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    #[test]
    fn chat_flag_parses_to_some_id() {
        let cli =
            Cli::try_parse_from(["televim", "--chat", "123"]).expect("a valid --chat id parses");
        assert_eq!(cli.chat, Some(123));
    }

    #[test]
    fn omitting_chat_yields_none() {
        let cli = Cli::try_parse_from(["televim"]).expect("no flag is the ordinary launch");
        assert_eq!(cli.chat, None);
    }

    #[test]
    fn help_names_the_chat_flag() {
        let mut help = Vec::new();
        Cli::command().write_help(&mut help).expect("help renders");
        let help = String::from_utf8(help).expect("help is text");
        assert!(help.contains("--chat"), "help shows the flag:\n{help}");
    }
}
