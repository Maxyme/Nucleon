use super::{d7vk, kosmickrisp, ui, vkd3d};
use anyhow::Result;
use clap::Subcommand;
use nucleon_core::runner;

#[derive(Subcommand, Debug, Clone)]
pub enum BackendsAction {
    /// Inspect status of configured Wine graphical backends (KosmicKrisp, VKD3D-Proton, D7VK)
    Status,
    /// Manage Mesa KosmicKrisp Vulkan driver and custom paths
    Kosmickrisp {
        #[command(subcommand)]
        action: kosmickrisp::KosmickrispAction,
    },
    /// Manage VKD3D-Proton (Direct3D 12 -> Vulkan 1.4) translation layer
    Vkd3d {
        #[command(subcommand)]
        action: vkd3d::Vkd3dAction,
    },
    /// Manage D7VK (DirectDraw / Direct3D 1-7 -> Vulkan 1.4) translation layer
    D7vk {
        #[command(subcommand)]
        action: d7vk::D7vkAction,
    },
}

pub fn run(action: BackendsAction) -> Result<()> {
    match action {
        BackendsAction::Status => {
            ui::header("Wine Graphics Translation Backends Status");

            // 1. Mesa KosmicKrisp
            ui::kv("Mesa KosmicKrisp (Vulkan 1.4):", "");
            if let Some(info) = runner::get_kosmickrisp_info() {
                ui::tree_kv("├─", "Status:", "✓ Detected / Active");
                ui::tree_kv("├─", "API Version:", &info.api_version);
                ui::tree_kv(
                    "└─",
                    "ICD Manifest:",
                    format!("{}", info.icd_path.display()),
                );
            } else {
                ui::tree_kv(
                    "└─",
                    "Status:",
                    "○ Optional (not detected; install LunarG Vulkan SDK)",
                );
            }

            // 2. VKD3D-Proton
            ui::kv("VKD3D-Proton (D3D12 -> Vulkan):", "");
            if let Some(bundle) = nucleon_core::vkd3d::find_vkd3d_proton() {
                ui::tree_kv("├─", "Status:", "✓ Installed");
                ui::tree_kv(
                    "├─",
                    "Version:",
                    bundle.version.as_deref().unwrap_or("Unknown"),
                );
                ui::tree_kv("└─", "Location:", format!("{}", bundle.root.display()));
            } else {
                ui::tree_kv(
                    "└─",
                    "Status:",
                    "○ Optional (run 'nucleon wine backends vkd3d fetch')",
                );
            }

            // 3. D7VK
            ui::kv("D7VK (DirectDraw / DX1-7 -> Vulkan):", "");
            if let Some(bundle) = nucleon_core::d7vk::find_d7vk() {
                ui::tree_kv("├─", "Status:", "✓ Installed");
                ui::tree_kv(
                    "├─",
                    "Version:",
                    bundle.version.as_deref().unwrap_or("Unknown"),
                );
                ui::tree_kv("└─", "Location:", format!("{}", bundle.root.display()));
            } else {
                ui::tree_kv(
                    "└─",
                    "Status:",
                    "○ Optional (run 'nucleon wine backends d7vk fetch')",
                );
            }

            println!("\nTo manage individual Wine backends: nucleon wine backends <kosmickrisp|vkd3d|d7vk> --help");
            println!("Note: Apple GPTK 4 is an independent runner & D3DMetal backend (manage with: nucleon gptk --help)");
        }
        BackendsAction::Kosmickrisp { action } => kosmickrisp::handle(action)?,
        BackendsAction::Vkd3d { action } => vkd3d::run(action)?,
        BackendsAction::D7vk { action } => d7vk::run(action)?,
    }

    Ok(())
}
