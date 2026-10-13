use super::ui;
use anyhow::Result;
use clap::Subcommand;
use nucleon_core::{gptk_abi, runner, wine};
use std::fs;
use std::path::PathBuf;

#[derive(Subcommand, Debug, Clone)]
pub enum GptkAction {
    /// Assemble an immutable GPTK runtime from the selected Wine and configured GPTK components
    Assemble {
        #[arg(long)]
        force: bool,
    },
    /// Inspect Apple Game Porting Toolkit detection, components, and runner status
    Status,
    /// Point Nucleon to an existing Apple GPTK components directory
    SetPath {
        /// Path to directory containing D3DMetal.framework and libd3dshared.dylib
        path: PathBuf,
    },
    /// Check a GPTK folder's Windows binaries against the active Wine runtime without staging anything
    Verify {
        /// Path to an unpacked GPTK distribution (its `redist/lib/wine` tree is inspected)
        path: PathBuf,
    },
    /// Clear configured custom Apple GPTK path
    ClearPath,
}

pub fn run(action: GptkAction) -> Result<()> {
    match action {
        GptkAction::Assemble { force } => {
            let path = runner::assemble_runner(force, None, None)?;
            ui::success(format!("Assembled GPTK runtime: {}", path.display()));
        }
        GptkAction::Status => {
            ui::header("Apple Game Porting Toolkit (GPTK) Status");
            let custom_file = runner::custom_gptk_path_file();
            if custom_file.is_file() {
                if let Ok(c) = fs::read_to_string(&custom_file) {
                    ui::kv("Custom path configured:", format!("✓ {}", c.trim()));
                }
            } else {
                ui::kv(
                    "Custom path configured:",
                    "○ None — point Nucleon at your GPTK folder: nucleon gptk set-path <DIR>",
                );
            }

            if let Some(runner_path) = runner::find_gptk_runner() {
                ui::kv(
                    "Assembled GPTK runner:",
                    format!("✓ {}", runner_path.display()),
                );
                match runner::runner_source_stamp(&runner_path) {
                    Some(stamp) => {
                        ui::kv("Assembled from Wine:", stamp.wine_root);
                        if stamp.abi_revision == gptk_abi::ABI_REVISION {
                            ui::kv(
                                "GPTK staging rules:",
                                format!("✓ revision {}", stamp.abi_revision),
                            );
                        } else {
                            ui::kv(
                                "GPTK staging rules:",
                                format!(
                                    "⚠ runner staged by revision {}, this build uses {}; run `nucleon setup`",
                                    stamp.abi_revision,
                                    gptk_abi::ABI_REVISION
                                ),
                            );
                        }
                    }
                    None => ui::kv(
                        "Assembled from:",
                        "○ unknown — `nucleon setup` will record and refresh it",
                    ),
                }
            } else {
                ui::kv(
                    "Assembled GPTK runner:",
                    "○ Not assembled (run 'nucleon setup')",
                );
            }

            match wine::get_active_wine_runtime() {
                Some(runtime) => {
                    let report = match gptk_abi::detect_wine_unix_call_abi(&runtime.root) {
                        Some(abi) => format!("✓ {abi} — GPTK binaries are staged to bind to it"),
                        None => "○ undetermined — GPTK binaries are staged unmodified".to_string(),
                    };
                    ui::kv("Wine Unix-call ABI:", report);
                }
                None => ui::kv("Wine Unix-call ABI:", "○ no active Wine runtime"),
            }

            match runner::find_gptk_components(None) {
                Ok(Some((fw, shared))) => {
                    ui::kv("D3DMetal.framework:", format!("✓ {}", fw.display()));
                    ui::kv("libd3dshared.dylib:", format!("✓ {}", shared.display()));
                }
                Ok(None) => {
                    ui::kv("GPTK Components:", "✗ Not found");
                    println!("  To configure custom GPTK:   nucleon gptk set-path /path/to/gptk");
                }
                Err(e) => {
                    ui::kv("GPTK Components error:", format!("✗ {:#}", e));
                }
            }
        }
        GptkAction::SetPath { path } => {
            ui::header(format!("Registering custom GPTK path: {}", path.display()));
            let (fw, shared) = runner::set_custom_gptk_path(&path)?;
            ui::success(format!(
                "Validated and registered Apple GPTK components at {}",
                path.display()
            ));
            ui::success(format!("D3DMetal.framework: {}", fw.display()));
            ui::success(format!("libd3dshared.dylib: {}", shared.display()));
            println!("\nRun 'nucleon setup' to assemble or refresh the runner with this GPTK.");
        }
        GptkAction::Verify { path } => {
            ui::header(format!("Verifying GPTK binaries in {}", path.display()));
            let host = wine::get_active_wine_runtime()
                .as_ref()
                .and_then(|runtime| gptk_abi::detect_wine_unix_call_abi(&runtime.root));
            match host {
                Some(abi) => ui::kv("Active Wine exposes:", abi.to_string()),
                None => ui::kv(
                    "Active Wine exposes:",
                    "undetermined — binaries would be staged unmodified; pairing is unverified",
                ),
            }
            let (framework, shared) = runner::validate_and_resolve_gptk_components(&path)?;
            let Some(windows_dir) = runner::gptk_windows_binaries_dir(&framework, &shared) else {
                println!(
                    "  ○ {} contains no x86_64-windows binaries to check",
                    path.display()
                );
                return Ok(());
            };
            let report = gptk_abi::inspect_windows_tree(&windows_dir, host)?;
            for (name, bridge) in &report.bridged {
                ui::success(format!(
                    "{name}: retargeted ({} symbol(s), {} init thunk(s))",
                    bridge.symbol_rewrites, bridge.init_sites_patched
                ));
            }
            if host.is_none() {
                ui::warn(format!(
                    "Inspected {} file(s); the active Wine Unix-call ABI could not be determined, so staging would preserve them unchanged and the pairing is unverified.",
                    report.passthrough
                ));
            } else if report.bridged.is_empty() {
                ui::success(format!(
                    "No GPTK files require retargeting for this Wine; {} file(s) would be copied unchanged",
                    report.passthrough
                ));
            }
        }
        GptkAction::ClearPath => {
            runner::clear_custom_gptk_path()?;
            ui::success("Cleared custom Apple GPTK path override.");
        }
    }

    Ok(())
}
