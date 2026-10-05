use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use anyhow::{bail, Context, Result};
use crate::paths;
use crate::vdf;

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

pub fn is_steam_patched() -> bool {
    let plist = paths::steam_info_plist();
    if !plist.exists() {
        return false;
    }
    let content = fs::read_to_string(&plist).unwrap_or_default();
    content.contains("LSEnvironment") && (content.contains("DYLD_INSERT_LIBRARIES") || content.contains("nucleon.dylib"))
}

pub fn patch_steam(hook_dylib: &Path) -> Result<()> {
    check_steam_installed()?;
    let plist = paths::steam_info_plist();
    let backup = paths::support_dir().join("Steam-Info.plist.bak");

    if !backup.exists() {
        fs::copy(&plist, &backup)
            .with_context(|| format!("Failed to backup Info.plist to {}", backup.display()))?;
    }

    // Remove legacy notproton.dylib if present from previous installations
    let legacy_dylib = paths::steam_app().join("Contents/MacOS/notproton.dylib");
    if legacy_dylib.exists() {
        let _ = fs::remove_file(&legacy_dylib);
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
                    &format!("Add :LSEnvironment:DYLD_INSERT_LIBRARIES string {}", dylib_str),
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
        fs::copy(&backup, &plist)
            .with_context(|| "Failed to restore Info.plist from backup")?;
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
                .args(["-c", "Delete :LSEnvironment:DYLD_INSERT_LIBRARIES", plist.to_str().unwrap()])
                .status();
        }
    }

    let dst_dylib = paths::steam_app().join("Contents/MacOS/nucleon.dylib");
    if dst_dylib.exists() {
        let _ = fs::remove_file(&dst_dylib);
    }
    let legacy_dylib = paths::steam_app().join("Contents/MacOS/notproton.dylib");
    if legacy_dylib.exists() {
        let _ = fs::remove_file(&legacy_dylib);
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

pub fn refresh_launch_services(app_path: &Path) -> Result<()> {
    let lsregister = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
    if Path::new(lsregister).exists() {
        let _ = Command::new(lsregister)
            .args(["-f", app_path.to_str().unwrap()])
            .status();
    }
    Ok(())
}

pub fn install_compatibility_tool(runner_bin: &Path) -> Result<()> {
    let nucleon_tool_dir = paths::steam_compat_tools_dir();
    vdf::write_tool_bundle(&nucleon_tool_dir, "nucleon", "Nucleon (Game Porting Toolkit 4)", runner_bin)?;

    // Also register backward-compatible notproton tool entry so existing games never break
    let notproton_tool_dir = paths::home_dir().join("Library/Application Support/Steam/compatibilitytools.d/notproton");
    vdf::write_tool_bundle(&notproton_tool_dir, "notproton", "Game Porting Toolkit 4 (Nucleon/NotProton)", runner_bin)?;

    // Migrate any legacy config.vdf mappings from notproton to nucleon
    let _ = migrate_compat_mappings();

    // Ensure all installed games in steamapps/ are registered in libraryfolders.vdf
    let _ = sync_library_folders();

    // Ensure Steam downloading and library permissions prevent "missing file privileges"
    let _ = fix_steam_permissions();

    // Patch SteamUI chunks to enable Install button and Compatibility settings
    let _ = patch_steamui_chunks();

    Ok(())
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

    if updated.contains("\"notproton\"") {
        updated = updated.replace("\"notproton\"", "\"nucleon\"");
        modified = true;
        log::info!("Migrated legacy 'notproton' compatibility tool mappings to 'nucleon' in config.vdf");
    }

    // Ensure wildcard mapping "0" is registered under CompatToolMapping
    if let Some(idx) = updated.find("\"CompatToolMapping\"") {
        let after_key = &updated[idx..];
        if let Some(brace_rel) = after_key.find('{') {
            let brace_idx = idx + brace_rel;
            let section_end = updated[brace_idx..].find('}').unwrap_or(0);
            let section = &updated[brace_idx..brace_idx + section_end];
            if !section.contains("\"0\"") {
                let wildcard = "\n\t\t\t\t\t\"0\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"name\"\t\t\"nucleon\"\n\t\t\t\t\t\t\"config\"\t\t\"\"\n\t\t\t\t\t\t\"priority\"\t\t\"250\"\n\t\t\t\t\t}";
                updated.insert_str(brace_idx + 1, wildcard);
                modified = true;
                log::info!("Injected global wildcard '0' -> 'nucleon' mapping into config.vdf");
            }
        }
    } else if let Some(steam_idx) = updated.find("\"Steam\"") {
        let after_steam = &updated[steam_idx..];
        if let Some(brace_rel) = after_steam.find('{') {
            let brace_idx = steam_idx + brace_rel;
            let compat_block = "\n\t\t\t\t\"CompatToolMapping\"\n\t\t\t\t{\n\t\t\t\t\t\"0\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"name\"\t\t\"nucleon\"\n\t\t\t\t\t\t\"config\"\t\t\"\"\n\t\t\t\t\t\t\"priority\"\t\t\"250\"\n\t\t\t\t\t}\n\t\t\t\t}";
            updated.insert_str(brace_idx + 1, compat_block);
            modified = true;
            log::info!("Created CompatToolMapping with global wildcard '0' -> 'nucleon' mapping in config.vdf");
        }
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

    log::info!("Safely cleared Steam CEF script/shader caches without disturbing user profile data");
    Ok(())
}

/// Ensures Steam CEF WebUI chunks on disk remain in their pristine Valve state
/// to prevent Steam's bootstrapper from detecting size mismatches and forcing update loops.
/// WebUI compatibility patches are injected dynamically in memory via nucleon.dylib (webpatch).
pub fn patch_steamui_chunks() -> Result<usize> {
    let steamui_dir = paths::steam_data_dir().join("Steam.AppBundle/Steam/Contents/MacOS/steamui");
    if !steamui_dir.is_dir() {
        return Ok(0);
    }

    let mut restored_count = 0;
    if let Ok(entries) = fs::read_dir(&steamui_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name.starts_with("chunk~") && name.ends_with(".js.bak") {
                let js_path = path.with_extension(""); // strips .bak -> .js
                if let Ok(pristine) = fs::read_to_string(&path) {
                    let _ = fs::write(&js_path, pristine);
                }
                let _ = fs::remove_file(&path);
                restored_count += 1;
            }
        }
    }

    if restored_count > 0 {
        log::info!("Restored {} pristine SteamUI WebUI chunk(s) on disk (in-memory dynamic patching active)", restored_count);
    }

    let _ = clear_cef_cache();
    Ok(restored_count)
}



