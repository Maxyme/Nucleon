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

        // Wait for wineserver to settle
        let wineserver = runner_dir.join("bin/wineserver");
        let _ = Command::new(&wineserver)
            .arg("-w")
            .env("WINEPREFIX", prefix_dir)
            .status();
    }

    configure_prefix_registry(prefix_dir, runner_dir)?;
    stage_bridge_libraries(prefix_dir)?;

    Ok(())
}

pub fn configure_prefix_registry(prefix_dir: &Path, runner_dir: &Path) -> Result<()> {
    let reg_file = prefix_dir.join("nucleon_tweaks.reg");
    let reg_content = r#"Windows Registry Editor Version 5.00

[HKEY_CURRENT_USER\Software\Wine\Mac Driver]
"OpenGLSurfaceMode"="behind"

[HKEY_CURRENT_USER\Software\Wine\Direct3D]
"csmt"=dword:00000001

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
        .arg("-w")
        .env("WINEPREFIX", prefix_dir)
        .status();

    let _ = fs::remove_file(&reg_file);
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
