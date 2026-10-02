use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use anyhow::{Context, Result};
use crate::paths;

#[derive(Debug, Clone)]
pub struct RunnerInfo {
    pub name: String,
    pub path: PathBuf,
    pub wine_bin: PathBuf,
    pub wineserver_bin: PathBuf,
    pub has_gptk: bool,
}

pub fn discover_wine_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    // 1. Existing nucleon runners
    let runners_dir = paths::runners_dir();
    if let Ok(entries) = fs::read_dir(&runners_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                candidates.push(p.clone());
                candidates.push(p.join("Contents/Resources/wine"));
            }
        }
    }

    // 2. Existing notproton runner if present
    candidates.push(paths::home_dir().join("Library/Application Support/notproton/runners/gptk-4-beta2"));

    // 3. System and Homebrew locations
    candidates.push(PathBuf::from("/Applications/Game Porting Toolkit.app/Contents/Resources/wine"));
    candidates.push(PathBuf::from("/Applications/Wine Crossover.app/Contents/Resources/wine"));
    candidates.push(PathBuf::from("/Applications/Wine Staging.app/Contents/Resources/wine"));
    candidates.push(PathBuf::from("/opt/homebrew/opt/game-porting-toolkit/Contents/Resources/wine"));
    candidates.push(PathBuf::from("/opt/homebrew/opt/wine-crossover/Contents/Resources/wine"));
    candidates.push(PathBuf::from("/opt/homebrew/opt/wine-crossover"));
    candidates.push(PathBuf::from("/opt/homebrew/opt/game-porting-toolkit"));

    // 4. Heroic tools directory if present
    candidates.push(paths::home_dir().join("Library/Application Support/heroic/tools/wine/Wine-Crossover-latest/Contents/Resources/wine"));

    candidates
}

pub fn find_valid_wine_runtime() -> Option<PathBuf> {
    for c in discover_wine_candidates() {
        let wine = c.join("bin/wine");
        let server = c.join("bin/wineserver");
        if wine.is_file() && server.is_file() {
            return Some(c);
        }
    }
    None
}

