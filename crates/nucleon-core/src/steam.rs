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
    let tool_dir = paths::steam_compat_tools_dir();
    fs::create_dir_all(&tool_dir)?;

    // Clean up legacy notproton compatibility tool directory if present
    let legacy_tool_dir = paths::home_dir().join("Library/Application Support/Steam/compatibilitytools.d/notproton");
    if legacy_tool_dir.exists() {
        let _ = fs::remove_dir_all(&legacy_tool_dir);
    }

    // Link/copy runner binary into compatibility tool directory
    let dst_runner = tool_dir.join("nucleon-runner");
    if dst_runner.exists() {
        let _ = fs::remove_file(&dst_runner);
    }
    fs::copy(runner_bin, &dst_runner)
        .with_context(|| format!("Failed to stage runner into {}", dst_runner.display()))?;

    // Create compatibilitytool.vdf
    let vdf_path = tool_dir.join("compatibilitytool.vdf");
    vdf::write_compatibilitytool_vdf(&vdf_path)?;

    Ok(())
}
