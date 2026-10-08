use crate::detector::TargetEngine;
use crate::paths;
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
        if let Ok(p) = std::env::var(var) {
            let pb = PathBuf::from(p);
            if pb.join("bin/wine").is_file() && pb.join("lib/external/D3DMetal.framework").is_dir()
            {
                return Some(pb);
            }
        }
    }

    // 2. Check staged/assembled runners
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

    // 3. Check current runner symlink
    let cur = paths::current_runner();
    if cur.join("bin/wine").is_file() && cur.join("lib/external/D3DMetal.framework").is_dir() {
        return Some(cur);
    }

    None
}

/// Returns the path to the active or default Wine runtime.
pub fn find_wine_staging_runtime() -> Option<PathBuf> {
    crate::wine::get_active_wine_runtime().map(|r| r.root)
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
    if let Ok(path) = std::env::var("KOSMICKRISP_ICD_PATH") {
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
    if let Ok(sdk) = std::env::var("VULKAN_SDK") {
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
    if let Ok(path) = std::env::var("KOSMICKRISP_DRIVER_PATH") {
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
            if let Some(staging) = find_wine_staging_runtime() {
                log::warn!(
                    "Apple GPTK runner not found; falling back to Wine-Staging for DirectX 11/12"
                );
                return Ok((staging, TargetEngine::WineStaging));
            }
            let assembled = assemble_runner(false, None, None)?;
            Ok((assembled, TargetEngine::Gptk))
        }
        TargetEngine::KosmicKrisp => {
            // KosmicKrisp runs under Wine configured with Mesa Vulkan 1.4 ICD
            if let Some(staging) = find_wine_staging_runtime() {
                return Ok((staging, TargetEngine::KosmicKrisp));
            }
            if let Some(gptk) = find_gptk_runner() {
                return Ok((gptk, TargetEngine::KosmicKrisp));
            }
            let assembled = assemble_runner(false, None, None)?;
            Ok((assembled, TargetEngine::KosmicKrisp))
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

    None
}

pub fn custom_gptk_path_file() -> PathBuf {
    paths::support_dir().join("custom_gptk_path.txt")
}

/// Sets a custom Apple GPTK components path and validates it.
/// The user is expected to have exported the components to a directory.
pub fn set_custom_gptk_path(path: &Path) -> Result<(PathBuf, PathBuf)> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("Path does not exist: {}", path.display()))?;

    if !canonical.is_dir() {
        anyhow::bail!(
            "Specified GPTK path is not a directory: {}. Please export Apple GPTK components to a directory containing D3DMetal.framework and libd3dshared.dylib.",
            canonical.display()
        );
    }

    let components = inspect_gptk_dir(&canonical).context(
        "Specified directory does not contain valid Apple GPTK components (D3DMetal.framework and libd3dshared.dylib not found)",
    )?;

    paths::ensure_dirs()?;
    fs::write(
        custom_gptk_path_file(),
        canonical.to_string_lossy().as_bytes(),
    )
    .with_context(|| format!("Failed to write {}", custom_gptk_path_file().display()))?;

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

pub fn find_gptk_components(custom_path: Option<&Path>) -> Result<Option<(PathBuf, PathBuf)>> {
    // 1. Explicitly provided custom path
    if let Some(p) = custom_path {
        if let Some(comps) = inspect_gptk_dir(p) {
            return Ok(Some(comps));
        }
        anyhow::bail!(
            "Specified custom GPTK path does not contain valid components: {}. Please point to the directory containing D3DMetal.framework and libd3dshared.dylib.",
            p.display()
        );
    }

    // 2. Explicit environment variables
    for var in &["NUCLEON_GPTK_PATH", "GPTK_PATH"] {
        if let Ok(val) = std::env::var(var) {
            let p = PathBuf::from(val);
            if let Some(comps) = inspect_gptk_dir(&p) {
                return Ok(Some(comps));
            }
        }
    }

    // 3. Persisted custom path
    let custom_file = custom_gptk_path_file();
    if custom_file.is_file() {
        if let Ok(content) = fs::read_to_string(&custom_file) {
            let p = PathBuf::from(content.trim());
            if let Some(comps) = inspect_gptk_dir(&p) {
                return Ok(Some(comps));
            }
        }
    }

    // 4. Check existing runner
    let cur = paths::current_runner();
    if let Some(comps) = inspect_gptk_dir(&cur) {
        return Ok(Some(comps));
    }

    // 5. Check well-known installed locations
    let well_known = [
        PathBuf::from("/Applications/Game Porting Toolkit.app"),
        PathBuf::from("/opt/homebrew/opt/game-porting-toolkit"),
        PathBuf::from("/usr/local/opt/game-porting-toolkit"),
    ];
    for p in &well_known {
        if let Some(comps) = inspect_gptk_dir(p) {
            return Ok(Some(comps));
        }
    }

    Ok(None)
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
        if let Some(rt) = crate::wine::inspect_wine_dir(custom, Some("custom"), Some("Custom Wine"))
        {
            rt.root
        } else {
            anyhow::bail!(
                "Specified custom Wine path is not a valid Wine runtime (bin/wine not found): {}",
                custom.display()
            );
        }
    } else {
        crate::wine::get_active_wine_runtime()
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
    let (d3dm_fw, d3d_shared) = find_gptk_components(custom_gptk)?
        .context("Could not find Apple Game Porting Toolkit components (D3DMetal.framework). Please export the GPTK components to a directory and configure via 'nucleon gptk set-path <DIR>', --gptk-path, or NUCLEON_GPTK_PATH.")?;

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
    env.insert("WINEMSYNC".to_string(), "1".to_string());
    env.insert("WINEESYNC".to_string(), "1".to_string());

    // If runner is CrossOver, set CrossOver root and dynamic linker paths
    let runner_str = runner_dir.to_string_lossy().to_lowercase();
    if runner_str.contains("crossover") || runner_dir.join("share/crossover").is_dir() {
        env.insert("CX_ROOT".to_string(), runner_dir.to_string_lossy().to_string());
        let cx_bin = runner_dir.join("bin");
        if cx_bin.is_dir() {
            let cur_path = std::env::var("PATH").unwrap_or_default();
            env.insert("PATH".to_string(), format!("{}:{}", cx_bin.display(), cur_path));
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
            if let Ok(cur_dyld) = std::env::var("DYLD_FALLBACK_LIBRARY_PATH") {
                dyld_paths.push(cur_dyld);
            }
            env.insert("DYLD_FALLBACK_LIBRARY_PATH".to_string(), dyld_paths.join(":"));
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
        client_path_str,
    );

    // Compose DYLD_INSERT_LIBRARIES: overlay-shim + Steam client loader/overlay
    let overlay_shim = paths::support_dir().join("overlay-shim.dylib");
    let steam_dyld = env::var("STEAM_DYLD_INSERT_LIBRARIES")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let loader = steam_client_dir.join("steamloader.dylib");
            let renderer = steam_client_dir.join("gameoverlayrenderer.dylib");
            if loader.exists() && renderer.exists() {
                Some(format!("{}:{}", loader.display(), renderer.display()))
            } else if loader.exists() {
                Some(loader.display().to_string())
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
            // Apple Silicon & GPTK 4 features
            env.insert("D3DM_MTL4".to_string(), "1".to_string());
            env.insert("D3DM_ENABLE_METALFX".to_string(), "1".to_string());
            env.insert("D3DM_SUPPORT_DXR".to_string(), "1".to_string());

            // DirectX 11 & 12 Metal DLL Overrides
            env.insert(
                "WINEDLLOVERRIDES".to_string(),
                "steamclient=n,b;steamclient64=n,b;lsteamclient=b;d3d11,dxgi,d3d12,d3d10core,d3dcompiler_47=n,b;nvapi64,nvngx=n,b".to_string(),
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
                    "{}:{}:{}:{}",
                    steam_dir.display(),
                    lib_ext.display(),
                    lib_unix.display(),
                    lib_dir.display()
                ),
            );
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

            // Wine DLL overrides: map Direct3D to DXVK/VKD3D/D7VK (n,b) and forward Vulkan to host KosmicKrisp
            let mut overrides =
                "steamclient=n,b;steamclient64=n,b;lsteamclient=b;winevulkan=b,n;vulkan-1=b,n;d3d11,dxgi,d3d10core,d3d9,d3d12,d3d12core=n,b"
                    .to_string();
            if crate::d7vk::find_d7vk().is_some() {
                overrides.push_str(";ddraw=n,b");
            }
            env.insert("WINEDLLOVERRIDES".to_string(), overrides);

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
                    "{}:{}:{}:{}",
                    steam_dir.display(),
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
                "steamclient=n,b;steamclient64=n,b;lsteamclient=b;d3d9,d3d10,d3d10_1,d3d10core=b,n"
                    .to_string(),
            );

            let lib_dir = runner_dir.join("lib");
            let lib_unix = runner_dir.join("lib/wine/x86_64-unix");
            env.insert(
                "DYLD_FALLBACK_LIBRARY_PATH".to_string(),
                format!(
                    "{}:{}:{}",
                    steam_dir.display(),
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
}
