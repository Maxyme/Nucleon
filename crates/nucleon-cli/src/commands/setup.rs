use super::ui;
use anyhow::{bail, Result};
use nucleon_core::{d7vk, guard, manifest, paths, prefix, runner, signatures, steam, vkd3d, wine};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct SetupArgs {
    pub non_interactive: bool,
    pub force: bool,
    pub kosmickrisp: bool,
    pub kosmickrisp_path: Option<PathBuf>,
    pub kosmickrisp_version: Option<String>,
    pub fetch_vkd3d: bool,
    pub vkd3d_path: Option<PathBuf>,
    pub fetch_d7vk: bool,
    pub d7vk_path: Option<PathBuf>,
    pub wine: Option<String>,
    pub wine_path: Option<PathBuf>,
    pub gptk_path: Option<PathBuf>,
    pub bridge_path: Option<PathBuf>,
}

fn read_user_input(prompt: &str) -> io::Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

fn show_installation_guidance(component: &str) {
    match component {
        "gptk" => {
            println!(
                r#"
--------------------------------------------------------------------------------
[!] Apple Game Porting Toolkit 4 (D3DMetal) Not Detected:
  Apple GPTK translates DirectX 11 and DirectX 12 games directly into Apple Metal.
  To install Apple GPTK:
    1. Download 'Game_Porting_Toolkit_4.0_beta_2.dmg' (or latest) from:
       https://developer.apple.com/games/game-porting-toolkit/
    2. Mount the dmg and extract components:
       mkdir -p "$HOME/Developer/gptk"
       cp -R "/Volumes/Game Porting Toolkit 4.0 beta 2/redist/lib/external/D3DMetal.framework" "$HOME/Developer/gptk/"
       cp "/Volumes/Game Porting Toolkit 4.0 beta 2/redist/lib/external/libd3dshared.dylib" "$HOME/Developer/gptk/"
    3. Register with Nucleon:
       nucleon gptk set-path "$HOME/Developer/gptk"
--------------------------------------------------------------------------------"#
            );
        }
        "wine" => {
            println!(
                r#"
--------------------------------------------------------------------------------
[!] Wine Runtime Not Detected:
  Nucleon requires a 64-bit Wine runtime (such as Wine-Staging or CrossOver).
  To install Wine:
    • Open-source Wine-Staging via Homebrew:
        brew install --cask wine-staging
    • Or if you have CrossOver installed in /Applications, Nucleon detects it automatically:
        nucleon wine use crossover
    • Or register any custom Wine installation:
        nucleon wine add my-wine /path/to/wine --use-now
--------------------------------------------------------------------------------"#
            );
        }
        "kosmickrisp" => {
            println!(
                r#"
--------------------------------------------------------------------------------
[!] Mesa KosmicKrisp Vulkan Driver Not Detected:
  KosmicKrisp provides a Khronos-conformant Vulkan 1.4 driver on Metal 4 for
  Apple Silicon, enabling VKD3D-Proton (Direct3D 12) and D7VK (DirectDraw/DX1-7).
  To install KosmicKrisp:
    1. Download the macOS LunarG Vulkan SDK installer from:
       https://vulkan.lunarg.com/sdk/home#mac
    2. Run the installer (it installs into /Library/Frameworks/Vulkan.framework or /usr/local).
    3. If installed to a custom directory, register it with Nucleon:
       nucleon kosmickrisp set-path /path/to/libkosmickrisp_icd.json
--------------------------------------------------------------------------------"#
            );
        }
        "vkd3d" => {
            println!(
                r#"
--------------------------------------------------------------------------------
[!] VKD3D-Proton (Direct3D 12 -> Vulkan 1.4) Not Detected:
  To install official VKD3D-Proton release binaries directly from GitHub:
    nucleon vkd3d fetch
  Or extract manually and configure:
    nucleon vkd3d set-path /path/to/extracted/vkd3d-proton
--------------------------------------------------------------------------------"#
            );
        }
        "d7vk" => {
            println!(
                r#"
--------------------------------------------------------------------------------
[!] D7VK (DirectDraw / Direct3D 1-7 -> Vulkan 1.4) Not Detected:
  To fetch official D7VK release binaries directly from GitHub:
    nucleon d7vk fetch
  Or extract manually and configure:
    nucleon d7vk set-path /path/to/extracted/d7vk
--------------------------------------------------------------------------------"#
            );
        }
        _ => {}
    }
}

