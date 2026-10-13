use super::ui;
use anyhow::Result;
use clap::Subcommand;
use nucleon_core::{dxmt, paths};
use std::path::PathBuf;

#[derive(Subcommand, Debug, Clone)]
pub enum DxmtAction {
    /// Inspect DXMT detection, version, and DLL locations
    Status,
    /// Fetch official DXMT release binaries from GitHub (https://github.com/3Shain/dxmt) without tracking in git
    Fetch {
        /// Specific version tag (default: latest, e.g. v0.80)
        #[arg(short, long)]
        version: Option<String>,
    },
    /// Point Nucleon to an existing DXMT installation directory
    SetPath {
        /// Path to directory containing d3d11.dll or x86_64-windows/d3d11.dll
        path: PathBuf,
    },
    /// Clear custom configured DXMT path
    ClearPath,
}

pub fn run(action: DxmtAction) -> Result<()> {
    match action {
        DxmtAction::Status => {
            ui::header("DXMT Status (Direct3D 11 -> Apple Metal - https://github.com/3Shain/dxmt)");
            if let Some(bundle) = dxmt::find_dxmt() {
                ui::kv("Status:", "✓ Installed / Detected");
                ui::kv("Location:", bundle.root.display());
                ui::kv("Version:", bundle.version.as_deref().unwrap_or("Unknown"));
                if let Some(ref p) = bundle.x64_d3d11 {
                    ui::kv("64-bit d3d11.dll:", p.display());
                }
                if let Some(ref p) = bundle.x64_d3d10core {
                    ui::kv("64-bit d3d10core.dll:", p.display());
                }
                if let Some(ref p) = bundle.x64_dxgi {
                    ui::kv("64-bit dxgi.dll:", p.display());
                }
                if let Some(ref p) = bundle.x64_winemetal {
                    ui::kv("64-bit winemetal.dll:", p.display());
                }
                if let Some(ref p) = bundle.x64_winemetal_so {
                    ui::kv("Unix winemetal.so:", p.display());
                }
                if let Some(ref p) = bundle.x86_d3d11 {
                    ui::kv("32-bit d3d11.dll:", p.display());
                }
                if let Some(ref p) = bundle.x86_dxgi {
                    ui::kv("32-bit dxgi.dll:", p.display());
                }
                ui::kv("Total DLLs:", bundle.dll_count());
            } else {
                ui::kv("Status:", "○ Not installed / Not detected");
                ui::kv("Managed Path:", paths::dxmt_dir().display());
                println!(
                    r#"
To install or configure DXMT (https://github.com/3Shain/dxmt):
  1. Fetch release archive from GitHub:          nucleon dxmt fetch
  2. Or point to extracted directory:           nucleon dxmt set-path /path/to/extracted/dxmt
  3. Or stage directly into:                    {}
  4. Or set environment variable:               export DXMT_PATH=/path/to/extracted/dxmt"#,
                    paths::dxmt_dir().display()
                );
            }
        }
        DxmtAction::Fetch { version } => {
            let ver_str = version.as_deref().unwrap_or(dxmt::DEFAULT_DXMT_VERSION);
            ui::header(format!(
                "Fetching official DXMT {ver_str} from GitHub (https://github.com/3Shain/dxmt)..."
            ));
            let bundle = dxmt::fetch_dxmt(version.as_deref(), None)?;
            ui::success(format!(
                "Successfully staged DXMT at {}",
                bundle.root.display()
            ));
            ui::success(format!(
                "Detected {} translation DLL(s)",
                bundle.dll_count()
            ));
            println!(
                "\nGames running under 'Nucleon (Wine + DXMT (DX11) + Apple Metal)' will now translate Direct3D 11 directly to Apple Metal."
            );
        }
        DxmtAction::SetPath { path } => {
            ui::header(format!("Registering custom DXMT path: {}", path.display()));
            let bundle = dxmt::set_custom_dxmt_path(&path)?;
            ui::success(format!(
                "Validated and registered DXMT at {}",
                bundle.root.display()
            ));
            ui::success(format!(
                "Detected {} translation DLL(s)",
                bundle.dll_count()
            ));
        }
        DxmtAction::ClearPath => {
            dxmt::clear_custom_dxmt_path()?;
            ui::success("Cleared custom DXMT path override");
        }
    }

    Ok(())
}
