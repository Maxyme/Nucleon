use crate::d7vk;
use crate::detector::TargetEngine;
use crate::dxvk;
use crate::paths;
use crate::steam;
use crate::vkd3d;
use crate::wine;
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::env;
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

pub fn find_gptk_runner() -> Option<PathBuf> {
    // 1. Check explicit environment variables
    for var in &["NUCLEON_GPTK_RUNNER", "NUCLEON_GPTK_PATH"] {
        if let Ok(p) = env::var(var) {
            let pb = PathBuf::from(p);
            if pb.join("bin/wine").is_file() && pb.join("lib/external/D3DMetal.framework").is_dir()
            {
                return Some(pb);
            }
        }
    }

    // 2. Check current runner symlink
    let cur = paths::current_runner();
    if cur.join("bin/wine").is_file() && cur.join("lib/external/D3DMetal.framework").is_dir() {
        return Some(cur);
    }

    // 3. Check staged/assembled runners in runners directory
    let runners_dir = paths::runners_dir();
    if let Ok(entries) = fs::read_dir(&runners_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir()
                && p.join("bin/wine").is_file()
                && p.join("lib/external/D3DMetal.framework").is_dir()
            {
                return Some(p);
            }
        }
    }

    None
}

/// Returns the path to the active or default Wine runtime.
pub fn find_wine_staging_runtime() -> Option<PathBuf> {
    wine::get_active_wine_runtime().map(|r| r.root)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct KosmicKrispInfo {
    pub icd_path: PathBuf,
    pub library_path: PathBuf,
    pub api_version: String,
    pub is_custom: bool,
}

pub fn custom_kosmickrisp_path_file() -> PathBuf {
    paths::support_dir().join("custom_kosmickrisp_path.txt")
}

pub fn write_kosmickrisp_icd(
    target_icd: &Path,
    library_path: &Path,
    api_version: &str,
) -> Result<()> {
    if let Some(parent) = target_icd.parent() {
        fs::create_dir_all(parent)?;
    }
    let json_content = serde_json::json!({
        "file_format_version": "1.0.0",
        "ICD": {
            "library_path": library_path.to_string_lossy(),
            "api_version": api_version
        }
    });
    fs::write(
        target_icd,
        serde_json::to_string_pretty(&json_content)? + "\n",
    )?;
    Ok(())
}

pub fn detect_vulkan_dylib_api_version(dylib_path: &Path) -> Option<String> {
    use std::io::Read;
    let mut f = fs::File::open(dylib_path).ok()?;
    let mut buffer = Vec::new();
    f.by_ref()
        .take(8 * 1024 * 1024)
        .read_to_end(&mut buffer)
        .ok()?;

    if let Some(pos) = buffer.windows(11).position(|w| w == b"vulkan-sdk-") {
        let after = &buffer[pos + 11..];
        let ver_bytes: Vec<u8> = after
            .iter()
            .copied()
            .take_while(|&b| b.is_ascii_digit() || b == b'.')
            .collect();
        if let Ok(ver_str) = String::from_utf8(ver_bytes) {
            let parts: Vec<&str> = ver_str.split('.').collect();
            if parts.len() >= 2 {
                return Some(format!("{}.{}.0", parts[0], parts[1]));
            }
        }
    }

    for prefix in &[b"1.4.", b"1.3."] {
        if let Some(pos) = buffer.windows(prefix.len()).position(|w| w == *prefix) {
            let after = &buffer[pos..];
            let ver_bytes: Vec<u8> = after
                .iter()
                .copied()
                .take_while(|&b| b.is_ascii_digit() || b == b'.')
                .collect();
            if let Ok(ver_str) = String::from_utf8(ver_bytes) {
                let parts: Vec<&str> = ver_str.split('.').collect();
                if parts.len() >= 3 {
                    return Some(format!("{}.{}.{}", parts[0], parts[1], parts[2]));
                } else if parts.len() == 2 {
                    return Some(format!("{}.{}.0", parts[0], parts[1]));
                }
            }
        }
    }

    None
}

pub fn inspect_kosmickrisp_candidate(
    path: &Path,
    api_version_override: Option<&str>,
) -> Result<KosmicKrispInfo> {
    if !path.exists() {
        bail!(
            "Specified KosmicKrisp path does not exist: {}",
            path.display()
        );
    }

    // 1. Path is a JSON ICD manifest
    if path.is_file()
        && (path.extension().and_then(|s| s.to_str()) == Some("json")
            || fs::read_to_string(path)
                .map(|c| c.contains("\"ICD\""))
                .unwrap_or(false))
    {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read ICD manifest at {}", path.display()))?;
        let val: serde_json::Value = serde_json::from_str(&content)
            .with_context(|| format!("Invalid JSON in ICD manifest at {}", path.display()))?;

        let icd_obj = val
            .get("ICD")
            .context("Missing 'ICD' section in Vulkan manifest")?;
        let lib_str = icd_obj
            .get("library_path")
            .and_then(|v| v.as_str())
            .context("Missing 'library_path' in ICD manifest")?;

        let resolved_lib = {
            let p = PathBuf::from(lib_str);
            if p.is_absolute() && p.is_file() {
                p
            } else if let Some(parent) = path.parent() {
                let cand = parent.join(lib_str);
                if let Ok(canon) = cand.canonicalize() {
                    canon
                } else {
                    cand
                }
            } else {
                p
            }
        };

        let detected_ver = icd_obj
            .get("api_version")
            .and_then(|v| v.as_str())
            .unwrap_or("1.4.0");

        let api_version = api_version_override.unwrap_or(detected_ver).to_string();

        return Ok(KosmicKrispInfo {
            icd_path: path.to_path_buf(),
            library_path: resolved_lib,
            api_version,
            is_custom: true,
        });
    }

    // 2. Path is a driver dylib
    if path.is_file() {
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if ext == "dylib"
            || path
                .file_name()
                .and_then(|s| s.to_str())
                .map(|n| n.contains("libvulkan"))
                .unwrap_or(false)
        {
            let api_version = if let Some(ver) = api_version_override {
                ver.to_string()
            } else if let Some(ver) = detect_vulkan_dylib_api_version(path) {
                ver
            } else {
                "1.4.0".to_string()
            };

            let target_icd = paths::kosmickrisp_dir().join("libkosmickrisp_icd.json");
            return Ok(KosmicKrispInfo {
                icd_path: target_icd,
                library_path: path.to_path_buf(),
                api_version,
                is_custom: true,
            });
        }
    }

    // 3. Path is a directory
    if path.is_dir() {
        let json_candidates = [
            path.join("libkosmickrisp_icd.json"),
            path.join("share/vulkan/icd.d/libkosmickrisp_icd.json"),
        ];
        for jc in &json_candidates {
            if jc.is_file() {
                return inspect_kosmickrisp_candidate(jc, api_version_override);
            }
        }

        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("json") {
                    if let Ok(content) = fs::read_to_string(&p) {
                        if content.contains("\"ICD\"") {
                            return inspect_kosmickrisp_candidate(&p, api_version_override);
                        }
                    }
                }
            }
        }

        let dylib_candidates = [
            path.join("libvulkan_kosmickrisp.dylib"),
            path.join("lib/libvulkan_kosmickrisp.dylib"),
        ];
        for dc in &dylib_candidates {
            if dc.is_file() {
                return inspect_kosmickrisp_candidate(dc, api_version_override);
            }
        }

        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("dylib") {
                    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    if name.contains("kosmickrisp") || name.contains("vulkan") {
                        return inspect_kosmickrisp_candidate(&p, api_version_override);
                    }
                }
            }
        }

        bail!(
            "No Vulkan ICD manifest (*.json) or driver dylib (*.dylib) found in directory: {}",
            path.display()
        );
    }

    bail!(
        "Invalid KosmicKrisp path: {}. Expected an ICD JSON manifest, driver dylib, or directory.",
        path.display()
    )
}

