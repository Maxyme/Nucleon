use super::{d7vk, gptk, kosmickrisp, ui, vkd3d};
use anyhow::Result;
use clap::Subcommand;
use nucleon_core::runner;

#[derive(Subcommand, Debug, Clone)]
pub enum BackendsAction {
    /// Inspect status of all configured graphical backends (GPTK, KosmicKrisp, VKD3D-Proton, D7VK)
    Status,
    /// Manage Apple Game Porting Toolkit 4 (GPTK 4 / D3DMetal) components
    Gptk {
        #[command(subcommand)]
        action: gptk::GptkAction,
    },
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
            ui::header("Graphics Translation Backends Status");

            // 1. Apple GPTK 4
            ui::kv("Apple GPTK 4 (D3DMetal):", "");
            if let Some(runner_path) = runner::find_gptk_runner() {
                ui::tree_kv(
                    "├─",
                    "Assembled Runner:",
                    format!("✓ {}", runner_path.display()),
                );
            } else {
                ui::tree_kv(
                    "├─",
                    "Assembled Runner:",
                    "○ Not assembled (run 'nucleon setup')",
                );
            }
            match runner::find_gptk_components(None) {
                Ok(Some((fw, _))) => {
                    ui::tree_kv(
                        "└─",
                        "Components:",
                        format!("✓ Detected ({})", fw.display()),
                    );
                }
                _ => {
                    ui::tree_kv(
                        "└─",
                        "Components:",
                        "✗ Not detected (run 'nucleon backends gptk set-path')",
                    );
                }
            }

            // 2. Mesa KosmicKrisp
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

            // 3. VKD3D-Proton
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
                    "○ Optional (run 'nucleon backends vkd3d fetch')",
                );
            }

            // 4. D7VK
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
                    "○ Optional (run 'nucleon backends d7vk fetch')",
                );
            }

            println!("\nTo manage individual backends: nucleon backends <gptk|kosmickrisp|vkd3d|d7vk> --help");
        }
        BackendsAction::Gptk { action } => gptk::run(action)?,
        BackendsAction::Kosmickrisp { action } => kosmickrisp::handle(action)?,
        BackendsAction::Vkd3d { action } => vkd3d::run(action)?,
        BackendsAction::D7vk { action } => d7vk::run(action)?,
    }

    Ok(())
}
