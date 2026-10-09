use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::fs_util::atomic_write_file;
use crate::{paths, signatures, steam};
use serde::{Deserialize, Serialize};

pub const GUARD_LABEL: &str = "com.nucleon.steam-guard";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardStatus {
    pub steam_installed: bool,
    pub adhoc_signed: bool,
    pub plist_patched: bool,
    pub hook_dylib_present: bool,
    pub webui_patched: bool,
    pub launchagent_installed: bool,
    pub launchagent_loaded: bool,
    pub last_heal_timestamp: Option<u64>,
    pub signatures_cached: bool,
    pub detected_steam_build: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HealResult {
    pub resigned: bool,
    pub plist_fixed: bool,
    pub hook_deployed: bool,
    pub webui_patched_count: usize,
    pub mappings_cleaned: bool,
    pub signatures_cached: bool,
    pub no_action_needed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GuardState {
    pub last_heal_timestamp: u64,
    pub last_status_ok: bool,
}

/// Loads the persistent guard state from Application Support.
pub fn load_guard_state() -> Result<GuardState> {
    let path = paths::guard_state();
    if !path.exists() {
        return Ok(GuardState::default());
    }
    let data = fs::read_to_string(&path)?;
    let state = serde_json::from_str(&data)?;
    Ok(state)
}

/// Saves the persistent guard state to Application Support.
pub fn save_guard_state(state: &GuardState) -> Result<()> {
    paths::ensure_dirs()?;
    let path = paths::guard_state();
    let data = serde_json::to_string_pretty(state)?;
    atomic_write_file(path, data)?;
    Ok(())
}

/// Checks current Steam codesigning, hook injection, WebUI patch, and LaunchAgent status.
pub fn check_guard_status() -> GuardStatus {
    let steam_installed = steam::is_steam_installed();
    let adhoc_signed = steam::is_adhoc_signed(&paths::steam_app())
        && steam::is_adhoc_signed(&paths::steam_executable());
    let hook_dest = paths::steam_app().join("Contents/MacOS/nucleon.dylib");
    let hook_dylib_present = hook_dest.is_file() && steam::is_adhoc_signed(&hook_dest);
    let plist_patched = steam::is_steam_patched();
    let webui_patched = steam::are_steamui_chunks_patched();
    let launchagent_installed = is_launchagent_installed();
    let launchagent_loaded = is_launchagent_loaded();

    let last_state = load_guard_state().ok();
    let last_heal_timestamp = last_state.and_then(|s| {
        if s.last_heal_timestamp > 0 {
            Some(s.last_heal_timestamp)
        } else {
            None
        }
    });

    let detected_steam_build = signatures::detect_installed_steam_build();
    let signatures_cached = if let Some(b) = detected_steam_build {
        paths::signatures_dir().join(format!("{b}.json")).is_file()
    } else {
        false
    };

    GuardStatus {
        steam_installed,
        adhoc_signed,
        plist_patched,
        hook_dylib_present,
        webui_patched,
        launchagent_installed,
        launchagent_loaded,
        last_heal_timestamp,
        signatures_cached,
        detected_steam_build,
    }
}

/// Resolves a candidate hook dylib to stage into Steam if missing.
pub fn resolve_hook_source(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }
    // Check bridge directory copy
    let bridge_hook = paths::bridge_dir().join("nucleon.dylib");
    if bridge_hook.exists() {
        return Some(bridge_hook);
    }
    let bridge_lib = paths::bridge_dir().join("libnucleon.dylib");
    if bridge_lib.exists() {
        return Some(bridge_lib);
    }
    // Check support dir
    let support_hook = paths::support_dir().join("nucleon.dylib");
    if support_hook.exists() {
        return Some(support_hook);
    }
    // Check current steam dylib
    let steam_hook = paths::steam_app().join("Contents/MacOS/nucleon.dylib");
    if steam_hook.exists() {
        return Some(steam_hook);
    }
    None
}

/// Verifies and heals Steam's ad-hoc signatures, Info.plist hook injection, and WebUI chunks.
pub fn heal_steam(hook_source: Option<&Path>) -> Result<HealResult> {
    if !steam::is_steam_installed() {
        return Ok(HealResult {
            no_action_needed: true,
            ..Default::default()
        });
    }

    let mut result = HealResult::default();
    let mut needs_resign = false;

    // 1. Check hook dylib in Contents/MacOS/nucleon.dylib
    let hook_dest = paths::steam_app().join("Contents/MacOS/nucleon.dylib");
    if !hook_dest.is_file() {
        if let Some(src_path) = resolve_hook_source(hook_source) {
            fs::copy(&src_path, &hook_dest).with_context(|| {
                format!(
                    "Failed to deploy hook dylib from {} to {}",
                    src_path.display(),
                    hook_dest.display()
                )
            })?;
            // Also preserve in bridge dir for future offline heals
            let bridge_dest = paths::bridge_dir().join("nucleon.dylib");
            let _ = fs::copy(&src_path, bridge_dest);
            result.hook_deployed = true;
            needs_resign = true;
        }
    }

    // 2. Check Info.plist DYLD_INSERT_LIBRARIES injection
    if !steam::is_steam_patched() {
        let plist = paths::steam_info_plist();
        if steam::inject_plist_dyld_insert(&plist, &hook_dest)? {
            result.plist_fixed = true;
            needs_resign = true;
        }
    }

    // 3. Check ad-hoc codesign validity
    let app_signed = steam::is_adhoc_signed(&paths::steam_app());
    let exe_signed = steam::is_adhoc_signed(&paths::steam_executable());
    let hook_signed = hook_dest.exists() && steam::is_adhoc_signed(&hook_dest);

    if needs_resign || !app_signed || !exe_signed || !hook_signed {
        if hook_dest.exists() {
            let _ = steam::sign_binary(&hook_dest);
        }
        steam::sign_binary(&paths::steam_executable())?;
        steam::sign_binary(&paths::steam_app())?;
        steam::refresh_launch_services(&paths::steam_app())?;
        result.resigned = true;
    }

    // 4. Check WebUI chunk patches
    if !steam::are_steamui_chunks_patched() {
        if let Ok(count) = steam::patch_steamui_chunks() {
            result.webui_patched_count = count;
        }
    }

    // 5. Clean native games and wildcard "0" from CompatToolMapping
    if steam::migrate_compat_mappings().is_ok() {
        result.mappings_cleaned = true;
    }

    // 6. Ensure signature DB exists for currently installed Steam build
    if let Ok((cached_path, _)) = signatures::ensure_signature_db_for_installed_steam() {
        if cached_path.is_file() {
            result.signatures_cached = true;
        }
    }

    // 7. Record heal timestamp to guard state
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let state = GuardState {
        last_heal_timestamp: now,
        last_status_ok: true,
    };
    let _ = save_guard_state(&state);

    if !result.resigned
        && !result.plist_fixed
        && !result.hook_deployed
        && result.webui_patched_count == 0
        && !result.mappings_cleaned
        && !result.signatures_cached
    {
        result.no_action_needed = true;
    }

    Ok(result)
}

/// Executes guard self-healing with debounce protection (< 5 seconds since last clean check).
pub fn run_guard_once(hook_source: Option<&Path>) -> Result<HealResult> {
    if let Ok(state) = load_guard_state() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if now.saturating_sub(state.last_heal_timestamp) < 5 {
            let status = check_guard_status();
            if status.adhoc_signed
                && status.plist_patched
                && status.hook_dylib_present
                && status.webui_patched
            {
                return Ok(HealResult {
                    no_action_needed: true,
                    ..Default::default()
                });
            }
        }
    }

    heal_steam(hook_source)
}

