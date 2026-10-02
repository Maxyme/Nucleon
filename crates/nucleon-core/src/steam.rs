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

/// Ensure all files in Steam's config, downloading, temp, and common game folders have owner read/write permissions
/// to prevent Steam from throwing 'Missing file privileges' (errno 13 EACCES) during updates or downloads.
pub fn fix_steam_permissions() -> Result<()> {
    let steam_dir = paths::steam_data_dir();
    let targets = [
        steam_dir.join("config"),
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

/// Deletes Steam CEF HTML and code caches so modified WebUI scripts load immediately.
pub fn clear_cef_cache() -> Result<()> {
    let htmlcache = paths::home_dir().join("Library/Application Support/Steam/config/htmlcache");
    if htmlcache.exists() {
        let _ = fs::remove_dir_all(&htmlcache);
        log::info!("Cleared Steam CEF cache at {}", htmlcache.display());
    }
    Ok(())
}

/// Patches Steam CEF WebUI chunks on disk to enable the Install button for Windows games
/// on macOS and expose the Steam Play / Compatibility settings tab.
pub fn patch_steamui_chunks() -> Result<usize> {
    let steamui_dir = paths::steam_data_dir().join("Steam.AppBundle/Steam/Contents/MacOS/steamui");
    if !steamui_dir.is_dir() {
        return Ok(0);
    }

    let mut patched_count = 0;
    for entry in fs::read_dir(&steamui_dir)?.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.starts_with("chunk~") && name.ends_with(".js") {
            let bak = path.with_extension("js.bak");
            // If backup exists, always read from pristine backup so updated patch sets apply cleanly
            let content = if bak.exists() {
                match fs::read_to_string(&bak) {
                    Ok(c) => c,
                    Err(_) => match fs::read_to_string(&path) {
                        Ok(c) => c,
                        Err(_) => continue,
                    },
                }
            } else {
                match fs::read_to_string(&path) {
                    Ok(c) => {
                        let _ = fs::copy(&path, &bak);
                        c
                    }
                    Err(_) => continue,
                }
            };

            let mut modified = content.clone();
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

            // 4. Force is_invalid_os_type to false -> Enables Install button in Steam UI
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
            let target_entry = r##"("#GameList_Entry_Invalid_OSType2")"##;
            let repl_entry = r##""Enable Nucleon under Properties > Compatibility to install and run the Windows version.""##;
            if modified.contains(target_entry) {
                modified = modified.replace(target_entry, repl_entry);
                changed = true;
            }

            if changed {
                let perms = fs::metadata(&path)?.permissions();
                let _ = fs::write(&path, modified);
                let _ = fs::set_permissions(&path, perms);
                log::info!("Patched SteamUI chunk on disk: {}", path.display());
                patched_count += 1;
            }
        }
    }

    let _ = clear_cef_cache();
    Ok(patched_count)
}



