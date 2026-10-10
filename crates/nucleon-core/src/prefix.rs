use crate::paths;
use crate::runner;
use anyhow::Result;
use std::fs;
use std::path::Path;
use std::process::Command;

pub fn ensure_prefix(prefix_dir: &Path, runner_dir: &Path) -> Result<()> {
    fs::create_dir_all(prefix_dir)?;
    let drive_c = prefix_dir.join("drive_c");

    if !drive_c.exists() {
        log::info!("Initializing new Wine prefix at {}", prefix_dir.display());
        let envs = runner::build_execution_env(runner_dir, prefix_dir, false);
        let wineboot = runner_dir.join("bin/wineboot");

        let mut cmd = Command::new(&wineboot);
        cmd.arg("-u");
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let _ = cmd.status();

        // Wait for wineserver to settle (up to 5 seconds)
        let wineserver = runner_dir.join("bin/wineserver");
        if let Ok(mut child) = Command::new(&wineserver)
            .arg("-w")
            .env("WINEPREFIX", prefix_dir)
            .spawn()
        {
            let start = std::time::Instant::now();
            loop {
                if let Ok(Some(_)) = child.try_wait() {
                    break;
                }
                if start.elapsed() >= std::time::Duration::from_millis(5000) {
                    let _ = child.kill();
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }

    isolate_user_shell_folders(prefix_dir)?;
    configure_prefix_registry(prefix_dir, runner_dir)?;
    stage_bridge_libraries(prefix_dir)?;

    Ok(())
}

/// Isolates user shell folders (Desktop, Downloads, Documents, etc.) inside the Wine prefix.
/// By default, Wine symlinks these to the user's macOS host directories (`/Users/<user>/Desktop`,
/// `/Users/<user>/Downloads`), which triggers invasive macOS TCC privacy permission prompts
/// ("nucleon would like to access files in your Desktop/Downloads folder") when games start.
///
/// Replacing these symlinks with local isolated directories inside `drive_c/users/<user>/`
/// prevents any host directory access and eliminates TCC permission dialogs.
pub fn isolate_user_shell_folders(prefix_dir: &Path) -> Result<()> {
    let users_dir = prefix_dir.join("drive_c/users");
    if !users_dir.is_dir() {
        return Ok(());
    }

    let folders = [
        "Desktop",
        "Downloads",
        "Documents",
        "Music",
        "Pictures",
        "Videos",
    ];

    if let Ok(entries) = fs::read_dir(&users_dir) {
        for entry in entries.flatten() {
            let user_path = entry.path();
            if user_path.is_dir() {
                for folder in &folders {
                    let target = user_path.join(folder);
                    // Check if target is a symlink (or broken symlink)
                    if target.is_symlink() {
                        let _ = fs::remove_file(&target);
                        let _ = fs::create_dir_all(&target);
                        log::info!(
                            "Isolated Wine user folder: replaced symlink at {} with local directory",
                            target.display()
                        );
                    } else if !target.exists() {
                        let _ = fs::create_dir_all(&target);
                    }
                }
            }
        }
    }

    Ok(())
}

/// Isolates user shell folders across all existing game prefixes in the Steam library.
pub fn isolate_all_steam_game_prefixes() -> Result<usize> {
    let compatdata = paths::steam_compat_data_dir();
    let mut count = 0;
    if compatdata.is_dir() {
        if let Ok(entries) = fs::read_dir(&compatdata) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    let pfx = p.join("pfx");
                    if pfx.is_dir() && isolate_user_shell_folders(&pfx).is_ok() {
                        count += 1;
                    }
                }
            }
        }
    }
    Ok(count)
}

