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
    #[arg(long, default_value = "televim.toml")]
    config: std::path::PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = config::Config::load(&cli.config)?;
    runtime::run(&cfg)
}