pub fn find_gptk_components() -> Result<Option<(PathBuf, PathBuf)>> {
    // 1. Check existing runner
    let cur = paths::current_runner();
    let fw = cur.join("lib/external/D3DMetal.framework");
    let shared = cur.join("lib/external/libd3dshared.dylib");
    if fw.is_dir() && shared.is_file() {
        return Ok(Some((fw, shared)));
    }

    // 2. Check /Volumes for mounted GPTK DMGs
    if let Ok(entries) = fs::read_dir("/Volumes") {
        for entry in entries.flatten() {
            let p = entry.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.contains("Evaluation") || name.contains("Game") || name.contains("GPTK") {
                let vol_fw = p.join("redist/lib/external/D3DMetal.framework");
                let vol_shared = p.join("redist/lib/external/libd3dshared.dylib");
                if vol_fw.is_dir() && vol_shared.is_file() {
                    return Ok(Some((vol_fw, vol_shared)));
                }
            }
        }
    }

    // 3. Check notproton runner if present
    let np_fw = paths::home_dir().join("Library/Application Support/notproton/runners/gptk-4-beta2/lib/external/D3DMetal.framework");
    let np_shared = paths::home_dir().join("Library/Application Support/notproton/runners/gptk-4-beta2/lib/external/libd3dshared.dylib");
    if np_fw.is_dir() && np_shared.is_file() {
        return Ok(Some((np_fw, np_shared)));
    }

    // 4. Check Downloads for DMG and mount if found
    let downloads = paths::home_dir().join("Downloads");
    if let Ok(entries) = fs::read_dir(&downloads) {
        for entry in entries.flatten() {
            let p = entry.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with(".dmg") && (name.contains("Evaluation") || name.contains("Porting") || name.contains("GPTK")) {
                if let Ok(output) = Command::new("hdiutil").args(["attach", "-nobrowse", "-readonly", p.to_str().unwrap()]).output() {
                    let out_str = String::from_utf8_lossy(&output.stdout);
                    for line in out_str.lines() {
                        if let Some(idx) = line.find("/Volumes/") {
                            let mount = PathBuf::from(&line[idx..]);
                            let vol_fw = mount.join("redist/lib/external/D3DMetal.framework");
                            let vol_shared = mount.join("redist/lib/external/libd3dshared.dylib");
                            if vol_fw.is_dir() && vol_shared.is_file() {
                                return Ok(Some((vol_fw, vol_shared)));
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(None)
}

pub fn assemble_runner(force: bool) -> Result<PathBuf> {
    paths::ensure_dirs()?;
    let target = paths::runners_dir().join("gptk-4-beta2");

    if !force && target.join("bin/wine").is_file() && target.join("lib/external/D3DMetal.framework").is_dir() {
        let cur = paths::current_runner();
        let _ = fs::remove_file(&cur);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &cur)?;
        return Ok(target);
    }

    let wine_src = find_valid_wine_runtime()
        .context("No compatible Wine runtime found. Install Game Porting Toolkit via Homebrew: brew tap gcenx/wine && brew install --cask --no-quarantine game-porting-toolkit")?;

    fs::create_dir_all(&target)?;

    // Sync Wine directory structure into target
    let _ = Command::new("rsync")
        .args([
            "-a",
            "--exclude", "lib/external",
            &format!("{}/", wine_src.display()),
            &format!("{}/", target.display()),
        ])
        .status()?;

    // Install GPTK 4 D3DMetal components
    let (d3dm_fw, d3d_shared) = find_gptk_components()?
        .context("Could not find Apple Game Porting Toolkit 4 components (D3DMetal.framework). Please download the GPTK 4 DMG from developer.apple.com/games and place it in ~/Downloads")?;

    let ext_dir = target.join("lib/external");
    fs::create_dir_all(&ext_dir)?;

    let _ = Command::new("cp")
        .args(["-R", "-f", d3dm_fw.to_str().unwrap(), ext_dir.to_str().unwrap()])
        .status()?;
    let _ = Command::new("cp")
        .args(["-f", d3d_shared.to_str().unwrap(), ext_dir.to_str().unwrap()])
        .status()?;

    // Install overlay-shim.dylib
    let overlay_dst = paths::support_dir().join("overlay-shim.dylib");
    fs::write(&overlay_dst, overlay_shim::OVERLAY_SHIM_BYTES)?;
    crate::steam::sign_binary(&overlay_dst)?;

    // Link current runner
    let cur = paths::current_runner();
    let _ = fs::remove_file(&cur);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &cur)?;

    Ok(target)
}

pub fn build_execution_env(
    runner_dir: &Path,
    prefix_dir: &Path,
    enable_hud: bool,
) -> HashMap<String, String> {
    let mut env = HashMap::new();

    env.insert("WINEPREFIX".to_string(), prefix_dir.to_string_lossy().to_string());
    env.insert("WINELOADER".to_string(), runner_dir.join("bin/wine").to_string_lossy().to_string());
    env.insert("WINESERVER".to_string(), runner_dir.join("bin/wineserver").to_string_lossy().to_string());

    // Apple Silicon & GPTK 4 features
    env.insert("D3DM_MTL4".to_string(), "1".to_string());
    env.insert("D3DM_ENABLE_METALFX".to_string(), "1".to_string());
    env.insert("D3DM_SUPPORT_DXR".to_string(), "1".to_string());
    env.insert("ROSETTA_ADVERTISE_AVX".to_string(), "1".to_string());
    env.insert("WINEMSYNC".to_string(), "1".to_string());
    env.insert("WINEESYNC".to_string(), "1".to_string());

    // DirectX / Metal DLL Overrides
    env.insert(
        "WINEDLLOVERRIDES".to_string(),
        "d3d11,dxgi,d3d12,d3d10core,d3dcompiler_47=n,b;nvapi64,nvngx=n,b;steamclient,steamclient64,lsteamclient=n,b".to_string(),
    );

    if enable_hud {
        env.insert("MTL_HUD_ENABLED".to_string(), "1".to_string());
    }

    // Dynamic linker paths
    let lib_ext = runner_dir.join("lib/external");
    let lib_dir = runner_dir.join("lib");
    let lib_unix = runner_dir.join("lib/wine/x86_64-unix");
    env.insert(
        "DYLD_FALLBACK_LIBRARY_PATH".to_string(),
        format!("{}:{}:{}", lib_ext.display(), lib_unix.display(), lib_dir.display()),
    );

    // Overlay shim for D3DMetal view & Metal 4 bridging
    let overlay_shim = paths::support_dir().join("overlay-shim.dylib");
    if overlay_shim.exists() {
        env.insert("DYLD_INSERT_LIBRARIES".to_string(), overlay_shim.to_string_lossy().to_string());
    }

    env
}
