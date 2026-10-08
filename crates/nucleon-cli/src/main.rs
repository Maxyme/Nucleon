mod commands;

use anyhow::Result;
use clap::{Parser, Subcommand};
use commands::{
    d7vk::D7vkAction, gptk::GptkAction, guard::GuardAction, kosmickrisp::KosmickrispAction,
    setup::SetupArgs, steam::SteamAction, vkd3d::Vkd3dAction, wine::WineAction,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "nucleon")]
#[command(
    about = "Nucleon: High-performance standalone Windows game translation on macOS using Wine & Apple GPTK 4"
)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Perform end-to-end setup of Nucleon, Wine runtime, Apple GPTK 4, and Steam compatibility tool
    Setup {
        /// Force re-assembly of runner and reinstall of components
        #[arg(short, long)]
        force: bool,
        /// Enable or stage KosmicKrisp (Mesa Vulkan 1.4) compatibility tool in Steam
        #[arg(short, long)]
        kosmickrisp: bool,
        /// Point to custom KosmicKrisp ICD manifest, driver dylib, or directory
        #[arg(long)]
        kosmickrisp_path: Option<PathBuf>,
        /// Optional Vulkan API version override for KosmicKrisp (e.g. 1.4.0)
        #[arg(long)]
        kosmickrisp_version: Option<String>,
        /// Automatically fetch and stage VKD3D-Proton (Direct3D 12 -> Vulkan) from GitHub
        #[arg(long)]
        fetch_vkd3d: bool,
        /// Point to custom VKD3D-Proton installation directory
        #[arg(long)]
        vkd3d_path: Option<PathBuf>,
        /// Automatically fetch and stage D7VK (DirectDraw / Direct3D 1-7 -> Vulkan) from GitHub
        #[arg(long)]
        fetch_d7vk: bool,
        /// Point to custom D7VK installation directory
        #[arg(long)]
        d7vk_path: Option<PathBuf>,
        /// Select desired Wine runtime flavor or path for Steam (e.g. staging, crossover)
        #[arg(long)]
        wine: Option<String>,
        /// Point to custom Wine installation directory
        #[arg(long)]
        wine_path: Option<PathBuf>,
        /// Point to custom Apple GPTK directory (containing D3DMetal.framework and libd3dshared.dylib)
        #[arg(long)]
        gptk_path: Option<PathBuf>,
        /// Point to extracted Valve client bridge directory (or set NUCLEON_BRIDGE_PATH)
        #[arg(long)]
        bridge_path: Option<PathBuf>,
    },
    /// Inspect status of Nucleon, Steam patches, runner, and prefix
    Status,
    /// Background Steam update guard and LaunchAgent management
    Guard {
        #[command(subcommand)]
        action: GuardAction,
    },
    /// Manage Wine runtimes (Heroic, Homebrew, Whisky, CrossOver, and custom)
    Wine {
        #[command(subcommand)]
        action: WineAction,
    },
    /// Manage Apple Game Porting Toolkit (GPTK) components and custom paths
    Gptk {
        #[command(subcommand)]
        action: GptkAction,
    },
    /// Manage Mesa KosmicKrisp Vulkan driver and custom paths
    Kosmickrisp {
        #[command(subcommand)]
        action: KosmickrispAction,
    },
    /// Manage VKD3D-Proton (Direct3D 12 -> Vulkan 1.4) translation layer
    Vkd3d {
        #[command(subcommand)]
        action: Vkd3dAction,
    },
    /// Manage D7VK (DirectDraw / Direct3D 1-7 -> Vulkan 1.4) translation layer
    D7vk {
        #[command(subcommand)]
        action: D7vkAction,
    },
    /// Inspect Graphics API dependencies and recommended engine for an executable or game folder
    Detect {
        /// Path to Windows .exe binary or game folder
        path: PathBuf,
    },
    /// Launch a Windows game by Steam AppID
    Launch {
        /// Steam Application ID
        appid: u32,
        /// Enable Apple Metal Performance HUD
        #[arg(long)]
        hud: bool,
        /// Force specific engine (gptk, kosmickrisp, or staging)
        #[arg(short, long)]
        engine: Option<String>,
    },
    /// Validate that a game window is actively displaying and presenting frames on macOS (0 screen capture)
    Validate {
        /// Steam Application ID
        appid: u32,
    },
    /// Steam patch and UI integration management
    Steam {
        #[command(subcommand)]
        action: SteamAction,
    },
    /// Remove Nucleon from Steam UI, compatibility tools, and game mappings
    Unregister,
    /// View Nucleon hook and runner logs
    Logs {
        /// Number of lines to display (default: 50)
        #[arg(short = 'n', long, default_value_t = 50)]
        lines: usize,
        /// Show only hook logs
        #[arg(long)]
        hook: bool,
        /// Show only runner logs
        #[arg(long)]
        runner: bool,
        /// Follow log output continuously
        #[arg(short, long)]
        follow: bool,
    },
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    match cli.command {
        Commands::Setup {
            force,
            kosmickrisp,
            kosmickrisp_path,
            kosmickrisp_version,
            fetch_vkd3d,
            vkd3d_path,
            fetch_d7vk,
            d7vk_path,
            wine,
            wine_path,
            gptk_path,
            bridge_path,
        } => commands::setup::run(SetupArgs {
            force,
            kosmickrisp,
            kosmickrisp_path,
            kosmickrisp_version,
            fetch_vkd3d,
            vkd3d_path,
            fetch_d7vk,
            d7vk_path,
            wine,
            wine_path,
            gptk_path,
            bridge_path,
        }),
        Commands::Status => commands::status::run(),
        Commands::Guard { action } => commands::guard::run(action),
        Commands::Wine { action } => commands::wine::run(action),
        Commands::Gptk { action } => commands::gptk::run(action),
        Commands::Kosmickrisp { action } => commands::kosmickrisp::handle(action),
        Commands::Vkd3d { action } => commands::vkd3d::run(action),
        Commands::D7vk { action } => commands::d7vk::run(action),
        Commands::Detect { path } => commands::detect::run(&path),
        Commands::Launch { appid, hud, engine } => commands::launch::run_launch(appid, hud, engine),
        Commands::Validate { appid } => commands::launch::run_validate(appid),
        Commands::Steam { action } => commands::steam::run(action),
        Commands::Unregister => commands::steam::run_unregister(),
        Commands::Logs {
            lines,
            hook,
            runner,
            follow,
        } => commands::logs::run(lines, hook, runner, follow),
    }
}
