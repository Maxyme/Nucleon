use super::ui;
use anyhow::Result;
use clap::Subcommand;
use nucleon_core::steam;

#[derive(Subcommand, Debug, Clone)]
pub enum SteamAction {
    /// Inject Nucleon dylib into Steam.app Info.plist and re-sign
    Patch,
    /// Restore original Steam.app Info.plist
    Restore,
    /// Remove Nucleon compatibility tools, game mappings, and WebUI patches from Steam UI
    Unregister,
}

pub fn run(action: SteamAction) -> Result<()> {
    match action {
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
    }
    Ok(())
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
