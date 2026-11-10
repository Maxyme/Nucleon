use super::ui;
use anyhow::{Result, bail};
use clap::Subcommand;
use nucleon_core::{backend, d7vk, dxmt, dxvk, manifest, paths, runner, steam, vkd3d, wine};
use std::path::{Path, PathBuf};

#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum SetupAction {
    /// Prepare Nucleon directories, runtime and Steamworks bridge
    Fix,
}

#[derive(Debug, Default, Clone)]
pub struct SetupArgs {
    pub action: Option<SetupAction>,
    pub non_interactive: bool,
    pub force: bool,
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

impl SetupArgs {
    pub fn has_remediation_flags(&self) -> bool {
        self.force
            || self.kosmickrisp_path.is_some()
            || self.kosmickrisp_version.is_some()
            || self.fetch_vkd3d
            || self.vkd3d_path.is_some()
            || self.fetch_d7vk
            || self.d7vk_path.is_some()
            || self.fetch_dxvk
            || self.dxvk_path.is_some()
            || self.fetch_dxmt
            || self.dxmt_path.is_some()
            || self.wine.is_some()
            || self.wine_path.is_some()
            || self.bridge_path.is_some()
    }
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
        required: false,
        details: if steam_installed {
            paths::steam_app().display().to_string()
        } else {
            "Steam.app not found in /Applications".to_string()
        },
        remediation: "Install Steam for macOS from https://store.steampowered.com/about/."
            .to_string(),
    });

    // 3. Wine Runtime
    let active_wine = wine::get_active_wine_runtime();
    let discovered_wine = wine::discover_wine_runtimes();
    let wine_ok = active_wine.is_some() || !discovered_wine.is_empty();
    let wine_details = if let Some(w) = active_wine {
        format!(
            "{} [{}]",
            w.name,
            w.version.as_deref().unwrap_or("detected")
        )
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

    // 4. Apple GPTK Components (User-supplied)
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
        name: "Apple GPTK (User-Supplied)",
        passed: gptk_ok,
        required: false,
        details: gptk_details,
        remediation: gptk_remedy,
    });

    // 4b. Whether the assembled runner's Wine implements what Apple's D3DMetal binaries import.
    // Wine stubs unresolved imports and only warns, so an incapable runtime otherwise shows up as a
    // game that dies at startup with no mention of the runtime being the cause.
    let (cap_ok, cap_details) = match runner::describe_d3dmetal_support(&paths::current_runner()) {
        Ok((supported, details)) => (supported, details),
        Err(err) => (false, format!("Could not inspect runner: {err:#}")),
    };
    checks.push(PrerequisiteCheck {
        name: "GPTK Runtime Capability",
        passed: gptk_ok && cap_ok,
        required: false,
        details: if gptk_ok {
            cap_details
        } else {
            "Skipped (no GPTK components configured)".to_string()
        },
        remediation: "Apple's D3DMetal binaries import entry points implemented by Wine itself. \
                      Select a Wine build that implements them ('nucleon wine use <name>' then \
                      'nucleon setup fix'), or run the title on DXMT/KosmicKrisp instead."
            .to_string(),
    });

    // Assembled runtime
    let runner_ok = paths::current_runner().exists();
    let runner_details = if paths::current_runner().exists() {
        paths::current_runner().display().to_string()
    } else {
        "Not assembled".to_string()
    };
    checks.push(PrerequisiteCheck {
        name: "Runtime",
        passed: runner_ok,
        required: false,
        details: runner_details,
        remediation: "Run 'nucleon setup fix' to assemble the Nucleon runtime.".to_string(),
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

/// Prepares Nucleon directories, bridge and runtime without changing Steam.
pub fn run_fix(args: &SetupArgs) -> Result<()> {
    let _ = args.non_interactive;
    ui::header("Remediating Nucleon Prerequisites ('nucleon setup fix')...");

    // 1. Ensure required support directories
    paths::ensure_dirs()?;
    ui::success("Ensured required support directories exist");

    // 2. Custom path configuration if provided in flags
    if let Some(ref gp) = args.gptk_path {
        let (fw, shared) = runner::set_custom_gptk_path(gp)?;
        ui::success(format!(
            "Registered custom Apple GPTK path at {}",
            gp.display()
        ));
        ui::tree_kv("└─", "D3DMetal.framework:", fw.display());
        ui::tree_kv("└─", "libd3dshared.dylib:", shared.display());
    }

    if let Some(ref wp) = args.wine_path {
        let rt = wine::set_custom_wine_path(wp)?;
        ui::success(format!(
            "Registered custom Wine runtime at {}",
            rt.root.display()
        ));
        wine::set_active_wine(&rt.id)?;
    }

    if let Some(ref kp) = args.kosmickrisp_path {
        let info = runner::set_custom_kosmickrisp_path(kp, args.kosmickrisp_version.as_deref())?;
        ui::success(format!(
            "Registered custom KosmicKrisp at {}",
            info.icd_path.display()
        ));
    }

    if let Some(ref desired_wine) = args.wine {
        let rt = wine::set_active_wine(desired_wine)?;
        ui::success(format!(
            "Selected active Wine runtime: {} ({})",
            rt.name,
            rt.root.display()
        ));
    }

    if let Some(ref p) = args.vkd3d_path {
        let info = vkd3d::set_custom_vkd3d_proton_path(p)?;
        ui::success(format!(
            "Registered custom VKD3D-Proton at {}",
            info.root.display()
        ));
    } else if args.fetch_vkd3d {
        let info = vkd3d::fetch_vkd3d_proton(None, None)?;
        ui::success(format!("Fetched VKD3D-Proton to {}", info.root.display()));
    }

    if let Some(ref p) = args.d7vk_path {
        let info = d7vk::set_custom_d7vk_path(p)?;
        ui::success(format!("Registered custom D7VK at {}", info.root.display()));
    } else if args.fetch_d7vk {
        let info = d7vk::fetch_d7vk(None, None)?;
        ui::success(format!("Fetched D7VK to {}", info.root.display()));
    }

    if let Some(ref p) = args.dxvk_path {
        let info = dxvk::set_custom_dxvk_path(p)?;
        ui::success(format!("Registered custom DXVK at {}", info.root.display()));
    } else if args.fetch_dxvk {
        let info = dxvk::fetch_dxvk(None, None)?;
        ui::success(format!("Fetched DXVK to {}", info.root.display()));
    }

    if let Some(ref p) = args.dxmt_path {
        let info = dxmt::set_custom_dxmt_path(p)?;
        ui::success(format!("Registered custom DXMT at {}", info.root.display()));
    } else if args.fetch_dxmt {
        let info = dxmt::fetch_dxmt(None, None)?;
        ui::success(format!("Fetched DXMT to {}", info.root.display()));
    }

    // 4. Stage Valve bridge packages
    match manifest::stage_valve_bridge(args.bridge_path.as_deref()) {
        Ok(()) => ui::success("Valve client bridge libraries staged"),
        Err(e) if args.bridge_path.is_some() => return Err(e),
        Err(e) => ui::warn(format!("Failed to stage bridge packages: {e}")),
    }

    // Assemble the Nucleon runtime if components are resolvable
    let gptk_comps = runner::find_gptk_components(args.gptk_path.as_deref())
        .ok()
        .flatten();
    let wine_rt = wine::get_active_wine_runtime();

    if gptk_comps.is_some() || wine_rt.is_some() {
        match runner::assemble_runner(
            args.force,
            args.wine_path.as_deref(),
            args.gptk_path.as_deref(),
        ) {
            Ok(runner_path) => {
                ui::success(format!("Runner assembled at: {}", runner_path.display()));
            }
            Err(e) => {
                ui::warn(format!("Could not assemble runner during fix: {e}"));
            }
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

    if args.has_remediation_flags() {
        bail!(
            "Setup remediation flags require 'nucleon setup fix' (for example: 'nucleon setup fix --bridge-path <DIR>')"
        );
    }

    // Default flow: idempotent, non-interactive check
    let checks = check_prerequisites(args.gptk_path.as_deref());
    print_prerequisites_table(&checks);

    let failed_required: Vec<&PrerequisiteCheck> =
        checks.iter().filter(|c| c.required && !c.passed).collect();

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
