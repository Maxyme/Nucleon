use anyhow::Result;
use clap::{Parser, Subcommand};
use nucleon_core::{guard, manifest, paths, runner, signatures, steam, validator, vkd3d, wine};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

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
        /// Automatically fetch and stage VKD3D-Proton (Direct3D 12 -> Vulkan) from GitHub
        #[arg(long)]
        fetch_vkd3d: bool,
        /// Point to custom VKD3D-Proton installation directory
        #[arg(long)]
        vkd3d_path: Option<PathBuf>,
        /// Select desired Wine runtime flavor or path for Steam (e.g. staging, crossover)
        #[arg(long)]
        wine: Option<String>,
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
    /// Manage VKD3D-Proton (Direct3D 12 -> Vulkan 1.4) translation layer
    Vkd3d {
        #[command(subcommand)]
        action: Vkd3dAction,
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
enum GuardAction {
    /// Inspect status of codesigning, WebUI patches, and LaunchAgent
    Status,
    /// Run verification and apply self-healing fixes immediately
    Run {
        /// Suppress output unless errors occur
        #[arg(short, long)]
        quiet: bool,
    },
    /// Install and register background LaunchAgent (monitors Steam updates via launchd)
    Install,
    /// Uninstall and unload background LaunchAgent
    Uninstall,
    /// Run live lightweight watcher daemon in the foreground
    Watch {
        /// Polling interval in seconds (default: 3)
        #[arg(short, long, default_value_t = 3)]
        interval: u64,
    },
}

#[derive(Subcommand)]
enum SteamAction {
    /// Inject Nucleon dylib into Steam.app Info.plist and re-sign
    Patch,
    /// Restore original Steam.app Info.plist
    Restore,
}

#[derive(Subcommand)]
enum WineAction {
    /// List all discovered Wine runtimes and show the currently active desired version
    List,
    /// Select desired active Wine version for Steam (e.g. 'crossover', 'staging', 'dxmt', or path)
    Use {
        /// Wine identifier, name keyword, or custom path
        version: String,
    },
    /// Inspect details of active Wine runtime and Steam registration
    Status,
    /// Point Nucleon to a custom Wine installation directory
    SetPath {
        /// Path to Wine installation directory (containing bin/wine or Contents/Resources/wine)
        path: PathBuf,
    },
    /// Reset active Wine to default recommended primary runtime
    Reset,
    /// Clear configured custom Wine path
    ClearPath,
}

#[derive(Subcommand)]
enum Vkd3dAction {
    /// Inspect VKD3D-Proton detection, version, and DLL locations
    Status,
    /// Fetch official VKD3D-Proton release binaries from GitHub without tracking in git
    Fetch {
        /// Specific version tag (default: latest, e.g. v3.0.1)
        #[arg(short, long)]
        version: Option<String>,
    },
    /// Point Nucleon to an existing VKD3D-Proton installation directory
    SetPath {
        /// Path to directory containing d3d12.dll or x64/d3d12.dll
        path: PathBuf,
    },
    /// Clear custom configured VKD3D-Proton path
    ClearPath,
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    match cli.command {
        Commands::Setup {
            force,
            kosmickrisp,
            fetch_vkd3d,
            vkd3d_path,
            wine,
            bridge_path,
        } => {
            println!("==> Setting up Nucleon with Wine and GPTK 4...");
            paths::ensure_dirs()?;

            if let Some(ref desired_wine) = wine {
                match wine::set_active_wine(desired_wine) {
                    Ok(rt) => println!(
                        "  ✓ Set active Wine runtime to: {} [{}]",
                        rt.name,
                        rt.version.as_deref().unwrap_or("Unknown")
                    ),
                    Err(e) => {
                        eprintln!("  ! Failed to set active Wine '{}': {:#}", desired_wine, e)
                    }
                }
            }

            if let Some(ref p) = vkd3d_path {
                match vkd3d::set_custom_vkd3d_proton_path(p) {
                    Ok(bundle) => println!(
                        "  ✓ Registered custom VKD3D-Proton path at {}",
                        bundle.root.display()
                    ),
                    Err(e) => eprintln!(
                        "  ! Failed to set VKD3D-Proton path {}: {:#}",
                        p.display(),
                        e
                    ),
                }
            } else if fetch_vkd3d {
                println!("==> Fetching VKD3D-Proton (Direct3D 12 -> Vulkan)...");
                match vkd3d::fetch_vkd3d_proton(None, None) {
                    Ok(bundle) => println!("  ✓ VKD3D-Proton staged at {}", bundle.root.display()),
                    Err(e) => eprintln!("  ! Failed to fetch VKD3D-Proton: {:#}", e),
                }
            }

            if kosmickrisp {
                std::env::set_var("KOSMICKRISP_FORCE", "1");
                let kk_icd = paths::kosmickrisp_dir().join("libkosmickrisp_icd.json");
                if !kk_icd.exists() && runner::find_kosmickrisp_icd().is_none() {
                    let template = r#"{
    "file_format_version": "1.0.0",
    "ICD": {
        "library_path": "libvulkan_kosmickrisp.dylib",
        "api_version": "1.4.0"
    }
}
"#;
                    let _ = fs::write(&kk_icd, template);
                    println!(
                        "  ✓ Staged KosmicKrisp driver manifest template at {}",
                        kk_icd.display()
                    );
                }
            }

            // 1. Ensure signature DB for installed Steam client
            println!("==> Ensuring Steam client signature database...");
            match signatures::ensure_signature_db_for_installed_steam() {
                Ok((path, db)) => {
                    println!(
                        "  ✓ Verified & cached Steam client signatures (build {}) at {}",
                        db.steam_build,
                        path.display()
                    );
                }
                Err(e) => {
                    eprintln!("  ! Note: Could not pre-cache signatures during setup: {e}");
                }
            }

            // 2. Assemble Wine + GPTK 4 runner
            println!("==> Resolving Wine runtime & GPTK 4 D3DMetal components...");
            let runner_path = runner::assemble_runner(force)?;
            println!("  ✓ Runner assembled at: {}", runner_path.display());

            // 3. Stage Valve bridge packages
            println!("==> Staging Valve bridge packages...");
            manifest::fetch_and_stage_valve_packages(bridge_path.as_deref())?;
            println!("  ✓ Valve client bridge libraries staged");

            // 4. Register compatibility tool
            let runner_bin = paths::support_dir().join("nucleon-runner");
            let current_exe = std::env::current_exe()?;
            let runner_src = current_exe.parent().unwrap().join("nucleon-runner");

            if runner_src.exists() {
                fs::copy(&runner_src, &runner_bin)?;
            } else {
                let target_runner = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/release/nucleon-runner");
                if target_runner.exists() {
                    fs::copy(&target_runner, &runner_bin)?;
                }
            }

            if runner_bin.exists() {
                steam::install_compatibility_tool(&runner_bin)?;
                println!(
                    "  ✓ Registered Steam compatibility tool: 'Nucleon (Game Porting Toolkit 4)'"
                );
                if steam::is_kosmickrisp_tool_registered() {
                    println!("  ✓ Registered Steam compatibility tool: 'Nucleon (KosmicKrisp)'");
                } else {
                    println!("  ○ Steam compatibility tool 'Nucleon (KosmicKrisp)': not registered (KosmicKrisp not detected; pass --kosmickrisp to enable)");
                }
                if steam::is_wine_tool_registered() {
                    let active_name = wine::get_active_wine_runtime()
                        .map(|r| {
                            format!(
                                "{} [{}]",
                                r.name,
                                r.version.as_deref().unwrap_or("detected")
                            )
                        })
                        .unwrap_or_else(|| "default".to_string());
                    println!(
                        "  ✓ Registered Steam compatibility tool: 'Nucleon (Wine)' -> {}",
                        active_name
                    );
                }
            }

            // 5. Restore any installed games that Steam may have unlinked
            let restored = steam::sync_library_folders()?;
            if !restored.is_empty() {
                println!(
                    "  ✓ Restored {} game(s) in Steam library: {:?}",
                    restored.len(),
                    restored
                );
            }

            // 5b. Sanitize all installed game manifests to Ready to Play (clearing queued updates)
            if let Ok(sanitized) = steam::sanitize_installed_app_manifests() {
                if sanitized > 0 {
                    println!("  ✓ Sanitized {} game manifest(s) to 'Ready to Play' (cleared pending update downloads)", sanitized);
                }
            }

            // 6. Patch Steam.app if hook dylib is available
            let hook_dylib = find_hook_dylib();
            if let Some(ref dylib) = hook_dylib {
                println!("==> Patching Steam client with {}...", dylib.display());
                steam::patch_steam(dylib)?;
                println!("  ✓ Steam.app patched and signed with ad-hoc signature");
            } else {
                println!("  ! Run 'just build' or 'cargo build --release' to compile hook dylib before patching Steam");
            }

            // 7. Apply size-preserving SteamUI chunk patches and clear CEF cache
            if let Ok(n) = steam::patch_steamui_chunks() {
                if n > 0 {
                    println!(
                        "  ✓ Patched {} SteamUI WebUI chunk(s) (size-preserving bypass active)",
                        n
                    );
                } else {
                    println!(
                        "  ✓ SteamUI WebUI compatibility verified (size-preserving bypass active)"
                    );
                }
            }

            // 8. Register background Steam Update Guard LaunchAgent
            if let Some(ref dylib) = hook_dylib {
                let _ = fs::copy(dylib, paths::bridge_dir().join("nucleon.dylib"));
            }
            match guard::install_launchagent(None) {
                Ok(p) => {
                    println!(
                        "  ✓ Background Steam Update Guard LaunchAgent installed & active ({})",
                        p.display()
                    );
                }
                Err(e) => {
                    println!("  ! Background Steam Update Guard notice: {:#}", e);
                }
            }

            println!(
                "\n=============================================================================="
            );
            println!("  Nucleon setup complete!");
            println!(
                "=============================================================================="
            );
            println!("To use Nucleon in Steam:");
            println!("  1. Restart Steam: pkill steam_osx && open -a /Applications/Steam.app");
            println!(
                "  2. The 'Install' button is now enabled for all Windows games in your library."
            );
            println!("  3. Clicking 'Install' begins downloading and routes the game via Nucleon.");
            println!("  4. To configure a specific runner/engine, right-click the game -> Properties -> Compatibility,");
            println!("     or use Steam Settings -> Compatibility.");
            println!("  5. Or launch directly from terminal: nucleon launch <AppID>");
        }

        Commands::Status => {
            println!("==> Nucleon Status Report");
            let steam_installed = steam::check_steam_installed().is_ok();
            println!(
                "  Steam.app installed:       {}",
                if steam_installed { "✓ Yes" } else { "✗ No" }
            );
            println!(
                "  Steam patched for Nucleon: {}",
                if steam::is_steam_patched() {
                    "✓ Yes"
                } else {
                    "✗ No"
                }
            );
            println!(
                "  Steam process running:     {}",
                if steam::is_steam_running() {
                    "● Running"
                } else {
                    "○ Stopped"
                }
            );

            let hook_log = paths::support_dir().join("nucleon-hook.log");
            if hook_log.is_file() {
                if let Ok(c) = fs::read_to_string(&hook_log) {
                    if c.contains(
                        "All compatibility hooks and instrumentation installed successfully",
                    ) {
                        println!(
                            "  Hook Injection Status:     ✓ Active (all compat hooks installed)"
                        );
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

            println!("  Tri-Engine Architecture:");
            if let Some(gptk) = runner::find_gptk_runner() {
                println!("    ● Apple GPTK 4 (DX11/12):        ✓ {}", gptk.display());
            } else {
                println!("    ○ Apple GPTK 4 (DX11/12):        ✗ Not found (run 'nucleon setup')");
            }
            if steam::is_gptk_tool_registered() {
                println!(
                    "      └─ Registered in Steam UI:     ✓ 'Nucleon (Game Porting Toolkit 4)'"
                );
            } else {
                println!(
                    "      └─ Registered in Steam UI:     ○ Not registered (run 'nucleon setup')"
                );
            }

            if let Some(kk) = runner::find_kosmickrisp_icd() {
                println!("    ● Mesa KosmicKrisp (Vulkan 1.4): ✓ {}", kk.display());
            } else {
                println!(
                    "    ○ Mesa KosmicKrisp (Vulkan 1.4): ○ Optional (install Vulkan SDK or Mesa)"
                );
            }
            if steam::is_kosmickrisp_tool_registered() {
                println!("      ├─ Registered in Steam UI:     ✓ 'Nucleon (KosmicKrisp)'");
            } else {
                println!("      ├─ Registered in Steam UI:     ○ Not registered (run 'nucleon setup --kosmickrisp')");
            }
            if let Some(vkd3d) = vkd3d::find_vkd3d_proton() {
                let ver = vkd3d.version.as_deref().unwrap_or("detected");
                println!(
                    "      └─ VKD3D-Proton (Direct3D 12): ✓ {} [{}]",
                    vkd3d.root.display(),
                    ver
                );
            } else {
                println!("      └─ VKD3D-Proton (Direct3D 12): ○ Optional (run 'nucleon vkd3d set-path <DIR>' or 'nucleon setup --vkd3d-path <DIR>')");
            }

            let active_wine = wine::get_active_wine_runtime();
            let all_wines = wine::discover_wine_runtimes();
            if let Some(ref aw) = active_wine {
                let ver_str = aw.version.as_deref().unwrap_or("detected");
                println!(
                    "    ● Wine (DX9/10/Legacy):          ✓ {} [{}]",
                    aw.name, ver_str
                );
                if steam::is_wine_tool_registered() {
                    println!("      ├─ Registered in Steam UI:     ✓ 'Nucleon (Wine)'");
                } else {
                    println!("      ├─ Registered in Steam UI:     ○ Not registered (run 'nucleon setup')");
                }
                if all_wines.len() > 1 {
                    let other_ids: Vec<String> = all_wines
                        .iter()
                        .filter(|r| r.root != aw.root)
                        .map(|r| r.id.clone())
                        .collect();
                    println!(
                        "      └─ Switchable versions:        {} (run 'nucleon wine list')",
                        other_ids.join(", ")
                    );
                }
            } else if let Some(staging) = runner::find_wine_staging_runtime() {
                println!(
                    "    ● Wine-Staging (DX9/10/Legacy):  ✓ {}",
                    staging.display()
                );
                if steam::is_staging_tool_registered() {
                    println!("      └─ Registered in Steam UI:     ✓ 'Nucleon (Wine)'");
                }
            } else {
                println!("    ○ Wine (DX9/10/Legacy):          ○ Optional (install Heroic Wine or brew install --cask wine-staging)");
            }

            let bridge = paths::bridge_dir();
            let has_bridge = bridge.join("steamclient64.dll").is_file()
                && bridge.join("tier0_s64.dll").is_file();
            println!(
                "  Bridge libraries staged:   {}",
                if has_bridge { "✓ Yes" } else { "✗ No" }
            );

            let guard_status = guard::check_guard_status();
            println!("  Background Steam Update Guard:");
            let la_str = if guard_status.launchagent_loaded {
                format!("✓ Active / Loaded ('{}')", guard::GUARD_LABEL)
            } else if guard_status.launchagent_installed {
                "○ Installed but not loaded (run 'nucleon guard install')".to_string()
            } else {
                "○ Not installed (run 'nucleon guard install')".to_string()
            };
            println!("    ● LaunchAgent Service:           {}", la_str);
            println!(
                "    ● Steam Ad-hoc Signature:        {}",
                if guard_status.adhoc_signed {
                    "✓ Valid"
                } else {
                    "✗ Invalid / Revoked (run 'nucleon guard run')"
                }
            );
            println!(
                "    ● WebUI Chunk Compatibility:     {}",
                if guard_status.webui_patched {
                    "✓ Patched"
                } else {
                    "✗ Unpatched (run 'nucleon guard run')"
                }
            );
            println!(
                "    ● Hook Dynamic Injection:        {}",
                if guard_status.plist_patched && guard_status.hook_dylib_present {
                    "✓ Active"
                } else {
                    "✗ Inactive"
                }
            );
            println!(
                "    ● Dynamic Signature Cache:       {}",
                if guard_status.signatures_cached {
                    format!(
                        "✓ Cached (build {})",
                        guard_status.detected_steam_build.unwrap_or(0)
                    )
                } else {
                    "○ Auto-generates on launch/guard run".to_string()
                }
            );
        }

        Commands::Wine { action } => match action {
            WineAction::List | WineAction::Status => {
                println!(
                    "==> Discovered Wine Runtimes (Heroic, Homebrew, Whisky, CrossOver, Custom)"
                );
                let runtimes = wine::discover_wine_runtimes();
                let active = wine::get_active_wine_runtime();

                if runtimes.is_empty() {
                    println!("  ○ No Wine runtimes detected.");
                    println!(
                        "  To configure a custom Wine runtime: nucleon wine set-path /path/to/wine"
                    );
                } else {
                    println!("  Found {} runtime(s):\n", runtimes.len());
                    for rt in &runtimes {
                        let is_active = active.as_ref().map(|a| a.root == rt.root).unwrap_or(false);
                        let ver = rt.version.as_deref().unwrap_or("Unknown version");
                        let marker = if is_active { " [ACTIVE]" } else { "" };
                        let symbol = if is_active { "●" } else { "○" };
                        println!(
                            "  {} {:<14} - {} [{}] {}",
                            symbol, rt.id, rt.name, ver, marker
                        );
                        println!("    Location: {}", rt.root.display());
                    }
                    let active_name = active.as_ref().map(|a| a.name.as_str()).unwrap_or("None");
                    println!(
                        "\n  Steam UI Tool: 'Nucleon (Wine)' -> currently using '{}'",
                        active_name
                    );
                    println!(
                        "  To switch active Wine:        nucleon wine use <staging|crossover|path>"
                    );
                    println!("  Per-game Steam Launch Option: NUCLEON_WINE=crossover %command%");
                }
            }
            WineAction::Use { version } => {
                println!("==> Setting active Wine runtime to '{}'...", version);
                let rt = wine::set_active_wine(&version)?;
                println!(
                    "  ✓ Active Wine set to: {} [{}]",
                    rt.name,
                    rt.version.as_deref().unwrap_or("Unknown")
                );
                println!("  ✓ Root path: {}", rt.root.display());
                if steam::is_wine_tool_registered() {
                    println!("  ✓ Steam compatibility tool 'Nucleon (Wine)' updated immediately.");
                } else {
                    println!("  ○ Run 'nucleon setup' to register 'Nucleon (Wine)' in Steam.");
                }
            }
            WineAction::Reset => {
                wine::clear_active_wine()?;
                println!("  ✓ Reset active Wine to default primary runtime.");
                if let Some(primary) = wine::find_primary_wine_runtime() {
                    println!(
                        "  ✓ Now using: {} [{}]",
                        primary.name,
                        primary.version.as_deref().unwrap_or("Unknown")
                    );
                }
            }
            WineAction::SetPath { path } => {
                println!("==> Registering custom Wine path: {}", path.display());
                let rt = wine::set_custom_wine_path(&path)?;
                println!(
                    "  ✓ Registered custom Wine: {} [{}]",
                    rt.name,
                    rt.version.as_deref().unwrap_or("Unknown")
                );
                println!("  ✓ Wine root: {}", rt.root.display());
                println!("To activate this Wine: nucleon wine use custom");
            }
            WineAction::ClearPath => {
                wine::clear_custom_wine_path()?;
                println!("  ✓ Cleared custom Wine path configuration.");
            }
        },

        Commands::Vkd3d { action } => match action {
            Vkd3dAction::Status => {
                println!("==> VKD3D-Proton Status (Direct3D 12 -> Vulkan 1.4 for KosmicKrisp)");
                if let Some(bundle) = vkd3d::find_vkd3d_proton() {
                    println!("  Status:           ✓ Installed / Detected");
                    println!("  Location:         {}", bundle.root.display());
                    println!(
                        "  Version:          {}",
                        bundle.version.as_deref().unwrap_or("Unknown")
                    );
                    println!("  64-bit d3d12.dll: {}", bundle.x64_d3d12.display());
                    if let Some(ref core) = bundle.x64_d3d12core {
                        println!("  64-bit d3d12core: {}", core.display());
                    }
                    if let Some(ref x86) = bundle.x86_d3d12 {
                        println!("  32-bit d3d12.dll: {}", x86.display());
                    }
                } else {
                    println!("  Status:           ○ Not installed / Not detected");
                    println!(
                        "  Managed Path:     {}",
                        paths::vkd3d_proton_dir().display()
                    );
                    println!("\nTo install or configure VKD3D-Proton without committing binaries:");
                    println!(
                        "  1. Extract official release archive (.tar.zst) from GitHub releases"
                    );
                    println!(
                        "  2. Point to extracted directory: nucleon vkd3d set-path /path/to/extracted/vkd3d-proton"
                    );
                    println!(
                        "  3. Or stage directly into:       {}",
                        paths::vkd3d_proton_dir().display()
                    );
                    println!(
                        "  4. Or set environment variable:  export VKD3D_PROTON_PATH=/path/to/extracted/vkd3d-proton"
                    );
                }
            }
            Vkd3dAction::Fetch { version } => {
                let ver_str = version.as_deref().unwrap_or(vkd3d::DEFAULT_VKD3D_VERSION);
                println!(
                    "==> Fetching official VKD3D-Proton {} from GitHub...",
                    ver_str
                );
                let bundle = vkd3d::fetch_vkd3d_proton(version.as_deref(), None)?;
                println!(
                    "  ✓ Successfully staged VKD3D-Proton at {}",
                    bundle.root.display()
                );
                println!("  ✓ 64-bit Direct3D 12 DLL: {}", bundle.x64_d3d12.display());
                if let Some(ref core) = bundle.x64_d3d12core {
                    println!("  ✓ 64-bit D3D12Core DLL:   {}", core.display());
                }
                println!("\nGames running under 'Nucleon (KosmicKrisp)' will now translate Direct3D 12 -> Vulkan 1.4.");
            }
            Vkd3dAction::SetPath { path } => {
                println!(
                    "==> Registering custom VKD3D-Proton path: {}",
                    path.display()
                );
                let bundle = vkd3d::set_custom_vkd3d_proton_path(&path)?;
                println!(
                    "  ✓ Validated and registered VKD3D-Proton at {}",
                    bundle.root.display()
                );
                println!("  ✓ 64-bit Direct3D 12 DLL: {}", bundle.x64_d3d12.display());
            }
            Vkd3dAction::ClearPath => {
                vkd3d::clear_custom_vkd3d_proton_path()?;
                println!("  ✓ Cleared custom VKD3D-Proton path override");
            }
        },

        Commands::Detect { path } => {
            println!("==> Analyzing binary / game directory: {}", path.display());
            let info = nucleon_core::detector::detect_target_engine(&path);
            println!("  Graphics API detected:     {:?}", info.api);
            println!(
                "  Detected library:          {}",
                info.detected_dll.as_deref().unwrap_or("None (heuristic)")
            );
            println!("  Recommended Engine:        {:?}", info.engine);
            println!(
                "  Target Pipeline:           {}",
                info.engine.display_name()
            );
        }

        Commands::Launch { appid, hud, engine } => {
            println!("==> Launching game AppID {}...", appid);

            // Ensure Steam update guard, permissions, manifests, WebUI, and compat mappings are clean
            let _ = guard::run_guard_once(find_hook_dylib().as_deref());
            let _ = steam::fix_steam_permissions();
            let _ = steam::sanitize_installed_app_manifests();
            let _ = steam::patch_steamui_chunks();
            let _ = steam::migrate_compat_mappings();

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
            println!(
                "==> Validating window presentation for AppID {} (zero screen capture)...",
                appid
            );
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

        Commands::Guard { action } => match action {
            GuardAction::Status => {
                println!("==> Steam Update Guard Status");
                let status = guard::check_guard_status();
                println!(
                    "  Steam.app installed:       {}",
                    if status.steam_installed {
                        "✓ Yes"
                    } else {
                        "✗ No"
                    }
                );
                println!(
                    "  Ad-hoc Codesignature:      {}",
                    if status.adhoc_signed {
                        "✓ Valid"
                    } else {
                        "✗ Revoked / Missing (run 'nucleon guard run')"
                    }
                );
                println!(
                    "  Info.plist Injection:      {}",
                    if status.plist_patched {
                        "✓ Valid"
                    } else {
                        "✗ Missing (run 'nucleon guard run')"
                    }
                );
                println!(
                    "  Hook Dylib Present:        {}",
                    if status.hook_dylib_present {
                        "✓ Present & Signed"
                    } else {
                        "✗ Missing / Unsigned (run 'nucleon guard run')"
                    }
                );
                println!(
                    "  WebUI Chunk Patches:       {}",
                    if status.webui_patched {
                        "✓ Patched"
                    } else {
                        "✗ Unpatched / Overwritten (run 'nucleon guard run')"
                    }
                );
                println!(
                    "  LaunchAgent Installed:     {}",
                    if status.launchagent_installed {
                        "✓ Yes"
                    } else {
                        "○ No (run 'nucleon guard install')"
                    }
                );
                println!(
                    "  LaunchAgent Running:       {}",
                    if status.launchagent_loaded {
                        "✓ Loaded"
                    } else {
                        "○ Not loaded"
                    }
                );
                if let Some(b) = status.detected_steam_build {
                    println!(
                        "  Signatures Cached:         {}",
                        if status.signatures_cached {
                            format!("✓ Cached for build {}", b)
                        } else {
                            format!(
                                "○ Not cached for build {} (auto-generates on launch/guard run)",
                                b
                            )
                        }
                    );
                }
                if let Some(ts) = status.last_heal_timestamp {
                    println!("  Last Guard Verification:   {} (unix timestamp)", ts);
                }
            }
            GuardAction::Run { quiet } => {
                if !quiet {
                    println!("==> Running Steam Update Guard verification and heal...");
                }
                let hook_dylib = find_hook_dylib();
                let res = guard::run_guard_once(hook_dylib.as_deref())?;
                if !quiet {
                    if res.no_action_needed {
                        println!("  ✓ Steam signatures, hook injection, and WebUI patches are fully healthy. No action needed.");
                    } else {
                        if res.hook_deployed {
                            println!("  ✓ Deployed Nucleon hook dylib into Steam bundle");
                        }
                        if res.plist_fixed {
                            println!("  ✓ Restored DYLD_INSERT_LIBRARIES in Steam Info.plist");
                        }
                        if res.resigned {
                            println!("  ✓ Re-signed Steam.app and binaries with ad-hoc signature");
                        }
                        if res.webui_patched_count > 0 {
                            println!(
                                "  ✓ Re-applied patches to {} WebUI chunk(s) and cleared CEF cache",
                                res.webui_patched_count
                            );
                        }
                        if res.mappings_cleaned {
                            println!("  ✓ Cleaned native macOS games from CompatToolMapping");
                        }
                        if res.signatures_cached {
                            println!("  ✓ Automatically scanned steamclient.dylib and cached signatures locally");
                        }
                    }
                }
            }
            GuardAction::Install => {
                println!("==> Installing Nucleon Steam Guard LaunchAgent...");
                let hook_dylib = find_hook_dylib();
                if let Some(ref dylib) = hook_dylib {
                    let _ = fs::copy(dylib, paths::bridge_dir().join("nucleon.dylib"));
                }
                let plist_path = guard::install_launchagent(None)?;
                println!(
                    "  ✓ Created and loaded LaunchAgent: {}",
                    plist_path.display()
                );
                println!("  ✓ Steam updates to /Applications/Steam.app or steamui will now automatically trigger self-healing.");
            }
            GuardAction::Uninstall => {
                println!("==> Uninstalling Nucleon Steam Guard LaunchAgent...");
                guard::uninstall_launchagent()?;
                println!("  ✓ Unloaded and removed LaunchAgent plist");
            }
            GuardAction::Watch { interval } => {
                println!(
                    "==> Starting foreground Steam Guard watcher (polling every {}s)...",
                    interval
                );
                println!("Press Ctrl+C to stop.");
                let term = Arc::new(AtomicBool::new(false));
                let term_clone = Arc::clone(&term);
                ctrlc::set_handler(move || {
                    term_clone.store(true, Ordering::SeqCst);
                })
                .ok();
                let hook_dylib = find_hook_dylib();
                guard::run_guard_watcher(
                    term,
                    Duration::from_secs(interval),
                    hook_dylib.as_deref(),
                )?;
                println!("Watcher stopped.");
            }
        },

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

        Commands::Logs {
            lines,
            hook,
            runner,
            follow,
        } => {
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
                let start = if all_lines.len() > lines {
                    all_lines.len() - lines
                } else {
                    0
                };
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
        let active_path = if show_runner {
            &runner_path
        } else {
            &hook_path
        };
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
            if dir.join("libnucleon.dylib").exists() {
                return Some(dir.join("libnucleon.dylib"));
            }
            if dir.join("nucleon.dylib").exists() {
                return Some(dir.join("nucleon.dylib"));
            }
        }
    }
    let manifest_release = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/release");
    if manifest_release.join("libnucleon.dylib").exists() {
        return Some(manifest_release.join("libnucleon.dylib"));
    }
    if manifest_release.join("nucleon.dylib").exists() {
        return Some(manifest_release.join("nucleon.dylib"));
    }
    None
}