pub fn set_custom_kosmickrisp_path(
    path: &Path,
    api_version_override: Option<&str>,
) -> Result<KosmicKrispInfo> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("Path does not exist: {}", path.display()))?;

    let info = inspect_kosmickrisp_candidate(&canonical, api_version_override)?;

    paths::ensure_dirs()?;
    if canonical.is_file() && canonical.extension().and_then(|s| s.to_str()) != Some("json") {
        write_kosmickrisp_icd(&info.icd_path, &info.library_path, &info.api_version)?;
    }

    fs::write(
        custom_kosmickrisp_path_file(),
        canonical.to_string_lossy().as_bytes(),
    )?;

    log::info!(
        "Registered custom KosmicKrisp driver (Vulkan {}) at {}",
        info.api_version,
        info.icd_path.display()
    );

    Ok(info)
}

pub fn clear_custom_kosmickrisp_path() -> Result<()> {
    let custom_file = custom_kosmickrisp_path_file();
    if custom_file.is_file() {
        let _ = fs::remove_file(&custom_file);
    }
    Ok(())
}

/// Locates the Mesa KosmicKrisp Vulkan ICD manifest on macOS.
pub fn find_kosmickrisp_icd() -> Option<PathBuf> {
    // 1. Check explicit environment variables
    if let Ok(path) = env::var("VK_DRIVER_FILES") {
        let p = PathBuf::from(path);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(path) = env::var("VK_ICD_FILENAMES") {
        let p = PathBuf::from(path);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(path) = env::var("KOSMICKRISP_ICD_PATH") {
        let p = PathBuf::from(path);
        if p.is_file() {
            return Some(p);
        }
    }

    // 2. Check custom configured KosmicKrisp path (set via `nucleon kosmickrisp set-path` or `--kosmickrisp-path`)
    let custom_file = custom_kosmickrisp_path_file();
    if custom_file.is_file() {
        if let Ok(content) = fs::read_to_string(&custom_file) {
            let p = PathBuf::from(content.trim());
            if p.is_file() {
                if p.extension().and_then(|s| s.to_str()) == Some("json") {
                    return Some(p);
                } else {
                    let staged = paths::kosmickrisp_dir().join("libkosmickrisp_icd.json");
                    if staged.is_file() {
                        return Some(staged);
                    }
                }
            } else if p.is_dir() {
                if let Ok(info) = inspect_kosmickrisp_candidate(&p, None) {
                    return Some(info.icd_path);
                }
            }
        }
    }

    // 3. Check VULKAN_SDK environment variable
    if let Ok(sdk) = env::var("VULKAN_SDK") {
        let sdk_icd = PathBuf::from(sdk).join("share/vulkan/icd.d/libkosmickrisp_icd.json");
        if sdk_icd.is_file() {
            return Some(sdk_icd);
        }
    }

    // 4. Check well-known LunarG SDK, Homebrew, and Nucleon driver paths
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
    if let Ok(path) = env::var("KOSMICKRISP_DRIVER_PATH") {
        let p = PathBuf::from(path);
        if p.is_file() {
            return Some(p);
        }
    }

    let custom_file = custom_kosmickrisp_path_file();
    if custom_file.is_file() {
        if let Ok(content) = fs::read_to_string(&custom_file) {
            let p = PathBuf::from(content.trim());
            if p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("dylib") {
                return Some(p);
            }
        }
    }

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
                        if let Ok(canon) = resolved.canonicalize() {
                            if canon.is_file() {
                                return Some(canon);
                            }
                        }
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

pub fn get_kosmickrisp_info() -> Option<KosmicKrispInfo> {
    let icd = find_kosmickrisp_icd()?;
    let content = fs::read_to_string(&icd).ok()?;
    let val = serde_json::from_str::<serde_json::Value>(&content).ok()?;
    let api_version = val["ICD"]["api_version"]
        .as_str()
        .unwrap_or("1.4.0")
        .to_string();
    let library_path = if let Some(lib_str) = val["ICD"]["library_path"].as_str() {
        let p = PathBuf::from(lib_str);
        if p.is_absolute() && p.is_file() {
            p
        } else if let Some(parent) = icd.parent() {
            let resolved = parent.join(lib_str);
            if let Ok(canon) = resolved.canonicalize() {
                if canon.is_file() {
                    canon
                } else {
                    find_kosmickrisp_driver_dylib().unwrap_or(resolved)
                }
            } else if resolved.is_file() {
                resolved
            } else {
                find_kosmickrisp_driver_dylib().unwrap_or(p)
            }
        } else {
            find_kosmickrisp_driver_dylib().unwrap_or(p)
        }
    } else {
        find_kosmickrisp_driver_dylib()
            .unwrap_or_else(|| PathBuf::from("libvulkan_kosmickrisp.dylib"))
    };

    let custom_file = custom_kosmickrisp_path_file();
    let is_custom = if custom_file.is_file() {
        if let Ok(c) = fs::read_to_string(&custom_file) {
            let p = PathBuf::from(c.trim());
            p == icd
                || p == library_path
                || icd == paths::kosmickrisp_dir().join("libkosmickrisp_icd.json")
        } else {
            false
        }
    } else {
        false
    };

    Some(KosmicKrispInfo {
        icd_path: icd,
        library_path,
        api_version,
        is_custom,
    })
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
    if env::var("KOSMICKRISP_DISABLE").is_ok() {
        return false;
    }
    env::var("KOSMICKRISP_FORCE").is_ok()
        || find_kosmickrisp_icd().is_some()
        || find_kosmickrisp_driver_dylib().is_some()
}

/// Resolves the optimal runner for the requested TargetEngine.
pub fn resolve_runner_for_engine(engine: TargetEngine) -> Result<(PathBuf, TargetEngine)> {
    match engine {
        TargetEngine::Auto => {
            // Auto runner is for Wine only
            if let Some(staging) = find_wine_staging_runtime() {
                return Ok((staging, TargetEngine::WineStaging));
            }
            if let Some(gptk) = find_gptk_runner() {
                log::warn!("Wine runtime not found; utilizing available GPTK wine binary");
                return Ok((gptk, TargetEngine::WineStaging));
            }
            let assembled = assemble_runner(false, None, None)?;
            Ok((assembled, TargetEngine::WineStaging))
        }
        TargetEngine::Gptk => {
            if let Some(gptk) = find_gptk_runner() {
                return Ok((gptk, TargetEngine::Gptk));
            }
            let assembled = assemble_runner(false, None, None).with_context(|| {
                "Failed to resolve Apple GPTK runner. Please ensure Apple GPTK 4 components are configured via 'nucleon gptk set-path <DIR>', --gptk-path, or NUCLEON_GPTK_PATH."
            })?;
            Ok((assembled, TargetEngine::Gptk))
        }
        TargetEngine::KosmicKrisp
        | TargetEngine::Dxmt
        | TargetEngine::Dxvk
        | TargetEngine::Vkd3d => {
            // KosmicKrisp, DXMT, DXVK, and VKD3D run under Wine
            if let Some(staging) = find_wine_staging_runtime() {
                return Ok((staging, engine));
            }
            if let Some(gptk) = find_gptk_runner() {
                return Ok((gptk, engine));
            }
            let assembled = assemble_runner(false, None, None)?;
            Ok((assembled, engine))
        }
        TargetEngine::WineStaging => {
            if let Some(staging) = find_wine_staging_runtime() {
                return Ok((staging, TargetEngine::WineStaging));
            }
            if let Some(gptk) = find_gptk_runner() {
                log::info!("Wine runtime not found (install via 'brew install --cask wine-staging'); utilizing GPTK runtime for legacy/DX9 pipeline");
                return Ok((gptk, TargetEngine::WineStaging));
            }
            let assembled = assemble_runner(false, None, None)?;
            Ok((assembled, TargetEngine::WineStaging))
        }
    }
}

/// Inspects a directory or bundle to locate Apple GPTK components (`D3DMetal.framework` and `libd3dshared.dylib`).
pub fn inspect_gptk_dir(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    if !dir.exists() {
        return None;
    }

    if dir.is_dir() && dir.file_name().and_then(|n| n.to_str()) == Some("D3DMetal.framework") {
        if let Some(parent) = dir.parent() {
            for sub in &[
                "libd3dshared.dylib",
                "lib/external/libd3dshared.dylib",
                "lib/libd3dshared.dylib",
            ] {
                let shared = parent.join(sub);
                if shared.is_file() {
                    return Some((dir.to_path_buf(), shared));
                }
            }
        }
    }

    let candidates = [
        (
            "lib/external/D3DMetal.framework",
            "lib/external/libd3dshared.dylib",
        ),
        (
            "redist/lib/external/D3DMetal.framework",
            "redist/lib/external/libd3dshared.dylib",
        ),
        (
            "Contents/Resources/wine/lib/external/D3DMetal.framework",
            "Contents/Resources/wine/lib/external/libd3dshared.dylib",
        ),
        ("D3DMetal.framework", "libd3dshared.dylib"),
        ("lib/D3DMetal.framework", "lib/libd3dshared.dylib"),
        (
            "redist/lib/D3DMetal.framework",
            "redist/lib/libd3dshared.dylib",
        ),
    ];

    for (fw_rel, shared_rel) in &candidates {
        let fw = dir.join(fw_rel);
        let shared = dir.join(shared_rel);
        if fw.is_dir() && shared.is_file() {
            return Some((fw, shared));
        }
    }

    // Check immediate subdirectories in dir (e.g. "Evaluation environment for Windows games 4.0 beta 2")
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                for (fw_rel, shared_rel) in &candidates {
                    let fw = p.join(fw_rel);
                    let shared = p.join(shared_rel);
                    if fw.is_dir() && shared.is_file() {
                        return Some((fw, shared));
                    }
                }
            }
        }
    }

    None
}

