use super::ui;
use anyhow::Result;
use clap::Subcommand;
use nucleon_core::runner;
use std::fs;
use std::path::PathBuf;

#[derive(Subcommand, Debug, Clone)]
pub enum GptkAction {
    /// Inspect Apple Game Porting Toolkit detection, components, and runner status
    Status,
    /// Point Nucleon to an existing Apple GPTK components directory
    SetPath {
        /// Path to directory containing D3DMetal.framework and libd3dshared.dylib
        path: PathBuf,
    },
    /// Clear configured custom Apple GPTK path
    ClearPath,
}

pub fn run(action: GptkAction) -> Result<()> {
    match action {
        GptkAction::Status => {
            ui::header("Apple Game Porting Toolkit (GPTK) Status");
            let custom_file = runner::custom_gptk_path_file();
            if custom_file.is_file() {
                if let Ok(c) = fs::read_to_string(&custom_file) {
                    ui::kv("Custom path configured:", format!("✓ {}", c.trim()));
                }
            } else {
                ui::kv("Custom path configured:", "○ None (auto-discovery active)");
            }

            if let Some(runner_path) = runner::find_gptk_runner() {
                ui::kv(
                    "Assembled GPTK runner:",
                    format!("✓ {}", runner_path.display()),
                );
            } else {
                ui::kv(
                    "Assembled GPTK runner:",
                    "○ Not assembled (run 'nucleon setup')",
                );
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
        GptkAction::ClearPath => {
            runner::clear_custom_gptk_path()?;
            ui::success("Cleared custom Apple GPTK path override.");
        }
    }

    Ok(())
}
