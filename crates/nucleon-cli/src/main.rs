mod commands;

use anyhow::Result;
use clap::{Parser, Subcommand};
use commands::{
    backends::BackendsAction, d7vk::D7vkAction, dxmt::DxmtAction, dxvk::DxvkAction,
    gptk::GptkAction, guard::GuardAction, kosmickrisp::KosmickrispAction, setup::SetupArgs,
    steam::SteamAction, vkd3d::Vkd3dAction, wine::WineAction,
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
    /// Perform idempotent setup check or remediate missing prerequisites ('nucleon setup fix')
    Setup {
        #[command(subcommand)]
        action: Option<commands::setup::SetupAction>,
        /// Run setup non-interactively with defaults or provided flags (skips the interactive prompt)
        #[arg(short = 'y', long)]
        non_interactive: bool,
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
        /// Automatically fetch and stage DXVK (Direct3D 9/10/11 -> Vulkan) from GitHub
        #[arg(long)]
        fetch_dxvk: bool,
        /// Point to custom DXVK installation directory
        #[arg(long)]
        dxvk_path: Option<PathBuf>,
        /// Automatically fetch and stage DXMT (Direct3D 11 -> Apple Metal) from GitHub
        #[arg(long)]
        fetch_dxmt: bool,
        /// Point to custom DXMT installation directory
        #[arg(long)]
        dxmt_path: Option<PathBuf>,
        /// Select desired Wine runtime flavor or path for Steam (e.g. staging, crossover, or /Applications/CrossOver.app)
        #[arg(long)]
        wine: Option<String>,
        /// Point to custom Wine installation directory or app bundle (e.g. /Applications/CrossOver.app)
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
    /// Manage Wine graphical translation backends (KosmicKrisp, VKD3D-Proton, D7VK, DXVK, DXMT)
    #[command(alias = "backend")]
    Backends {
        #[command(subcommand)]
        action: BackendsAction,
    },
    /// Manage Apple Game Porting Toolkit 4 (runner & D3DMetal backend)
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
    /// Manage DXVK (Direct3D 9/10/11 -> Vulkan 1.4) translation layer
    Dxvk {
        #[command(subcommand)]
        action: DxvkAction,
    },
    /// Manage DXMT (Direct3D 11 -> Apple Metal) translation layer
    Dxmt {
        #[command(subcommand)]
        action: DxmtAction,
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
    /// Diagnostic troubleshooting tool: verify whether a game window is actively displaying on macOS (0 screen capture)
    Validate {
        /// Steam Application ID
        appid: u32,
    },
    /// Steam patch and UI integration management
    Steam {
        #[command(subcommand)]
        action: SteamAction,
    },
    /// Map a Steam game (by AppID) to a Nucleon compatibility tool in Steam config.vdf
    Map {
        /// Steam Application ID (e.g. 601150)
        appid: u32,
        /// Optional tool name (default: 'nucleon')
        #[arg(long, short)]
        tool: Option<String>,
    },
    /// Unmap a Steam game (by AppID) from Steam config.vdf
    Unmap {
        /// Steam Application ID (e.g. 601150)
        appid: u32,
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
    /// Internal compatibility tool runner invoked by Steam (or via symlink 'nucleon-runner')
    #[command(external_subcommand)]
    Runner(Vec<String>),
}

fn main() -> Result<()> {
    env_logger::init();

    // Multi-call binary support: if invoked as 'nucleon-runner', dispatch directly to runner
    let raw_args: Vec<String> = std::env::args().collect();
    if let Some(arg0) = raw_args.first() {
        let exe_name = std::path::Path::new(arg0)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if exe_name == "nucleon-runner" {
            return nucleon_runner::run_with_args(&raw_args);
        }
    }

    // Direct invocation via 'nucleon runner <verb> [args...]'
    if raw_args.len() >= 2 && raw_args[1] == "runner" {
        let mut runner_args = vec![raw_args[0].clone()];
        runner_args.extend_from_slice(&raw_args[2..]);
        return nucleon_runner::run_with_args(&runner_args);
    }
    let cli = Cli::parse();

    match cli.command {
        Commands::Setup {
            action,
            non_interactive,
            force,
            kosmickrisp,
            kosmickrisp_path,
            kosmickrisp_version,
            fetch_vkd3d,
            vkd3d_path,
            fetch_d7vk,
            d7vk_path,
            fetch_dxvk,
            dxvk_path,
            fetch_dxmt,
            dxmt_path,
            wine,
            wine_path,
            gptk_path,
            bridge_path,
        } => commands::setup::run(SetupArgs {
            action,
            non_interactive,
            force,
            kosmickrisp,
            kosmickrisp_path,
            kosmickrisp_version,
            fetch_vkd3d,
            vkd3d_path,
            fetch_d7vk,
            d7vk_path,
            fetch_dxvk,
            dxvk_path,
            fetch_dxmt,
            dxmt_path,
            wine,
            wine_path,
            gptk_path,
            bridge_path,
        }),
        Commands::Status => commands::status::run(),
        Commands::Guard { action } => commands::guard::run(action),
        Commands::Wine { action } => commands::wine::run(action),
        Commands::Backends { action } => commands::backends::run(action),
        Commands::Gptk { action } => commands::gptk::run(action),
        Commands::Kosmickrisp { action } => commands::kosmickrisp::handle(action),
        Commands::Vkd3d { action } => commands::vkd3d::run(action),
        Commands::D7vk { action } => commands::d7vk::run(action),
        Commands::Dxvk { action } => commands::dxvk::run(action),
        Commands::Dxmt { action } => commands::dxmt::run(action),
        Commands::Detect { path } => commands::detect::run(&path),
        Commands::Launch { appid, hud, engine } => commands::launch::run_launch(appid, hud, engine),
        Commands::Validate { appid } => commands::launch::run_validate(appid),
        Commands::Steam { action } => commands::steam::run(action),
        Commands::Map { appid, tool } => commands::steam::run(SteamAction::Map { appid, tool }),
        Commands::Unmap { appid } => commands::steam::run(SteamAction::Unmap { appid }),
        Commands::Unregister => commands::steam::run_unregister(),
        Commands::Logs {
            lines,
            hook,
            runner,
            follow,
        } => commands::logs::run(lines, hook, runner, follow),
        Commands::Runner(mut args) => {
            let mut runner_args = vec!["nucleon-runner".to_string()];
            runner_args.append(&mut args);
            nucleon_runner::run_with_args(&runner_args)
        }
    }
}
