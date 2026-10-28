mod commands;

use anyhow::Result;
use clap::{Parser, Subcommand};
use commands::{
    backends::BackendsAction, d7vk::D7vkAction, dxmt::DxmtAction, dxvk::DxvkAction,
    gptk::GptkAction, kosmickrisp::KosmickrispAction, setup::SetupArgs, steam::SteamAction,
    vkd3d::Vkd3dAction, wine::WineAction,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "nucleon")]
#[command(
    about = "Nucleon: Windows games and programs on macOS with Wine and independent Steam game launching"
)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_launch_commands_accept_arbitrary_exes_and_explicit_authentication() {
        for args in [
            vec![
                "nucleon",
                "app",
                "add",
                "other-game",
                "/Games/Other Game.exe",
                "--prefix",
                "/Prefixes/Other",
                "--wine",
                "wine-id",
                "--env",
                "CUSTOM=with spaces=and equals",
                "--",
                "--game-option",
                "",
            ],
            vec!["nucleon", "app", "import", "talos2", "/config/launch.json"],
            vec!["nucleon", "app", "run", "talos2", "--dry-run"],
            vec![
                "nucleon",
                "app",
                "run",
                "other-game",
                "--exe",
                "/moved/Another.exe",
                "--engine",
                "dxmt",
                "--",
                "a b",
            ],
            vec!["nucleon", "app", "check", "talos2"],
            vec![
                "nucleon",
                "app",
                "add",
                "epic-game",
                "Game.exe",
                "--legendary-app",
                "arbitrary-id",
                "--legendary",
                "/tools/legendary",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_ok());
        }
        assert!(
            Cli::try_parse_from([
                "nucleon",
                "app",
                "add",
                "bad",
                "Game.exe",
                "--legendary-app",
                "id"
            ])
            .is_err()
        );
    }

    #[test]
    fn upscaler_commands_and_saved_preferences_parse_without_launching() {
        for args in [
            vec!["nucleon", "run", "Game.exe", "--upscaler", "fsr3.1"],
            vec!["nucleon", "launch", "123", "--upscaler", "fsr4-native-lite"],
            vec!["nucleon", "run", "Game.exe", "--upscaler", "fsr4.1.2"],
            vec![
                "nucleon",
                "upscaler",
                "install",
                "native412",
                "--wine",
                "/runtime",
            ],
            vec![
                "nucleon",
                "upscaler",
                "prepare",
                "Game.exe",
                "--backend",
                "metalfx",
                "--input",
                "dlss",
                "--dry-run",
            ],
            vec![
                "nucleon", "upscaler", "install", "native", "--wine", "/runtime",
            ],
            vec!["nucleon", "upscaler", "restore", "Game.exe"],
            vec![
                "nucleon",
                "bottle",
                "configure",
                "game",
                "--clear",
                "upscaler",
            ],
            vec![
                "nucleon",
                "bottle",
                "add",
                "game",
                "game",
                "Game.exe",
                "--upscaler",
                "fsr2",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_ok());
        }
        assert!(
            Cli::try_parse_from(["nucleon", "run", "Game.exe", "--upscaler", "fsr4.1.3"]).is_err()
        );
    }

    #[test]
    fn standalone_cli_parses_options_and_preserves_forwarded_arguments() {
        let cli = Cli::try_parse_from([
            "nucleon",
            "run",
            "Game with ünicode.exe",
            "--bottle",
            "my-game",
            "--wine",
            "fex-preview",
            "--engine",
            "dxvk",
            "--hud=false",
            "--",
            "--wine",
            "a game option",
            "",
        ])
        .unwrap();
        let Commands::Run(args) = cli.command else {
            panic!("expected Run");
        };
        assert_eq!(args.bottle.as_deref(), Some("my-game"));
        assert_eq!(args.arguments, ["--wine", "a game option", ""]);
        assert_eq!(args.executable, PathBuf::from("Game with ünicode.exe"));
        let help = Cli::try_parse_from(["nucleon", "run", "--help"])
            .err()
            .unwrap();
        assert_eq!(help.kind(), clap::error::ErrorKind::DisplayHelp);
        for args in [
            vec![
                "nucleon",
                "bottle",
                "create",
                "my-game",
                "--emulator",
                "fex",
            ],
            vec![
                "nucleon",
                "bottle",
                "configure",
                "my-game",
                "--clear",
                "wine",
                "--clear",
                "hud",
            ],
            vec![
                "nucleon",
                "bottle",
                "add",
                "my-game",
                "game",
                "Game.exe",
                "--",
                "--fullscreen",
            ],
            vec![
                "nucleon", "bottle", "run", "my-game", "game", "--engine", "wine",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_ok());
        }
    }
    #[test]
    fn wine_install_accepts_named_versions_and_explicit_sources() {
        for args in [
            vec![
                "nucleon",
                "wine",
                "install",
                "isotope",
                "--version",
                "0.1.0-rc.1",
            ],
            vec!["nucleon", "wine", "install", "/tmp/runtime.tar.gz"],
            vec![
                "nucleon",
                "wine",
                "install",
                "https://example.invalid/runtime.tar.gz",
                "--sha256",
                &"a".repeat(64),
            ],
        ] {
            assert!(matches!(
                Cli::try_parse_from(args).unwrap().command,
                Commands::Wine {
                    action: WineAction::Install { .. }
                }
            ));
        }
    }
    #[test]
    fn launch_parses_complete_runtime_and_emulator_constraints() {
        for emulator in ["rosetta", "fex"] {
            let cli = Cli::try_parse_from([
                "nucleon",
                "launch",
                "690790",
                "--wine",
                "fex-preview",
                "--emulator",
                emulator,
                "--engine",
                "dxmt",
            ])
            .unwrap();
            assert!(matches!(
                cli.command,
                Commands::Launch(commands::steam::PlayArgs {
                    wine: Some(_),
                    emulator: Some(_),
                    ..
                })
            ));
        }
        assert!(
            Cli::try_parse_from(["nucleon", "launch", "690790", "--emulator", "box64"]).is_err()
        );
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Save and launch EXEs with reusable runtime, prefix and store-authentication profiles
    App {
        #[command(subcommand)]
        action: commands::app::Action,
    },
    /// Run a Windows EXE directly, without Steam; use -- to pass program arguments
    Run(commands::application::RunArgs),
    /// Manage named environments shared by installers and installed programs
    Bottle {
        #[command(subcommand)]
        action: commands::application::BottleAction,
    },
    /// Prepare and inspect experimental game-local temporal upscaling (does not start games)
    Upscaler {
        #[command(subcommand)]
        action: commands::upscaler::Action,
    },
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
        /// Select desired Wine runtime flavor or path (e.g. staging, crossover, or /Applications/CrossOver.app)
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
    /// Inspect runtime, graphics and Steam launch configuration
    Status,
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
    /// Manage Apple Game Porting Toolkit (runner & D3DMetal backend)
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
    /// Launch a Steam game directly through Nucleon
    Launch(commands::steam::PlayArgs),
    /// Diagnostic troubleshooting tool: verify whether a game window is actively displaying on macOS (0 screen capture)
    Validate {
        /// Steam Application ID
        appid: u32,
    },
    /// Download and launch Steam games without modifying Steam
    Steam {
        #[command(subcommand)]
        action: SteamAction,
    },
    /// View per-launch Nucleon logs
    Logs {
        /// Number of lines to display (default: 50)
        #[arg(short = 'n', long, default_value_t = 50)]
        lines: usize,
        /// Follow log output continuously
        #[arg(short, long)]
        follow: bool,
    },
}

fn main() -> Result<()> {
    env_logger::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::App { action } => commands::app::run(action),
        Commands::Run(args) => commands::application::run(args),
        Commands::Bottle { action } => commands::application::bottle(action),
        Commands::Upscaler { action } => commands::upscaler::run(action),
        Commands::Setup {
            action,
            non_interactive,
            force,
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
        Commands::Wine { action } => commands::wine::run(action),
        Commands::Backends { action } => commands::backends::run(action),
        Commands::Gptk { action } => commands::gptk::run(action),
        Commands::Kosmickrisp { action } => commands::kosmickrisp::run(action),
        Commands::Vkd3d { action } => commands::vkd3d::run(action),
        Commands::D7vk { action } => commands::d7vk::run(action),
        Commands::Dxvk { action } => commands::dxvk::run(action),
        Commands::Dxmt { action } => commands::dxmt::run(action),
        Commands::Detect { path } => commands::detect::run(&path),
        Commands::Launch(args) => commands::launch::run_launch(args),
        Commands::Validate { appid } => commands::launch::run_validate(appid),
        Commands::Steam { action } => commands::steam::run(action),
        Commands::Logs { lines, follow } => commands::logs::run(lines, follow),
    }
}
