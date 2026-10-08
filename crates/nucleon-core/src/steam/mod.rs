pub mod codesign;
pub mod native_detect;
pub mod vdf_mapping;
pub mod webui;

pub use codesign::*;
pub use native_detect::*;
pub use vdf_mapping::*;
pub use webui::*;

use crate::paths;
use crate::runner;
use crate::vdf;
use anyhow::Result;
use std::fs;
use std::path::Path;

pub fn is_gptk_tool_registered() -> bool {
    paths::steam_compat_tools_dir()
        .join("compatibilitytool.vdf")
        .is_file()
}

pub fn is_kosmickrisp_tool_registered() -> bool {
    paths::steam_kosmickrisp_compat_tools_dir()
        .join("compatibilitytool.vdf")
        .is_file()
}

pub fn is_wine_tool_registered() -> bool {
    paths::home_dir()
        .join("Library/Application Support/Steam/compatibilitytools.d/nucleon-wine/compatibilitytool.vdf")
        .is_file()
}

pub fn is_wine_runtime_tool_registered(tool_id: &str) -> bool {
    paths::home_dir()
        .join(format!(
            "Library/Application Support/Steam/compatibilitytools.d/{}/compatibilitytool.vdf",
            tool_id
        ))
        .is_file()
}

pub fn extract_vdf_display_name(content: &str) -> Option<&str> {
    content.split("\"display_name\"").nth(1)?.split('"').nth(1)
}

pub fn registered_wine_tools() -> Vec<(String, String)> {
    let base = paths::home_dir().join("Library/Application Support/Steam/compatibilitytools.d");
    let mut tools = Vec::new();
    if let Ok(entries) = fs::read_dir(&base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let vdf_path = path.join("compatibilitytool.vdf");
                if vdf_path.is_file() {
                    let folder_name = entry.file_name().to_string_lossy().to_string();
                    if folder_name.starts_with("nucleon-wine") {
                        if let Ok(content) = fs::read_to_string(&vdf_path) {
                            let display_name = extract_vdf_display_name(&content)
                                .unwrap_or(&folder_name)
                                .to_string();
                            tools.push((folder_name, display_name));
                        }
                    }
                }
            }
        }
    }
    tools.sort_by(|a, b| a.0.cmp(&b.0));
    tools
}

pub fn is_staging_tool_registered() -> bool {
    is_wine_tool_registered()
        || is_wine_runtime_tool_registered("nucleon-wine-staging")
        || paths::home_dir()
            .join("Library/Application Support/Steam/compatibilitytools.d/nucleon-staging/compatibilitytool.vdf")
            .is_file()
}

pub fn install_compatibility_tool(runner_bin: &Path) -> Result<()> {
    let nucleon_tool_dir = paths::steam_compat_tools_dir();
    vdf::write_tool_bundle(
        &nucleon_tool_dir,
        "nucleon",
        "Nucleon (Game Porting Toolkit 4)",
        runner_bin,
    )?;

    // Clean up legacy notproton compatibility tool directory if present
    let notproton_tool_dir =
        paths::home_dir().join("Library/Application Support/Steam/compatibilitytools.d/notproton");
    if notproton_tool_dir.exists() {
        let _ = fs::remove_dir_all(&notproton_tool_dir);
    }

    // Register KosmicKrisp compatibility tool if installed/detected
    let kk_tool_dir = paths::steam_kosmickrisp_compat_tools_dir();
    if runner::is_kosmickrisp_installed() {
        vdf::write_tool_bundle_with_engine(
            &kk_tool_dir,
            "nucleon-kosmickrisp",
            "Nucleon (KosmicKrisp)",
            runner_bin,
            Some("kosmickrisp"),
        )?;
        log::info!("Registered Steam compatibility tool: Nucleon (KosmicKrisp)");
    } else if kk_tool_dir.exists() {
        let _ = fs::remove_dir_all(&kk_tool_dir);
        log::info!("Cleaned up unregistered KosmicKrisp Steam tool bundle (not detected)");
    }

    // Register single unified Wine compatibility tool: 'Nucleon (Wine)' pointing to active desired Wine runtime
    let wine_tools_base =
        paths::home_dir().join("Library/Application Support/Steam/compatibilitytools.d");
    let active_wine = crate::wine::get_active_wine_runtime();

    if let Some(ref aw) = active_wine {
        let wine_primary_dir = wine_tools_base.join("nucleon-wine");
        vdf::write_tool_bundle_with_wine(
            &wine_primary_dir,
            "nucleon-wine",
            "Nucleon (Wine)",
            runner_bin,
            Some("staging"),
            Some(&aw.root),
        )?;
        log::info!(
            "Registered Steam compatibility tool: 'Nucleon (Wine)' -> {} ({})",
            aw.name,
            aw.root.display()
        );
    } else {
        let wine_primary_dir = wine_tools_base.join("nucleon-wine");
        if wine_primary_dir.exists() {
            let _ = fs::remove_dir_all(&wine_primary_dir);
        }
    }

    // Clean up all individual version-specific Wine sub-tools (e.g. nucleon-wine-staging, nucleon-wine-crossover, etc.)
    // so Steam's compatibility dropdown stays minimal and uncluttered!
    if let Ok(entries) = fs::read_dir(&wine_tools_base) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("nucleon-wine-") || name == "nucleon-staging" {
                let _ = fs::remove_dir_all(entry.path());
                log::info!("Cleaned up individual Wine sub-tool from Steam: {}", name);
            }
        }
    }

    // Clean and sanitize config.vdf compatibility tool mappings
    let _ = migrate_compat_mappings();

    // Ensure all installed games in steamapps/ are registered in libraryfolders.vdf
    let _ = sync_library_folders();

    // Sanitize any app manifests stuck in UpdateRequired (StateFlags 6)
    let _ = sanitize_installed_app_manifests();

    // Ensure Steam downloading and library permissions prevent "missing file privileges"
    let _ = fix_steam_permissions();

    // Patch SteamUI chunks to enable Install button and Compatibility settings
    let _ = patch_steamui_chunks();

    Ok(())
}

