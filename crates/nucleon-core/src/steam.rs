use crate::paths;
use crate::runner;
use crate::vdf;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn is_steam_running() -> bool {
    Command::new("pgrep")
        .arg("steam_osx")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn check_steam_installed() -> Result<PathBuf> {
    let app = paths::steam_app();
    if !app.exists() {
        bail!("Steam.app not found at {}", app.display());
    }
    let exe = paths::steam_executable();
    if !exe.exists() {
        bail!("steam_osx executable not found at {}", exe.display());
    }
    Ok(app)
}

pub fn is_steam_installed() -> bool {
    check_steam_installed().is_ok()
}

pub fn is_steam_patched() -> bool {
    let plist = paths::steam_info_plist();
    if !plist.exists() {
        return false;
    }
    let content = fs::read_to_string(&plist).unwrap_or_default();
    content.contains("LSEnvironment")
        && (content.contains("DYLD_INSERT_LIBRARIES") || content.contains("nucleon.dylib"))
}

pub fn patch_steam(hook_dylib: &Path) -> Result<()> {
    check_steam_installed()?;
    let plist = paths::steam_info_plist();
    let backup = paths::support_dir().join("Steam-Info.plist.bak");

    if !backup.exists() {
        fs::copy(&plist, &backup)
            .with_context(|| format!("Failed to backup Info.plist to {}", backup.display()))?;
    }

    // Deploy hook dylib into Steam app bundle MacOS dir
    let dst_dylib = paths::steam_app().join("Contents/MacOS/nucleon.dylib");
    fs::copy(hook_dylib, &dst_dylib)
        .with_context(|| format!("Failed to copy hook dylib to {}", dst_dylib.display()))?;

    // Use /usr/libexec/PlistBuddy to inject LSEnvironment
    let plist_str = plist.to_str().unwrap();
    let dylib_str = dst_dylib.to_str().unwrap();

    // Ensure LSEnvironment dictionary exists
    let _ = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Add :LSEnvironment dict", plist_str])
        .output();

    // Set or add DYLD_INSERT_LIBRARIES
    let set_res = Command::new("/usr/libexec/PlistBuddy")
        .args([
            "-c",
            &format!("Set :LSEnvironment:DYLD_INSERT_LIBRARIES {}", dylib_str),
            plist_str,
        ])
        .output();

    if let Ok(res) = set_res {
        if !res.status.success() {
            let _ = Command::new("/usr/libexec/PlistBuddy")
                .args([
                    "-c",
                    &format!(
                        "Add :LSEnvironment:DYLD_INSERT_LIBRARIES string {}",
                        dylib_str
                    ),
                    plist_str,
                ])
                .status();
        }
    }

    // Ad-hoc re-sign binaries
    sign_binary(&dst_dylib)?;
    sign_binary(&paths::steam_executable())?;
    sign_binary(&paths::steam_app())?;

    // Refresh LaunchServices
    refresh_launch_services(&paths::steam_app())?;

    Ok(())
}

pub fn restore_steam() -> Result<()> {
    let plist = paths::steam_info_plist();
    let backup = paths::support_dir().join("Steam-Info.plist.bak");
    if backup.exists() {
        fs::copy(&backup, &plist).with_context(|| "Failed to restore Info.plist from backup")?;
        fs::remove_file(&backup)?;
    } else {
        // Also check legacy in-bundle backup if any
        let old_backup = paths::steam_info_plist().with_extension("plist.nucleon-bak");
        if old_backup.exists() {
            let _ = fs::copy(&old_backup, &plist);
            let _ = fs::remove_file(&old_backup);
        } else {
            // Remove LSEnvironment:DYLD_INSERT_LIBRARIES
            let _ = Command::new("/usr/libexec/PlistBuddy")
                .args([
                    "-c",
                    "Delete :LSEnvironment:DYLD_INSERT_LIBRARIES",
                    plist.to_str().unwrap(),
                ])
                .status();
        }
    }

    let dst_dylib = paths::steam_app().join("Contents/MacOS/nucleon.dylib");
    if dst_dylib.exists() {
        let _ = fs::remove_file(&dst_dylib);
    }

    sign_binary(&paths::steam_executable())?;
    sign_binary(&paths::steam_app())?;
    refresh_launch_services(&paths::steam_app())?;

    Ok(())
}

pub fn sign_binary(path: &Path) -> Result<()> {
    let status = Command::new("codesign")
        .args(["-fs", "-", path.to_str().unwrap()])
        .status()
        .with_context(|| format!("Failed to sign {}", path.display()))?;
    if !status.success() {
        bail!("codesign failed for {}", path.display());
    }
    Ok(())
}

pub fn is_adhoc_signed(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    let verify = Command::new("codesign")
        .args(["-v", path.to_str().unwrap()])
        .output();
    match verify {
        Ok(v) if v.status.success() => {}
        _ => return false,
    }

    let detail = Command::new("codesign")
        .args(["-d", "-vvv", path.to_str().unwrap()])
        .output();
    if let Ok(d) = detail {
        let combined = format!(
            "{}\n{}",
            String::from_utf8_lossy(&d.stdout),
            String::from_utf8_lossy(&d.stderr)
        );
        combined.contains("Signature=adhoc") || combined.contains("flags=0x2(adhoc)")
    } else {
        false
    }
}

pub fn are_steamui_chunks_patched() -> bool {
    let steamui_dir = paths::steam_data_dir().join("Steam.AppBundle/Steam/Contents/MacOS/steamui");
    if !steamui_dir.is_dir() {
        return true;
    }

    if let Ok(entries) = fs::read_dir(&steamui_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if !name.starts_with("chunk~") || !name.ends_with(".js") || name.ends_with(".bak") {
                continue;
            }

            if let Ok(content) = fs::read_to_string(&path) {
                let is_candidate = content.contains("case u.Ul.jw:return s.UM;")
                    || content.contains("case D.Ul.pd:case D.Ul.K5:case D.Ul.KR:case D.Ul.Mu:return!0")
                    || content.contains("(!o||\"darwin\"!=o)&&(!l||\"darwin\"!=l)")
                    || content.contains("r.k_EClientUINotification_FamilySharingDeviceAvailable,v=r.k_EClientUINotification_SteamPlay")
                    || content.contains("\"Playable on this computer via Steam Play\"");

                if is_candidate {
                    let has_patches = content.contains("\"Playable via Steam Play (Nucleon)\"")
                        || content.contains("get is_invalid_os_type(){return false}");
                    if !has_patches {
                        return false;
                    }
                }
            }
        }
    }
    true
}