fn interactive_customize_menu(args: &mut SetupArgs) -> Result<()> {
    loop {
        let gptk_status = match runner::find_gptk_components(args.gptk_path.as_deref()) {
            Ok(Some(_)) => "✓ Detected",
            _ => "✗ Not detected",
        };

        let wine_runtimes = wine::discover_wine_runtimes();
        let selected_wine_display = if let Some(ref w) = args.wine {
            format!("{w} (override)")
        } else if let Some(ref wp) = args.wine_path {
            format!("{} (custom path)", wp.display())
        } else if let Some(active) = wine::get_active_wine_runtime() {
            format!(
                "{} [{}]",
                active.name,
                active.version.as_deref().unwrap_or("detected")
            )
        } else if !wine_runtimes.is_empty() {
            format!("{} (auto)", wine_runtimes[0].name)
        } else {
            "✗ None detected".to_string()
        };

        let kk_detected = runner::is_kosmickrisp_installed();
        let kk_status = if args.kosmickrisp_path.is_some() {
            "✓ Custom path configured"
        } else if args.kosmickrisp || kk_detected {
            "✓ Enabled / Detected"
        } else {
            "○ Optional (not enabled)"
        };

        let vkd3d_status = if args.fetch_vkd3d {
            "✓ Auto-fetch from GitHub"
        } else if args.vkd3d_path.is_some() || vkd3d::find_vkd3d_proton().is_some() {
            "✓ Detected / Custom path"
        } else {
            "○ Optional (not configured)"
        };

        let d7vk_status = if args.fetch_d7vk {
            "✓ Auto-fetch from GitHub"
        } else if args.d7vk_path.is_some() || d7vk::find_d7vk().is_some() {
            "✓ Detected / Custom path"
        } else {
            "○ Optional (not configured)"
        };

        println!(
            r#"
Customize Nucleon Setup Options:
  [1] Runner Runtimes:
      1) Wine Runtime (Active/Default):                {selected_wine_display}
      2) Apple Game Porting Toolkit 4 (Runner & D3DMetal): {gptk_status}

  [2] Wine Graphics Translation Backends:
      3) Mesa KosmicKrisp (Vulkan 1.4):                {kk_status}
      4) VKD3D-Proton (Direct3D 12 -> Vulkan):         {vkd3d_status}
      5) D7VK (DirectDraw/DX1-7 -> Vulkan):            {d7vk_status}

  [3] Setup Actions:
      6) Toggle Force Rebuild:                         {} (re-assembles runner from scratch; rarely needed)
      7) Return to main menu and proceed
      8) Abort setup
