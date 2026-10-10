use super::ui;
use anyhow::Result;
use clap::Subcommand;
use nucleon_core::runner;
use std::fs;
use std::path::PathBuf;

#[derive(Subcommand, Debug, Clone)]
pub enum KosmickrispAction {
    /// Show current Mesa KosmicKrisp driver configuration and detected status
    Status,
    /// Point Nucleon to a custom KosmicKrisp ICD manifest, driver dylib, or installation directory
    SetPath {
        /// Path to libkosmickrisp_icd.json, libvulkan_kosmickrisp.dylib, or driver directory
        path: PathBuf,
        /// Optional Vulkan API version override (e.g. "1.4.0", "1.4.304")
        #[arg(short, long)]
        version: Option<String>,
    },
    /// Clear configured custom KosmicKrisp path override
    ClearPath,
}

pub fn handle(action: KosmickrispAction) -> Result<()> {
    eprintln!("Notice: 'nucleon kosmickrisp' is deprecated; use 'nucleon backends kosmickrisp' instead.\n");
    execute(action)
}

pub fn execute(action: KosmickrispAction) -> Result<()> {
    match action {
        KosmickrispAction::Status => {
            ui::header("Mesa KosmicKrisp Driver Status");

            let custom_file = runner::custom_kosmickrisp_path_file();
            if custom_file.is_file() {
                if let Ok(c) = fs::read_to_string(&custom_file) {
                    println!("  Custom Path Override:  {}", c.trim());
                }
            } else {
                println!("  Custom Path Override:  (none)");
            }

            if let Some(info) = runner::get_kosmickrisp_info() {
                println!(
                    "  Vulkan API Version:    {}\n  ICD Manifest:          {}\n  Driver Library:        {}\n  Custom Active:         {}\n  Driver Installed:      ✓ Yes",
                    info.api_version,
                    info.icd_path.display(),
                    info.library_path.display(),
                    if info.is_custom { "Yes" } else { "No" }
                );
            } else {
                println!(
                    "  Driver Installed:      ✗ Not detected\n\n  To configure custom KosmicKrisp: nucleon kosmickrisp set-path /path/to/driver"
                );
            }
        }
        KosmickrispAction::SetPath { path, version } => {
            ui::header(format!(
                "Registering custom KosmicKrisp path: {}",
                path.display()
            ));
            let info = runner::set_custom_kosmickrisp_path(&path, version.as_deref())?;
            ui::success(format!(
                "Registered custom KosmicKrisp (Vulkan {}):\n  ICD Manifest:   {}\n  Driver Library: {}",
                info.api_version,
                info.icd_path.display(),
                info.library_path.display()
            ));
            println!(
                "\nGames running under 'Nucleon (Wine + Mesa KosmicKrisp Vulkan)' will now use this driver."
            );
        }
        KosmickrispAction::ClearPath => {
            runner::clear_custom_kosmickrisp_path()?;
            ui::success("Cleared custom KosmicKrisp path override.");
        }
    }

    Ok(())
}
