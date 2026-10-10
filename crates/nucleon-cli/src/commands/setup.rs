use super::ui;
use anyhow::{bail, Result};
use nucleon_core::{
    backend, d7vk, dxmt, dxvk, guard, manifest, paths, runner, signatures, steam, vkd3d, wine,
};
use clap::Subcommand;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Subcommand, Debug, Clone)]
pub enum SetupAction {
    /// Remediate missing directories, cache signatures, and register tools
    Fix,
}

#[derive(Debug, Default, Clone)]
pub struct SetupArgs {
    pub action: Option<SetupAction>,
    pub non_interactive: bool,
    pub force: bool,
    pub kosmickrisp: bool,
    pub kosmickrisp_path: Option<PathBuf>,
    pub kosmickrisp_version: Option<String>,
    pub fetch_vkd3d: bool,
    pub vkd3d_path: Option<PathBuf>,
    pub fetch_d7vk: bool,
    pub d7vk_path: Option<PathBuf>,
    pub fetch_dxvk: bool,
    pub dxvk_path: Option<PathBuf>,
    pub fetch_dxmt: bool,
    pub dxmt_path: Option<PathBuf>,
    pub wine: Option<String>,
    pub wine_path: Option<PathBuf>,
    pub gptk_path: Option<PathBuf>,
    pub bridge_path: Option<PathBuf>,
}

pub struct PrerequisiteCheck {
    pub name: &'static str,
    pub passed: bool,
    pub required: bool,
    pub details: String,
    pub remediation: String,
}

/// Idempotent, non-interactive check verifying all Nucleon prerequisites.
pub fn check_prerequisites(gptk_override: Option<&Path>) -> Vec<PrerequisiteCheck> {
    let mut checks = Vec::new();

    // 1. Support Directories
    let support_ok = paths::support_dir().is_dir() && paths::runners_dir().is_dir();
    checks.push(PrerequisiteCheck {
        name: "Support Directories",
        passed: support_ok,
        required: true,
        details: paths::support_dir().display().to_string(),
        remediation: "Run 'nucleon setup fix' to create missing support directories.".to_string(),
    });

    // 2. Steam Installation
    let steam_installed = steam::check_steam_installed().is_ok();
    checks.push(PrerequisiteCheck {
        name: "Steam Installation",
        passed: steam_installed,
        required: true,
        details: if steam_installed {
            paths::steam_app().display().to_string()
        } else {
            "Steam.app not found in /Applications".to_string()
        },
        remediation: "Install Steam for macOS from https://store.steampowered.com/about/.".to_string(),
    });

    // 3. Wine Runtime
    let active_wine = wine::get_active_wine_runtime();
    let discovered_wine = wine::discover_wine_runtimes();
    let wine_ok = active_wine.is_some() || !discovered_wine.is_empty();
    let wine_details = if let Some(w) = active_wine {
        format!("{} [{}]", w.name, w.version.as_deref().unwrap_or("detected"))
    } else if !discovered_wine.is_empty() {
        format!(
            "{} [{}] (auto-detected)",
            discovered_wine[0].name,
            discovered_wine[0].version.as_deref().unwrap_or("detected")
        )
    } else {
        "No Wine runtime found".to_string()
    };
    checks.push(PrerequisiteCheck {
        name: "Wine Runtime",
        passed: wine_ok,
        required: true,
        details: wine_details,
        remediation: "Install Wine via Homebrew ('brew install --cask wine-staging') or configure existing Wine ('nucleon wine add <name> <path>').".to_string(),
    });

    // 4. Apple GPTK 4 Components (User-supplied)
    let gptk_info = runner::get_gptk_resolution_info(gptk_override);
    let gptk_ok = gptk_info.is_some();
    let (gptk_details, gptk_remedy) = if let Some((comps, origin)) = gptk_info {
        (format!("{} ({origin})", comps.0.display()), String::new())
    } else {
        (
            "Not configured".to_string(),
            "Apple GPTK components must be supplied by the user. Mount the Apple Game Porting Toolkit DMG and register via: 'nucleon gptk set-path <DIR>'.".to_string(),
        )
    };
    checks.push(PrerequisiteCheck {
        name: "Apple GPTK 4 (User-Supplied)",
        passed: gptk_ok,
        required: true,
        details: gptk_details,
        remediation: gptk_remedy,
    });

    // 5. Runner & Steam Compatibility Tools
    let runner_ok = paths::current_runner().exists() || steam::is_auto_tool_registered();
    let runner_details = if paths::current_runner().exists() {
        paths::current_runner().display().to_string()
    } else {
        "Not assembled / not registered".to_string()
    };
    checks.push(PrerequisiteCheck {
        name: "Runner & Steam Tools",
        passed: runner_ok,
        required: false,
        details: runner_details,
        remediation: "Run 'nucleon setup fix' to assemble runner and register Steam compatibility tools.".to_string(),
    });

    // 6. Graphics Backend
    let (b, b_origin) = backend::get_active_backend_with_origin();
    checks.push(PrerequisiteCheck {
        name: "Graphics Backend",
        passed: true,
        required: false,
        details: format!("{} ({b_origin})", b.display_name()),
        remediation: String::new(),
    });

    checks
}

