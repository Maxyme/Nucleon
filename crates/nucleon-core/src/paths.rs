use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::PathBuf;

pub fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

pub fn support_dir() -> PathBuf {
    home_dir().join("Library/Application Support/nucleon")
}

pub fn runners_dir() -> PathBuf {
    support_dir().join("runners")
}

pub fn current_runner() -> PathBuf {
    runners_dir().join("current")
}

pub fn bridge_dir() -> PathBuf {
    support_dir().join("bridge")
}

pub fn downloads_dir() -> PathBuf {
    support_dir().join("downloads")
}

pub fn signatures_dir() -> PathBuf {
    support_dir().join("signatures/macos.arm64")
}

pub fn steam_app() -> PathBuf {
    PathBuf::from("/Applications/Steam.app")
}

pub fn steam_executable() -> PathBuf {
    steam_app().join("Contents/MacOS/steam_osx")
}

pub fn steam_info_plist() -> PathBuf {
    steam_app().join("Contents/Info.plist")
}

pub fn steam_data_dir() -> PathBuf {
    home_dir().join("Library/Application Support/Steam")
}

/// Relative paths within `steam_data_dir()` where Steam client package manifests may be located.
pub const STEAM_CLIENT_MANIFEST_REL_PATHS: &[&str] = &[
    "Steam.AppBundle/Steam/Contents/MacOS/package/steam_client_signed-2_osx.manifest",
    "Steam.AppBundle/Steam/Contents/MacOS/package/steam_client_osx.manifest",
    "package/steam_client_signed-2_osx.manifest",
    "package/steam_client_osx.manifest",
];

/// Returns absolute paths to all possible Steam client package manifest locations.
pub fn steam_client_manifest_candidates() -> Vec<PathBuf> {
    let data_dir = steam_data_dir();
    STEAM_CLIENT_MANIFEST_REL_PATHS
        .iter()
        .map(|rel| data_dir.join(rel))
        .collect()
}

pub fn steam_compat_tools_dir() -> PathBuf {
    home_dir().join("Library/Application Support/Steam/compatibilitytools.d/nucleon")
}

pub fn steam_gptk_compat_tools_dir() -> PathBuf {
    home_dir().join("Library/Application Support/Steam/compatibilitytools.d/nucleon-gptk")
}

pub fn steam_kosmickrisp_compat_tools_dir() -> PathBuf {
    home_dir().join("Library/Application Support/Steam/compatibilitytools.d/nucleon-kosmickrisp")
}

pub fn steam_wine_compat_tools_dir() -> PathBuf {
    home_dir().join("Library/Application Support/Steam/compatibilitytools.d/nucleon-wine")
}

pub fn steam_compat_data_dir() -> PathBuf {
    home_dir().join("Library/Application Support/Steam/steamapps/compatdata")
}

pub fn kosmickrisp_dir() -> PathBuf {
    support_dir().join("drivers/kosmickrisp")
}

pub fn kosmickrisp_shim_dir() -> PathBuf {
    support_dir().join("shims/kosmickrisp")
}

pub fn vkd3d_proton_dir() -> PathBuf {
    support_dir().join("vkd3d-proton")
}

pub fn d7vk_dir() -> PathBuf {
    support_dir().join("d7vk")
}

pub fn launch_agents_dir() -> PathBuf {
    home_dir().join("Library/LaunchAgents")
}

pub fn steam_guard_plist() -> PathBuf {
    launch_agents_dir().join("com.nucleon.steam-guard.plist")
}

pub fn guard_bin() -> PathBuf {
    support_dir().join("bin/nucleon")
}

pub fn guard_log() -> PathBuf {
    support_dir().join("guard.log")
}

pub fn guard_state() -> PathBuf {
    support_dir().join("guard-state.json")
}

pub fn ensure_dirs() -> Result<()> {
    for d in &[
        support_dir(),
        support_dir().join("bin"),
        runners_dir(),
        bridge_dir(),
        downloads_dir(),
        signatures_dir(),
        steam_compat_tools_dir(),
        kosmickrisp_dir(),
        kosmickrisp_shim_dir(),
        vkd3d_proton_dir(),
        d7vk_dir(),
        launch_agents_dir(),
    ] {
        fs::create_dir_all(d)
            .with_context(|| format!("Failed to create directory: {}", d.display()))?;
    }
    Ok(())
}
