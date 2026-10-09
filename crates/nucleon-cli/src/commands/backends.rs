use super::{d7vk, dxmt, dxvk, kosmickrisp, ui, vkd3d};
use anyhow::Result;
use clap::Subcommand;
use nucleon_core::runner;

#[derive(Subcommand, Debug, Clone)]
pub enum BackendsAction {
    /// Inspect status of configured Wine graphical backends (KosmicKrisp, VKD3D-Proton, D7VK, DXVK, DXMT)
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
    /// Manage DXVK (Direct3D 9/10/11 -> Vulkan 1.4) translation layer
    Dxvk {
        #[command(subcommand)]
        action: dxvk::DxvkAction,
    },
    /// Manage DXMT (Direct3D 11 -> Apple Metal) translation layer
    Dxmt {
        #[command(subcommand)]
        action: dxmt::DxmtAction,
    },
}

pub fn run(action: BackendsAction) -> Result<()> {
    match action {
        BackendsAction::Status => {
            ui::header("Wine Graphics Translation Stack Status");
            println!(
                "Wine executes games using layered graphics translation: Direct3D -> Vulkan -> Apple Metal.\n"
            );

            // Layer 1: Direct3D Translation Layers
            ui::kv("[1] Direct3D Translation Layers (Direct3D -> Vulkan):", "");

            // DXVK
            ui::tree_kv("  ├─", "DXVK (Direct3D 9/10/11 -> Vulkan):", "");
            if let Some(bundle) = nucleon_core::dxvk::find_dxvk() {
                ui::tree_kv("  │   ├─", "Status:", "✓ Installed");
                ui::tree_kv(
                    "  │   ├─",
                    "Version:",
                    bundle.version.as_deref().unwrap_or("Detected"),
                );
                ui::tree_kv("  │   └─", "Location:", bundle.root.display().to_string());
            } else {
                ui::tree_kv(
                    "  │   └─",
                    "Status:",
                    "○ Optional (run 'nucleon wine backends dxvk fetch')",
                );
            }

            // VKD3D-Proton
            ui::tree_kv("  ├─", "VKD3D-Proton (Direct3D 12 -> Vulkan):", "");
            if let Some(bundle) = nucleon_core::vkd3d::find_vkd3d_proton() {
                ui::tree_kv("  │   ├─", "Status:", "✓ Installed");
                ui::tree_kv(
                    "  │   ├─",
                    "Version:",
                    bundle.version.as_deref().unwrap_or("Detected"),
                );
                ui::tree_kv("  │   └─", "Location:", bundle.root.display().to_string());
            } else {
                ui::tree_kv(
                    "  │   └─",
                    "Status:",
                    "○ Optional (run 'nucleon wine backends vkd3d fetch')",
                );
            }

            // D7VK
            ui::tree_kv("  ├─", "D7VK (DirectDraw / Direct3D 1-7 -> Vulkan):", "");
            if let Some(bundle) = nucleon_core::d7vk::find_d7vk() {
                ui::tree_kv("  │   ├─", "Status:", "✓ Installed");
                ui::tree_kv(
                    "  │   ├─",
                    "Version:",
                    bundle.version.as_deref().unwrap_or("Detected"),
                );
                ui::tree_kv("  │   └─", "Location:", bundle.root.display().to_string());
            } else {
                ui::tree_kv(
                    "  │   └─",
                    "Status:",
                    "○ Optional (run 'nucleon wine backends d7vk fetch')",
                );
            }

            // DXMT
            ui::tree_kv("  └─", "DXMT (Direct3D 11 -> Apple Metal):", "");
            if let Some(bundle) = nucleon_core::dxmt::find_dxmt() {
                ui::tree_kv("      ├─", "Status:", "✓ Installed");
                ui::tree_kv(
                    "      ├─",
                    "Version:",
                    bundle.version.as_deref().unwrap_or("Detected"),
                );
                ui::tree_kv("      └─", "Location:", bundle.root.display().to_string());
            } else {
                ui::tree_kv(
                    "      └─",
                    "Status:",
                    "○ Optional (run 'nucleon dxmt fetch')",
                );
            }

            // Layer 2: Graphics Driver
            println!();
            ui::kv("[2] Graphics Driver (Vulkan -> Apple Metal):", "");
            ui::tree_kv("  └─", "Mesa KosmicKrisp (Vulkan 1.4 Driver):", "");
            if let Some(info) = runner::get_kosmickrisp_info() {
                ui::tree_kv("      ├─", "Status:", "✓ Detected / Active");
                ui::tree_kv("      ├─", "API Version:", &info.api_version);
                ui::tree_kv(
                    "      └─",
                    "ICD Manifest:",
                    info.icd_path.display().to_string(),
                );
            } else {
                ui::tree_kv(
                    "      └─",
                    "Status:",
                    "○ Optional (install LunarG Vulkan SDK or run 'nucleon kosmickrisp set-path')",
                );
            }

            println!(
                "\nTranslation Flows:\n  • Vulkan Driver Flow: [Windows Game] -> [Wine] -> [DXVK / VKD3D] -> [KosmicKrisp Vulkan] -> [Mac Metal]\n  • Direct Metal Flow:  [Windows Game] -> [Wine] -> [DXMT (DX11)] -> [Mac Metal]"
            );
            println!(
                "To manage individual components: nucleon wine backends <dxvk|vkd3d|d7vk|dxmt|kosmickrisp> --help"
            );
            println!(
                "Note: Apple GPTK 4 is an independent runner & D3DMetal backend (manage with: nucleon gptk --help)"
            );
        }
        BackendsAction::Kosmickrisp { action } => kosmickrisp::handle(action)?,
        BackendsAction::Vkd3d { action } => vkd3d::run(action)?,
        BackendsAction::D7vk { action } => d7vk::run(action)?,
        BackendsAction::Dxvk { action } => dxvk::run(action)?,
        BackendsAction::Dxmt { action } => dxmt::run(action)?,
    }

    Ok(())
}