pub fn custom_gptk_path_file() -> PathBuf {
    paths::support_dir().join("custom_gptk_path.txt")
}

/// Validates that a user-specified path exists, is a directory, and contains valid GPTK components
/// (`D3DMetal.framework` and `libd3dshared.dylib`).
pub fn validate_and_resolve_gptk_components(path: &Path) -> Result<(PathBuf, PathBuf)> {
    if !path.exists() {
        bail!(
            "Specified GPTK path does not exist: {}. Please provide a valid directory containing Apple GPTK components.",
            path.display()
        );
    }
    if !path.is_dir() {
        bail!(
            "Specified GPTK path is not a directory: {}. Please provide a directory containing D3DMetal.framework and libd3dshared.dylib.",
            path.display()
        );
    }
    inspect_gptk_dir(path).with_context(|| {
        format!(
            "Directory does not contain valid Apple GPTK components (D3DMetal.framework and libd3dshared.dylib not found in {}).",
            path.display()
        )
    })
}

/// Sets a custom Apple GPTK components path and validates it.
/// The user is expected to have exported the components to a directory.
pub fn set_custom_gptk_path(path: &Path) -> Result<(PathBuf, PathBuf)> {
    let components = validate_and_resolve_gptk_components(path)?;
    let canonical = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf());

    paths::ensure_dirs()?;
    let custom_file = custom_gptk_path_file();
    fs::write(
        &custom_file,
        canonical.to_string_lossy().as_bytes(),
    )
    .with_context(|| format!("Failed to write {}", custom_file.display()))?;

    log::info!("Registered custom GPTK path at {}", canonical.display());
    Ok(components)
}

/// Clears any configured custom Apple GPTK path.
pub fn clear_custom_gptk_path() -> Result<()> {
    let custom_file = custom_gptk_path_file();
    if custom_file.is_file() {
        let _ = fs::remove_file(&custom_file);
    }
    Ok(())
}

/// Resolves Apple GPTK components from user configuration.
/// Priority:
/// 1. Explicitly provided custom path
/// 2. Environment variables (`NUCLEON_GPTK_PATH`, `GPTK_PATH`)
/// 3. Persisted user configuration file (`custom_gptk_path.txt`)
///
/// Automatic search fallback is strictly removed: if a user path is configured, it is validated
/// and returns an error if invalid. If no path is configured, returns Ok(None).
pub fn find_gptk_components(custom_path: Option<&Path>) -> Result<Option<(PathBuf, PathBuf)>> {
    // 1. Explicitly provided custom path
    if let Some(p) = custom_path {
        let comps = validate_and_resolve_gptk_components(p)?;
        return Ok(Some(comps));
    }

    // 2. Explicit environment variables
    for var in &["NUCLEON_GPTK_PATH", "GPTK_PATH"] {
        if let Ok(val) = env::var(var) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                let p = PathBuf::from(trimmed);
                let comps = validate_and_resolve_gptk_components(&p).with_context(|| {
                    format!("Environment variable {} points to an invalid GPTK path", var)
                })?;
                return Ok(Some(comps));
            }
        }
    }

    // 3. Persisted custom path (configured manually by user via `nucleon gptk set-path`)
    let custom_file = custom_gptk_path_file();
    if custom_file.is_file() {
        let content = fs::read_to_string(&custom_file)
            .with_context(|| format!("Failed to read custom GPTK setting from {}", custom_file.display()))?;
        let trimmed = content.trim();
        if !trimmed.is_empty() {
            let p = PathBuf::from(trimmed);
            let comps = validate_and_resolve_gptk_components(&p).with_context(|| {
                format!(
                    "Configured custom GPTK path in {} is invalid",
                    custom_file.display()
                )
            })?;
            return Ok(Some(comps));
        }
    }

    Ok(None)
}