"#,
            if args.force {
                "Enabled"
            } else {
                "Disabled (recommended)"
            }
        );

        let choice = read_user_input("Select an option (1-8): ").unwrap_or_default();
        match choice.as_str() {
            "1" => {
                println!("\nConfigure Wine Runtime:");
                if wine_runtimes.is_empty() {
                    ui::warn("No Wine runtimes automatically detected on this system.");
                    show_installation_guidance("wine");
                } else {
                    println!("Discovered Wine runtimes:");
                    for (i, rt) in wine_runtimes.iter().enumerate() {
                        println!(
                            "  {}) {} [{}] ({})",
                            i + 1,
                            rt.name,
                            rt.version.as_deref().unwrap_or("unknown"),
                            rt.root.display()
                        );
                    }
                }
                println!("\nEnter a number (1-{}), a runtime name/ID (e.g. 'crossover'), a path, or press Enter to keep current:", wine_runtimes.len());
                let input = read_user_input("> ")?;
                if !input.is_empty() {
                    if let Ok(idx) = input.parse::<usize>() {
                        if idx > 0 && idx <= wine_runtimes.len() {
                            args.wine = Some(wine_runtimes[idx - 1].id.clone());
                            ui::success(format!("Selected Wine: {}", wine_runtimes[idx - 1].name));
                            continue;
                        }
                    }
                    let p = PathBuf::from(&input);
                    if p.is_dir() && wine::inspect_wine_dir(&p, None, None).is_some() {
                        args.wine_path = Some(p);
                        ui::success("Custom Wine path updated.");
                    } else {
                        args.wine = Some(input);
                        ui::success("Wine preference updated.");
                    }
                }
            }
            "2" => {
                println!("\nConfigure Apple Game Porting Toolkit 4 (GPTK 4 / D3DMetal):");
                println!(
                    "Enter directory path containing D3DMetal.framework and libd3dshared.dylib,"
                );
                println!("or type 'help' for instructions, or press Enter to keep current:");
                let input = read_user_input("> ")?;
                if input.eq_ignore_ascii_case("help") {
                    show_installation_guidance("gptk");
                } else if !input.is_empty() {
                    let p = PathBuf::from(&input);
                    if runner::inspect_gptk_dir(&p).is_some() {
                        args.gptk_path = Some(p);
                        ui::success("GPTK path validated and updated.");
                    } else {
                        ui::warn(format!("Could not find D3DMetal.framework in '{}'.", input));
                        show_installation_guidance("gptk");
                    }
                }
            }
            "3" => {
                println!("\nConfigure Mesa KosmicKrisp Vulkan Driver:");
                println!("  e) Enable with auto-detected/standard path");
                println!("  p) Set custom path to libkosmickrisp_icd.json or driver directory");
                println!("  h) View installation instructions");
                println!("  d) Disable KosmicKrisp");
                let sub = read_user_input("Select (e/p/h/d): ")?;
                match sub.to_lowercase().as_str() {
                    "e" => {
                        args.kosmickrisp = true;
                        if !runner::is_kosmickrisp_installed() {
                            show_installation_guidance("kosmickrisp");
                        } else {
                            ui::success("KosmicKrisp enabled.");
                        }
                    }
                    "p" => {
                        let path_str =
                            read_user_input("Enter path to KosmicKrisp ICD JSON or directory: ")?;
                        if !path_str.is_empty() {
                            args.kosmickrisp_path = Some(PathBuf::from(path_str));
                            args.kosmickrisp = true;
                            ui::success("KosmicKrisp path updated.");
                        }
                    }
                    "h" => show_installation_guidance("kosmickrisp"),
                    "d" => {
                        args.kosmickrisp = false;
                        args.kosmickrisp_path = None;
                        ui::info("KosmicKrisp disabled.");
                    }
                    _ => {}
                }
            }
            "4" => {
                println!("\nConfigure VKD3D-Proton (Direct3D 12 -> Vulkan):");
                println!("  f) Automatically fetch latest from official GitHub release");
                println!("  p) Set path to locally extracted VKD3D-Proton directory");
                println!("  h) View installation instructions");
                println!("  d) Disable / keep optional");
                let sub = read_user_input("Select (f/p/h/d): ")?;
                match sub.to_lowercase().as_str() {
                    "f" => {
                        args.fetch_vkd3d = true;
                        ui::success("VKD3D-Proton will be downloaded during setup.");
                    }
                    "p" => {
                        let path_str = read_user_input("Enter path to VKD3D-Proton directory: ")?;
                        if !path_str.is_empty() {
                            args.vkd3d_path = Some(PathBuf::from(path_str));
                            ui::success("VKD3D-Proton path updated.");
                        }
                    }
                    "h" => show_installation_guidance("vkd3d"),
                    "d" => {
                        args.fetch_vkd3d = false;
                        args.vkd3d_path = None;
                    }
                    _ => {}
                }
            }
            "5" => {
                println!("\nConfigure D7VK (DirectDraw / Direct3D 1-7 -> Vulkan):");
                println!("  f) Automatically fetch latest from official GitHub release");
                println!("  p) Set path to locally extracted D7VK directory");
                println!("  h) View installation instructions");
                println!("  d) Disable / keep optional");
                let sub = read_user_input("Select (f/p/h/d): ")?;
                match sub.to_lowercase().as_str() {
                    "f" => {
                        args.fetch_d7vk = true;
                        ui::success("D7VK will be downloaded during setup.");
                    }
                    "p" => {
                        let path_str = read_user_input("Enter path to D7VK directory: ")?;
                        if !path_str.is_empty() {
                            args.d7vk_path = Some(PathBuf::from(path_str));
                            ui::success("D7VK path updated.");
                        }
                    }
                    "h" => show_installation_guidance("d7vk"),
                    "d" => {
                        args.fetch_d7vk = false;
                        args.d7vk_path = None;
                    }
                    _ => {}
                }
            }
            "6" => {
                args.force = !args.force;
                ui::info(format!("Force rebuild set to: {}", args.force));
            }
            "7" => break,
            "8" => bail!("Setup aborted by user."),
            _ => println!("Invalid option, please choose between 1 and 8."),
        }
    }
    Ok(())
}

