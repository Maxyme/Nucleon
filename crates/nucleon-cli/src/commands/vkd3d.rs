use super::ui;
use anyhow::Result;
use clap::Subcommand;
use nucleon_core::{paths, vkd3d};
use std::path::PathBuf;

#[derive(Subcommand, Debug, Clone)]
pub enum Vkd3dAction {
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

pub fn run(action: Vkd3dAction) -> Result<()> {
    eprintln!("Notice: 'nucleon vkd3d' is deprecated; use 'nucleon backends vkd3d' instead.\n");
    execute(action)
}

pub fn execute(action: Vkd3dAction) -> Result<()> {
    match action {
        Vkd3dAction::Status => {
            ui::header("VKD3D-Proton Status (Direct3D 12 -> Vulkan 1.4 for KosmicKrisp)");
            if let Some(bundle) = vkd3d::find_vkd3d_proton() {
                ui::kv("Status:", "✓ Installed / Detected");
                ui::kv("Location:", bundle.root.display());
                ui::kv("Version:", bundle.version.as_deref().unwrap_or("Unknown"));
                ui::kv("64-bit d3d12.dll:", bundle.x64_d3d12.display());
                if let Some(ref core) = bundle.x64_d3d12core {
                    ui::kv("64-bit d3d12core:", core.display());
                }
                if let Some(ref x86) = bundle.x86_d3d12 {
                    ui::kv("32-bit d3d12.dll:", x86.display());
                }
            } else {
                ui::kv("Status:", "○ Not installed / Not detected");
                ui::kv("Managed Path:", paths::vkd3d_proton_dir().display());
                println!(
                    r#"
To install or configure VKD3D-Proton without committing binaries:
  1. Extract official release archive (.tar.zst) from GitHub releases
  2. Point to extracted directory: nucleon vkd3d set-path /path/to/extracted/vkd3d-proton
  3. Or stage directly into:       {}
  4. Or set environment variable:  export VKD3D_PROTON_PATH=/path/to/extracted/vkd3d-proton"#,
                    paths::vkd3d_proton_dir().display()
                );
            }
        }
        Vkd3dAction::Fetch { version } => {
            let ver_str = version.as_deref().unwrap_or(vkd3d::DEFAULT_VKD3D_VERSION);
            ui::header(format!(
                "Fetching official VKD3D-Proton {} from GitHub...",
                ver_str
            ));
            let bundle = vkd3d::fetch_vkd3d_proton(version.as_deref(), None)?;
            ui::success(format!(
                "Successfully staged VKD3D-Proton at {}",
                bundle.root.display()
            ));
            ui::success(format!(
                "64-bit Direct3D 12 DLL: {}",
                bundle.x64_d3d12.display()
            ));
            if let Some(ref core) = bundle.x64_d3d12core {
                ui::success(format!("64-bit D3D12Core DLL:   {}", core.display()));
            }
            println!(
                "\nGames running under 'Nucleon (Wine + Mesa KosmicKrisp Vulkan)' will now translate Direct3D 12 -> Vulkan 1.4."
            );
        }
        Vkd3dAction::SetPath { path } => {
            ui::header(format!(
                "Registering custom VKD3D-Proton path: {}",
                path.display()
            ));
            let bundle = vkd3d::set_custom_vkd3d_proton_path(&path)?;
            ui::success(format!(
                "Validated and registered VKD3D-Proton at {}",
                bundle.root.display()
            ));
            ui::success(format!(
                "64-bit Direct3D 12 DLL: {}",
                bundle.x64_d3d12.display()
            ));
        }
        Vkd3dAction::ClearPath => {
            vkd3d::clear_custom_vkd3d_proton_path()?;
            ui::success("Cleared custom VKD3D-Proton path override");
        }
    }

    Ok(())
}