/// Resolves user-provided GPTK components, failing loudly if not configured or invalid.
pub fn resolve_gptk_components(custom_path: Option<&Path>) -> Result<(PathBuf, PathBuf)> {
    find_gptk_components(custom_path)?.context(
        "Apple Game Porting Toolkit 4.0 path is not configured. The GPTK4 path must be provided by the user: configure it via 'nucleon gptk set-path <DIR>', pass '--gptk-path <DIR>', or set NUCLEON_GPTK_PATH=<DIR>.",
    )
}

/// Detects the version of Apple Game Porting Toolkit (GPTK) from D3DMetal.framework
/// plists (version.plist / Info.plist) or directory naming conventions.
pub fn detect_gptk_version(custom_path: Option<&Path>) -> Option<String> {
    // 1. Try to inspect D3DMetal.framework plists
    if let Ok(Some((fw, _))) = find_gptk_components(custom_path) {
        let candidates = [
            fw.join("Resources/version.plist"),
            fw.join("Versions/Current/Resources/version.plist"),
            fw.join("version.plist"),
            fw.join("Resources/Info.plist"),
            fw.join("Versions/Current/Resources/Info.plist"),
            fw.join("Info.plist"),
        ];

        for c in &candidates {
            if c.is_file() {
                if let Ok(plist::Value::Dictionary(dict)) = plist::Value::from_file(c) {
                    if let Some(plist::Value::String(ver)) = dict.get("CFBundleShortVersionString")
                    {
                        let trimmed = ver.trim();
                        if !trimmed.is_empty() {
                            return Some(trimmed.to_string());
                        }
                    }
                }
            }
        }
    }

    // 2. Fallback to runner directory name or custom path naming
    let paths_to_check = [
        custom_path.map(Path::to_path_buf),
        find_gptk_runner(),
    ];

    for p_opt in paths_to_check.into_iter().flatten() {
        let p_str = p_opt.to_string_lossy().to_lowercase();
        if p_str.contains("gptk-4-beta2") || p_str.contains("4.0b2") || p_str.contains("4.0_beta_2")
        {
            return Some("4.0b2".to_string());
        } else if p_str.contains("gptk-4") || p_str.contains("4.0") {
            return Some("4.0".to_string());
        } else if p_str.contains("gptk-2") || p_str.contains("2.0") {
            return Some("2.0".to_string());
        }
    }

    None
}

/// Formats the raw GPTK version into a clean, human-readable display string for Steam dropdowns.
/// e.g. "4.0b2" -> "4.0 Beta 2", "4.0" -> "4.0", "2.0" -> "2.0"
pub fn format_gptk_version(raw_ver: &str) -> String {
    let lower = raw_ver.to_lowercase();
    if lower == "4.0b2" || lower == "4.0-beta2" || lower == "4.0_beta_2" {
        "4.0 Beta 2".to_string()
    } else if lower == "4.0b1" || lower == "4.0-beta1" || lower == "4.0_beta_1" {
        "4.0 Beta 1".to_string()
    } else if lower == "2.0b1" || lower == "2.0-beta1" {
        "2.0 Beta 1".to_string()
    } else if let Some(stripped) = lower.strip_prefix('v') {
        stripped.to_string()
    } else {
        raw_ver.to_string()
    }
}

/// Formats a raw Vulkan API version (e.g. "1.4.0", "1.4.304", or "1.4")
/// to a clean major.minor Vulkan version string (e.g. "1.4").
pub fn format_vulkan_version(raw_ver: &str) -> String {
    let trimmed = raw_ver.trim();
    let parts: Vec<&str> = trimmed.split('.').collect();
    if parts.len() >= 2 {
        format!("{}.{}", parts[0], parts[1])
    } else {
        trimmed.to_string()
    }
}

/// Recursively copies a directory tree including files, directories, and symlinks.
fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_child = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst_child)?;
        } else if ty.is_symlink() {
            #[cfg(unix)]
            {
                let target = fs::read_link(entry.path())?;
                if dst_child.exists() || dst_child.is_symlink() {
                    let _ = fs::remove_file(&dst_child);
                }
                std::os::unix::fs::symlink(target, &dst_child)?;
            }
        } else {
            fs::copy(entry.path(), &dst_child)?;
        }
    }
    Ok(())
}