fn prompt_interactive_setup(args: &mut SetupArgs) -> Result<()> {
    let gptk_display = if runner::find_gptk_components(args.gptk_path.as_deref())
        .ok()
        .flatten()
        .is_some()
    {
        "Apple GPTK 4 (Runner & D3DMetal)"
    } else {
        "None (run custom setup or help)"
    };

    let wine_display = if let Some(ref w) = args.wine {
        format!("{w} (override)")
    } else if let Some(active) = wine::get_active_wine_runtime() {
        format!(
            "{} [{}]",
            active.name,
            active.version.as_deref().unwrap_or("detected")
        )
    } else {
        "None (run custom setup or help)".to_string()
    };

    let kk_display = if args.kosmickrisp || runner::is_kosmickrisp_installed() {
        "Mesa KosmicKrisp Vulkan 1.4"
    } else {
        "None (optional)"
    };

    println!(
        r#"
================================================================================
                    Welcome to Nucleon Setup
================================================================================
Nucleon configures the native macOS Steam client to download and launch Windows
games using Apple Game Porting Toolkit 4, Wine, and Mesa KosmicKrisp.

Current Detected Defaults:
  • Apple GPTK 4 (Runner & D3DMetal):  {gptk_display}
  • Wine Runtime (Runner):             {wine_display}
  • Wine Vulkan Driver (KosmicKrisp):  {kk_display}

Options:
  1) Proceed with setup (default: GPTK 4: {gptk_display}, Wine: {wine_display})
  2) Customize setup options (select Wine, configure GPTK / KosmicKrisp paths)
  3) Cancel setup
"#
    );

    let choice = read_user_input("Select an option [1]: ").unwrap_or_default();
    match choice.as_str() {
        "" | "1" => {
            // Verify minimum prerequisites and show instructions if missing
            if runner::find_gptk_components(args.gptk_path.as_deref())
                .ok()
                .flatten()
                .is_none()
            {
                show_installation_guidance("gptk");
            }
            if wine::get_active_wine_runtime().is_none()
                && wine::discover_wine_runtimes().is_empty()
            {
                show_installation_guidance("wine");
            }
            Ok(())
        }
        "2" => {
            interactive_customize_menu(args)?;
            Ok(())
        }
        "3" => bail!("Setup cancelled by user."),
        _ => {
            println!("Invalid selection, proceeding with default setup...");
            Ok(())
        }
    }
}

