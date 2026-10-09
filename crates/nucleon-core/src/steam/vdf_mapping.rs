use super::native_detect::find_native_mac_appids;
use crate::paths;
use crate::runner;
use anyhow::Result;
use std::collections::HashSet;
use std::fs;

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
    let mut native_appids = find_native_mac_appids();
    native_appids.insert("0".to_string());

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
                        if let Ok(partial) = keyvalues_parser::parse(&content) {
                            let vdf = keyvalues_parser::Vdf::from(partial);
                            if let keyvalues_parser::Value::Obj(ref obj) = vdf.value {
                                let appid = obj
                                    .get("appid")
                                    .and_then(|v| v.first())
                                    .and_then(|v| v.get_str())
                                    .map(|s| s.to_string());
                                let size = obj
                                    .get("SizeOnDisk")
                                    .and_then(|v| v.first())
                                    .and_then(|v| v.get_str())
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| "0".to_string());
                                if let Some(id) = appid {
                                    installed_apps.push((id, size));
                                }
                            }
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

        let Ok(partial) = keyvalues_parser::parse(&content) else {
            continue;
        };
        let mut vdf = keyvalues_parser::Vdf::from(partial).into_owned();
        let mut modified = false;

        if let keyvalues_parser::Value::Obj(ref mut root_obj) = vdf.value {
            for folder_vals in root_obj.values_mut() {
                for folder_val in folder_vals {
                    if let keyvalues_parser::Value::Obj(ref mut folder_obj) = folder_val {
                        if let Some(apps_obj) = folder_obj
                            .get_mut("apps")
                            .and_then(|v| v.first_mut())
                            .and_then(|v| v.get_mut_obj())
                        {
                            for (id, sz) in &installed_apps {
                                if !apps_obj.contains_key(id.as_str()) {
                                    apps_obj.insert(
                                        std::borrow::Cow::Owned(id.clone()),
                                        vec![keyvalues_parser::Value::Str(
                                            std::borrow::Cow::Owned(sz.clone()),
                                        )],
                                    );
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
            let _ = fs::write(lib_path, vdf.to_string());
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

fn set_vdf_str_field(obj: &mut keyvalues_parser::Obj<'static>, key: &str, new_val: &str) -> bool {
    let current = obj
        .get(key)
        .and_then(|v| v.first())
        .and_then(|v| v.get_str());
    if current != Some(new_val) {
        obj.insert(
            std::borrow::Cow::Owned(key.to_string()),
            vec![keyvalues_parser::Value::Str(std::borrow::Cow::Owned(
                new_val.to_string(),
            ))],
        );
        true
    } else {
        false
    }
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
                if let Ok(content) = fs::read_to_string(&p) {
                    let Ok(partial) = keyvalues_parser::parse(&content) else {
                        continue;
                    };
                    let mut vdf = keyvalues_parser::Vdf::from(partial).into_owned();

                    let installdir = if let keyvalues_parser::Value::Obj(ref obj) = vdf.value {
                        obj.get("installdir")
                            .and_then(|v| v.first())
                            .and_then(|v| v.get_str())
                            .map(|s| s.to_string())
                    } else {
                        None
                    };

                    let game_dir = installdir
                        .as_ref()
                        .map(|dir| steamapps.join("common").join(dir));
                    let is_installed = game_dir.as_ref().map(|d| d.exists()).unwrap_or(false);

                    if is_installed {
                        if let keyvalues_parser::Value::Obj(ref mut obj) = vdf.value {
                            let mut changed = false;

                            // 1. Remove temporary sections
                            if obj.remove("StagedDepots").is_some() {
                                changed = true;
                            }
                            if obj.remove("DlcDownloads").is_some() {
                                changed = true;
                            }

                            // 2. Set StateFlags to 4 (fully installed)
                            if set_vdf_str_field(obj, "StateFlags", "4") {
                                changed = true;
                            }

                            // 3. Set AutoUpdateBehavior to 1 (only update on launch)
                            if set_vdf_str_field(obj, "AutoUpdateBehavior", "1") {
                                changed = true;
                            }

                            // 4. Zero download and staging counters
                            for field in &[
                                "BytesToDownload",
                                "BytesDownloaded",
                                "BytesToStage",
                                "BytesStaged",
                                "ScheduledAutoUpdate",
                                "UpdateResult",
                            ] {
                                if set_vdf_str_field(obj, field, "0") {
                                    changed = true;
                                }
                            }

                            if changed {
                                #[cfg(unix)]
                                {
                                    use std::os::unix::fs::PermissionsExt;
                                    if let Ok(meta) = fs::metadata(&p) {
                                        let mut perms = meta.permissions();
                                        perms.set_mode(0o644);
                                        let _ = fs::set_permissions(&p, perms);
                                    }
                                }
                                let _ = fs::write(&p, vdf.to_string());
                                #[cfg(unix)]
                                {
                                    use std::os::unix::fs::PermissionsExt;
                                    if let Ok(meta) = fs::metadata(&p) {
                                        let mut perms = meta.permissions();
                                        perms.set_mode(0o644);
                                        let _ = fs::set_permissions(&p, perms);
                                    }
                                }
                                log::info!(
                                    "Sanitized {} to StateFlags 4 & AutoUpdateBehavior 1 (ready to play)",
                                    name
                                );
                                fixed_count += 1;
                            }
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

/// Helper to recursively find a nested object by key in a KeyValues AST.
fn find_child_obj_mut<'a>(
    obj: &'a mut keyvalues_parser::Obj<'static>,
    target_key: &str,
) -> Option<&'a mut keyvalues_parser::Obj<'static>> {
    if obj.contains_key(target_key) {
        let values = obj.get_mut(target_key)?;
        for val in values {
            if let keyvalues_parser::Value::Obj(child_obj) = val {
                return Some(child_obj);
            }
        }
        return None;
    }

    for values in obj.values_mut() {
        for val in values {
            if let keyvalues_parser::Value::Obj(child_obj) = val {
                if let Some(found) = find_child_obj_mut(child_obj, target_key) {
                    return Some(found);
                }
            }
        }
    }

    None
}

/// Ensures all target Windows AppIDs are mapped to "nucleon", while stripping native AppIDs and wildcard "0".
pub fn update_compat_tool_mapping(
    content: &str,
    target_appids: &[String],
    native_appids: &HashSet<String>,
) -> (String, bool) {
    let Ok(partial) = keyvalues_parser::parse(content) else {
        return (content.to_string(), false);
    };
    let mut vdf = keyvalues_parser::Vdf::from(partial).into_owned();
    let mut modified = false;

    // Locate CompatToolMapping or create it under Steam
    let compat_obj = if vdf.key == "CompatToolMapping" {
        vdf.value.get_mut_obj()
    } else if let keyvalues_parser::Value::Obj(ref mut root_obj) = vdf.value {
        if let Some(found) = find_child_obj_mut(root_obj, "CompatToolMapping") {
            Some(found)
        } else {
            // CompatToolMapping not found, check if Steam exists
            let steam_obj = if vdf.key == "Steam" {
                vdf.value.get_mut_obj()
            } else {
                find_child_obj_mut(root_obj, "Steam")
            };

            if let Some(steam) = steam_obj {
                steam.insert(
                    std::borrow::Cow::Borrowed("CompatToolMapping"),
                    vec![keyvalues_parser::Value::Obj(keyvalues_parser::Obj::new())],
                );
                modified = true;
                steam
                    .get_mut("CompatToolMapping")
                    .and_then(|v| v.first_mut())
                    .and_then(|v| v.get_mut_obj())
            } else {
                None
            }
        }
    } else {
        None
    };

    let Some(compat) = compat_obj else {
        return (content.to_string(), false);
    };

    // 1. Remove native AppIDs and wildcard "0"
    let to_remove: Vec<String> = compat
        .keys()
        .filter(|appid| native_appids.contains(appid.as_ref()))
        .map(|k| k.to_string())
        .collect();
    for appid in to_remove {
        compat.remove(appid.as_str());
        modified = true;
    }

    // 2. Revert KosmicKrisp if not installed
    if !runner::is_kosmickrisp_installed() {
        for values in compat.values_mut() {
            for val in values {
                if let keyvalues_parser::Value::Obj(entry_obj) = val {
                    let is_kosmickrisp = entry_obj
                        .get("name")
                        .and_then(|v| v.first())
                        .and_then(|v| v.get_str())
                        .map(|s| s == "nucleon-kosmickrisp")
                        .unwrap_or(false);
                    if is_kosmickrisp {
                        entry_obj.insert(
                            std::borrow::Cow::Borrowed("name"),
                            vec![keyvalues_parser::Value::Str(std::borrow::Cow::Borrowed(
                                "nucleon",
                            ))],
                        );
                        modified = true;
                    }
                }
            }
        }
    }

    // 3. Ensure target AppIDs are mapped
    for target_id in target_appids {
        if !compat.contains_key(target_id.as_str()) {
            let mut entry_obj = keyvalues_parser::Obj::new();
            entry_obj.insert(
                std::borrow::Cow::Borrowed("name"),
                vec![keyvalues_parser::Value::Str(std::borrow::Cow::Borrowed(
                    "nucleon",
                ))],
            );
            entry_obj.insert(
                std::borrow::Cow::Borrowed("config"),
                vec![keyvalues_parser::Value::Str(std::borrow::Cow::Borrowed(""))],
            );
            entry_obj.insert(
                std::borrow::Cow::Borrowed("priority"),
                vec![keyvalues_parser::Value::Str(std::borrow::Cow::Borrowed(
                    "250",
                ))],
            );
            compat.insert(
                std::borrow::Cow::Owned(target_id.clone()),
                vec![keyvalues_parser::Value::Obj(entry_obj)],
            );
            modified = true;
        }
    }

    if modified {
        (vdf.to_string(), true)
    } else {
        (content.to_string(), false)
    }
}

/// Removes any AppID mappings in `content` whose tool name starts with `nucleon` or is `notproton`.
pub fn unmap_all_nucleon_compat_mappings(content: &str) -> (String, bool) {
    let Ok(partial) = keyvalues_parser::parse(content) else {
        return (content.to_string(), false);
    };
    let mut vdf = keyvalues_parser::Vdf::from(partial).into_owned();
    let mut modified = false;

    let compat_obj = if vdf.key == "CompatToolMapping" {
        vdf.value.get_mut_obj()
    } else if let keyvalues_parser::Value::Obj(ref mut root_obj) = vdf.value {
        find_child_obj_mut(root_obj, "CompatToolMapping")
    } else {
        None
    };

    let Some(compat) = compat_obj else {
        return (content.to_string(), false);
    };

    let to_remove: Vec<String> = compat
        .iter()
        .filter(|(_appid, values)| {
            values.iter().any(|val| {
                if let keyvalues_parser::Value::Obj(entry_obj) = val {
                    entry_obj
                        .get("name")
                        .and_then(|v| v.first())
                        .and_then(|v| v.get_str())
                        .map(|s| s == "nucleon" || s.starts_with("nucleon-") || s == "notproton")
                        .unwrap_or(false)
                } else {
                    false
                }
            })
        })
        .map(|(k, _)| k.to_string())
        .collect();

    for appid in to_remove {
        compat.remove(appid.as_str());
        modified = true;
    }

    if modified {
        (vdf.to_string(), true)
    } else {
        (content.to_string(), false)
    }
}

/// Removes all Nucleon mappings from Steam's config.vdf on disk.
pub fn remove_nucleon_compat_mappings() -> Result<bool> {
    let config_path = paths::steam_data_dir().join("config/config.vdf");
    if !config_path.is_file() {
        return Ok(false);
    }
    let content = fs::read_to_string(&config_path)?;
    let (new_content, modified) = unmap_all_nucleon_compat_mappings(&content);
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
        fs::write(&config_path, new_content)?;
    }
    Ok(modified)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(
            result.contains("\"name\"\t\"nucleon\"") || result.contains(r#""name"		"nucleon""#)
        );

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
        assert!(
            result.contains("\"name\"\t\"nucleon-kosmickrisp\"")
                || result.contains(r#""name"		"nucleon-kosmickrisp""#)
        );
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
        assert!(
            result.contains("\"name\"\t\"nucleon\"") || result.contains(r#""name"		"nucleon""#)
        );
        assert!(!result.contains("nucleon-kosmickrisp"));
    }

    #[test]
    fn test_parse_real_config_vdf() {
        let path = paths::home_dir().join("Library/Application Support/Steam/config/config.vdf");
        if path.exists() {
            let content = fs::read_to_string(&path).unwrap();
            let parsed = keyvalues_parser::parse(&content);
            assert!(
                parsed.is_ok(),
                "Failed to parse real config.vdf: {:?}",
                parsed.err()
            );
        }
    }

    #[test]
    fn test_unmap_all_nucleon_compat_mappings() {
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
					"730"
					{
						"name"		"proton_experimental"
						"config"		""
						"priority"		"250"
					}
					"12345"
					{
						"name"		"nucleon"
						"config"		""
						"priority"		"250"
					}
					"67890"
					{
						"name"		"nucleon-kosmickrisp"
						"config"		""
						"priority"		"250"
					}
					"11111"
					{
						"name"		"nucleon-wine"
						"config"		""
						"priority"		"250"
					}
				}
			}
		}
	}
}
"#;
        let (result, modified) = unmap_all_nucleon_compat_mappings(sample);
        assert!(modified);
        assert!(
            result.contains("\"730\""),
            "Proton mapping must be preserved"
        );
        assert!(
            !result.contains("\"12345\""),
            "Nucleon mapping must be removed"
        );
        assert!(
            !result.contains("\"67890\""),
            "KosmicKrisp mapping must be removed"
        );
        assert!(
            !result.contains("\"11111\""),
            "Wine mapping must be removed"
        );
    }
}
