//! `televim` binary entry point.

#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::module_name_repetitions)]

mod config;
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // `.env` is a convenience, not a requirement: a run with no such file is a
    // run that takes its configuration from the file, the environment, or the
    // defaults, and that is legitimate.
    let _ = dotenvy::dotenv();

    let cfg = config::Config::load(&cli.config)?;

    runtime::run(&cfg, &cli.config)
}