pub fn configure_prefix_registry(prefix_dir: &Path, runner_dir: &Path) -> Result<()> {
    let marker = prefix_dir.join(".nucleon_configured_v2");
    if marker.exists() {
        return Ok(());
    }

    let reg_file = prefix_dir.join("nucleon_tweaks.reg");
    let reg_content = r#"Windows Registry Editor Version 5.00

[HKEY_CURRENT_USER\Software\Wine\Mac Driver]
"OpenGLSurfaceMode"="behind"

[HKEY_CURRENT_USER\Software\Wine\Direct3D]
"csmt"=dword:00000001

[HKEY_CURRENT_USER\Software\Wine\DirectSound]
"DefaultCapture"=""
"DefaultVoiceCapture"=""

[HKEY_LOCAL_MACHINE\Software\Classes\steam]
"URL Protocol"=""

[HKEY_LOCAL_MACHINE\Software\Classes\steam\shell\open\command]
@="\"C:\\Program Files (x86)\\Steam\\steam.exe\" \"%1\""

[HKEY_LOCAL_MACHINE\Software\Microsoft\Windows NT\CurrentVersion\AeDebug]
"Auto"="0"

[HKEY_LOCAL_MACHINE\Software\Wow6432Node\Microsoft\Windows NT\CurrentVersion\AeDebug]
"Auto"="0"

[HKEY_CURRENT_USER\Software\Wine\WineDbg]
"ShowCrashDialog"=dword:00000000

[HKEY_CURRENT_USER\Software\Wine\DllOverrides]
"winemenubuilder.exe"=""

[HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders]
"Desktop"="%USERPROFILE%\\Desktop"
"Personal"="%USERPROFILE%\\Documents"
"{374DE290-123F-4565-9164-39C4925E467B}"="%USERPROFILE%\\Downloads"
"My Music"="%USERPROFILE%\\Music"
"My Pictures"="%USERPROFILE%\\Pictures"
"My Video"="%USERPROFILE%\\Videos"
"#;

    fs::write(&reg_file, reg_content)?;

    let envs = runner::build_execution_env(runner_dir, prefix_dir, false);
    let regedit = runner_dir.join("bin/regedit");

    let mut cmd = Command::new(&regedit);
    cmd.arg(&reg_file);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let _ = cmd.status();

    let wineserver = runner_dir.join("bin/wineserver");
    let _ = Command::new(&wineserver)
        .arg("-k")
        .env("WINEPREFIX", prefix_dir)
        .status();

    let _ = fs::remove_file(&reg_file);
    let _ = fs::write(&marker, b"1");
    Ok(())
}