#[derive(Debug, Clone, Default)]
pub struct UnregisterSummary {
    pub tools_removed: Vec<String>,
    pub mappings_cleaned: bool,
    pub webui_chunks_restored: usize,
    pub steam_restored: bool,
    pub launchagent_uninstalled: bool,
}

/// Removes all Nucleon compatibility tool bundles from the specified base directory.
pub fn remove_compatibility_tools_in(base: &Path) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    if base.is_dir() {
        if let Ok(entries) = fs::read_dir(base) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name == "nucleon" || name.starts_with("nucleon-") || name == "notproton" {
                    if let Ok(()) = fs::remove_dir_all(entry.path()) {
                        log::info!("Removed Steam compatibility tool bundle: {}", name);
                        removed.push(name);
                    }
                }
            }
        }
    }
    removed.sort();
    Ok(removed)
}

/// Removes all Nucleon compatibility tools from ~/Library/Application Support/Steam/compatibilitytools.d.
pub fn remove_compatibility_tools() -> Result<Vec<String>> {
    let base = paths::home_dir().join("Library/Application Support/Steam/compatibilitytools.d");
    remove_compatibility_tools_in(&base)
}

/// Fully unregisters Nucleon from Steam UI:
/// 1. Removes all Nucleon compatibility tool bundles
/// 2. Cleans Nucleon game mappings from config.vdf
/// 3. Restores original WebUI chunk files and invalidates CEF cache
/// 4. Restores Steam.app Info.plist and removes injected dylib
/// 5. Uninstalls Steam Update Guard LaunchAgent (if installed)
pub fn unregister_from_steam_ui() -> Result<UnregisterSummary> {
    // 1. Remove compatibility tool bundles
    let tools_removed = remove_compatibility_tools()?;

    // 2. Remove Nucleon game mappings from config.vdf
    let mappings_cleaned = remove_nucleon_compat_mappings()?;

    // 3. Restore WebUI chunk patches and clear CEF cache
    let webui_chunks_restored = restore_steamui_chunks()?;

    let mut summary = UnregisterSummary {
        tools_removed,
        mappings_cleaned,
        webui_chunks_restored,
        ..Default::default()
    };

    // 4. Restore Steam.app Info.plist and remove hook dylib
    if is_steam_patched() {
        let _ = restore_steam();
        summary.steam_restored = true;
    }

    // 5. If LaunchAgent is installed/loaded, uninstall it so it doesn't re-patch
    if (crate::guard::is_launchagent_loaded() || crate::guard::is_launchagent_installed())
        && crate::guard::uninstall_launchagent().is_ok()
    {
        summary.launchagent_uninstalled = true;
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remove_compatibility_tools_in() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();

        let tool1 = base.join("nucleon");
        let tool2 = base.join("nucleon-kosmickrisp");
        let tool3 = base.join("nucleon-wine");
        let other = base.join("custom-tool");

        fs::create_dir_all(&tool1).unwrap();
        fs::create_dir_all(&tool2).unwrap();
        fs::create_dir_all(&tool3).unwrap();
        fs::create_dir_all(&other).unwrap();

        let removed = remove_compatibility_tools_in(base).unwrap();
        assert_eq!(
            removed,
            vec!["nucleon", "nucleon-kosmickrisp", "nucleon-wine"]
        );

        assert!(!tool1.exists());
        assert!(!tool2.exists());
        assert!(!tool3.exists());
        assert!(other.exists(), "Other tools must not be deleted");
    }

    #[test]
    fn test_extract_vdf_display_name() {
        let sample = r#""compatibilitytools"
{
  "compat_tools"
  {
    "nucleon-wine"
    {
      "install_path" "."
      "display_name" "Nucleon (Wine 9.0)"
      "from_oslist" "windows"
      "to_oslist" "macos"
    }
  }
}
"#;
        assert_eq!(extract_vdf_display_name(sample), Some("Nucleon (Wine 9.0)"));

        // Fallback test with non-standard whitespace / formatting
        let flat = "\"display_name\"\t\"Custom Wine Tool\"";
        assert_eq!(extract_vdf_display_name(flat), Some("Custom Wine Tool"));
    }
}
