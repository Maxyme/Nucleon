use crate::paths;
use anyhow::{bail, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::process::Command;

pub fn fetch_and_stage_valve_packages() -> Result<()> {
    paths::ensure_dirs()?;
    let bridge_dir = paths::bridge_dir();
    let downloads_dir = paths::downloads_dir();

    // Check if files are already staged
    if bridge_dir.join("steamclient64.dll").is_file() && bridge_dir.join("tier0_s64.dll").is_file()
    {
        return Ok(());
    }

    let bases = [
        "https://client-update.akamai.steamstatic.com",
        "https://steamcdn-a.akamaihd.net/client",
        "https://media.steampowered.com/client",
    ];

    let packages = [
        (
            "bins_misc_ubuntu12.zip.3f92810725ee673827371a0470cd4f8c7ea8cfae",
            "026b984726c728bbf81ae3ba16623bcd96e2bf837a859c235acf0ee279d940fd",
        ),
        (
            "bins_win64.zip.36f5d9202e79ab2aa3e3c5902e84bbd799d31fc0",
            "93f5b6bea0267fd85dc8cc823fdab5c5fb55d7f3a1deab0598acefef0e133bce",
        ),
    ];

    for (pkg_name, expected_sha) in packages {
        let pkg_path = downloads_dir.join(pkg_name);
        if !pkg_path.is_file() {
            let mut downloaded = false;
            for base in &bases {
                let url = format!("{}/{}", base, pkg_name);
                let status = Command::new("curl")
                    .args([
                        "-fSL",
                        "--retry",
                        "3",
                        "-o",
                        pkg_path.to_str().unwrap(),
                        &url,
                    ])
                    .status();
                if let Ok(st) = status {
                    if st.success() {
                        downloaded = true;
                        break;
                    }
                }
            }
            if !downloaded {
                bail!("Failed to download Valve package: {}", pkg_name);
            }
        }

        // Verify SHA
        let data = fs::read(&pkg_path)?;
        let mut hasher = Sha256::new();
        hasher.update(&data);
        let actual_sha = hex::encode(hasher.finalize());
        if actual_sha != expected_sha {
            let _ = fs::remove_file(&pkg_path);
            bail!(
                "Checksum mismatch for {}: expected {}, got {}",
                pkg_name,
                expected_sha,
                actual_sha
            );
        }

        // Extract package
        let extract_dir = downloads_dir.join(format!("{}.extract", pkg_name));
        fs::create_dir_all(&extract_dir)?;
        let _ = Command::new("unzip")
            .args([
                "-q",
                "-o",
                pkg_path.to_str().unwrap(),
                "-d",
                extract_dir.to_str().unwrap(),
            ])
            .status();

        // Copy needed files into bridge
        let legacy = bridge_dir.join("legacycompat");
        fs::create_dir_all(&legacy)?;

        if pkg_name.contains("ubuntu12") {
            let _ = fs::copy(
                extract_dir.join("legacycompat/Steam.dll"),
                legacy.join("Steam.dll"),
            );
            let _ = fs::copy(
                extract_dir.join("legacycompat/SteamService.exe"),
                legacy.join("SteamService.exe"),
            );
            let _ = fs::copy(
                extract_dir.join("legacycompat/iscriptevaluator.exe"),
                legacy.join("iscriptevaluator.exe"),
            );
            let _ = fs::copy(
                extract_dir.join("legacycompat/steamclient.dll"),
                legacy.join("steamclient.dll"),
            );
            let _ = fs::copy(
                extract_dir.join("steamclient64.dll"),
                legacy.join("steamclient64.dll"),
            );
            let _ = fs::copy(
                extract_dir.join("GameOverlayRenderer64.dll"),
                legacy.join("GameOverlayRenderer64.dll"),
            );
            let _ = fs::copy(
                extract_dir.join("legacycompat/steamclient.dll"),
                bridge_dir.join("steamclient.dll"),
            );
            let _ = fs::copy(
                extract_dir.join("steamclient64.dll"),
                bridge_dir.join("steamclient64.dll"),
            );
        } else if pkg_name.contains("win64") {
            let _ = fs::copy(
                extract_dir.join("tier0_s64.dll"),
                bridge_dir.join("tier0_s64.dll"),
            );
            let _ = fs::copy(
                extract_dir.join("vstdlib_s64.dll"),
                bridge_dir.join("vstdlib_s64.dll"),
            );
            let _ = fs::copy(
                extract_dir.join("tier0_s.dll"),
                bridge_dir.join("tier0_s.dll"),
            );
            let _ = fs::copy(
                extract_dir.join("vstdlib_s.dll"),
                bridge_dir.join("vstdlib_s.dll"),
            );
        }
    }

    Ok(())
}
