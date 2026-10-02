use std::fs;
use std::path::PathBuf;
use std::process::Command;
use anyhow::Result;
use clap::{Parser, Subcommand};
use nucleon_core::{manifest, paths, runner, steam, validator};

#[derive(Parser)]
#[command(name = "nucleon")]
#[command(about = "Nucleon: High-performance standalone Windows game translation on macOS using Wine & Apple GPTK 4")]
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
    },
    /// Inspect status of Nucleon, Steam patches, runner, and prefix
    Status,
    /// Launch a Windows game by Steam AppID
    Launch {
        /// Steam Application ID
        appid: u32,
        /// Enable Apple Metal Performance HUD
        #[arg(long)]
        hud: bool,
    },
    /// Validate that a game window is actively displaying and presenting frames on macOS (0 screen capture)
    Validate {
        /// Steam Application ID
        appid: u32,
    },
    /// Steam patch management
    Steam {
        #[command(subcommand)]
        action: SteamAction,
    },
}

#[derive(Subcommand)]
enum SteamAction {
    /// Inject Nucleon dylib into Steam.app Info.plist and re-sign
    Patch,
    /// Restore original Steam.app Info.plist
    Restore,
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    match cli.command {
        Commands::Setup { force } => {
            println!("==> Setting up Nucleon with Wine and GPTK 4...");
            paths::ensure_dirs()?;

            // 1. Copy signature files into Application Support
            let signatures_src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../signatures/macos.arm64");
            let signatures_dst = paths::signatures_dir();
            if signatures_src.is_dir() {
                for entry in fs::read_dir(&signatures_src)?.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|s| s.to_str()) == Some("json") {
                        fs::copy(&p, signatures_dst.join(p.file_name().unwrap()))?;
                    }
                }
                println!("  ✓ Installed Steam client signature databases");
            }

            // 2. Assemble Wine + GPTK 4 runner
            println!("==> Resolving Wine runtime & GPTK 4 D3DMetal components...");
            let runner_path = runner::assemble_runner(force)?;
            println!("  ✓ Runner assembled at: {}", runner_path.display());

            // 3. Stage Valve bridge packages
            println!("==> Staging Valve bridge packages...");
            manifest::fetch_and_stage_valve_packages()?;
            println!("  ✓ Valve client bridge libraries staged");

            // 4. Register compatibility tool
            let runner_bin = paths::support_dir().join("nucleon-runner");
            let current_exe = std::env::current_exe()?;
            let runner_src = current_exe.parent().unwrap().join("nucleon-runner");

            if runner_src.exists() {
                fs::copy(&runner_src, &runner_bin)?;
            } else {
                // If built via cargo run, fallback to target directory
                let target_runner = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/release/nucleon-runner");
                if target_runner.exists() {
                    fs::copy(&target_runner, &runner_bin)?;
                }
            }

            if runner_bin.exists() {
                steam::install_compatibility_tool(&runner_bin)?;
                println!("  ✓ Registered Steam compatibility tool: 'Nucleon (Game Porting Toolkit 4)'");
            }

            // 5. Patch Steam.app if hook dylib is available
            if let Some(hook_dylib) = find_hook_dylib() {
                println!("==> Patching Steam client with {}...", hook_dylib.display());
                steam::patch_steam(&hook_dylib)?;
                println!("  ✓ Steam.app patched and signed with ad-hoc signature");
            } else {
                println!("  ! Run 'make build' or 'cargo build --release' to compile hook dylib before patching Steam");
            }

            println!("\n==============================================================================");
            println!("  Nucleon setup complete!");
            println!("==============================================================================");
            println!("To use Nucleon in Steam:");
            println!("  1. Restart Steam: pkill steam_osx && open -a /Applications/Steam.app");
            println!("  2. Open Steam Settings -> Compatibility -> Enable Steam Play -> Select 'Nucleon'");
            println!("  3. Or run from terminal: nucleon launch <AppID>");
        }

        Commands::Status => {
            println!("==> Nucleon Status Report");
            let steam_installed = steam::check_steam_installed().is_ok();
            println!("  Steam.app installed:       {}", if steam_installed { "✓ Yes" } else { "✗ No" });
            println!("  Steam patched for Nucleon: {}", if steam::is_steam_patched() { "✓ Yes" } else { "✗ No" });
            println!("  Steam process running:     {}", if steam::is_steam_running() { "● Running" } else { "○ Stopped" });

            let runner_cur = paths::current_runner();
            if runner_cur.exists() {
                println!("  Active Runner:             ✓ {}", runner_cur.display());
            } else {
                println!("  Active Runner:             ✗ Not configured (run 'nucleon setup')");
            }

            let bridge = paths::bridge_dir();
            let has_bridge = bridge.join("steamclient64.dll").is_file() && bridge.join("tier0_s64.dll").is_file();
            println!("  Bridge libraries staged:   {}", if has_bridge { "✓ Yes" } else { "✗ No" });
        }

        Commands::Launch { appid, hud } => {
            println!("==> Launching game AppID {}...", appid);
            let mut cmd = Command::new("open");
            cmd.arg(&format!("steam://run/{}", appid));
            if hud {
                cmd.env("MTL_HUD_ENABLED", "1");
            }
            cmd.status()?;
            println!("  ✓ Sent launch command to Steam");
        }

        Commands::Validate { appid } => {
            println!("==> Validating window presentation for AppID {} (zero screen capture)...", appid);
            let presented = validator::check_window_presentation(None)?;
            if presented.is_empty() {
                println!("  ! No active Wine window currently presenting on screen.");
            } else {
                for w in presented {
                    println!(
                        "  ✓ Window ID: {} | Owner: {} | Layer: {} | OnScreen: {}",
                        w.window_id, w.owner_name, w.layer, w.is_onscreen
                    );
                }
            }
        }

        Commands::Steam { action } => match action {
            SteamAction::Patch => {
                let hook_dylib = find_hook_dylib()
                    .ok_or_else(|| anyhow::anyhow!("Hook dylib not found. Build it first with 'make build' or 'cargo build --release'"))?;
                steam::patch_steam(&hook_dylib)?;
                println!("  ✓ Successfully patched and signed Steam.app");
            }
            SteamAction::Restore => {
                steam::restore_steam()?;
                println!("  ✓ Successfully restored original Steam.app");
            }
        },
    }

    Ok(())
}

fn find_hook_dylib() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if dir.join("nucleon.dylib").exists() {
                return Some(dir.join("nucleon.dylib"));
            }
            if dir.join("libnucleon.dylib").exists() {
                return Some(dir.join("libnucleon.dylib"));
            }
        }
    }
    let manifest_release = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/release");
    if manifest_release.join("nucleon.dylib").exists() {
        return Some(manifest_release.join("nucleon.dylib"));
    }
    if manifest_release.join("libnucleon.dylib").exists() {
        return Some(manifest_release.join("libnucleon.dylib"));
    }
    None
}