/// Prints a structured status table for all prerequisite checks.
pub fn print_prerequisites_table(checks: &[PrerequisiteCheck]) {
    println!("\n================================================================================");
    println!("                          Nucleon Prerequisite Check");
    println!("================================================================================");
    println!("{:<32} {:<10} Details", "Prerequisite", "Status");
    println!("--------------------------------------------------------------------------------");
    for check in checks {
        let status = if check.passed {
            "PASS"
        } else if check.required {
            "FAIL"
        } else {
            "OPTIONAL"
        };
        println!("{:<32} {:<10} {}", check.name, status, check.details);
    }
    println!("--------------------------------------------------------------------------------");
}

/// Executes remediation of missing directories, signatures, bridges, and runner staging.
pub fn run_fix(args: &SetupArgs) -> Result<()> {
    let _ = args.non_interactive;
    if args.kosmickrisp {
        std::env::set_var("KOSMICKRISP_FORCE", "1");
    }
    ui::header("Remediating Nucleon Prerequisites ('nucleon setup fix')...");

    // 1. Ensure required support directories
    paths::ensure_dirs()?;
    ui::success("Ensured required support directories exist");

    // 2. Custom path configuration if provided in flags
    if let Some(ref gp) = args.gptk_path {
        match runner::set_custom_gptk_path(gp) {
            Ok((fw, shared)) => {
                ui::success(format!("Registered custom Apple GPTK path at {}", gp.display()));
                ui::tree_kv("└─", "D3DMetal.framework:", fw.display());
                ui::tree_kv("└─", "libd3dshared.dylib:", shared.display());
            }
            Err(e) => ui::warn(format!("Failed to set custom GPTK path {}: {:#}", gp.display(), e)),
        }
    }

    if let Some(ref wp) = args.wine_path {
        match wine::set_custom_wine_path(wp) {
            Ok(rt) => {
                ui::success(format!("Registered custom Wine runtime at {}", rt.root.display()));
                let _ = wine::set_active_wine(&rt.id);
            }
            Err(e) => ui::warn(format!("Failed to set custom Wine path {}: {:#}", wp.display(), e)),
        }
    }

    if let Some(ref kp) = args.kosmickrisp_path {
        match runner::set_custom_kosmickrisp_path(kp, args.kosmickrisp_version.as_deref()) {
            Ok(info) => {
                ui::success(format!("Registered custom KosmicKrisp at {}", info.icd_path.display()));
            }
            Err(e) => ui::warn(format!("Failed to set custom KosmicKrisp path: {:#}", e)),
        }
    }

    if let Some(ref desired_wine) = args.wine {
        let _ = wine::set_active_wine(desired_wine);
    }

    if let Some(ref p) = args.vkd3d_path {
        let _ = vkd3d::set_custom_vkd3d_proton_path(p);
    } else if args.fetch_vkd3d {
        let _ = vkd3d::fetch_vkd3d_proton(None, None);
    }

    if let Some(ref p) = args.d7vk_path {
        let _ = d7vk::set_custom_d7vk_path(p);
    } else if args.fetch_d7vk {
        let _ = d7vk::fetch_d7vk(None, None);
    }

    if let Some(ref p) = args.dxvk_path {
        let _ = dxvk::set_custom_dxvk_path(p);
    } else if args.fetch_dxvk {
        let _ = dxvk::fetch_dxvk(None, None);
    }

    if let Some(ref p) = args.dxmt_path {
        let _ = dxmt::set_custom_dxmt_path(p);
    } else if args.fetch_dxmt {
        let _ = dxmt::fetch_dxmt(None, None);
    }

    // 3. Pre-cache Steam signatures
    match signatures::ensure_signature_db_for_installed_steam() {
        Ok((path, db)) => {
            ui::success(format!(
                "Verified & cached Steam client signatures (build {}) at {}",
                db.steam_build,
                path.display()
            ));
        }
        Err(e) => {
            ui::warn(format!("Note: Could not pre-cache signatures: {e}"));
        }
    }

    // 4. Stage Valve bridge packages
    match manifest::fetch_and_stage_valve_packages(args.bridge_path.as_deref()) {
        Ok(()) => ui::success("Valve client bridge libraries staged"),
        Err(e) => ui::warn(format!("Failed to stage bridge packages: {e}")),
    }

    // 5. Assemble runner & register Steam compatibility tools if components are resolvable
    let gptk_comps = runner::find_gptk_components(args.gptk_path.as_deref()).ok().flatten();
    let wine_rt = wine::get_active_wine_runtime();

    if gptk_comps.is_some() || wine_rt.is_some() {
        match runner::assemble_runner(args.force, args.wine_path.as_deref(), args.gptk_path.as_deref()) {
            Ok(runner_path) => {
                ui::success(format!("Runner assembled at: {}", runner_path.display()));

                let runner_bin = paths::support_dir().join("nucleon-runner");
                let current_exe = std::env::current_exe()?;
                let runner_src = current_exe
                    .parent()
                    .map(|p| p.join("nucleon-runner"))
                    .unwrap_or_else(|| PathBuf::from("nucleon-runner"));

                if runner_src.exists() {
                    let _ = fs::copy(&runner_src, &runner_bin);
                } else {
                    let _ = fs::copy(&current_exe, &runner_bin);
                }

                if runner_bin.exists() {
                    if let Err(e) = steam::install_compatibility_tool(&runner_bin) {
                        ui::warn(format!("Failed to register Steam compatibility tools: {e}"));
                    } else {
                        ui::success("Registered Steam compatibility tools");
                    }
                }
            }
            Err(e) => {
                ui::warn(format!("Could not assemble runner during fix: {e}"));
            }
        }
    }

    // 6. Install guard LaunchAgent if missing
    let plist = paths::steam_guard_plist();
    if !plist.exists() {
        if let Ok(bin) = std::env::current_exe() {
            let _ = guard::install_launchagent(Some(&bin));
            ui::success("Installed Steam guard background LaunchAgent");
        }
    }

    Ok(())
}

pub fn run(args: SetupArgs) -> Result<()> {
    // If fix action is requested, remediate what can be fixed
    if let Some(SetupAction::Fix) = args.action {
        run_fix(&args)?;
        let checks = check_prerequisites(args.gptk_path.as_deref());
        print_prerequisites_table(&checks);
        return Ok(());
    }

    // Default flow: idempotent, non-interactive check
    let checks = check_prerequisites(args.gptk_path.as_deref());
    print_prerequisites_table(&checks);

    let failed_required: Vec<&PrerequisiteCheck> = checks
        .iter()
        .filter(|c| c.required && !c.passed)
        .collect();

    if !failed_required.is_empty() {
        println!("\nMissing Prerequisites & Remediation Instructions:");
        for check in &failed_required {
            println!("  • {}: {}", check.name, check.remediation);
        }
        println!();
        bail!(
            "Setup check failed: {} required prerequisite(s) missing. Run 'nucleon setup fix' or configure missing components as shown above.",
            failed_required.len()
        );
    }

    ui::success("All prerequisites satisfied! System is ready to run Windows games.");
    Ok(())
}
