use super::ui;
use anyhow::Result;
use clap::Subcommand;
use nucleon_core::{d7vk, paths};
use std::path::PathBuf;

#[derive(Subcommand, Debug, Clone)]
pub enum D7vkAction {
    /// Inspect D7VK detection, version, and DLL locations
    Status,
    /// Fetch official D7VK release binaries from GitHub without tracking in git
    Fetch {
        /// Specific version tag (default: latest, e.g. v2.3)
        #[arg(short, long)]
        version: Option<String>,
    },
    /// Point Nucleon to an existing D7VK installation directory
    SetPath {
        /// Path to directory containing ddraw.dll or x32/ddraw.dll
        path: PathBuf,
    },
    /// Clear custom configured D7VK path
    ClearPath,
}

pub fn run(action: D7vkAction) -> Result<()> {
    match action {
        D7vkAction::Status => {
            ui::header("D7VK Status (DirectDraw / Direct3D 1-7 -> Vulkan 1.4 for KosmicKrisp)");
            if let Some(bundle) = d7vk::find_d7vk() {
                ui::kv("Status:", "✓ Installed / Detected");
                ui::kv("Location:", bundle.root.display());
                ui::kv("Version:", bundle.version.as_deref().unwrap_or("Unknown"));
                ui::kv("32-bit ddraw.dll:", bundle.x86_ddraw.display());
            } else {
                ui::kv("Status:", "○ Not installed / Not detected");
                ui::kv("Managed Path:", paths::d7vk_dir().display());
                println!(
                    r#"
To install or configure D7VK without committing binaries:
  1. Fetch release archive (.zip) from GitHub:  nucleon d7vk fetch
  2. Or point to extracted directory:           nucleon d7vk set-path /path/to/extracted/d7vk
  3. Or stage directly into:                    {}
  4. Or set environment variable:               export D7VK_PATH=/path/to/extracted/d7vk"#,
                    paths::d7vk_dir().display()
                );
            }
        }
        D7vkAction::Fetch { version } => {
            let ver_str = version.as_deref().unwrap_or(d7vk::DEFAULT_D7VK_VERSION);
            ui::header(format!("Fetching official D7VK {ver_str} from GitHub..."));
            let bundle = d7vk::fetch_d7vk(version.as_deref(), None)?;
            ui::success(format!(
                "Successfully staged D7VK at {}",
                bundle.root.display()
            ));
            ui::success(format!(
                "32-bit DirectDraw DLL: {}",
                bundle.x86_ddraw.display()
            ));
            println!("\nGames running under 'Nucleon (KosmicKrisp)' will now translate DirectDraw / Direct3D 1-7 -> Vulkan 1.4.");
        }
        D7vkAction::SetPath { path } => {
            ui::header(format!("Registering custom D7VK path: {}", path.display()));
            let bundle = d7vk::set_custom_d7vk_path(&path)?;
            ui::success(format!(
                "Validated and registered D7VK at {}",
                bundle.root.display()
            ));
            ui::success(format!(
                "32-bit DirectDraw DLL: {}",
                bundle.x86_ddraw.display()
            ));
        }
        D7vkAction::ClearPath => {
            d7vk::clear_custom_d7vk_path()?;
            ui::success("Cleared custom D7VK path override");
        }
    }

    Ok(())
}
