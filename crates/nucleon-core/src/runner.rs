use crate::detector::TargetEngine;
use crate::paths;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

    // 2. Apple Game Porting Toolkit locations
    candidates.push(PathBuf::from(
        "/Applications/Game Porting Toolkit.app/Contents/Resources/wine",
    ));
    candidates.push(PathBuf::from(
        "/opt/homebrew/opt/game-porting-toolkit/Contents/Resources/wine",
    ));
    candidates.push(PathBuf::from("/opt/homebrew/opt/game-porting-toolkit"));

    // 4. Wine Staging locations
    candidates.push(PathBuf::from(
        "/Applications/Wine Staging.app/Contents/Resources/wine",
    ));
    candidates.push(PathBuf::from("/Applications/Wine Staging.app"));
    candidates.push(PathBuf::from(
        "/opt/homebrew/opt/wine-staging/Contents/Resources/wine",
    ));
    candidates.push(PathBuf::from("/opt/homebrew/opt/wine-staging"));
    candidates.push(PathBuf::from("/usr/local/opt/wine-staging"));

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

pub fn find_gptk_runner() -> Option<PathBuf> {
    let gptk_beta = paths::runners_dir().join("gptk-4-beta2");
    if gptk_beta.join("bin/wine").is_file()
        && gptk_beta.join("lib/external/D3DMetal.framework").is_dir()
    {
        return Some(gptk_beta);
    }
    let gptk = paths::runners_dir().join("gptk-4");
    if gptk.join("bin/wine").is_file() && gptk.join("lib/external/D3DMetal.framework").is_dir() {
        return Some(gptk);
    }
    None
}

pub fn find_wine_staging_runtime() -> Option<PathBuf> {
    // 1. Explicit NUCLEON_WINE_PATH env var
    if let Ok(p) = std::env::var("NUCLEON_WINE_PATH") {
        let pb = PathBuf::from(p);
        if pb.join("bin/wine").is_file() && pb.join("bin/wineserver").is_file() {
            return Some(pb);
        }
    }

    // 2. Desired/active Wine runtime (Heroic, Homebrew, Whisky, CrossOver, custom, etc.)
    if let Some(active) = crate::wine::get_active_wine_runtime() {
        return Some(active.root);
    }

    // 3. Staged runner in nucleon runners dir
    let staging_staged = paths::runners_dir().join("wine-staging");
    if staging_staged.join("bin/wine").is_file() {
        return Some(staging_staged);
    }

    // 4. Check system/homebrew paths
    let staging_candidates = [
        PathBuf::from("/opt/homebrew/opt/wine-staging/Contents/Resources/wine"),
        PathBuf::from("/opt/homebrew/opt/wine-staging"),
        PathBuf::from("/Applications/Wine Staging.app/Contents/Resources/wine"),
        PathBuf::from("/Applications/Wine Staging.app"),
        PathBuf::from("/usr/local/opt/wine-staging"),
    ];

    for c in &staging_candidates {
        if c.join("bin/wine").is_file() && c.join("bin/wineserver").is_file() {
            return Some(c.clone());
        }
    }

    None
}