pub fn refresh_launch_services(app_path: &Path) -> Result<()> {
    let lsregister = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
    if Path::new(lsregister).exists() {
        let _ = Command::new(lsregister)
            .args(["-f", app_path.to_str().unwrap()])
            .status();
    }
    Ok(())
}

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
    paths::home_dir().join("Library/Application Support/Steam/compatibilitytools.d/nucleon-wine/compatibilitytool.vdf").is_file()
}

pub fn is_wine_runtime_tool_registered(tool_id: &str) -> bool {
    paths::home_dir()
        .join(format!(
            "Library/Application Support/Steam/compatibilitytools.d/{}/compatibilitytool.vdf",
            tool_id
        ))
        .is_file()
}

pub fn registered_wine_tools() -> Vec<(String, String)> {
    let base = paths::home_dir().join("Library/Application Support/Steam/compatibilitytools.d");
    let mut tools = Vec::new();
    let display_re = regex::Regex::new(r#""display_name"\s+"([^"]+)""#).unwrap();
    if let Ok(entries) = fs::read_dir(&base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let vdf_path = path.join("compatibilitytool.vdf");
                if vdf_path.is_file() {
                    let folder_name = entry.file_name().to_string_lossy().to_string();
                    if folder_name.starts_with("nucleon-wine") {
                        if let Ok(content) = fs::read_to_string(&vdf_path) {
                            let display_name = display_re
                                .captures(&content)
                                .and_then(|c| c.get(1))
                                .map(|m| m.as_str().to_string())
                                .unwrap_or_else(|| folder_name.clone());
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
        || paths::home_dir().join("Library/Application Support/Steam/compatibilitytools.d/nucleon-staging/compatibilitytool.vdf").is_file()
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

/// Known native macOS Steam AppIDs that should run natively without compatibility tools.
pub const KNOWN_NATIVE_MAC_APPIDS: &[&str] = &[
    "281990",  // Stellaris
    "421020",  // DiRT 4
    "1091500", // Cyberpunk 2077
    "391220",  // Rise of the Tomb Raider
    "2366970", // Arco
];

/// Checks if a file is a Mach-O binary by reading its 4-byte magic number.
pub fn is_macho_binary(path: &Path) -> bool {
    if let Ok(mut f) = fs::File::open(path) {
        use std::io::Read;
        let mut magic = [0u8; 4];
        if f.read_exact(&mut magic).is_ok() {
            return matches!(
                &magic,
                [0xfe, 0xed, 0xfa, 0xce]
                    | [0xfe, 0xed, 0xfa, 0xcf]
                    | [0xce, 0xfa, 0xed, 0xfe]
                    | [0xcf, 0xfa, 0xed, 0xfe]
                    | [0xca, 0xfe, 0xba, 0xbe]
                    | [0xbe, 0xba, 0xfe, 0xca]
            );
        }
    }
    false
}

/// Recursively checks if a game directory contains a macOS .app bundle or Mach-O executable.
pub fn is_directory_native_mac(dir: &Path, max_depth: u32) -> bool {
    if !dir.is_dir() || max_depth == 0 {
        return false;
    }
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if p.extension().and_then(|s| s.to_str()) == Some("app") {
                    return true;
                }
                if is_directory_native_mac(&p, max_depth - 1) {
                    return true;
                }
            } else if p.is_file() && is_macho_binary(&p) {
                return true;
            }
        }
    }
    false
}

pub fn migrate_compat_mappings() -> Result<()> {
    let config_path = paths::home_dir().join("Library/Application Support/Steam/config/config.vdf");
    if !config_path.exists() {
        return Ok(());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(&config_path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o644);
            let _ = fs::set_permissions(&config_path, perms);
        }
    }

    let content = match fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };

    let mut updated = content;
    let mut modified = false;

    // Collect native macOS AppIDs to ensure they are never forced into CompatToolMapping.
    // Also include "0" (global wildcard) so that wildcard priority 250 does not hijack native games.
    let mut native_appids = std::collections::HashSet::new();
    native_appids.insert("0".to_string());
    for &id in KNOWN_NATIVE_MAC_APPIDS {
        native_appids.insert(id.to_string());
    }

    let steamapps = paths::steam_data_dir().join("steamapps");
    if let Ok(entries) = fs::read_dir(&steamapps) {
        for entry in entries.flatten() {
            let p = entry.path();
            if let Some(file_name) = p.file_name().and_then(|s| s.to_str()) {
                if file_name.starts_with("appmanifest_") && file_name.ends_with(".acf") {
                    let id = file_name
                        .trim_start_matches("appmanifest_")
                        .trim_end_matches(".acf");
                    if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
                        let mut is_native_mac = KNOWN_NATIVE_MAC_APPIDS.contains(&id);
                        if !is_native_mac {
                            if let Ok(acf_content) = fs::read_to_string(&p) {
                                for line in acf_content.lines() {
                                    let trimmed = line.trim();
                                    let quotes: Vec<&str> = trimmed.split('"').collect();
                                    if quotes.len() >= 4 && quotes[1] == "installdir" {
                                        let dir_name = quotes[3];
                                        let game_dir = steamapps.join("common").join(dir_name);
                                        if is_directory_native_mac(&game_dir, 3) {
                                            is_native_mac = true;
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        if is_native_mac {
                            native_appids.insert(id.to_string());
                        }
                    }
                }
            }
        }
    }

    // We do NOT inject unmapped installed games into CompatToolMapping with priority 250.
    // Doing so locks the "Force compatibility tool" checkbox on and converts native macOS games into Wine games.
    let target_appids: Vec<String> = Vec::new();
    let (new_content, modified_compat) =
        update_compat_tool_mapping(&updated, &target_appids, &native_appids);
    if modified_compat {
        updated = new_content;
        modified = true;
    }

    if modified {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(&config_path) {
                let mut perms = meta.permissions();
                perms.set_mode(0o644);
                let _ = fs::set_permissions(&config_path, perms);
            }
        }
        let _ = fs::write(&config_path, updated);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(&config_path) {
                let mut perms = meta.permissions();
                perms.set_mode(0o644);
                let _ = fs::set_permissions(&config_path, perms);
            }
        }
    }
    Ok(())
}

/// Scans steamapps directory for all appmanifest_*.acf files and ensures
/// each installed game is registered under "apps" in libraryfolders.vdf.
pub fn sync_library_folders() -> Result<Vec<u32>> {
    let steamapps = paths::home_dir().join("Library/Application Support/Steam/steamapps");
    if !steamapps.is_dir() {
        return Ok(vec![]);
    }

    // Collect all appmanifest entries: (appid_str, size_on_disk_str)
    let mut installed_apps: Vec<(String, String)> = Vec::new();
    if let Ok(entries) = fs::read_dir(&steamapps) {
        for entry in entries.flatten() {
            let p = entry.path();
            if let Some(file_name) = p.file_name().and_then(|s| s.to_str()) {
                if file_name.starts_with("appmanifest_") && file_name.ends_with(".acf") {
                    if let Ok(content) = fs::read_to_string(&p) {
                        let mut appid = None;
                        let mut size = None;
                        for line in content.lines() {
                            let trimmed = line.trim();
                            if trimmed.starts_with("\"appid\"") {
                                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                                if parts.len() >= 2 {
                                    appid = Some(parts[1].trim_matches('"').to_string());
                                }
                            } else if trimmed.starts_with("\"SizeOnDisk\"") {
                                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                                if parts.len() >= 2 {
                                    size = Some(parts[1].trim_matches('"').to_string());
                                }
                            }
                        }
                        if let (Some(id), Some(sz)) = (appid, size) {
                            installed_apps.push((id, sz));
                        }
                    }
                }
            }
        }
    }

    let libraryfolders_paths = [
        paths::home_dir().join("Library/Application Support/Steam/steamapps/libraryfolders.vdf"),
        paths::home_dir().join("Library/Application Support/Steam/config/libraryfolders.vdf"),
    ];

    let mut restored_appids = Vec::new();

    for lib_path in &libraryfolders_paths {
        if !lib_path.exists() {
            continue;
        }
        let content = match fs::read_to_string(lib_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let mut modified = false;
        let mut new_lines = Vec::new();

        for line in content.lines() {
            new_lines.push(line.to_string());
            if line.contains("\"apps\"") {
                for (id, sz) in &installed_apps {
                    let needle = format!("\"{id}\"");
                    if !content.contains(&needle) {
                        new_lines.push(format!("\t\t\t\"{id}\"\t\t\"{sz}\""));
                        modified = true;
                        if let Ok(num) = id.parse::<u32>() {
                            if !restored_appids.contains(&num) {
                                restored_appids.push(num);
                            }
                        }
                    }
                }
            }
        }

        if modified {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut write_perms = fs::metadata(lib_path)?.permissions();
                write_perms.set_mode(0o644);
                let _ = fs::set_permissions(lib_path, write_perms);
            }
            let _ = fs::write(lib_path, new_lines.join("\n") + "\n");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut write_perms = fs::metadata(lib_path)?.permissions();
                write_perms.set_mode(0o644);
                let _ = fs::set_permissions(lib_path, write_perms);
            }
            log::info!("Restored missing app entries in {}", lib_path.display());
        }
    }

    Ok(restored_appids)
}

/// Ensure all files in Steam's config, htmlcache, downloading, temp, and common game folders have owner read/write permissions
/// to prevent Steam and CEF from throwing 'Missing file privileges' or 'Profile error occurred'.
pub fn fix_steam_permissions() -> Result<()> {
    let steam_dir = paths::steam_data_dir();
    let targets = [
        steam_dir.join("config"),
        steam_dir.join("config/htmlcache"),
        steam_dir.join("steamapps"),
        steam_dir.join("steamapps/downloading"),
        steam_dir.join("steamapps/temp"),
        steam_dir.join("steamapps/common"),
    ];

    for target in &targets {
        if target.exists() {
            let status = Command::new("/bin/chmod")
                .arg("-R")
                .arg("u+rwX")
                .arg(target)
                .status();
            if let Err(e) = status {
                log::warn!("Failed to fix permissions on {}: {e}", target.display());
            }
        }
    }
    Ok(())
}

/// Deletes Steam CEF script and shader caches so modified WebUI scripts load immediately,
/// while safely preserving Chromium profile databases (Web Data, Cookies, Preferences, Login Data).
pub fn clear_cef_cache() -> Result<()> {
    let htmlcache = paths::home_dir().join("Library/Application Support/Steam/config/htmlcache");
    if !htmlcache.is_dir() {
        return Ok(());
    }

    let cache_targets = [
        htmlcache.join("Default/Cache"),
        htmlcache.join("Default/Code Cache"),
        htmlcache.join("Default/GPUCache"),
        htmlcache.join("Default/DawnGraphiteCache"),
        htmlcache.join("Default/DawnWebGPUCache"),
        htmlcache.join("GrShaderCache"),
        htmlcache.join("ShaderCache"),
        htmlcache.join("GraphiteDawnCache"),
    ];

    for target in &cache_targets {
        if target.exists() {
            let _ = fs::remove_dir_all(target);
        }
    }

    // Clean up any old_* directories generated by Chromium
    if let Ok(entries) = fs::read_dir(&htmlcache) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("old_") {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
    if let Ok(entries) = fs::read_dir(htmlcache.join("Default")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("old_") {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }

    log::info!(
        "Safely cleared Steam CEF script/shader caches without disturbing user profile data"
    );
    Ok(())
}

/// Scans all appmanifest_*.acf files and clears UpdateRequired / UpdateQueued / UpdatePaused
/// for apps whose game directories are already populated on disk, setting them to StateFlags 4 (fully installed),
/// AutoUpdateBehavior 1 (only update on launch, preventing background download queueing), and zeroing download counters.
pub fn sanitize_installed_app_manifests() -> Result<usize> {
    let steamapps = paths::steam_data_dir().join("steamapps");
    if !steamapps.is_dir() {
        return Ok(0);
    }
    let mut fixed_count = 0;
    if let Ok(entries) = fs::read_dir(&steamapps) {
        for entry in entries.flatten() {
            let p = entry.path();
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name.starts_with("appmanifest_") && name.ends_with(".acf") {
                let id = name
                    .trim_start_matches("appmanifest_")
                    .trim_end_matches(".acf");
                if let Ok(content) = fs::read_to_string(&p) {
                    let mut installdir = None;
                    for line in content.lines() {
                        let trimmed = line.trim();
                        let quotes: Vec<&str> = trimmed.split('"').collect();
                        if quotes.len() >= 4 && quotes[1] == "installdir" {
                            installdir = Some(quotes[3].to_string());
                        }
                    }
                    let game_dir = installdir
                        .as_ref()
                        .map(|dir| steamapps.join("common").join(dir));
                    let is_installed = game_dir.as_ref().map(|d| d.exists()).unwrap_or(false);

                    // If this is a known native macOS game, but it currently lacks its native macOS executable
                    // (e.g. because Windows depots were previously downloaded), do not force StateFlags 4.
                    // Allow Steam to update / fetch the native macOS binaries.
                    if KNOWN_NATIVE_MAC_APPIDS.contains(&id) {
                        if let Some(ref dir) = game_dir {
                            if !is_directory_native_mac(dir, 3) {
                                continue;
                            }
                        }
                    }

                    if is_installed {
                        let mut skipping_section = false;
                        let mut section_depth = 0;
                        let mut has_auto_update_behavior = false;
                        let mut new_lines = Vec::new();
                        let mut changed = false;

                        for line in content.lines() {
                            let trimmed = line.trim();
                            if skipping_section {
                                if trimmed.contains('{') {
                                    section_depth += 1;
                                }
                                if trimmed.contains('}') {
                                    section_depth -= 1;
                                    if section_depth == 0 {
                                        skipping_section = false;
                                        changed = true;
                                    }
                                }
                                continue;
                            }

                            if trimmed.starts_with("\"StagedDepots\"")
                                || trimmed.starts_with("\"DlcDownloads\"")
                            {
                                skipping_section = true;
                                section_depth = 0;
                                if trimmed.contains('{') {
                                    section_depth += 1;
                                }
                                changed = true;
                                continue;
                            }

                            if trimmed.starts_with("\"StateFlags\"") {
                                if trimmed != "\"StateFlags\"\t\t\"4\""
                                    && trimmed != "\"StateFlags\" \"4\""
                                {
                                    new_lines.push("\t\"StateFlags\"\t\t\"4\"".to_string());
                                    changed = true;
                                } else {
                                    new_lines.push(line.to_string());
                                }
                            } else if trimmed.starts_with("\"AutoUpdateBehavior\"") {
                                has_auto_update_behavior = true;
                                if trimmed != "\"AutoUpdateBehavior\"\t\t\"1\""
                                    && trimmed != "\"AutoUpdateBehavior\" \"1\""
                                {
                                    new_lines.push("\t\"AutoUpdateBehavior\"\t\t\"1\"".to_string());
                                    changed = true;
                                } else {
                                    new_lines.push(line.to_string());
                                }
                            } else if trimmed.starts_with("\"BytesToDownload\"") {
                                if trimmed != "\"BytesToDownload\"\t\t\"0\""
                                    && trimmed != "\"BytesToDownload\" \"0\""
                                {
                                    new_lines.push("\t\"BytesToDownload\"\t\t\"0\"".to_string());
                                    changed = true;
                                } else {
                                    new_lines.push(line.to_string());
                                }
                            } else if trimmed.starts_with("\"BytesDownloaded\"") {
                                if trimmed != "\"BytesDownloaded\"\t\t\"0\""
                                    && trimmed != "\"BytesDownloaded\" \"0\""
                                {
                                    new_lines.push("\t\"BytesDownloaded\"\t\t\"0\"".to_string());
                                    changed = true;
                                } else {
                                    new_lines.push(line.to_string());
                                }
                            } else if trimmed.starts_with("\"BytesToStage\"") {
                                if trimmed != "\"BytesToStage\"\t\t\"0\""
                                    && trimmed != "\"BytesToStage\" \"0\""
                                {
                                    new_lines.push("\t\"BytesToStage\"\t\t\"0\"".to_string());
                                    changed = true;
                                } else {
                                    new_lines.push(line.to_string());
                                }
                            } else if trimmed.starts_with("\"BytesStaged\"") {
                                if trimmed != "\"BytesStaged\"\t\t\"0\""
                                    && trimmed != "\"BytesStaged\" \"0\""
                                {
                                    new_lines.push("\t\"BytesStaged\"\t\t\"0\"".to_string());
                                    changed = true;
                                } else {
                                    new_lines.push(line.to_string());
                                }
                            } else if trimmed.starts_with("\"ScheduledAutoUpdate\"") {
                                if trimmed != "\"ScheduledAutoUpdate\"\t\t\"0\""
                                    && trimmed != "\"ScheduledAutoUpdate\" \"0\""
                                {
                                    new_lines
                                        .push("\t\"ScheduledAutoUpdate\"\t\t\"0\"".to_string());
                                    changed = true;
                                } else {
                                    new_lines.push(line.to_string());
                                }
                            } else if trimmed.starts_with("\"UpdateResult\"") {
                                if trimmed != "\"UpdateResult\"\t\t\"0\""
                                    && trimmed != "\"UpdateResult\" \"0\""
                                {
                                    new_lines.push("\t\"UpdateResult\"\t\t\"0\"".to_string());
                                    changed = true;
                                } else {
                                    new_lines.push(line.to_string());
                                }
                            } else {
                                new_lines.push(line.to_string());
                            }
                        }

                        if !has_auto_update_behavior {
                            if let Some(pos) = new_lines
                                .iter()
                                .position(|l| l.trim().starts_with("\"StateFlags\""))
                            {
                                new_lines.insert(
                                    pos + 1,
                                    "\t\"AutoUpdateBehavior\"\t\t\"1\"".to_string(),
                                );
                                changed = true;
                            }
                        }

                        if changed {
                            let fixed_str = new_lines.join("\n") + "\n";
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::PermissionsExt;
                                if let Ok(meta) = fs::metadata(&p) {
                                    let mut perms = meta.permissions();
                                    perms.set_mode(0o644);
                                    let _ = fs::set_permissions(&p, perms);
                                }
                            }
                            let _ = fs::write(&p, fixed_str);
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::PermissionsExt;
                                if let Ok(meta) = fs::metadata(&p) {
                                    let mut perms = meta.permissions();
                                    perms.set_mode(0o644);
                                    let _ = fs::set_permissions(&p, perms);
                                }
                            }
                            log::info!("Sanitized {} to StateFlags 4 & AutoUpdateBehavior 1 (ready to play)", name);
                            fixed_count += 1;
                        }
                    }
                }
            }
        }
    }

    // Clean up temporary download staging/patch files in steamapps/downloading
    let downloading = steamapps.join("downloading");
    if downloading.is_dir() {
        if let Ok(entries) = fs::read_dir(&downloading) {
            for entry in entries.flatten() {
                let p = entry.path();
                let _ = if p.is_dir() {
                    fs::remove_dir_all(&p)
                } else {
                    fs::remove_file(&p)
                };
            }
        }
    }

    Ok(fixed_count)
}

/// Patches Steam CEF WebUI chunks on disk with exact size-preserving padding
/// so that Steam's bootstrapper passes file verification without downloading updates,
/// while the WebUI gets full Steam Play / compatibility functionality (Play/Install enabled,
/// no crossed-out prohibition icons, Compatibility tab unlocked).
pub fn patch_steamui_chunks() -> Result<usize> {
    let steamui_dir = paths::steam_data_dir().join("Steam.AppBundle/Steam/Contents/MacOS/steamui");
    if !steamui_dir.is_dir() {
        return Ok(0);
    }

    // Try reading expected file sizes from package manifest if available
    let package_manifest = paths::steam_data_dir()
        .join("Steam.AppBundle/Steam/Contents/MacOS/package/steam_client_osx.installed");
    let mut expected_sizes: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    if let Ok(manifest_content) = fs::read_to_string(&package_manifest) {
        for line in manifest_content.lines() {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 2 {
                let filename = parts[0].trim().trim_start_matches("steamui/");
                if let Some(size_part) = parts[1].split(';').next() {
                    if let Ok(sz) = size_part.trim().parse::<usize>() {
                        expected_sizes.insert(filename.to_string(), sz);
                    }
                }
            }
        }
    }

    let mut patched_count = 0;

    if let Ok(entries) = fs::read_dir(&steamui_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if !name.starts_with("chunk~") || !name.ends_with(".js") || name.ends_with(".bak") {
                continue;
            }

            let raw_bytes = match fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };

            let expected_size = expected_sizes.get(name).copied().unwrap_or(raw_bytes.len());

            let content = match String::from_utf8(raw_bytes) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Check if already patched and matching expected size
            if content.contains("\"Playable via Steam Play (Nucleon)\"")
                && content.contains("get is_invalid_os_type(){return false}")
                && !content.contains("(0,h.we)\"Enable Nucleon")
                && content.len() == expected_size
            {
                continue;
            }

            let mut modified = content;
            let mut changed = false;

            // 1. Enable Install button in PlayBar (_e returns s.UM "Install" for u.Ul.KR)
            let target_install_btn = "case u.Ul.jw:return s.UM;";
            let repl_install_btn = "case u.Ul.jw:case u.Ul.KR:return s.UM;";
            if modified.contains(target_install_btn) {
                modified = modified.replace(target_install_btn, repl_install_btn);
                changed = true;
            }

            // 2. Do not treat InvalidPlatform as permanently unavailable
            let target_perm_unavail =
                "case D.Ul.pd:case D.Ul.K5:case D.Ul.KR:case D.Ul.Mu:return!0";
            let repl_perm_unavail = "case D.Ul.pd:case D.Ul.K5:case D.Ul.Mu:return!0";
            if modified.contains(target_perm_unavail) {
                modified = modified.replace(target_perm_unavail, repl_perm_unavail);
                changed = true;
            }

            // 3. Make is_available_on_current_platform return true
            let target_avail_platform = "get is_available_on_current_platform(){return this.local_per_client_data&&this.local_per_client_data.is_available_on_current_platform}";
            let repl_avail_platform = "get is_available_on_current_platform(){return true}";
            if modified.contains(target_avail_platform) {
                modified = modified.replace(target_avail_platform, repl_avail_platform);
                changed = true;
            }

            // 4. Force is_invalid_os_type to false -> Removes crossed prohibition icon & enables Install/Play in Steam UI
            let target_invalid_os = "get is_invalid_os_type(){return this.most_available_per_client_data.is_invalid_os_type}";
            let repl_invalid_os = "get is_invalid_os_type(){return false}";
            if modified.contains(target_invalid_os) {
                modified = modified.replace(target_invalid_os, repl_invalid_os);
                changed = true;
            }

            // 5. Replace InvalidPlatform status text with "Playable via Steam Play (Nucleon)"
            let target_status_text = r##"(0,W.we)("#DisplayStatus_InvalidPlatform")"##;
            let repl_status_text = r##""Playable via Steam Play (Nucleon)""##;
            if modified.contains(target_status_text) {
                modified = modified.replace(target_status_text, repl_status_text);
                changed = true;
            }

            // 6. Do not exclude InvalidPlatform games from collection platform filter
            let target_filter = "r&&e.BIsPerClientDataLocal(r)&&r.display_status==ze.Ul.KR&&(t=!1)";
            let repl_filter = "false&&(t=!1)";
            if modified.contains(target_filter) {
                modified = modified.replace(target_filter, repl_filter);
                changed = true;
            }

            // 7. Enable Compatibility tab in Game Properties
            let target_compat =
                r##"(0,f.CI)()&&o.push({title:(0,A.we)("#AppProperties_CompatibilityPage")"##;
            let repl_compat =
                r##"true&&o.push({title:(0,A.we)("#AppProperties_CompatibilityPage")"##;
            if modified.contains(target_compat) {
                modified = modified.replace(target_compat, repl_compat);
                changed = true;
            }

            // 8. Always enable Compatibility tool force checkbox in Game Properties
            let target_compat_enabled = "()=>u.rV.settings.bCompatEnabled";
            let repl_compat_enabled = "()=>true";
            if modified.contains(target_compat_enabled) {
                modified = modified.replace(target_compat_enabled, repl_compat_enabled);
                changed = true;
            }

            // 9. Compatibility tab in Steam Settings
            let target_settings = "Compatibility:{visible:t&&(0,f.CI)()&&!(0,f.rf)()";
            let repl_settings = "Compatibility:{visible:t&&true&&!(0,f.rf)()";
            if modified.contains(target_settings) {
                modified = modified.replace(target_settings, repl_settings);
                changed = true;
            }

            // 10. Fallback global compat tool in Steam Settings dropdown
            let target_tool_default = "const t=(0,c.t0)().strCompatTool,";
            let repl_tool_default =
                r#"const t=(0,c.t0)().strCompatTool||(A.length?A[0].data:"nucleon"),"#;
            if modified.contains(target_tool_default) {
                modified = modified.replace(target_tool_default, repl_tool_default);
                changed = true;
            }

            // 11. SteamPlay section in Steam Settings
            if modified.contains("function ue(e){return(0,T.CI)()?") {
                modified = modified.replace(
                    "function ue(e){return(0,T.CI)()?",
                    "function ue(e){return true?",
                );
                changed = true;
            }

            // 12. Add Non-Steam EXE filter
            let target_exe = r##"("#AddNonSteam_Filter_Exe_MacOS"),rFilePatterns:["*.app"]"##;
            let repl_exe = r##"("#AddNonSteam_Filter_Exe_MacOS"),rFilePatterns:["*.app","*.exe"]"##;
            if modified.contains(target_exe) {
                modified = modified.replace(target_exe, repl_exe);
                changed = true;
            }

            // 13. Allow .exe in image / executable filters
            let target_img = r##"{strFileTypeName:"Image Files (*.tga,*.png)",rFilePatterns:["*.tga","*.png"]}"##;
            let repl_img = r##"{strFileTypeName:"Image Files (*.tga,*.png,*.exe)",rFilePatterns:["*.tga","*.png","*.exe"]}"##;
            if modified.contains(target_img) {
                modified = modified.replace(target_img, repl_img);
                changed = true;
            }

            // 14. Game list entry notice for Windows apps
            let target_entry = r##"(0,h.we)("#GameList_Entry_Invalid_OSType2")"##;
            let repl_entry = r##""Enable Nucleon under Properties > Compatibility to install and run the Windows version.""##;
            if modified.contains(target_entry) {
                modified = modified.replace(target_entry, repl_entry);
                changed = true;
            }
            if modified.contains("(0,h.we)\"Enable Nucleon") {
                modified = modified.replace("(0,h.we)\"Enable Nucleon", "\"Enable Nucleon");
                changed = true;
            }

            if changed {
                // Strip any existing padding comment before recalculating
                if let Some(pos) = modified.rfind("/*") {
                    if modified[pos..].ends_with("*/") && modified[pos..].contains('*') {
                        modified.truncate(pos);
                    }
                }

                // Exact-size padding: ensure modified.len() == expected_size
                let new_len = modified.len();
                if new_len < expected_size {
                    let diff = expected_size - new_len;
                    if diff >= 4 {
                        let pad = format!("/*{}*/", "*".repeat(diff - 4));
                        modified.push_str(&pad);
                    } else {
                        modified.push_str(&" ".repeat(diff));
                    }
                }

                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(meta) = fs::metadata(&path) {
                        let mut perms = meta.permissions();
                        perms.set_mode(0o755);
                        let _ = fs::set_permissions(&path, perms);
                    }
                }

                let _ = fs::write(&path, modified);

                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(meta) = fs::metadata(&path) {
                        let mut perms = meta.permissions();
                        perms.set_mode(0o755);
                        let _ = fs::set_permissions(&path, perms);
                    }
                }

                log::info!(
                    "Patched SteamUI chunk {} (size preserved: {} bytes)",
                    name,
                    expected_size
                );
                patched_count += 1;
            }
        }
    }

    if patched_count > 0 {
        let _ = clear_cef_cache();
    }

    Ok(patched_count)
}

