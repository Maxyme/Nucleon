#![forbid(unsafe_code)]
mod common;
mod diagnostic;
mod steam_probe;
use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::{num::NonZeroU64, path::PathBuf, sync::atomic::Ordering};
#[derive(Parser)]
#[command(
    about = "Nucleon integration diagnostics; runtime builds live in ../isotope",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    Launch(DiagnosticArgs),
    ProbeSteam(steam_probe::Args),
}
#[derive(Args)]
pub struct DiagnosticArgs {
    #[arg(value_parser=["dirt","dmc5"])]
    game: String,
    #[arg(long, default_value = "target/arm64-fex")]
    work: PathBuf,
    #[arg(long)]
    runtime: Option<PathBuf>,
    #[arg(long)]
    game_dir: Option<PathBuf>,
    #[arg(long)]
    bridge: Option<PathBuf>,
    #[arg(long)]
    cli: Option<PathBuf>,
    #[arg(long,default_value="dx12",value_parser=["dx11","dx12"])]
    api: String,
    #[arg(long)]
    timeout: Option<NonZeroU64>,
    #[arg(long)]
    debug: bool,
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();
    ctrlc::set_handler(|| common::CANCELLED.store(true, Ordering::SeqCst))?;
    match cli.command {
        Commands::Launch(args) => diagnostic::game(&args),
        Commands::ProbeSteam(args) => steam_probe::probe(&args),
    }
}
