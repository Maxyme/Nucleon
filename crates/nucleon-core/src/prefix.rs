use std::fs;
use std::path::Path;
use std::process::Command;
use anyhow::Result;
use crate::paths;
use crate::runner;

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
        let _ = Command::new(&wineserver).arg("-w").status();
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
    let _ = Command::new(&wineserver).arg("-w").status();

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

    // Also check notproton bridge directory if nucleon bridge doesn't have all files yet
    let np_bridge = paths::home_dir().join("Library/Application Support/notproton/bridge");

    let source_dirs = [bridge, np_bridge];

    for src_dir in &source_dirs {
        if !src_dir.is_dir() {
            continue;
        }

        // Copy files to steam_dir and system directories
        let files_to_stage = [
            ("steamclient64.dll", vec![steam_dir.join("steamclient64.dll"), sys32.join("steamclient64.dll")]),
            ("steamclient.dll", vec![steam_dir.join("steamclient.dll"), syswow64.join("steamclient.dll")]),
            ("tier0_s64.dll", vec![steam_dir.join("tier0_s64.dll")]),
            ("tier0_s.dll", vec![steam_dir.join("tier0_s.dll")]),
            ("vstdlib_s64.dll", vec![steam_dir.join("vstdlib_s64.dll")]),
            ("vstdlib_s.dll", vec![steam_dir.join("vstdlib_s.dll")]),
            ("steam.exe", vec![steam_dir.join("steam.exe")]),
            ("lsteamclient.dll", vec![
                steam_dir.join("lsteamclient.dll"),
                sys32.join("lsteamclient.dll"),
                syswow64.join("lsteamclient.dll"),
            ]),
            ("lsteamclient.so", vec![steam_dir.join("lsteamclient.so")]),
        ];

        for (name, targets) in files_to_stage {
            let src_file = src_dir.join(name);
            if src_file.is_file() {
                for tgt in targets {
                    let _ = fs::copy(&src_file, &tgt);
                }
            }
        }

        // Copy legacycompat files
        let legacy_src = src_dir.join("legacycompat");
        if legacy_src.is_dir() {
            if let Ok(entries) = fs::read_dir(&legacy_src) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.is_file() {
                        let _ = fs::copy(&p, legacy_dir.join(e.file_name()));
                    }
                }
            }
        }
    }

    Ok(())
}