/// Parses and updates CompatToolMapping inside `config.vdf` content safely using balanced brace counting.
/// Ensures all target Windows AppIDs are mapped to "nucleon", while stripping native AppIDs and wildcard "0".
pub fn update_compat_tool_mapping(
    content: &str,
    target_appids: &[String],
    native_appids: &std::collections::HashSet<String>,
) -> (String, bool) {
    let mut updated = content.to_string();
    let mut modified = false;

    let entry_re = regex::Regex::new(r#""(\d+)"\s*\{([^}]*)\}"#).unwrap();
    let name_re = regex::Regex::new(r#""name"\s+"([^"]*)""#).unwrap();
    let config_re = regex::Regex::new(r#""config"\s+"([^"]*)""#).unwrap();
    let priority_re = regex::Regex::new(r#""priority"\s+"([^"]*)""#).unwrap();

    let mut entries_map: std::collections::BTreeMap<String, (String, String, String)> =
        std::collections::BTreeMap::new();

    if let Some(compat_idx) = updated.find("\"CompatToolMapping\"") {
        if let Some(open_rel) = updated[compat_idx..].find('{') {
            let open_pos = compat_idx + open_rel;
            let mut depth = 1;
            let mut close_pos = None;
            for (i, c) in updated[open_pos + 1..].char_indices() {
                if c == '{' {
                    depth += 1;
                } else if c == '}' {
                    depth -= 1;
                    if depth == 0 {
                        close_pos = Some(open_pos + 1 + i);
                        break;
                    }
                }
            }

            if let Some(close_pos) = close_pos {
                let inner = &updated[open_pos + 1..close_pos];
                for cap in entry_re.captures_iter(inner) {
                    let appid = cap[1].to_string();
                    let body = &cap[2];
                    if native_appids.contains(&appid) {
                        modified = true;
                        continue;
                    }
                    let mut name = name_re
                        .captures(body)
                        .and_then(|c| c.get(1))
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_else(|| "nucleon".to_string());
                    if !runner::is_kosmickrisp_installed() && name == "nucleon-kosmickrisp" {
                        name = "nucleon".to_string();
                        modified = true;
                    }
                    let config = config_re
                        .captures(body)
                        .and_then(|c| c.get(1))
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_default();
                    let priority = priority_re
                        .captures(body)
                        .and_then(|c| c.get(1))
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_else(|| "250".to_string());
                    entries_map.insert(appid, (name, config, priority));
                }

                for target_id in target_appids {
                    if !entries_map.contains_key(target_id) {
                        entries_map.insert(
                            target_id.clone(),
                            ("nucleon".to_string(), "".to_string(), "250".to_string()),
                        );
                        modified = true;
                    }
                }

                let mut new_inner = String::new();
                for (id, (name, config, priority)) in &entries_map {
                    new_inner.push_str(&format!(
                        "\n\t\t\t\t\t\"{id}\"\n\t\t\t\t\t{{\n\t\t\t\t\t\t\"name\"\t\t\"{name}\"\n\t\t\t\t\t\t\"config\"\t\t\"{config}\"\n\t\t\t\t\t\t\"priority\"\t\t\"{priority}\"\n\t\t\t\t\t}}"
                    ));
                }
                new_inner.push_str("\n\t\t\t\t");

                if new_inner != inner {
                    updated.replace_range(open_pos + 1..close_pos, &new_inner);
                    modified = true;
                }
            }
        }
    } else if let Some(steam_idx) = updated.find("\"Steam\"") {
        if let Some(open_rel) = updated[steam_idx..].find('{') {
            let open_pos = steam_idx + open_rel;
            for target_id in target_appids {
                entries_map.insert(
                    target_id.clone(),
                    ("nucleon".to_string(), "".to_string(), "250".to_string()),
                );
            }
            let mut entries_str = String::new();
            for (id, (name, config, priority)) in &entries_map {
                entries_str.push_str(&format!(
                    "\n\t\t\t\t\t\"{id}\"\n\t\t\t\t\t{{\n\t\t\t\t\t\t\"name\"\t\t\"{name}\"\n\t\t\t\t\t\t\"config\"\t\t\"{config}\"\n\t\t\t\t\t\t\"priority\"\t\t\"{priority}\"\n\t\t\t\t\t}}"
                ));
            }
            entries_str.push_str("\n\t\t\t\t");
            let compat_block =
                format!("\n\t\t\t\t\"CompatToolMapping\"\n\t\t\t\t{{{entries_str}}}");
            updated.insert_str(open_pos + 1, &compat_block);
            modified = true;
        }
    }

    (updated, modified)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_update_compat_tool_mapping_excludes_native_and_wildcard() {
        let sample = r#""InstallConfigStore"
{
	"Software"
	{
		"Valve"
		{
			"Steam"
			{
				"AutoUpdateWindowEnabled"		"0"
				"CompatToolMapping"
				{
					"0"
					{
						"name"		"nucleon"
						"config"		""
						"priority"		"250"
					}
					"1091500"
					{
						"name"		"nucleon"
						"config"		""
						"priority"		"250"
					}
					"480"
					{
						"name"		"nucleon"
						"config"		""
						"priority"		"250"
					}
				}
				"ipv6check_http_state"		"bad"
			}
		}
	}
}"#;

        let target_appids = vec!["228980".to_string(), "690790".to_string()];
        let mut native_appids = HashSet::new();
        native_appids.insert("0".to_string());
        native_appids.insert("1091500".to_string());

        let (result, modified) = update_compat_tool_mapping(sample, &target_appids, &native_appids);
        assert!(modified);

        // Wildcard 0 and Cyberpunk 1091500 must not be in CompatToolMapping
        assert!(!result.contains(
            r#""0"
					{"#
        ));
        assert!(!result.contains(r#""1091500""#));

        // Windows apps 228980 and 690790 must be mapped to nucleon
        assert!(result.contains(r#""228980""#));
        assert!(result.contains(r#""690790""#));
        assert!(result.contains(r#""name"		"nucleon""#));

        // Verify balance of braces
        let mut depth = 0;
        for c in result.chars() {
            if c == '{' {
                depth += 1;
            } else if c == '}' {
                depth -= 1;
            }
        }
        assert_eq!(depth, 0);
    }

    #[test]
    fn test_update_compat_tool_mapping_removes_stellaris_and_dirt4_native_games() {
        let sample = r#""InstallConfigStore"
{
	"Software"
	{
		"Valve"
		{
			"Steam"
			{
				"CompatToolMapping"
				{
					"281990"
					{
						"name"		"nucleon"
						"config"		""
						"priority"		"250"
					}
					"421020"
					{
						"name"		"nucleon"
						"config"		""
						"priority"		"250"
					}
					"690790"
					{
						"name"		"nucleon"
						"config"		""
						"priority"		"250"
					}
				}
			}
		}
	}
}"#;

        let mut native_appids = HashSet::new();
        native_appids.insert("0".to_string());
        for &id in KNOWN_NATIVE_MAC_APPIDS {
            native_appids.insert(id.to_string());
        }

        let (result, modified) = update_compat_tool_mapping(sample, &[], &native_appids);
        assert!(modified);

        // Stellaris (281990) and DiRT 4 (421020) must be stripped from CompatToolMapping
        assert!(!result.contains(r#""281990""#));
        assert!(!result.contains(r#""421020""#));

        // Windows game DiRT Rally 2.0 (690790) must be preserved
        assert!(result.contains(r#""690790""#));
        assert!(result.contains(r#""name"		"nucleon""#));
    }

    #[test]
    fn test_update_compat_tool_mapping_creates_section_when_missing() {
        let sample = r#""InstallConfigStore"
{
	"Software"
	{
		"Valve"
		{
			"Steam"
			{
				"AutoUpdateWindowEnabled"		"0"
			}
		}
	}
}"#;

        let target_appids = vec!["437570".to_string()];
        let native_appids = HashSet::new();

        let (result, modified) = update_compat_tool_mapping(sample, &target_appids, &native_appids);
        assert!(modified);
        assert!(result.contains(r#""CompatToolMapping""#));
        assert!(result.contains(r#""437570""#));

        let mut depth = 0;
        for c in result.chars() {
            if c == '{' {
                depth += 1;
            } else if c == '}' {
                depth -= 1;
            }
        }
        assert_eq!(depth, 0);
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_update_compat_tool_mapping_preserves_kosmickrisp_when_installed() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("KOSMICKRISP_FORCE", "1");
        let sample = r#""InstallConfigStore"
{
	"Software"
	{
		"Valve"
		{
			"Steam"
			{
				"CompatToolMapping"
				{
					"228980"
					{
						"name"		"nucleon-kosmickrisp"
						"config"		""
						"priority"		"250"
					}
				}
			}
		}
	}
}"#;

        let target_appids = vec!["228980".to_string()];
        let native_appids = HashSet::new();

        let (result, _) = update_compat_tool_mapping(sample, &target_appids, &native_appids);
        std::env::remove_var("KOSMICKRISP_FORCE");
        assert!(result.contains(r#""name"		"nucleon-kosmickrisp""#));
    }

    #[test]
    fn test_update_compat_tool_mapping_reverts_kosmickrisp_when_not_installed() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("KOSMICKRISP_DISABLE", "1");
        let sample = r#""InstallConfigStore"
{
	"Software"
	{
		"Valve"
		{
			"Steam"
			{
				"CompatToolMapping"
				{
					"228980"
					{
						"name"		"nucleon-kosmickrisp"
						"config"		""
						"priority"		"250"
					}
				}
			}
		}
	}
}"#;

        let target_appids = vec!["228980".to_string()];
        let native_appids = HashSet::new();

        let (result, modified) = update_compat_tool_mapping(sample, &target_appids, &native_appids);
        std::env::remove_var("KOSMICKRISP_DISABLE");
        assert!(modified);
        assert!(result.contains(r#""name"		"nucleon""#));
        assert!(!result.contains("nucleon-kosmickrisp"));
    }
}
