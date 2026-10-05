use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;
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
        /// Force specific engine (gptk or staging)
        #[arg(short, long)]
        engine: Option<String>,
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
                let target_runner = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/release/nucleon-runner");
                if target_runner.exists() {
                    fs::copy(&target_runner, &runner_bin)?;
                }
            }

            if runner_bin.exists() {
                steam::install_compatibility_tool(&runner_bin)?;
                println!("  ✓ Registered Steam compatibility tool: 'Nucleon (Game Porting Toolkit 4)'");
            }

            // 5. Restore any installed games that Steam may have unlinked
            let restored = steam::sync_library_folders()?;
            if !restored.is_empty() {
                println!("  ✓ Restored {} game(s) in Steam library: {:?}", restored.len(), restored);
            }

            // 6. Patch Steam.app if hook dylib is available
            if let Some(hook_dylib) = find_hook_dylib() {
                println!("==> Patching Steam client with {}...", hook_dylib.display());
                steam::patch_steam(&hook_dylib)?;
                println!("  ✓ Steam.app patched and signed with ad-hoc signature");
            } else {
                println!("  ! Run 'just build' or 'cargo build --release' to compile hook dylib before patching Steam");
            }

            // 7. Ensure pristine SteamUI chunks and clear CEF cache
            if let Ok(n) = steam::patch_steamui_chunks() {
                if n > 0 {
                    println!("  ✓ Restored {} pristine SteamUI WebUI chunk(s) (in-memory dynamic patching active)", n);
                } else {
                    println!("  ✓ SteamUI WebUI compatibility configured (in-memory dynamic patching active)");
                }
            }

            println!("\n==============================================================================");
            println!("  Nucleon setup complete!");
            println!("==============================================================================");
            println!("To use Nucleon in Steam:");
            println!("  1. Restart Steam: pkill steam_osx && open -a /Applications/Steam.app");
            println!("  2. The 'Install' button is now enabled for all Windows games in your library.");
            println!("  3. Clicking 'Install' begins downloading and routes the game via Nucleon.");
            println!("  4. To configure a specific runner/engine, right-click the game -> Properties -> Compatibility,");
            println!("     or use Steam Settings -> Compatibility.");
            println!("  5. Or launch directly from terminal: nucleon launch <AppID>");
        }

        Commands::Status => {
            println!("==> Nucleon Status Report");
            let steam_installed = steam::check_steam_installed().is_ok();
            println!("  Steam.app installed:       {}", if steam_installed { "✓ Yes" } else { "✗ No" });
            println!("  Steam patched for Nucleon: {}", if steam::is_steam_patched() { "✓ Yes" } else { "✗ No" });
            println!("  Steam process running:     {}", if steam::is_steam_running() { "● Running" } else { "○ Stopped" });

            let hook_log = paths::support_dir().join("nucleon-hook.log");
            if hook_log.is_file() {
                if let Ok(c) = fs::read_to_string(&hook_log) {
                    if c.contains("All compatibility hooks and instrumentation installed successfully") {
                        println!("  Hook Injection Status:     ✓ Active (all compat hooks installed)");
                    } else if let Some(last) = c.lines().rev().find(|l| !l.trim().is_empty()) {
                        println!("  Hook Injection Status:     ● {}", last.trim());
                    }
                }
            } else {
                println!("  Hook Injection Status:     ○ No hook log found yet (restart Steam to inject)");
            }

            let runner_cur = paths::current_runner();
            if runner_cur.exists() {
                println!("  Active Default Runner:     ✓ {}", runner_cur.display());
            } else {
                println!("  Active Default Runner:     ✗ Not configured (run 'nucleon setup')");
            }

            println!("  Dual-Engine Runtimes:");
            if let Some(gptk) = runner::find_gptk_runner() {
                println!("    ● GPTK (DX11/12):        ✓ {}", gptk.display());
            } else {
                println!("    ○ GPTK (DX11/12):        ✗ Not found (run 'nucleon setup')");
            }
            if let Some(staging) = runner::find_wine_staging_runtime() {
                println!("    ● Wine-Staging (DX9/10): ✓ {}", staging.display());
            } else {
                println!("    ○ Wine-Staging (DX9/10): ○ Optional (brew install --cask wine-staging)");
            }

            let bridge = paths::bridge_dir();
            let has_bridge = bridge.join("steamclient64.dll").is_file() && bridge.join("tier0_s64.dll").is_file();
            println!("  Bridge libraries staged:   {}", if has_bridge { "✓ Yes" } else { "✗ No" });
        }

        Commands::Detect { path } => {
            println!("==> Analyzing binary / game directory: {}", path.display());
            let info = nucleon_core::detector::detect_target_engine(&path);
            println!("  Graphics API detected:     {:?}", info.api);
            println!("  Detected library:          {}", info.detected_dll.as_deref().unwrap_or("None (heuristic)"));
            println!("  Recommended Engine:        {:?}", info.engine);
            match info.engine {
                nucleon_core::detector::TargetEngine::Gptk => {
                    println!("  Target Pipeline:           Apple Game Porting Toolkit (D3DMetal, Metal 4, MSync)");
                }
                nucleon_core::detector::TargetEngine::WineStaging => {
                    println!("  Target Pipeline:           Wine-Staging (WineD3D / Legacy Stack)");
                }
            }
        }

        Commands::Launch { appid, hud, engine } => {
            println!("==> Launching game AppID {}...", appid);

            // Ensure Steam permissions are clean
            let _ = steam::fix_steam_permissions();

            // Persist launch overrides so nucleon-runner reads them even with running Steam
            let override_file = paths::support_dir().join(format!("launch_override_{appid}.json"));
            let mut ov = serde_json::json!({
                "hud": hud,
            });
            if let Some(ref eng) = engine {
                ov["engine"] = serde_json::Value::String(eng.clone());
            }
            let _ = fs::write(&override_file, ov.to_string());

            let mut cmd = Command::new("open");
            cmd.arg(format!("steam://run/{}", appid));
            if hud {
                cmd.env("MTL_HUD_ENABLED", "1");
            }
            if let Some(eng) = engine {
                cmd.env("NUCLEON_ENGINE", eng);
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
                    .ok_or_else(|| anyhow::anyhow!("Hook dylib not found. Build it first with 'just build' or 'cargo build --release'"))?;
                steam::patch_steam(&hook_dylib)?;
                println!("  ✓ Successfully patched and signed Steam.app");
            }
            SteamAction::Restore => {
                steam::restore_steam()?;
                println!("  ✓ Successfully restored original Steam.app");
            }
        },

        Commands::Logs { lines, hook, runner, follow } => {
            show_logs(lines, hook, runner, follow)?;
        }
    }

    Ok(())
}