pub fn run(mut args: SetupArgs) -> Result<()> {
    let has_explicit_flags = args.force
        || args.kosmickrisp
        || args.kosmickrisp_path.is_some()
        || args.fetch_vkd3d
        || args.vkd3d_path.is_some()
        || args.fetch_d7vk
        || args.d7vk_path.is_some()
        || args.wine.is_some()
        || args.wine_path.is_some()
        || args.gptk_path.is_some()
        || args.bridge_path.is_some();

    if !args.non_interactive && !has_explicit_flags {
        prompt_interactive_setup(&mut args)?;
    }

    ui::header("Setting up Nucleon with Wine and GPTK 4...");
    paths::ensure_dirs()?;

    if let Some(ref wp) = args.wine_path {
        match wine::set_custom_wine_path(wp) {
            Ok(rt) => {
                ui::success(format!(
                    "Registered custom Wine runtime at {}",
                    rt.root.display()
                ));
                let _ = wine::set_active_wine(&rt.id);
            }
            Err(e) => ui::warn(format!(
                "Failed to set custom Wine path {}: {:#}",
                wp.display(),
                e
            )),
        }
    }

    if let Some(ref gp) = args.gptk_path {
        match runner::set_custom_gptk_path(gp) {
            Ok((fw, shared)) => {
                ui::success(format!(
                    "Registered custom Apple GPTK path at {}",
                    gp.display()
                ));
                ui::tree_kv("└─", "D3DMetal.framework:", fw.display());
                ui::tree_kv("└─", "libd3dshared.dylib:", shared.display());
            }
            Err(e) => ui::warn(format!(
                "Failed to set custom GPTK path {}: {:#}",
                gp.display(),
                e
            )),
        }
    }

    if let Some(ref kp) = args.kosmickrisp_path {
        match runner::set_custom_kosmickrisp_path(kp, args.kosmickrisp_version.as_deref()) {
            Ok(info) => {
                ui::success(format!(
                    "Registered custom KosmicKrisp (Vulkan {}) at {}",
                    info.api_version,
                    info.icd_path.display()
                ));
            }
            Err(e) => ui::warn(format!(
                "Failed to set custom KosmicKrisp path {}: {:#}",
                kp.display(),
                e
            )),
        }
    }

    if let Some(ref desired_wine) = args.wine {
        match wine::set_active_wine(desired_wine) {
            Ok(rt) => ui::success(format!(
                "Set active Wine runtime to: {} [{}]",
                rt.name,
                rt.version.as_deref().unwrap_or("Unknown")
            )),
            Err(e) => ui::warn(format!(
                "Failed to set active Wine '{}': {:#}",
                desired_wine, e
            )),
        }
    }

    if let Some(ref p) = args.vkd3d_path {
        match vkd3d::set_custom_vkd3d_proton_path(p) {
            Ok(bundle) => ui::success(format!(
                "Registered custom VKD3D-Proton path at {}",
                bundle.root.display()
            )),
            Err(e) => ui::warn(format!(
                "Failed to set VKD3D-Proton path {}: {:#}",
                p.display(),
                e
            )),
        }
    } else if args.fetch_vkd3d {
        ui::header("Fetching VKD3D-Proton (Direct3D 12 -> Vulkan)...");
        match vkd3d::fetch_vkd3d_proton(None, None) {
            Ok(bundle) => ui::success(format!("VKD3D-Proton staged at {}", bundle.root.display())),
            Err(e) => ui::warn(format!("Failed to fetch VKD3D-Proton: {:#}", e)),
        }
    }

    if let Some(ref p) = args.d7vk_path {
        match d7vk::set_custom_d7vk_path(p) {
            Ok(bundle) => ui::success(format!(
                "Registered custom D7VK path at {}",
                bundle.root.display()
            )),
            Err(e) => ui::warn(format!("Failed to set D7VK path {}: {:#}", p.display(), e)),
        }
    } else if args.fetch_d7vk {
        ui::header("Fetching D7VK (DirectDraw / Direct3D 1-7 -> Vulkan)...");
        match d7vk::fetch_d7vk(None, None) {
            Ok(bundle) => ui::success(format!("D7VK staged at {}", bundle.root.display())),
            Err(e) => ui::warn(format!("Failed to fetch D7VK: {:#}", e)),
        }
    }

    if args.kosmickrisp || args.kosmickrisp_path.is_some() {
        std::env::set_var("KOSMICKRISP_FORCE", "1");
        let kk_icd = paths::kosmickrisp_dir().join("libkosmickrisp_icd.json");
        if !kk_icd.exists() && runner::find_kosmickrisp_icd().is_none() {
            let dylib = runner::find_kosmickrisp_driver_dylib();
            let lib_path = dylib
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "libvulkan_kosmickrisp.dylib".to_string());
            let version = args.kosmickrisp_version.as_deref().unwrap_or("1.4.0");

            match runner::write_kosmickrisp_icd(&kk_icd, Path::new(&lib_path), version) {
                Ok(()) => {
                    ui::success(format!(
                        "Staged KosmicKrisp driver manifest (Vulkan {}) at {}",
                        version,
                        kk_icd.display()
                    ));
                }
                Err(e) => {
                    ui::warn(format!("Failed to write KosmicKrisp ICD manifest: {:#}", e));
                }
            }
        }
    }

    // 1. Ensure signature DB for installed Steam client
    ui::header("Ensuring Steam client signature database...");
    match signatures::ensure_signature_db_for_installed_steam() {
        Ok((path, db)) => {
            ui::success(format!(
                "Verified & cached Steam client signatures (build {}) at {}",
                db.steam_build,
                path.display()
            ));
        }
        Err(e) => {
            ui::warn(format!(
                "Note: Could not pre-cache signatures during setup: {e}"
            ));
        }
    }

    // 2. Assemble Wine + GPTK 4 runner
    ui::header("Resolving Wine runtime & GPTK 4 D3DMetal components...");
    let runner_path = runner::assemble_runner(
        args.force,
        args.wine_path.as_deref(),
        args.gptk_path.as_deref(),
    )?;
    ui::success(format!("Runner assembled at: {}", runner_path.display()));

    // 3. Stage Valve bridge packages
    ui::header("Staging Valve bridge packages...");
    manifest::fetch_and_stage_valve_packages(args.bridge_path.as_deref())?;
    ui::success("Valve client bridge libraries staged");

    // 4. Register compatibility tool
    let runner_bin = paths::support_dir().join("nucleon-runner");
    let current_exe = std::env::current_exe()?;
    let runner_src = current_exe
        .parent()
        .map(|p| p.join("nucleon-runner"))
        .unwrap_or_else(|| PathBuf::from("nucleon-runner"));

    if runner_src.exists() {
        fs::copy(&runner_src, &runner_bin)?;
    } else {
        let target_runner =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/release/nucleon-runner");
        if target_runner.exists() {
            fs::copy(&target_runner, &runner_bin)?;
        } else {
            // Consolidated single binary: nucleon acts as its own runner via multi-call or 'runner' subcommand
            fs::copy(&current_exe, &runner_bin)?;
        }
    }

    if runner_bin.exists() {
        steam::install_compatibility_tool(&runner_bin)?;
        if steam::is_auto_tool_registered() {
            ui::success("Registered Steam compatibility tool: 'Nucleon (Wine + Automatic Graphics Backend)'");
        }
        if steam::is_gptk_tool_registered() {
            let gptk_display = steam::registered_gptk_tool_display_name().unwrap_or_else(|| {
                if let Some(v) = runner::detect_gptk_version(args.gptk_path.as_deref()) {
                    format!(
                        "Nucleon (GPTK {} + Apple D3DMetal)",
                        runner::format_gptk_version(&v)
                    )
                } else {
                    "Nucleon (GPTK + Apple D3DMetal)".to_string()
                }
            });
            ui::success(format!(
                "Registered Steam compatibility tool: '{gptk_display}'"
            ));
        }
        if steam::is_kosmickrisp_tool_registered() {
            ui::success(
                "Registered Steam compatibility tool: 'Nucleon (Wine + Mesa KosmicKrisp Vulkan)'",
            );
        } else {
            ui::info("Steam compatibility tool 'Nucleon (Wine + Mesa KosmicKrisp Vulkan)': not registered (KosmicKrisp not detected; pass --kosmickrisp to enable)");
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
            ui::success(format!(
                "Registered Steam compatibility tool: 'Nucleon (Wine + WineD3D OpenGL)' -> {}",
                active_name
            ));
        }
    }

    // 5. Restore any installed games that Steam may have unlinked
    let restored = steam::sync_library_folders()?;
    if !restored.is_empty() {
        ui::success(format!(
            "Restored {} game(s) in Steam library: {:?}",
            restored.len(),
            restored
        ));
    }

    // 5b. Sanitize all installed game manifests to Ready to Play (clearing queued updates)
    if let Ok(sanitized) = steam::sanitize_installed_app_manifests() {
        if sanitized > 0 {
            ui::success(format!("Sanitized {} game manifest(s) to 'Ready to Play' (cleared pending update downloads)", sanitized));
        }
    }

    // 5c. Sandbox user shell folders across all existing game prefixes (eliminates Desktop/Downloads permission popups)
    if let Ok(isolated) = prefix::isolate_all_steam_game_prefixes() {
        if isolated > 0 {
            ui::success(format!(
                "Isolated user shell folders in {} existing Steam game prefix(es) (preventing macOS Desktop/Downloads permission popups)",
                isolated
            ));
        }
    }

    // 6. Patch Steam.app if hook dylib is available
    let hook_dylib = super::find_hook_dylib();
    if let Some(ref dylib) = hook_dylib {
        ui::header(format!("Patching Steam client with {}...", dylib.display()));
        steam::patch_steam(dylib)?;
        ui::success("Steam.app patched and signed with ad-hoc signature");
    } else {
        ui::warn("Run 'just build' or 'cargo build --release' to compile hook dylib before patching Steam");
    }

    // 7. Apply size-preserving SteamUI chunk patches and clear CEF cache
    if let Ok(n) = steam::patch_steamui_chunks() {
        if n > 0 {
            ui::success(format!(
                "Patched {} SteamUI WebUI chunk(s) (size-preserving bypass active)",
                n
            ));
        } else {
            ui::success("SteamUI WebUI compatibility verified (size-preserving bypass active)");
        }
    }

    // 8. Register background Steam Update Guard LaunchAgent
    if let Some(ref dylib) = hook_dylib {
        let _ = fs::copy(dylib, paths::bridge_dir().join("nucleon.dylib"));
    }
    match guard::install_launchagent(None) {
        Ok(p) => {
            ui::success(format!(
                "Background Steam Update Guard LaunchAgent installed & active ({})",
                p.display()
            ));
        }
        Err(e) => {
            ui::warn(format!("Background Steam Update Guard notice: {:#}", e));
        }
    }

    let gptk_banner_display = steam::registered_gptk_tool_display_name().unwrap_or_else(|| {
        if let Some(v) = runner::detect_gptk_version(args.gptk_path.as_deref()) {
            format!(
                "Nucleon (GPTK {} + Apple D3DMetal)",
                runner::format_gptk_version(&v)
            )
        } else {
            "Nucleon (GPTK + Apple D3DMetal)".to_string()
        }
    });

    println!(
        r#"
==============================================================================
  Nucleon setup complete!
==============================================================================
To use Nucleon in Steam:
  1. Restart Steam: pkill steam_osx && open -a /Applications/Steam.app
  2. The 'Install' button is now enabled for all Windows games in your library.
  3. Clicking 'Install' begins downloading and routes the game via Nucleon.
  4. Compatibility tools available in Steam:
     - 'Nucleon (Wine + Automatic Graphics Backend)' [Default]
     - '{gptk_banner_display}'
     - 'Nucleon (Wine + Mesa KosmicKrisp Vulkan)'
     - 'Nucleon (Wine + WineD3D OpenGL)'
  5. Or launch directly from terminal: nucleon launch <AppID>"#
    );

    Ok(())
}