pub fn assemble_runner(
    force: bool,
    custom_wine: Option<&Path>,
    custom_gptk: Option<&Path>,
) -> Result<PathBuf> {
    paths::ensure_dirs()?;
    let target = paths::runners_dir().join("gptk-4-beta2");

    let has_explicit = custom_wine.is_some() || custom_gptk.is_some();
    if !force
        && !has_explicit
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

    let wine_src = if let Some(custom) = custom_wine {
        if let Some(rt) = wine::inspect_wine_dir(custom, Some("custom"), Some("Custom Wine")) {
            rt.root
        } else {
            anyhow::bail!(
                "Specified custom Wine path is not a valid Wine runtime (bin/wine not found): {}",
                custom.display()
            );
        }
    } else {
        wine::get_active_wine_runtime()
            .map(|r| r.root)
            .context("No compatible Wine runtime found. Install Game Porting Toolkit via Homebrew: brew tap gcenx/wine && brew install --cask --no-quarantine game-porting-toolkit")?
    };

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
    let (d3dm_fw, d3d_shared) = resolve_gptk_components(custom_gptk)?;

    let ext_dir = target.join("lib/external");
    fs::create_dir_all(&ext_dir)?;

    let target_fw = ext_dir.join("D3DMetal.framework");
    let is_same_fw = if let (Ok(src_canon), Ok(tgt_canon)) =
        (d3dm_fw.canonicalize(), target_fw.canonicalize())
    {
        src_canon == tgt_canon
    } else {
        d3dm_fw == target_fw
    };

    if !is_same_fw {
        if target_fw.exists() {
            let _ = fs::remove_dir_all(&target_fw);
        }
        copy_dir_all(&d3dm_fw, &target_fw)?;
    }

    let target_shared = ext_dir.join("libd3dshared.dylib");
    let is_same_shared = if let (Ok(src_canon), Ok(tgt_canon)) =
        (d3d_shared.canonicalize(), target_shared.canonicalize())
    {
        src_canon == tgt_canon
    } else {
        d3d_shared == target_shared
    };

    if !is_same_shared {
        if target_shared.exists() || target_shared.is_symlink() {
            let _ = fs::remove_file(&target_shared);
        }
        fs::copy(&d3d_shared, &target_shared)?;
    }

    // Copy Apple GPTK D3DMetal Wine DLLs and unix SOs if present in the source redist
    if let Some(gptk_lib_dir) = d3d_shared.parent().and_then(|p| p.parent()) {
        let redist_win = gptk_lib_dir.join("wine/x86_64-windows");
        if redist_win.is_dir() {
            let target_win = target.join("lib/wine/x86_64-windows");
            let _ = copy_dir_all(&redist_win, &target_win);
        }
        let redist_unix = gptk_lib_dir.join("wine/x86_64-unix");
        if redist_unix.is_dir() {
            let target_unix = target.join("lib/wine/x86_64-unix");
            let _ = copy_dir_all(&redist_unix, &target_unix);
        }
    }

    // Install overlay-shim.dylib
    let overlay_dst = paths::support_dir().join("overlay-shim.dylib");
    fs::write(&overlay_dst, overlay_shim::OVERLAY_SHIM_BYTES)?;
    steam::sign_binary(&overlay_dst)?;

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

/// Assembles and configures environment variables required for Apple Game Porting Toolkit (GPTK)
/// and D3DMetal runtime execution (Direct3D 11 & 12 -> Metal translation).
pub fn apply_gptk_execution_env(
    env: &mut HashMap<String, String>,
    runner_dir: &Path,
    steam_dir: &Path,
    client_path_str: &str,
    enable_hud: bool,
) {
    // Apple Silicon & GPTK 4 features
    env.insert("D3DM_MTL4".to_string(), "1".to_string());
    env.insert("D3DM_ENABLE_METALFX".to_string(), "1".to_string());
    env.insert("D3DM_SUPPORT_DXR".to_string(), "1".to_string());

    // DirectX 11 & 12 Metal DLL Overrides
    env.insert(
        "WINEDLLOVERRIDES".to_string(),
        "steamclient=n,b;steamclient64=n,b;lsteamclient=b;d3d11,dxgi,d3d12,d3d10core,d3dcompiler_47=n,b;nvapi64,nvngx=n,b".to_string(),
    );

    env.insert("WINEDEBUG".to_string(), "warn+all,err+all,+seh".to_string());

    if enable_hud {
        env.insert("MTL_HUD_ENABLED".to_string(), "1".to_string());
    }

    // Dynamic linker paths including external D3DMetal
    let lib_ext = runner_dir.join("lib/external");
    let lib_fw_res =
        runner_dir.join("lib/external/D3DMetal.framework/Versions/Current/Resources");
    let lib_dir = runner_dir.join("lib");
    let lib_unix = runner_dir.join("lib/wine/x86_64-unix");
    env.insert(
        "DYLD_FALLBACK_LIBRARY_PATH".to_string(),
        format!(
            "{}:{}:{}:{}:{}:{}",
            steam_dir.display(),
            client_path_str,
            lib_ext.display(),
            lib_fw_res.display(),
            lib_unix.display(),
            lib_dir.display()
        ),
    );
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
    let wine_loader = if runner_dir.join("bin/wine").is_file() {
        runner_dir.join("bin/wine")
    } else {
        runner_dir.join("bin/wine64")
    };
    env.insert(
        "WINELOADER".to_string(),
        wine_loader.to_string_lossy().to_string(),
    );
    env.insert(
        "WINESERVER".to_string(),
        runner_dir
            .join("bin/wineserver")
            .to_string_lossy()
            .to_string(),
    );
    env.insert("ROSETTA_ADVERTISE_AVX".to_string(), "1".to_string());
    if engine != TargetEngine::Gptk {
        env.insert("WINEMSYNC".to_string(), "1".to_string());
    }
    env.insert("WINEESYNC".to_string(), "1".to_string());

    // If runner is CrossOver, set CrossOver root and dynamic linker paths
    let runner_str = runner_dir.to_string_lossy().to_lowercase();
    if runner_str.contains("crossover") || runner_dir.join("share/crossover").is_dir() {
        env.insert(
            "CX_ROOT".to_string(),
            runner_dir.to_string_lossy().to_string(),
        );
        let cx_bin = runner_dir.join("bin");
        if cx_bin.is_dir() {
            let cur_path = env::var("PATH").unwrap_or_default();
            env.insert(
                "PATH".to_string(),
                format!("{}:{}", cx_bin.display(), cur_path),
            );
        }
        let mut dyld_paths = Vec::new();
        let cx_lib = runner_dir.join("lib");
        let cx_lib64 = runner_dir.join("lib64");
        if cx_lib.is_dir() {
            dyld_paths.push(cx_lib.to_string_lossy().to_string());
        }
        if cx_lib64.is_dir() {
            dyld_paths.push(cx_lib64.to_string_lossy().to_string());
        }
        if !dyld_paths.is_empty() {
            if let Ok(cur_dyld) = env::var("DYLD_FALLBACK_LIBRARY_PATH") {
                dyld_paths.push(cur_dyld);
            }
            env.insert(
                "DYLD_FALLBACK_LIBRARY_PATH".to_string(),
                dyld_paths.join(":"),
            );
        }
    }

    // Wine library and DLL search path including prefix Steam directory
    let steam_dir = prefix_dir.join("drive_c/Program Files (x86)/Steam");
    let mut dll_paths = vec![steam_dir.to_string_lossy().to_string()];
    for sub in &[
        "lib/wine/x86_64-windows",
        "lib/wine/x86_64-unix",
        "lib/wine/aarch64-unix",
        "lib64/wine/x86_64-windows",
        "lib64/wine/x86_64-unix",
        "lib/wine",
        "lib64/wine",
    ] {
        let p = runner_dir.join(sub);
        if p.is_dir() {
            dll_paths.push(p.to_string_lossy().to_string());
        }
    }
    if dll_paths.len() == 1 {
        let win_dlls = runner_dir.join("lib/wine/x86_64-windows");
        let unix_dlls = runner_dir.join("lib/wine/x86_64-unix");
        dll_paths.push(win_dlls.to_string_lossy().to_string());
        dll_paths.push(unix_dlls.to_string_lossy().to_string());
    }
    env.insert("WINEDLLPATH".to_string(), dll_paths.join(":"));

    // Native Steam client installation path for lsteamclient bridge
    let steam_bundle = paths::steam_data_dir().join("Steam.AppBundle/Steam/Contents/MacOS");
    let steam_client_dir = if steam_bundle.is_dir() {
        steam_bundle
    } else {
        paths::steam_app().join("Contents/MacOS")
    };
    let client_path_str = env::var("STEAM_COMPAT_CLIENT_INSTALL_PATH")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| steam_client_dir.to_string_lossy().to_string());
    env.insert(
        "STEAM_COMPAT_CLIENT_INSTALL_PATH".to_string(),
        client_path_str.clone(),
    );

    // Compose DYLD_INSERT_LIBRARIES: overlay-shim + Steam in-game overlay
    // Note: Do not include steamloader.dylib as it crashes Wine/Proton x86_64 runtimes with SIGFPE (136).
    let overlay_shim = paths::support_dir().join("overlay-shim.dylib");
    let needs_overlay_write = !overlay_shim.exists()
        || fs::metadata(&overlay_shim)
            .map(|m| m.len() != overlay_shim::OVERLAY_SHIM_BYTES.len() as u64)
            .unwrap_or(true);
    if needs_overlay_write && fs::write(&overlay_shim, overlay_shim::OVERLAY_SHIM_BYTES).is_ok() {
        let _ = steam::sign_binary(&overlay_shim);
    }
    let renderer = steam_client_dir.join("gameoverlayrenderer.dylib");
    let steam_dyld = env::var("STEAM_DYLD_INSERT_LIBRARIES")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.split(':')
                .filter(|part| !part.contains("steamloader.dylib"))
                .collect::<Vec<_>>()
                .join(":")
        })
        .filter(|s| !s.is_empty())
        .or_else(|| {
            if renderer.exists() {
                Some(renderer.display().to_string())
            } else {
                None
            }
        });

    let insert_val = match (overlay_shim.exists(), steam_dyld) {
        (true, Some(s_dyld)) => format!("{}:{}", overlay_shim.display(), s_dyld),
        (true, None) => overlay_shim.to_string_lossy().to_string(),
        (false, Some(s_dyld)) => s_dyld,
        (false, None) => String::new(),
    };
    if !insert_val.is_empty() {
        env.insert("DYLD_INSERT_LIBRARIES".to_string(), insert_val);
    }

    match engine {
        TargetEngine::Auto => {
            return build_execution_env_for_engine(
                runner_dir,
                prefix_dir,
                TargetEngine::WineStaging,
                enable_hud,
            );
        }
        TargetEngine::Gptk => {
            apply_gptk_execution_env(
                &mut env,
                runner_dir,
                &steam_dir,
                &client_path_str,
                enable_hud,
            );
        }
        TargetEngine::KosmicKrisp | TargetEngine::Dxvk | TargetEngine::Vkd3d => {
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
            // Route OpenGL through Mesa Zink (OpenGL implementation over Vulkan)
            // unless explicitly disabled by user environment
            if env::var("GALLIUM_DRIVER").is_err() {
                env.insert("GALLIUM_DRIVER".to_string(), "zink".to_string());
            }
            if env::var("MESA_GL_VERSION_OVERRIDE").is_err() {
                env.insert("MESA_GL_VERSION_OVERRIDE".to_string(), "4.6".to_string());
            }
            if env::var("MESA_GLSL_VERSION_OVERRIDE").is_err() {
                env.insert("MESA_GLSL_VERSION_OVERRIDE".to_string(), "460".to_string());
            }

            // Prepare MoltenVK -> KosmicKrisp shim for winemac.so if driver dylib exists
            let shim_dir =
                setup_kosmickrisp_shim().unwrap_or_else(|_| paths::kosmickrisp_shim_dir());

            // Wine DLL overrides: map Direct3D to DXVK/VKD3D/D7VK (n,b) and forward Vulkan to host KosmicKrisp
            let mut overrides =
                "steamclient=n,b;steamclient64=n,b;lsteamclient=b;winevulkan=b,n;vulkan-1=b,n;d3d12,d3d12core=n,b"
                    .to_string();
            if dxvk::find_dxvk().is_some() {
                overrides.push_str(";d3d11,dxgi,d3d10core,d3d9=n,b");
            }
            if d7vk::find_d7vk().is_some() {
                overrides.push_str(";ddraw=n,b");
            }
            env.insert("WINEDLLOVERRIDES".to_string(), overrides);

            // Configure DXVK logging if available
            if dxvk::find_dxvk().is_some() {
                env.insert("DXVK_LOG_LEVEL".to_string(), "info".to_string());
                env.insert(
                    "DXVK_LOG_PATH".to_string(),
                    paths::support_dir().to_string_lossy().to_string(),
                );
            }

            // Configure VKD3D-Proton features if available
            if vkd3d::find_vkd3d_proton().is_some() {
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
                    "{}:{}:{}:{}:{}",
                    steam_dir.display(),
                    client_path_str,
                    shim_dir.display(),
                    lib_unix.display(),
                    lib_dir.display()
                ),
            );
        }
        TargetEngine::Dxmt => {
            // DXMT (DirectX 11 -> Apple Metal)
            // Maps d3d11, dxgi, d3d10core, and winemetal to native DXMT DLLs bridging directly to Metal
            env.insert(
                "WINEDLLOVERRIDES".to_string(),
                "steamclient=n,b;steamclient64=n,b;lsteamclient=b;d3d11,dxgi,d3d10core,winemetal=n,b;nvapi64,nvngx=n,b".to_string(),
            );

            if enable_hud {
                env.insert("MTL_HUD_ENABLED".to_string(), "1".to_string());
            }

            let lib_dir = runner_dir.join("lib");
            let lib_unix = runner_dir.join("lib/wine/x86_64-unix");
            let mut dyld_paths = vec![
                steam_dir.to_string_lossy().to_string(),
                client_path_str.clone(),
                lib_unix.to_string_lossy().to_string(),
                lib_dir.to_string_lossy().to_string(),
            ];
            if let Some(dxmt) = crate::dxmt::find_dxmt() {
                let dxmt_unix = dxmt.root.join("x86_64-unix");
                if dxmt_unix.is_dir() {
                    dyld_paths.push(dxmt_unix.to_string_lossy().to_string());
                }
            }
            env.insert(
                "DYLD_FALLBACK_LIBRARY_PATH".to_string(),
                dyld_paths.join(":"),
            );
        }
        TargetEngine::WineStaging => {
            // Wine-Staging legacy overrides: map DX9, DX10 to built-in WineD3D / OpenGL
            env.insert(
                "WINEDLLOVERRIDES".to_string(),
                "steamclient=n,b;steamclient64=n,b;lsteamclient=b;d3d9,d3d10,d3d10_1,d3d10core=b,n"
                    .to_string(),
            );

            let lib_dir = runner_dir.join("lib");
            let lib_unix = runner_dir.join("lib/wine/x86_64-unix");
            env.insert(
                "DYLD_FALLBACK_LIBRARY_PATH".to_string(),
                format!(
                    "{}:{}:{}:{}",
                    steam_dir.display(),
                    client_path_str,
                    lib_unix.display(),
                    lib_dir.display()
                ),
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
        assert_eq!(env.get("GALLIUM_DRIVER").map(|s| s.as_str()), Some("zink"));
        assert_eq!(
            env.get("MESA_GL_VERSION_OVERRIDE").map(|s| s.as_str()),
            Some("4.6")
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
    fn test_build_execution_env_gptk_and_wine_regression() {
        let runner_dir = PathBuf::from("/tmp/test_runner");
        let prefix_dir = PathBuf::from("/tmp/test_prefix");

        // 1. GPTK mode (e.g. DiRT Rally 2.0 scenario)
        let env_gptk = build_execution_env_for_engine(
            &runner_dir,
            &prefix_dir,
            TargetEngine::Gptk,
            false,
        );
        let dyld_gptk = env_gptk.get("DYLD_FALLBACK_LIBRARY_PATH").unwrap();
        assert!(dyld_gptk.contains("lib/external"));
        assert!(dyld_gptk.contains("D3DMetal.framework/Versions/Current/Resources"));

        let overrides_gptk = env_gptk.get("WINEDLLOVERRIDES").unwrap();
        assert!(overrides_gptk.contains("d3d11,dxgi,d3d12,d3d10core,d3dcompiler_47=n,b"));

        // Verify steamloader.dylib is stripped from DYLD_INSERT_LIBRARIES to prevent SIGFPE (136)
        if let Some(insert) = env_gptk.get("DYLD_INSERT_LIBRARIES") {
            assert!(!insert.contains("steamloader.dylib"));
        }

        // Verify WINEMSYNC is disabled for GPTK to prevent SIGFPE in msync, while WINEESYNC remains active
        assert!(!env_gptk.contains_key("WINEMSYNC"));
        assert_eq!(env_gptk.get("WINEESYNC").map(|s| s.as_str()), Some("1"));

        // 2. Wine mode (e.g. GoNNER / standard Wine scenario)
        let env_wine = build_execution_env_for_engine(
            &runner_dir,
            &prefix_dir,
            TargetEngine::WineStaging,
            false,
        );
        let dyld_wine = env_wine.get("DYLD_FALLBACK_LIBRARY_PATH").unwrap();
        assert!(!dyld_wine.contains("D3DMetal.framework"));
        assert!(!dyld_wine.contains("lib/external"));
        assert_eq!(env_wine.get("WINEMSYNC").map(|s| s.as_str()), Some("1"));
        assert_eq!(env_wine.get("WINEESYNC").map(|s| s.as_str()), Some("1"));
    }

    #[test]
    fn test_find_kosmickrisp_icd_env() {
        let dir = tempdir().unwrap();
        let icd_file = dir.path().join("test_icd.json");
        fs::write(&icd_file, "{}").unwrap();

        env::set_var("VK_DRIVER_FILES", &icd_file);
        let found = find_kosmickrisp_icd();
        assert_eq!(found, Some(icd_file.clone()));
        env::remove_var("VK_DRIVER_FILES");
    }

    #[test]
    fn test_inspect_gptk_dir_flat() {
        let dir = tempdir().unwrap();
        let fw_dir = dir.path().join("D3DMetal.framework");
        fs::create_dir_all(&fw_dir).unwrap();
        let shared = dir.path().join("libd3dshared.dylib");
        fs::write(&shared, "fake").unwrap();

        let comps = inspect_gptk_dir(dir.path());
        assert!(comps.is_some());
        let (fw, sh) = comps.unwrap();
        assert_eq!(fw, fw_dir);
        assert_eq!(sh, shared);
    }

    #[test]
    fn test_inspect_gptk_dir_redist() {
        let dir = tempdir().unwrap();
        let fw_dir = dir.path().join("redist/lib/external/D3DMetal.framework");
        fs::create_dir_all(&fw_dir).unwrap();
        let shared = dir.path().join("redist/lib/external/libd3dshared.dylib");
        fs::write(&shared, "fake").unwrap();

        let comps = inspect_gptk_dir(dir.path());
        assert!(comps.is_some());
        let (fw, sh) = comps.unwrap();
        assert_eq!(fw, fw_dir);
        assert_eq!(sh, shared);
    }

    #[test]
    fn test_set_custom_gptk_path_rejects_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("dummy.dmg");
        fs::write(&file_path, "not a directory").unwrap();

        let result = set_custom_gptk_path(&file_path);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Specified GPTK path is not a directory"));
    }

    #[test]
    fn test_inspect_kosmickrisp_candidate_json() {
        let dir = tempdir().unwrap();
        let icd_path = dir.path().join("custom_icd.json");
        let dylib_path = dir.path().join("libvulkan_custom.dylib");
        fs::write(&dylib_path, "fake_dylib").unwrap();

        let content = serde_json::json!({
            "file_format_version": "1.0.0",
            "ICD": {
                "library_path": "libvulkan_custom.dylib",
                "api_version": "1.4.304"
            }
        });
        fs::write(&icd_path, serde_json::to_string(&content).unwrap()).unwrap();

        let info = inspect_kosmickrisp_candidate(&icd_path, None).unwrap();
        assert_eq!(info.api_version, "1.4.304");
        assert_eq!(info.icd_path, icd_path);
        assert_eq!(info.library_path, dylib_path.canonicalize().unwrap());
        assert!(info.is_custom);

        // Test with explicit version override
        let info_override = inspect_kosmickrisp_candidate(&icd_path, Some("1.4.1")).unwrap();
        assert_eq!(info_override.api_version, "1.4.1");
    }

    #[test]
    fn test_inspect_kosmickrisp_candidate_dylib() {
        let dir = tempdir().unwrap();
        let dylib_path = dir.path().join("libvulkan_kosmickrisp.dylib");
        fs::write(&dylib_path, "fake_dylib_bytes").unwrap();

        let info = inspect_kosmickrisp_candidate(&dylib_path, Some("1.4.2")).unwrap();
        assert_eq!(info.api_version, "1.4.2");
        assert_eq!(info.library_path, dylib_path);

        // Verify write_kosmickrisp_icd
        let target_icd = dir.path().join("test_icd.json");
        write_kosmickrisp_icd(&target_icd, &info.library_path, &info.api_version).unwrap();
        let icd_content = fs::read_to_string(&target_icd).unwrap();
        let val: serde_json::Value = serde_json::from_str(&icd_content).unwrap();
        assert_eq!(val["ICD"]["api_version"], "1.4.2");
        assert_eq!(val["ICD"]["library_path"], dylib_path.to_str().unwrap());
    }

    #[test]
    fn test_format_gptk_version() {
        assert_eq!(format_gptk_version("4.0b2"), "4.0 Beta 2");
        assert_eq!(format_gptk_version("4.0-beta2"), "4.0 Beta 2");
        assert_eq!(format_gptk_version("4.0b1"), "4.0 Beta 1");
        assert_eq!(format_gptk_version("2.0b1"), "2.0 Beta 1");
        assert_eq!(format_gptk_version("v4.0"), "4.0");
        assert_eq!(format_gptk_version("4.0"), "4.0");
        assert_eq!(format_gptk_version("2.0"), "2.0");
    }

    #[test]
    fn test_format_vulkan_version() {
        assert_eq!(format_vulkan_version("1.4.0"), "1.4");
        assert_eq!(format_vulkan_version("1.4.304"), "1.4");
        assert_eq!(format_vulkan_version("1.3.275"), "1.3");
        assert_eq!(format_vulkan_version("1.4"), "1.4");
    }

    #[test]
    fn test_detect_gptk_version_from_plist() {
        let dir = tempdir().unwrap();
        let fw = dir.path().join("D3DMetal.framework");
        let res = fw.join("Resources");
        fs::create_dir_all(&res).unwrap();
        let shared = dir.path().join("libd3dshared.dylib");
        fs::write(&shared, "fake").unwrap();

        let plist_content = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleShortVersionString</key>
    <string>4.0b2</string>
</dict>
</plist>"#;
        fs::write(res.join("version.plist"), plist_content).unwrap();

        let detected = detect_gptk_version(Some(dir.path()));
        assert_eq!(detected, Some("4.0b2".to_string()));
    }

    #[test]
    fn test_apply_gptk_execution_env() {
        let mut env = HashMap::new();
        let runner_dir = PathBuf::from("/tmp/test_runner");
        let steam_dir = PathBuf::from("/tmp/test_steam");
        let client_path_str = "/tmp/test_client";

        apply_gptk_execution_env(
            &mut env,
            &runner_dir,
            &steam_dir,
            client_path_str,
            true,
        );

        assert_eq!(env.get("D3DM_MTL4").map(|s| s.as_str()), Some("1"));
        assert_eq!(env.get("D3DM_ENABLE_METALFX").map(|s| s.as_str()), Some("1"));
        assert_eq!(env.get("D3DM_SUPPORT_DXR").map(|s| s.as_str()), Some("1"));
        assert_eq!(env.get("MTL_HUD_ENABLED").map(|s| s.as_str()), Some("1"));
        assert_eq!(env.get("WINEDEBUG").map(|s| s.as_str()), Some("warn+all,err+all,+seh"));
        let overrides = env.get("WINEDLLOVERRIDES").unwrap();
        assert!(overrides.contains("d3d11,dxgi,d3d12,d3d10core,d3dcompiler_47=n,b"));
        let dyld = env.get("DYLD_FALLBACK_LIBRARY_PATH").unwrap();
        assert!(dyld.contains("/tmp/test_steam"));
        assert!(dyld.contains("/tmp/test_client"));
        assert!(dyld.contains("/tmp/test_runner/lib/external"));
    }

    #[test]
    fn test_validate_and_resolve_gptk_components() {
        let dir = tempdir().unwrap();

        // 1. Nonexistent path
        let non_existent = dir.path().join("does_not_exist");
        let err1 = validate_and_resolve_gptk_components(&non_existent).unwrap_err();
        assert!(err1.to_string().contains("Specified GPTK path does not exist"));

        // 2. File instead of directory
        let file_path = dir.path().join("file.txt");
        fs::write(&file_path, "not a dir").unwrap();
        let err2 = validate_and_resolve_gptk_components(&file_path).unwrap_err();
        assert!(err2.to_string().contains("Specified GPTK path is not a directory"));

        // 3. Directory with missing components
        let empty_dir = dir.path().join("empty");
        fs::create_dir_all(&empty_dir).unwrap();
        let err3 = validate_and_resolve_gptk_components(&empty_dir).unwrap_err();
        assert!(err3.to_string().contains("Directory does not contain valid Apple GPTK components"));

        // 4. Directory with valid components (flat layout)
        let valid_dir = dir.path().join("valid");
        let fw = valid_dir.join("D3DMetal.framework");
        let shared = valid_dir.join("libd3dshared.dylib");
        fs::create_dir_all(&fw).unwrap();
        fs::write(&shared, "dylib").unwrap();
        let (found_fw, found_shared) = validate_and_resolve_gptk_components(&valid_dir).unwrap();
        assert_eq!(found_fw, fw);
        assert_eq!(found_shared, shared);
    }

    #[test]
    fn test_find_and_resolve_gptk_components_strict_validation() {
        let dir = tempdir().unwrap();
        let valid_gptk = dir.path().join("valid_gptk");
        fs::create_dir_all(valid_gptk.join("D3DMetal.framework")).unwrap();
        fs::write(valid_gptk.join("libd3dshared.dylib"), "dylib").unwrap();

        let invalid_gptk = dir.path().join("invalid_gptk");
        fs::create_dir_all(&invalid_gptk).unwrap();

        // 1. Explicit path (valid and invalid)
        assert!(find_gptk_components(Some(&valid_gptk)).unwrap().is_some());
        assert!(find_gptk_components(Some(&invalid_gptk)).is_err());

        // 2. Environment variable
        env::set_var("NUCLEON_GPTK_PATH", &valid_gptk);
        assert!(find_gptk_components(None).unwrap().is_some());
        env::set_var("NUCLEON_GPTK_PATH", &invalid_gptk);
        assert!(find_gptk_components(None).is_err());
        env::remove_var("NUCLEON_GPTK_PATH");

        // 3. Unconfigured returns Ok(None) for find, and Err for resolve
        let fake_support = dir.path().join("support");
        fs::create_dir_all(&fake_support).unwrap();
        env::set_var("NUCLEON_SUPPORT_DIR", &fake_support);

        assert!(find_gptk_components(None).unwrap().is_none());
        let resolve_err = resolve_gptk_components(None).unwrap_err();
        assert!(resolve_err.to_string().contains("Apple Game Porting Toolkit 4.0 path is not configured"));

        // 4. Configured flag file via set_custom_gptk_path
        set_custom_gptk_path(&valid_gptk).unwrap();
        assert!(find_gptk_components(None).unwrap().is_some());
        assert!(resolve_gptk_components(None).is_ok());

        clear_custom_gptk_path().unwrap();
        assert!(find_gptk_components(None).unwrap().is_none());

        env::remove_var("NUCLEON_SUPPORT_DIR");
    }

    #[test]
    fn test_resolve_runner_for_engine_gptk_fails_loudly_when_not_configured() {
        let dir = tempdir().unwrap();
        let fake_support = dir.path().join("support");
        fs::create_dir_all(&fake_support).unwrap();
        env::set_var("NUCLEON_SUPPORT_DIR", &fake_support);
        env::remove_var("NUCLEON_GPTK_PATH");
        env::remove_var("GPTK_PATH");
        env::remove_var("NUCLEON_GPTK_RUNNER");

        let res = resolve_runner_for_engine(TargetEngine::Gptk);
        assert!(res.is_err(), "resolve_runner_for_engine(TargetEngine::Gptk) must fail loudly when GPTK is not configured");
        let err_msg = res.unwrap_err().to_string();
        assert!(err_msg.contains("GPTK") || err_msg.contains("Apple Game Porting Toolkit"));

        env::remove_var("NUCLEON_SUPPORT_DIR");
    }
}
