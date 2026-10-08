use crate::paths;
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
    is_plist_dyld_injected(&paths::steam_info_plist(), "nucleon.dylib")
}

pub fn is_plist_dyld_injected(plist_path: &Path, needle: &str) -> bool {
    if !plist_path.exists() {
        return false;
    }
    let Ok(value) = plist::Value::from_file(plist_path) else {
        return false;
    };
    value
        .as_dictionary()
        .and_then(|dict| dict.get("LSEnvironment"))
        .and_then(|ls_env| ls_env.as_dictionary())
        .and_then(|ls_dict| ls_dict.get("DYLD_INSERT_LIBRARIES"))
        .and_then(|val| val.as_string())
        .map(|s| s.contains(needle))
        .unwrap_or(false)
}

pub fn inject_plist_dyld_insert(plist_path: &Path, dylib_path: &Path) -> Result<bool> {
    let mut root = plist::Value::from_file(plist_path)
        .with_context(|| format!("Failed to read plist at {}", plist_path.display()))?;

    let dict = root
        .as_dictionary_mut()
        .context("Root of Info.plist is not a dictionary")?;

    let dylib_str = dylib_path
        .to_str()
        .context("Invalid non-UTF8 path for dylib")?;

    if !dict.contains_key("LSEnvironment") {
        dict.insert(
            "LSEnvironment".to_string(),
            plist::Value::Dictionary(plist::Dictionary::new()),
        );
    }

    let ls_dict = dict
        .get_mut("LSEnvironment")
        .and_then(|v| v.as_dictionary_mut())
        .context("LSEnvironment is not a dictionary")?;

    if let Some(current) = ls_dict
        .get("DYLD_INSERT_LIBRARIES")
        .and_then(|v| v.as_string())
    {
        if current == dylib_str {
            return Ok(false);
        }
    }

    ls_dict.insert(
        "DYLD_INSERT_LIBRARIES".to_string(),
        plist::Value::String(dylib_str.to_string()),
    );

    root.to_file_xml(plist_path)
        .with_context(|| format!("Failed to write plist to {}", plist_path.display()))?;

    Ok(true)
}

pub fn remove_plist_dyld_insert(plist_path: &Path) -> Result<bool> {
    if !plist_path.exists() {
        return Ok(false);
    }

    let mut root = plist::Value::from_file(plist_path)
        .with_context(|| format!("Failed to read plist at {}", plist_path.display()))?;

    let dict = root
        .as_dictionary_mut()
        .context("Root of Info.plist is not a dictionary")?;

    let mut modified = false;
    let mut remove_ls_env = false;

    if let Some(ls_dict) = dict
        .get_mut("LSEnvironment")
        .and_then(|v| v.as_dictionary_mut())
    {
        if ls_dict.remove("DYLD_INSERT_LIBRARIES").is_some() {
            modified = true;
        }
        if ls_dict.is_empty() {
            remove_ls_env = true;
        }
    }

    if remove_ls_env {
        dict.remove("LSEnvironment");
    }

    if modified {
        root.to_file_xml(plist_path)
            .with_context(|| format!("Failed to write plist to {}", plist_path.display()))?;
    }

    Ok(modified)
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

    // Inject DYLD_INSERT_LIBRARIES into Info.plist
    inject_plist_dyld_insert(&plist, &dst_dylib)?;

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
            remove_plist_dyld_insert(&plist)?;
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

pub fn refresh_launch_services(app_path: &Path) -> Result<()> {
    let lsregister = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
    if Path::new(lsregister).exists() {
        let _ = Command::new(lsregister)
            .args(["-f", app_path.to_str().unwrap()])
            .status();
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plist_dyld_injection_lifecycle() {
        let temp_dir = tempfile::tempdir().unwrap();
        let plist_path = temp_dir.path().join("Info.plist");
        let dylib_path = temp_dir.path().join("nucleon.dylib");

        let initial_plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key>
	<string>Steam</string>
	<key>LSEnvironment</key>
	<dict>
		<key>LC_ALL</key>
		<string>en_US.UTF-8</string>
	</dict>
</dict>
</plist>"#;
        fs::write(&plist_path, initial_plist).unwrap();

        assert!(!is_plist_dyld_injected(&plist_path, "nucleon.dylib"));

        // First injection should modify
        assert!(inject_plist_dyld_insert(&plist_path, &dylib_path).unwrap());
        assert!(is_plist_dyld_injected(&plist_path, "nucleon.dylib"));

        // Idempotent injection should return false (no change)
        assert!(!inject_plist_dyld_insert(&plist_path, &dylib_path).unwrap());

        // Verify other keys in LSEnvironment are preserved
        let root = plist::Value::from_file(&plist_path).unwrap();
        let dict = root.as_dictionary().unwrap();
        let ls_dict = dict.get("LSEnvironment").unwrap().as_dictionary().unwrap();
        assert_eq!(
            ls_dict.get("LC_ALL").unwrap().as_string(),
            Some("en_US.UTF-8")
        );
        assert_eq!(
            ls_dict.get("DYLD_INSERT_LIBRARIES").unwrap().as_string(),
            dylib_path.to_str()
        );

        // Remove DYLD_INSERT_LIBRARIES
        assert!(remove_plist_dyld_insert(&plist_path).unwrap());
        assert!(!is_plist_dyld_injected(&plist_path, "nucleon.dylib"));

        // Second removal is idempotent
        assert!(!remove_plist_dyld_insert(&plist_path).unwrap());

        // LC_ALL still preserved, LSEnvironment dict still exists because not empty
        let root_after = plist::Value::from_file(&plist_path).unwrap();
        let dict_after = root_after.as_dictionary().unwrap();
        let ls_dict_after = dict_after
            .get("LSEnvironment")
            .unwrap()
            .as_dictionary()
            .unwrap();
        assert_eq!(
            ls_dict_after.get("LC_ALL").unwrap().as_string(),
            Some("en_US.UTF-8")
        );
        assert!(!ls_dict_after.contains_key("DYLD_INSERT_LIBRARIES"));
    }

    #[test]
    fn test_plist_dyld_injection_creates_ls_environment_and_cleans_up() {
        let temp_dir = tempfile::tempdir().unwrap();
        let plist_path = temp_dir.path().join("Info.plist");
        let dylib_path = temp_dir.path().join("nucleon.dylib");

        let initial_plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key>
	<string>Steam</string>
</dict>
</plist>"#;
        fs::write(&plist_path, initial_plist).unwrap();

        assert!(!is_plist_dyld_injected(&plist_path, "nucleon.dylib"));
        assert!(inject_plist_dyld_insert(&plist_path, &dylib_path).unwrap());
        assert!(is_plist_dyld_injected(&plist_path, "nucleon.dylib"));

        // Removing when LSEnvironment had only DYLD_INSERT_LIBRARIES cleans up empty dict
        assert!(remove_plist_dyld_insert(&plist_path).unwrap());
        assert!(!is_plist_dyld_injected(&plist_path, "nucleon.dylib"));

        let root_after = plist::Value::from_file(&plist_path).unwrap();
        let dict_after = root_after.as_dictionary().unwrap();
        assert!(!dict_after.contains_key("LSEnvironment"));
    }

    #[test]
    fn test_plist_dyld_nonexistent_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let nonexistent = temp_dir.path().join("does_not_exist.plist");

        assert!(!is_plist_dyld_injected(&nonexistent, "nucleon.dylib"));
        assert!(!remove_plist_dyld_insert(&nonexistent).unwrap());
    }
}