/// Locates the Mesa KosmicKrisp Vulkan ICD manifest on macOS.
pub fn find_kosmickrisp_icd() -> Option<PathBuf> {
    // 1. Check explicit environment variables
    if let Ok(path) = std::env::var("VK_DRIVER_FILES") {
        let p = PathBuf::from(path);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(path) = std::env::var("VK_ICD_FILENAMES") {
        let p = PathBuf::from(path);
        if p.is_file() {
            return Some(p);
        }
    }

    // 2. Check VULKAN_SDK environment variable
    if let Ok(sdk) = std::env::var("VULKAN_SDK") {
        let sdk_icd = PathBuf::from(sdk).join("share/vulkan/icd.d/libkosmickrisp_icd.json");
        if sdk_icd.is_file() {
            return Some(sdk_icd);
        }
    }

    // 3. Check well-known LunarG SDK, Homebrew, and Nucleon driver paths
    let candidates = [
        paths::kosmickrisp_dir().join("libkosmickrisp_icd.json"),
        paths::runners_dir().join("kosmickrisp/libkosmickrisp_icd.json"),
        PathBuf::from(
            "/Library/Frameworks/Vulkan.framework/Resources/vulkan/icd.d/libkosmickrisp_icd.json",
        ),
        PathBuf::from("/opt/homebrew/share/vulkan/icd.d/libkosmickrisp_icd.json"),
        PathBuf::from("/usr/local/share/vulkan/icd.d/libkosmickrisp_icd.json"),
        paths::home_dir().join(".local/share/vulkan/icd.d/libkosmickrisp_icd.json"),
    ];

    for c in &candidates {
        if c.is_file() {
            return Some(c.clone());
        }
    }

    None
}

/// Locates the Mesa KosmicKrisp Vulkan driver dylib (`libvulkan_kosmickrisp.dylib`).
pub fn find_kosmickrisp_driver_dylib() -> Option<PathBuf> {
    if let Some(icd) = find_kosmickrisp_icd() {
        if let Ok(content) = fs::read_to_string(&icd) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(lib_str) = val["ICD"]["library_path"].as_str() {
                    let p = PathBuf::from(lib_str);
                    if p.is_absolute() && p.is_file() {
                        return Some(p);
                    }
                    if let Some(parent) = icd.parent() {
                        let resolved = parent.join(lib_str);
                        if resolved.is_file() {
                            return Some(resolved);
                        }
                    }
                }
            }
        }
        if let Some(parent) = icd.parent() {
            let sibling = parent.join("libvulkan_kosmickrisp.dylib");
            if sibling.is_file() {
                return Some(sibling);
            }
        }
    }

    let candidates = [
        paths::kosmickrisp_dir().join("libvulkan_kosmickrisp.dylib"),
        PathBuf::from("/opt/homebrew/lib/libvulkan_kosmickrisp.dylib"),
        PathBuf::from("/usr/local/lib/libvulkan_kosmickrisp.dylib"),
        PathBuf::from("/Library/Frameworks/Vulkan.framework/Resources/libvulkan_kosmickrisp.dylib"),
    ];

    for c in &candidates {
        if c.is_file() {
            return Some(c.clone());
        }
    }

    None
}

/// Sets up a shim directory where `libMoltenVK.dylib` links to `libvulkan_kosmickrisp.dylib`,
/// allowing Wine's `winemac.so` (which calls `dlopen("libMoltenVK.dylib")`) to load KosmicKrisp.
pub fn setup_kosmickrisp_shim() -> Result<PathBuf> {
    let shim_dir = paths::kosmickrisp_shim_dir();
    fs::create_dir_all(&shim_dir)?;

    if let Some(kk_lib) = find_kosmickrisp_driver_dylib() {
        let dst_moltenvk = shim_dir.join("libMoltenVK.dylib");
        if dst_moltenvk.exists() || dst_moltenvk.is_symlink() {
            let _ = fs::remove_file(&dst_moltenvk);
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&kk_lib, &dst_moltenvk)?;
    }

    Ok(shim_dir)
}

/// Returns true if the KosmicKrisp driver is detected on the system or forced via environment.
pub fn is_kosmickrisp_installed() -> bool {
    if std::env::var("KOSMICKRISP_DISABLE").is_ok() {
        return false;
    }
    std::env::var("KOSMICKRISP_FORCE").is_ok()
        || find_kosmickrisp_icd().is_some()
        || find_kosmickrisp_driver_dylib().is_some()
}