/// Generates the launchd LaunchAgent XML plist configuration.
pub fn generate_launchagent_plist(binary_path: &Path) -> String {
    let home = paths::home_dir();
    let home_str = home.to_string_lossy();
    let bin_str = binary_path.to_string_lossy();
    let log_path = paths::guard_log();
    let log_str = log_path.to_string_lossy();

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{label}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{bin}</string>
		<string>guard</string>
		<string>run</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>ThrottleInterval</key>
	<integer>10</integer>
	<key>WatchPaths</key>
	<array>
		<string>/Applications/Steam.app/Contents/Info.plist</string>
		<string>/Applications/Steam.app/Contents/MacOS</string>
		<string>{home}/Library/Application Support/Steam/package</string>
		<string>{home}/Library/Application Support/Steam/Steam.AppBundle/Steam</string>
		<string>{home}/Library/Application Support/Steam/Steam.AppBundle/Steam/Contents/MacOS/steamui</string>
	</array>
	<key>StandardOutPath</key>
	<string>{log}</string>
	<key>StandardErrorPath</key>
	<string>{log}</string>
</dict>
</plist>
"#,
        label = GUARD_LABEL,
        bin = bin_str,
        home = home_str,
        log = log_str
    )
}

/// Checks if the LaunchAgent plist file exists in ~/Library/LaunchAgents.
pub fn is_launchagent_installed() -> bool {
    paths::steam_guard_plist().is_file()
}

