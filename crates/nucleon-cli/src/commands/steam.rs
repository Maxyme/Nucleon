use super::ui;
use anyhow::Result;
use clap::Subcommand;
use nucleon_core::{steam, steam_contract};
use std::path::PathBuf;

#[derive(Subcommand, Debug, Clone)]
pub enum SteamAction {
    /// Inspect a reviewed Steam client contract without writing caches or installing hooks
    InspectContract {
        /// Path to the native steamclient.dylib
        path: PathBuf,
    },
    /// Inject Nucleon dylib into Steam.app Info.plist and re-sign
    Patch,
    /// Restore original Steam.app Info.plist
    Restore,
    /// Remove Nucleon compatibility tools, game mappings, and WebUI patches from Steam UI
    Unregister,
    /// Map a Steam game (by AppID) to a Nucleon compatibility tool (e.g. 601150 for Devil May Cry 5)
    Map {
        /// Steam Application ID (e.g. 601150)
        appid: u32,
        /// Optional compatibility tool name (default: 'nucleon', or 'nucleon-gptk', 'nucleon-dxvk', 'nucleon-dxmt', 'nucleon-kosmickrisp', 'nucleon-wine')
        #[arg(long, short)]
        tool: Option<String>,
    },
    /// Unmap a Steam game (by AppID) from Steam config.vdf
    Unmap {
        /// Steam Application ID (e.g. 601150)
        appid: u32,
    },
}

pub fn run(action: SteamAction) -> Result<()> {
    match action {
        SteamAction::InspectContract { path } => {
            let reviewed = steam_contract::inspect_contract_image(&path)?;
            ui::success("Steam client matches the reviewed contract");
            ui::tree_kv("└─", "UUID:", &reviewed.contract().client.uuid);
            ui::tree_kv(
                "└─",
                "Executable text SHA-256:",
                &reviewed.contract().client.text_sha256,
            );
            ui::tree_kv(
                "└─",
                "Reviewed patch sites:",
                reviewed.contract().patches.len(),
            );
            ui::tree_kv("└─", "Evidence:", &reviewed.contract().provenance.method);
            ui::tree_kv(
                "└─",
                "Provenance limit:",
                &reviewed.contract().provenance.limitations,
            );
        }
        SteamAction::Patch => {
            let hook_dylib = super::find_hook_dylib()
                .ok_or_else(|| anyhow::anyhow!("Hook dylib not found. Build it first with 'just build' or 'cargo build --release'"))?;
            steam::patch_steam(&hook_dylib)?;
            ui::success("Successfully patched and signed Steam.app");
        }
        SteamAction::Restore => {
            steam::restore_steam()?;
            ui::success("Successfully restored original Steam.app");
        }
        SteamAction::Unregister => {
            run_unregister()?;
        }
        SteamAction::Map { appid, tool } => {
            let tool_name = tool.as_deref().unwrap_or("nucleon");
            steam::map_app_compat_tool(appid, Some(tool_name))?;
            ui::success(format!(
                "Successfully mapped AppID {appid} to '{tool_name}' in Steam config.vdf"
            ));
            warn_if_steam_running();
        }
        SteamAction::Unmap { appid } => {
            if steam::unmap_app_compat_tool(appid)? {
                ui::success(format!(
                    "Successfully unmapped AppID {appid} from Steam config.vdf"
                ));
            } else {
                ui::info(format!("AppID {appid} was not mapped in Steam config.vdf"));
            }
        }
    }
    Ok(())
}

/// Reminds the user that a running Steam client holds `config.vdf` in memory and rewrites it when
/// it quits, which silently discards mappings - and therefore the compatibility tool (engine) that
/// was just chosen for the game.
fn warn_if_steam_running() {
    if steam::steam_client_is_running() {
        ui::warn(
            "Steam is running. Quit Steam completely and reopen it before launching, or Steam will \n  rewrite config.vdf on exit and keep using the previously selected tool.",
        );
    }
}

pub fn run_unregister() -> Result<()> {
    ui::header("Removing Nucleon from Steam UI...");
    let summary = steam::unregister_from_steam_ui()?;

    if summary.tools_removed.is_empty() {
        ui::info("No compatibility tool bundles found in compatibilitytools.d");
    } else {
        ui::success(format!(
            "Removed {} compatibility tool bundle(s): {}",
            summary.tools_removed.len(),
            summary.tools_removed.join(", ")
        ));
    }

    if summary.mappings_cleaned {
        ui::success("Cleaned Nucleon game mappings from config.vdf");
    } else {
        ui::info("No Nucleon game mappings found in config.vdf");
    }

    if summary.webui_chunks_restored > 0 {
        ui::success(format!(
            "Restored {} WebUI chunk(s) and cleared CEF cache",
            summary.webui_chunks_restored
        ));
    } else {
        ui::info("WebUI chunks already clean");
    }

    if summary.steam_restored {
        ui::success("Restored Steam.app Info.plist and removed hook dylib");
    }

    if summary.launchagent_uninstalled {
        ui::success("Unloaded and removed Steam Update Guard LaunchAgent");
    }

    println!(
        r#"
✓ Nucleon successfully removed from Steam UI.
  Restart Steam (pkill steam_osx && open -a /Applications/Steam.app) to refresh."#
    );
    Ok(())
}