/// Resolves the optimal runner for the requested TargetEngine.
pub fn resolve_runner_for_engine(engine: TargetEngine) -> Result<(PathBuf, TargetEngine)> {
    match engine {
        TargetEngine::Gptk => {
            if let Some(gptk) = find_gptk_runner() {
                return Ok((gptk, TargetEngine::Gptk));
            }
            if let Some(staging) = find_wine_staging_runtime() {
                log::warn!(
                    "Apple GPTK runner not found; falling back to Wine-Staging for DirectX 11/12"
                );
                return Ok((staging, TargetEngine::WineStaging));
            }
            let assembled = assemble_runner(false)?;
            Ok((assembled, TargetEngine::Gptk))
        }
        TargetEngine::KosmicKrisp => {
            // KosmicKrisp utilizes a 64-bit Wine runtime configured with Mesa Vulkan 1.4 ICD
            if let Some(gptk) = find_gptk_runner() {
                return Ok((gptk, TargetEngine::KosmicKrisp));
            }
            if let Some(staging) = find_wine_staging_runtime() {
                return Ok((staging, TargetEngine::KosmicKrisp));
            }
            let assembled = assemble_runner(false)?;
            Ok((assembled, TargetEngine::KosmicKrisp))
        }
        TargetEngine::WineStaging => {
            if let Some(staging) = find_wine_staging_runtime() {
                return Ok((staging, TargetEngine::WineStaging));
            }
            if let Some(gptk) = find_gptk_runner() {
                log::info!("Wine-Staging not found (install via 'brew install --cask wine-staging'); utilizing GPTK runtime for legacy/DX9 pipeline");
                return Ok((gptk, TargetEngine::Gptk));
            }
            let assembled = assemble_runner(false)?;
            Ok((assembled, TargetEngine::Gptk))
        }
    }
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

    // 3. Check Downloads for DMG and mount if found
    let downloads = paths::home_dir().join("Downloads");
    if let Ok(entries) = fs::read_dir(&downloads) {
        for entry in entries.flatten() {
            let p = entry.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with(".dmg")
                && (name.contains("Evaluation")
                    || name.contains("Porting")
                    || name.contains("GPTK"))
            {
                if let Ok(output) = Command::new("hdiutil")
                    .args(["attach", "-nobrowse", "-readonly", p.to_str().unwrap()])
                    .output()
                {
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

    if !force
        && target.join("bin/wine").is_file()
        && target.join("lib/external/D3DMetal.framework").is_dir()
    {
        let cur = paths::current_runner();
        if cur.is_symlink() || cur.is_file() {
            let _ = fs::remove_file(&cur);
        } else if cur.is_dir() {
            let _ = fs::remove_dir_all(&cur);
        }
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
            "--exclude",
            "lib/external",
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
        .args([
            "-R",
            "-f",
            d3dm_fw.to_str().unwrap(),
            ext_dir.to_str().unwrap(),
        ])
        .status()?;
    let _ = Command::new("cp")
        .args([
            "-f",
            d3d_shared.to_str().unwrap(),
            ext_dir.to_str().unwrap(),
        ])
        .status()?;

    // Install overlay-shim.dylib
    let overlay_dst = paths::support_dir().join("overlay-shim.dylib");
    fs::write(&overlay_dst, overlay_shim::OVERLAY_SHIM_BYTES)?;
    crate::steam::sign_binary(&overlay_dst)?;

    // Link current runner
    let cur = paths::current_runner();
    if cur.is_symlink() || cur.is_file() {
        let _ = fs::remove_file(&cur);
    } else if cur.is_dir() {
        let _ = fs::remove_dir_all(&cur);
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &cur)?;

    Ok(target)
}

pub fn build_execution_env(
    runner_dir: &Path,
    prefix_dir: &Path,
    enable_hud: bool,
) -> HashMap<String, String> {
    build_execution_env_for_engine(runner_dir, prefix_dir, TargetEngine::Gptk, enable_hud)
}

pub fn build_execution_env_for_engine(
    runner_dir: &Path,
    prefix_dir: &Path,
    engine: TargetEngine,
    enable_hud: bool,
) -> HashMap<String, String> {
    let mut env = HashMap::new();

    env.insert(
        "WINEPREFIX".to_string(),
        prefix_dir.to_string_lossy().to_string(),
    );
    env.insert(
        "WINELOADER".to_string(),
        runner_dir.join("bin/wine").to_string_lossy().to_string(),
    );
    env.insert(
        "WINESERVER".to_string(),
        runner_dir
            .join("bin/wineserver")
            .to_string_lossy()
            .to_string(),
    );
    env.insert("ROSETTA_ADVERTISE_AVX".to_string(), "1".to_string());
    env.insert("WINEMSYNC".to_string(), "1".to_string());
    env.insert("WINEESYNC".to_string(), "1".to_string());

    match engine {
        TargetEngine::Gptk => {
            // Apple Silicon & GPTK 4 features
            env.insert("D3DM_MTL4".to_string(), "1".to_string());
            env.insert("D3DM_ENABLE_METALFX".to_string(), "1".to_string());
            env.insert("D3DM_SUPPORT_DXR".to_string(), "1".to_string());

            // DirectX 11 & 12 Metal DLL Overrides
            env.insert(
                "WINEDLLOVERRIDES".to_string(),
                "d3d11,dxgi,d3d12,d3d10core,d3dcompiler_47=n,b;nvapi64,nvngx=n,b;steamclient,steamclient64,lsteamclient=n,b".to_string(),
            );

            if enable_hud {
                env.insert("MTL_HUD_ENABLED".to_string(), "1".to_string());
            }

            // Dynamic linker paths including external D3DMetal
            let lib_ext = runner_dir.join("lib/external");
            let lib_dir = runner_dir.join("lib");
            let lib_unix = runner_dir.join("lib/wine/x86_64-unix");
            env.insert(
                "DYLD_FALLBACK_LIBRARY_PATH".to_string(),
                format!(
                    "{}:{}:{}",
                    lib_ext.display(),
                    lib_unix.display(),
                    lib_dir.display()
                ),
            );

            // Overlay shim for D3DMetal view & Metal 4 bridging
            let overlay_shim = paths::support_dir().join("overlay-shim.dylib");
            if overlay_shim.exists() {
                env.insert(
                    "DYLD_INSERT_LIBRARIES".to_string(),
                    overlay_shim.to_string_lossy().to_string(),
                );
            }
        }
        TargetEngine::KosmicKrisp => {
            // Configure Vulkan loader to point to Mesa KosmicKrisp driver
            if let Some(icd) = find_kosmickrisp_icd() {
                env.insert(
                    "VK_DRIVER_FILES".to_string(),
                    icd.to_string_lossy().to_string(),
                );
                env.insert(
                    "VK_ICD_FILENAMES".to_string(),
                    icd.to_string_lossy().to_string(),
                );
            }
            env.insert(
                "MESA_LOADER_DRIVER_OVERRIDE".to_string(),
                "kosmickrisp".to_string(),
            );

            // Prepare MoltenVK -> KosmicKrisp shim for winemac.so if driver dylib exists
            let shim_dir =
                setup_kosmickrisp_shim().unwrap_or_else(|_| paths::kosmickrisp_shim_dir());

            // Wine DLL overrides: map Direct3D to DXVK/VKD3D (n,b) and forward Vulkan to host KosmicKrisp
            env.insert(
                "WINEDLLOVERRIDES".to_string(),
                "steamclient,steamclient64,lsteamclient=n,b;winevulkan=b,n;vulkan-1=b,n;d3d11,dxgi,d3d10core,d3d9,d3d12,d3d12core=n,b".to_string(),
            );

            // Configure VKD3D-Proton features if available
            if crate::vkd3d::find_vkd3d_proton().is_some() {
                env.insert("VKD3D_CONFIG".to_string(), "dxr11,dxr".to_string());
            }

            // Redirect Mesa shader cache to macOS Library/Caches
            let mesa_cache = paths::home_dir().join("Library/Caches/Mesa");
            let _ = fs::create_dir_all(&mesa_cache);
            env.insert(
                "MESA_SHADER_CACHE_DIR".to_string(),
                mesa_cache.to_string_lossy().to_string(),
            );

            if enable_hud {
                env.insert("MTL_HUD_ENABLED".to_string(), "1".to_string());
                env.insert("VKD3D_DEBUG".to_string(), "warn".to_string());
            }

            let lib_dir = runner_dir.join("lib");
            let lib_unix = runner_dir.join("lib/wine/x86_64-unix");
            env.insert(
                "DYLD_FALLBACK_LIBRARY_PATH".to_string(),
                format!(
                    "{}:{}:{}",
                    shim_dir.display(),
                    lib_unix.display(),
                    lib_dir.display()
                ),
            );
        }
        TargetEngine::WineStaging => {
            // Wine-Staging legacy overrides: map DX9, DX10 to built-in WineD3D / OpenGL
            env.insert(
                "WINEDLLOVERRIDES".to_string(),
                "steamclient,steamclient64,lsteamclient=n,b;d3d9,d3d10,d3d10_1,d3d10core=b,n"
                    .to_string(),
            );

            let lib_dir = runner_dir.join("lib");
            let lib_unix = runner_dir.join("lib/wine/x86_64-unix");
            env.insert(
                "DYLD_FALLBACK_LIBRARY_PATH".to_string(),
                format!("{}:{}", lib_unix.display(), lib_dir.display()),
            );
        }
    }

    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_build_execution_env_kosmickrisp() {
        let runner_dir = PathBuf::from("/tmp/test_runner");
        let prefix_dir = PathBuf::from("/tmp/test_prefix");

        let env = build_execution_env_for_engine(
            &runner_dir,
            &prefix_dir,
            TargetEngine::KosmicKrisp,
            true,
        );

        assert_eq!(
            env.get("MESA_LOADER_DRIVER_OVERRIDE").map(|s| s.as_str()),
            Some("kosmickrisp")
        );
        assert_eq!(env.get("MTL_HUD_ENABLED").map(|s| s.as_str()), Some("1"));
        assert_eq!(env.get("WINEMSYNC").map(|s| s.as_str()), Some("1"));

        let dll_overrides = env.get("WINEDLLOVERRIDES").unwrap();
        assert!(dll_overrides.contains("winevulkan=b,n"));
        assert!(dll_overrides.contains("d3d12,d3d12core=n,b"));

        let dyld_fallback = env.get("DYLD_FALLBACK_LIBRARY_PATH").unwrap();
        assert!(dyld_fallback.contains("shims/kosmickrisp"));
    }

    #[test]
    fn test_find_kosmickrisp_icd_env() {
        let dir = tempdir().unwrap();
        let icd_file = dir.path().join("test_icd.json");
        fs::write(&icd_file, "{}").unwrap();

        std::env::set_var("VK_DRIVER_FILES", &icd_file);
        let found = find_kosmickrisp_icd();
        assert_eq!(found, Some(icd_file.clone()));
        std::env::remove_var("VK_DRIVER_FILES");
    }
}
