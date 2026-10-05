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

    // Sanitize any app manifests stuck in UpdateRequired (StateFlags 6)
    let _ = sanitize_installed_app_manifests();

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

    // Collect all installed appids from steamapps/
    let mut target_appids = vec!["0".to_string()];
    let steamapps = paths::steam_data_dir().join("steamapps");
    if let Ok(entries) = fs::read_dir(&steamapps) {
        for entry in entries.flatten() {
            let p = entry.path();
            if let Some(file_name) = p.file_name().and_then(|s| s.to_str()) {
                if file_name.starts_with("appmanifest_") && file_name.ends_with(".acf") {
                    let id = file_name.trim_start_matches("appmanifest_").trim_end_matches(".acf");
                    if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
                        target_appids.push(id.to_string());
                    }
                }
            }
        }
    }

    // Ensure mapping for wildcard "0" and all installed apps is registered under CompatToolMapping
    if let Some(idx) = updated.find("\"CompatToolMapping\"") {
        let after_key = &updated[idx..];
        if let Some(brace_rel) = after_key.find('{') {
            let brace_idx = idx + brace_rel;
            let section_end = updated[brace_idx..].find('}').unwrap_or(0);
            let section = updated[brace_idx..brace_idx + section_end].to_string();
            let mut to_insert = String::new();
            for id in &target_appids {
                let needle = format!("\"{id}\"");
                if !section.contains(&needle) {
                    to_insert.push_str(&format!("\n\t\t\t\t\t\"{id}\"\n\t\t\t\t\t{{\n\t\t\t\t\t\t\"name\"\t\t\"nucleon\"\n\t\t\t\t\t\t\"config\"\t\t\"\"\n\t\t\t\t\t\t\"priority\"\t\t\"250\"\n\t\t\t\t\t}}"));
                    log::info!("Injected '{id}' -> 'nucleon' mapping into config.vdf");
                }
            }
            if !to_insert.is_empty() {
                updated.insert_str(brace_idx + 1, &to_insert);
                modified = true;
            }
        }
    } else if let Some(steam_idx) = updated.find("\"Steam\"") {
        let after_steam = &updated[steam_idx..];
        if let Some(brace_rel) = after_steam.find('{') {
            let brace_idx = steam_idx + brace_rel;
            let mut entries_str = String::new();
            for id in &target_appids {
                entries_str.push_str(&format!("\n\t\t\t\t\t\"{id}\"\n\t\t\t\t\t{{\n\t\t\t\t\t\t\"name\"\t\t\"nucleon\"\n\t\t\t\t\t\t\"config\"\t\t\"\"\n\t\t\t\t\t\t\"priority\"\t\t\"250\"\n\t\t\t\t\t}}"));
            }
            let compat_block = format!("\n\t\t\t\t\"CompatToolMapping\"\n\t\t\t\t{{{entries_str}\n\t\t\t\t}}");
            updated.insert_str(brace_idx + 1, &compat_block);
            modified = true;
            log::info!("Created CompatToolMapping with installed apps -> 'nucleon' mappings in config.vdf");
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

/// Scans all appmanifest_*.acf files and clears UpdateRequired (StateFlags 6 or 2)
/// for apps whose game directories are already populated on disk, setting them to StateFlags 4 (fully installed).
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
                if let Ok(content) = fs::read_to_string(&p) {
                    if content.contains("\"StateFlags\"\t\t\"6\"") || content.contains("\"StateFlags\"\t\t\"2\"") {
                        let mut installdir = None;
                        for line in content.lines() {
                            let trimmed = line.trim();
                            if trimmed.starts_with("\"installdir\"") {
                                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                                if parts.len() >= 2 {
                                    installdir = Some(parts[1].trim_matches('"').to_string());
                                }
                            }
                        }
                        let is_installed = if let Some(ref dir) = installdir {
                            steamapps.join("common").join(dir).exists()
                        } else {
                            false
                        };

                        if is_installed {
                            let mut fixed = content.replace("\"StateFlags\"\t\t\"6\"", "\"StateFlags\"\t\t\"4\"");
                            fixed = fixed.replace("\"StateFlags\"\t\t\"2\"", "\"StateFlags\"\t\t\"4\"");
                            let mut lines = Vec::new();
                            for line in fixed.lines() {
                                if line.trim().starts_with("\"BytesToDownload\"") {
                                    lines.push("\t\"BytesToDownload\"\t\t\"0\"".to_string());
                                } else if line.trim().starts_with("\"BytesDownloaded\"") {
                                    lines.push("\t\"BytesDownloaded\"\t\t\"0\"".to_string());
                                } else {
                                    lines.push(line.to_string());
                                }
                            }
                            let fixed_str = lines.join("\n") + "\n";
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
                            log::info!("Sanitized {} to StateFlags 4 (fully installed)", name);
                            fixed_count += 1;
                        }
                    }
                }
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
    let package_manifest = paths::steam_data_dir().join("Steam.AppBundle/Steam/Contents/MacOS/package/steam_client_osx.installed");
    let mut expected_sizes: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
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
            let target_perm_unavail = "case D.Ul.pd:case D.Ul.K5:case D.Ul.KR:case D.Ul.Mu:return!0";
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
            let target_compat = r##"(0,f.CI)()&&o.push({title:(0,A.we)("#AppProperties_CompatibilityPage")"##;
            let repl_compat = r##"true&&o.push({title:(0,A.we)("#AppProperties_CompatibilityPage")"##;
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
            let repl_tool_default = r#"const t=(0,c.t0)().strCompatTool||(A.length?A[0].data:"nucleon"),"#;
            if modified.contains(target_tool_default) {
                modified = modified.replace(target_tool_default, repl_tool_default);
                changed = true;
            }

            // 11. SteamPlay section in Steam Settings
            if modified.contains("function ue(e){return(0,T.CI)()?") {
                modified = modified.replace("function ue(e){return(0,T.CI)()?", "function ue(e){return true?");
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

                log::info!("Patched SteamUI chunk {} (size preserved: {} bytes)", name, expected_size);
                patched_count += 1;
            }
        }
    }

    if patched_count > 0 {
        let _ = clear_cef_cache();
    }

    Ok(patched_count)
}