fn show_logs(lines: usize, show_hook: bool, show_runner: bool, follow: bool) -> Result<()> {
    let hook_path = paths::support_dir().join("nucleon-hook.log");
    let runner_path = paths::support_dir().join("nucleon-runner.log");

    let targets: Vec<(&str, &PathBuf)> = if show_hook && !show_runner {
        vec![("Hook Log", &hook_path)]
    } else if show_runner && !show_hook {
        vec![("Runner Log", &runner_path)]
    } else {
        vec![("Hook Log", &hook_path), ("Runner Log", &runner_path)]
    };


    for (title, path) in &targets {
        println!("==> {} ({})", title, path.display());
        if path.is_file() {
            if let Ok(content) = fs::read_to_string(path) {
                let all_lines: Vec<&str> = content.lines().collect();
                let start = if all_lines.len() > lines { all_lines.len() - lines } else { 0 };
                for line in &all_lines[start..] {
                    println!("{line}");
                }
            }
        } else {
            println!("  (no log file exists yet)");
        }
        println!();
    }

    if follow {
        println!("==> Following logs (press Ctrl+C to exit)...");
        let active_path = if show_runner { &runner_path } else { &hook_path };
        if !active_path.exists() {
            let _ = fs::File::create(active_path);
        }
        let mut file = File::open(active_path)?;
        let mut pos = file.seek(SeekFrom::End(0))?;

        loop {
            thread::sleep(Duration::from_millis(500));
            let metadata = fs::metadata(active_path)?;
            let len = metadata.len();
            if len > pos {
                file.seek(SeekFrom::Start(pos))?;
                let mut reader = BufReader::new(&file);
                let mut line = String::new();
                while reader.read_line(&mut line)? > 0 {
                    print!("{line}");
                    line.clear();
                }
                pos = file.stream_position()?;
            }
        }
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