pub fn stage_bridge_libraries(prefix_dir: &Path) -> Result<()> {
    let bridge = paths::bridge_dir();
    let steam_dir = prefix_dir.join("drive_c/Program Files (x86)/Steam");
    let legacy_dir = steam_dir.join("legacycompat");
    let sys32 = prefix_dir.join("drive_c/windows/system32");
    let syswow64 = prefix_dir.join("drive_c/windows/syswow64");

    for d in &[&steam_dir, &legacy_dir, &sys32, &syswow64] {
        fs::create_dir_all(d)?;
    }

    if !bridge.is_dir() {
        return Ok(());
    }

    // Copy files to steam_dir and system directories
    let files_to_stage = [
        (
            "steamclient64.dll",
            vec![
                steam_dir.join("steamclient64.dll"),
                sys32.join("steamclient64.dll"),
            ],
        ),
        (
            "steamclient.dll",
            vec![
                steam_dir.join("steamclient.dll"),
                syswow64.join("steamclient.dll"),
            ],
        ),
        ("tier0_s64.dll", vec![steam_dir.join("tier0_s64.dll")]),
        ("tier0_s.dll", vec![steam_dir.join("tier0_s.dll")]),
        ("vstdlib_s64.dll", vec![steam_dir.join("vstdlib_s64.dll")]),
        ("vstdlib_s.dll", vec![steam_dir.join("vstdlib_s.dll")]),
        ("steam.exe", vec![steam_dir.join("steam.exe")]),
        (
            "lsteamclient.dll",
            vec![
                steam_dir.join("lsteamclient.dll"),
                sys32.join("lsteamclient.dll"),
                syswow64.join("lsteamclient.dll"),
            ],
        ),
        (
            "lsteamclient.so",
            vec![
                steam_dir.join("lsteamclient.so"),
                steam_dir.join("steamclient.so"),
                steam_dir.join("steamclient64.so"),
            ],
        ),
        ("ntdll_compat.so", vec![steam_dir.join("ntdll_compat.so")]),
    ];

    for (name, targets) in files_to_stage {
        let src_file = bridge.join(name);
        if src_file.is_file() {
            for tgt in targets {
                let _ = fs::copy(&src_file, &tgt);
            }
        }
    }

    // Copy architecture-specific triggers if present
    let x64_lsteam = bridge.join("x86_64-windows/lsteamclient.dll");
    if x64_lsteam.is_file() {
        let _ = fs::copy(&x64_lsteam, sys32.join("lsteamclient.dll"));
        let _ = fs::copy(&x64_lsteam, sys32.join("steamclient64.dll"));
    }
    let i386_lsteam = bridge.join("i386-windows/lsteamclient.dll");
    if i386_lsteam.is_file() {
        let _ = fs::copy(&i386_lsteam, syswow64.join("lsteamclient.dll"));
    }

    // Copy architecture-specific unix bridge libraries if present
    let x64_unix_lsteam = bridge.join("x86_64-unix/lsteamclient.so");
    if x64_unix_lsteam.is_file() {
        let _ = fs::copy(&x64_unix_lsteam, steam_dir.join("lsteamclient.so"));
        let _ = fs::copy(&x64_unix_lsteam, steam_dir.join("steamclient.so"));
        let _ = fs::copy(&x64_unix_lsteam, steam_dir.join("steamclient64.so"));
    }
    let ntdll_compat = bridge.join("x86_64-unix/ntdll_compat.so");
    if ntdll_compat.is_file() {
        let _ = fs::copy(&ntdll_compat, steam_dir.join("ntdll_compat.so"));
    }

    // Copy legacycompat files and legacy Steam.dll
    let legacy_src = bridge.join("legacycompat");
    if legacy_src.is_dir() {
        if let Ok(entries) = fs::read_dir(&legacy_src) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_file() {
                    let fname = e.file_name();
                    let _ = fs::copy(&p, legacy_dir.join(&fname));
                    if fname == "Steam.dll" {
                        let _ = fs::copy(&p, syswow64.join("Steam.dll"));
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_stage_bridge_libraries_structure() {
        let prefix_dir = tempdir().unwrap();
        let res = stage_bridge_libraries(prefix_dir.path());
        assert!(res.is_ok());

        let steam_dir = prefix_dir.path().join("drive_c/Program Files (x86)/Steam");
        assert!(steam_dir.is_dir());
        let sys32 = prefix_dir.path().join("drive_c/windows/system32");
        assert!(sys32.is_dir());
        let syswow64 = prefix_dir.path().join("drive_c/windows/syswow64");
        assert!(syswow64.is_dir());
    }

    #[test]
    fn test_isolate_user_shell_folders() {
        let prefix_dir = tempdir().unwrap();
        let user_dir = prefix_dir.path().join("drive_c/users/testuser");
        fs::create_dir_all(&user_dir).unwrap();

        // Create a dummy host directory outside the prefix
        let host_dir = tempdir().unwrap();
        let host_desktop = host_dir.path().join("Desktop");
        let host_downloads = host_dir.path().join("Downloads");
        fs::create_dir_all(&host_desktop).unwrap();
        fs::create_dir_all(&host_downloads).unwrap();

        // Simulate Wine's symlinks
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&host_desktop, user_dir.join("Desktop")).unwrap();
            std::os::unix::fs::symlink(&host_downloads, user_dir.join("Downloads")).unwrap();
            assert!(user_dir.join("Desktop").is_symlink());
            assert!(user_dir.join("Downloads").is_symlink());
        }

        isolate_user_shell_folders(prefix_dir.path()).unwrap();

        let desktop = user_dir.join("Desktop");
        let downloads = user_dir.join("Downloads");
        let documents = user_dir.join("Documents");

        assert!(desktop.is_dir());
        assert!(!desktop.is_symlink(), "Desktop must no longer be a symlink");
        assert!(downloads.is_dir());
        assert!(
            !downloads.is_symlink(),
            "Downloads must no longer be a symlink"
        );
        assert!(documents.is_dir());
    }
}