/// Checks if the LaunchAgent job is currently loaded in launchd.
pub fn is_launchagent_loaded() -> bool {
    let uid = nix::unistd::getuid();
    let domain = format!("gui/{}", uid);

    if let Ok(out) = Command::new("launchctl")
        .args(["print", &format!("{}/{}", domain, GUARD_LABEL)])
        .output()
    {
        if out.status.success() {
            return true;
        }
    }

    if let Ok(out) = Command::new("launchctl").args(["list"]).output() {
        let text = String::from_utf8_lossy(&out.stdout);
        return text.contains(GUARD_LABEL);
    }

    false
}

/// Installs and loads the LaunchAgent to monitor Steam updates.
pub fn install_launchagent(binary_path: Option<&Path>) -> Result<PathBuf> {
    paths::ensure_dirs()?;

    let target_bin = if let Some(p) = binary_path {
        p.to_path_buf()
    } else if let Ok(current_exe) = std::env::current_exe() {
        // Stage a copy into Application Support/nucleon/bin/nucleon for stability
        let guard_bin = paths::guard_bin();
        if let Some(parent) = guard_bin.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::copy(&current_exe, &guard_bin);
        if guard_bin.exists() {
            guard_bin
        } else {
            current_exe
        }
    } else {
        paths::guard_bin()
    };

    let plist_content = generate_launchagent_plist(&target_bin);
    let plist_path = paths::steam_guard_plist();

    if let Some(parent) = plist_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    fs::write(&plist_path, plist_content).with_context(|| {
        format!(
            "Failed to write LaunchAgent plist to {}",
            plist_path.display()
        )
    })?;

    // Load with launchctl
    load_launchagent(&plist_path)?;

    Ok(plist_path)
}

/// Loads a LaunchAgent plist using launchctl.
pub fn load_launchagent(plist_path: &Path) -> Result<()> {
    let uid = nix::unistd::getuid();
    let domain = format!("gui/{}", uid);

    // Ensure previous instance is stopped before reloading
    let _ = Command::new("launchctl")
        .args(["bootout", &domain])
        .arg(plist_path)
        .output();
    let _ = Command::new("launchctl")
        .args(["unload", "-w"])
        .arg(plist_path)
        .output();

    // Try modern bootstrap first
    let bootstrap_res = Command::new("launchctl")
        .args(["bootstrap", &domain])
        .arg(plist_path)
        .status();

    if let Ok(st) = bootstrap_res {
        if st.success() {
            return Ok(());
        }
    }

    // Fallback to load -w
    let load_res = Command::new("launchctl")
        .args(["load", "-w"])
        .arg(plist_path)
        .status()
        .with_context(|| format!("Failed to load LaunchAgent {}", plist_path.display()))?;

    if !load_res.success() {
        bail!("launchctl load failed with status {}", load_res);
    }

    Ok(())
}

/// Unloads and removes the LaunchAgent plist.
pub fn uninstall_launchagent() -> Result<()> {
    let plist_path = paths::steam_guard_plist();
    if plist_path.exists() {
        let uid = nix::unistd::getuid();
        let domain = format!("gui/{}", uid);

        let _ = Command::new("launchctl")
            .args(["bootout", &domain])
            .arg(&plist_path)
            .output();
        let _ = Command::new("launchctl")
            .args(["unload", "-w"])
            .arg(&plist_path)
            .output();

        let _ = fs::remove_file(&plist_path);
    }
    Ok(())
}

/// Runs a continuous lightweight in-process watcher loop.
pub fn run_guard_watcher(
    stop_flag: Arc<AtomicBool>,
    poll_interval: Duration,
    hook_source: Option<&Path>,
) -> Result<()> {
    while !stop_flag.load(Ordering::SeqCst) {
        let status = check_guard_status();
        if status.steam_installed
            && (!status.adhoc_signed
                || !status.plist_patched
                || !status.hook_dylib_present
                || !status.webui_patched
                || !status.signatures_cached)
        {
            let _ = heal_steam(hook_source);
        }
        std::thread::sleep(poll_interval);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_launchagent_plist() {
        let bin = PathBuf::from("/Users/test/nucleon");
        let plist = generate_launchagent_plist(&bin);

        assert!(plist.contains("<string>com.nucleon.steam-guard</string>"));
        assert!(plist.contains("<string>/Users/test/nucleon</string>"));
        assert!(plist.contains("<string>guard</string>"));
        assert!(plist.contains("<string>run</string>"));
        assert!(plist.contains("<key>ThrottleInterval</key>"));
        assert!(plist.contains("<key>WatchPaths</key>"));
        assert!(plist.contains("<string>/Applications/Steam.app/Contents/Info.plist</string>"));
    }

    #[test]
    fn test_guard_state_serialization() {
        let state = GuardState {
            last_heal_timestamp: 123456789,
            last_status_ok: true,
        };
        let json = serde_json::to_string(&state).unwrap();
        let parsed: GuardState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.last_heal_timestamp, 123456789);
        assert!(parsed.last_status_ok);
    }
}
