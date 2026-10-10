use super::ui;
use anyhow::Result;
use clap::Subcommand;
use nucleon_core::{dxvk, paths};
use std::path::PathBuf;

#[derive(Subcommand, Debug, Clone)]
pub enum DxvkAction {
    /// Inspect DXVK detection, version, and DLL locations
    Status,
    /// Fetch official DXVK release binaries from GitHub without tracking in git
    Fetch {
        /// Specific version tag (default: latest, e.g. v2.4.1)
        #[arg(short, long)]
        version: Option<String>,
    },
    /// Point Nucleon to an existing DXVK installation directory
    SetPath {
        /// Path to directory containing d3d11.dll or x64/d3d11.dll
        path: PathBuf,
    },
    /// Clear custom configured DXVK path
    ClearPath,
}

pub fn run(action: DxvkAction) -> Result<()> {
    eprintln!("Notice: 'nucleon dxvk' is deprecated; use 'nucleon backends dxvk' instead.\n");
    execute(action)
}

pub fn execute(action: DxvkAction) -> Result<()> {
    match action {
        DxvkAction::Status => {
            ui::header("DXVK Status (Direct3D 9/10/11 -> Vulkan 1.4 for KosmicKrisp)");
            if let Some(bundle) = dxvk::find_dxvk() {
                ui::kv("Status:", "✓ Installed / Detected");
                ui::kv("Location:", bundle.root.display());
                ui::kv("Version:", bundle.version.as_deref().unwrap_or("Unknown"));
                if let Some(ref p) = bundle.x64_d3d11 {
                    ui::kv("64-bit d3d11.dll:", p.display());
                }
                if let Some(ref p) = bundle.x64_dxgi {
                    ui::kv("64-bit dxgi.dll:", p.display());
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
                ui::kv("Managed Path:", paths::dxvk_dir().display());
                println!(
                    r#"
To install or configure DXVK without committing binaries:
  1. Fetch release archive from GitHub:          nucleon dxvk fetch
  2. Or point to extracted directory:           nucleon dxvk set-path /path/to/extracted/dxvk
  3. Or stage directly into:                    {}
  4. Or set environment variable:               export DXVK_PATH=/path/to/extracted/dxvk"#,
                    paths::dxvk_dir().display()
                );
            }
        }
        DxvkAction::Fetch { version } => {
            let ver_str = version.as_deref().unwrap_or(dxvk::DEFAULT_DXVK_VERSION);
            ui::header(format!("Fetching official DXVK {ver_str} from GitHub..."));
            let bundle = dxvk::fetch_dxvk(version.as_deref(), None)?;
            ui::success(format!(
                "Successfully staged DXVK at {}",
                bundle.root.display()
            ));
            ui::success(format!(
                "Detected {} translation DLL(s)",
                bundle.dll_count()
            ));
            println!(
                "\nGames running under 'Nucleon (Wine + Mesa KosmicKrisp Vulkan)' will now translate Direct3D 9/10/11 -> Vulkan 1.4."
            );
        }
        DxvkAction::SetPath { path } => {
            ui::header(format!("Registering custom DXVK path: {}", path.display()));
            let bundle = dxvk::set_custom_dxvk_path(&path)?;
            ui::success(format!(
                "Validated and registered DXVK at {}",
                bundle.root.display()
            ));
            ui::success(format!(
                "Detected {} translation DLL(s)",
                bundle.dll_count()
            ));
        }
        DxvkAction::ClearPath => {
            dxvk::clear_custom_dxvk_path()?;
            ui::success("Cleared custom DXVK path override");
        }
    }

    Ok(())
}
