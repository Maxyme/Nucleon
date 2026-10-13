use crate::detector::TargetEngine;
use crate::fs_util::atomic_write_file;
use crate::gptk_abi;
use crate::paths;
use crate::wine;
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct RunnerInfo {
    pub name: String,
    pub path: PathBuf,
    pub wine_bin: PathBuf,
    pub wineserver_bin: PathBuf,
    pub has_gptk: bool,
}

fn runner_is_assembled(path: &Path) -> bool {
    path.join("bin/wine").is_file() && path.join("lib/external/D3DMetal.framework").is_dir()
}

/// Returns an immutable, canonical path for a complete assembled runner.
///
/// A launch must never keep resolving paths through `runners/current`: a rebuild can repoint that
/// symlink while Wine is being prepared. Resolving it once gives each launch one fixed generation.
fn complete_runner_path(path: &Path) -> Option<PathBuf> {
    if !runner_is_assembled(path) || RunnerSourceStamp::read(path).is_none() {
        return None;
    }
    path.canonicalize().ok()
}

/// Looks for an older visible completed generation only when `current` is absent.
///
/// In-progress generations are hidden temporary directories and deliberately excluded. A complete
/// generation has its source stamp written before publication, so both conditions are required.
fn find_completed_runner_in(runners_dir: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(runners_dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        if let Some(runner) = complete_runner_path(&path) {
            return Some(runner);
        }
    }
    None
}

pub fn find_gptk_runner() -> Option<PathBuf> {
    // 1. Check explicit environment variables
    for var in &["NUCLEON_GPTK_RUNNER", "NUCLEON_GPTK_PATH"] {
        if let Ok(p) = env::var(var) {
            let pb = PathBuf::from(p);
            if runner_is_assembled(&pb) {
                return pb.canonicalize().ok().or(Some(pb));
            }
        }
    }

    // 2. Check current runner symlink
    let cur = paths::current_runner();
    if runner_is_assembled(&cur) {
        return cur.canonicalize().ok();
    }

    // 3. Check staged/assembled runners in runners directory
    find_completed_runner_in(&paths::runners_dir())
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
                if p.is_file()
                    && p.extension().and_then(|s| s.to_str()) == Some("json")
                    && let Ok(content) = fs::read_to_string(&p)
                    && content.contains("\"ICD\"")
                {
                    return inspect_kosmickrisp_candidate(&p, api_version_override);
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
    // 1. Explicit environment variables, in priority order.
    let env_paths = [
        "VK_DRIVER_FILES",
        "VK_ICD_FILENAMES",
        "KOSMICKRISP_ICD_PATH",
    ]
    .iter()
    .filter_map(|var| env::var(var).ok().map(PathBuf::from))
    .collect::<Vec<_>>();

    find_kosmickrisp_icd_with(&env_paths)
}

/// Resolves the KosmicKrisp ICD manifest, preferring `env_paths` (the environment-provided
/// candidates, highest priority first). Split out so resolution can be tested without mutating
/// process state.
fn find_kosmickrisp_icd_with(env_paths: &[PathBuf]) -> Option<PathBuf> {
    for p in env_paths {
        if p.is_file() {
            return Some(p.clone());
        }
    }

    // 2. Check custom configured KosmicKrisp path (set via `nucleon kosmickrisp set-path` or `--kosmickrisp-path`)
    let custom_file = custom_kosmickrisp_path_file();
    if custom_file.is_file()
        && let Ok(content) = fs::read_to_string(&custom_file)
    {
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
        } else if p.is_dir()
            && let Ok(info) = inspect_kosmickrisp_candidate(&p, None)
        {
            return Some(info.icd_path);
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
    if custom_file.is_file()
        && let Ok(content) = fs::read_to_string(&custom_file)
    {
        let p = PathBuf::from(content.trim());
        if p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("dylib") {
            return Some(p);
        }
    }

    if let Some(icd) = find_kosmickrisp_icd() {
        if let Ok(content) = fs::read_to_string(&icd)
            && let Ok(val) = serde_json::from_str::<serde_json::Value>(&content)
            && let Some(lib_str) = val["ICD"]["library_path"].as_str()
        {
            let p = PathBuf::from(lib_str);
            if p.is_absolute() && p.is_file() {
                return Some(p);
            }
            if let Some(parent) = icd.parent() {
                let resolved = parent.join(lib_str);
                if let Ok(canon) = resolved.canonicalize()
                    && canon.is_file()
                {
                    return Some(canon);
                }
                if resolved.is_file() {
                    return Some(resolved);
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

static FORCE_KOSMICKRISP_DETECTION: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Override detection in this process without mutating the native process environment.
pub fn force_kosmickrisp_detection() {
    FORCE_KOSMICKRISP_DETECTION.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Returns true if the driver is detected or explicitly forced in this process/environment.
pub fn is_kosmickrisp_installed() -> bool {
    if env::var("KOSMICKRISP_DISABLE").is_ok() {
        return false;
    }
    FORCE_KOSMICKRISP_DETECTION.load(std::sync::atomic::Ordering::Relaxed)
        || env::var("KOSMICKRISP_FORCE").is_ok()
        || find_kosmickrisp_icd().is_some()
        || find_kosmickrisp_driver_dylib().is_some()
}

/// What the host already provides for runner resolution.
///
/// Discovery is captured once so [`plan_runner`] holds no environment or filesystem lookups.
/// That keeps the engine-selection rules unit-testable without mutating process-global state
/// (env vars) or the real support directory, which makes those tests safe to run in parallel.
#[derive(Debug, Clone, Default)]
struct RunnerHostState {
    /// Active or default Wine runtime, if one is installed/configured.
    pub wine_runtime: Option<PathBuf>,
    /// Runner that already ships GPTK (D3DMetal) components and can serve them directly.
    pub gptk_runner: Option<PathBuf>,
}

impl RunnerHostState {
    /// Discovers the runners available on this machine.
    pub fn discover() -> Self {
        Self {
            wine_runtime: find_wine_staging_runtime(),
            gptk_runner: find_gptk_runner(),
        }
    }
}

/// Where a runner should come from to satisfy a requested engine.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RunnerPlan {
    /// An already-installed runtime satisfies the request.
    Existing(PathBuf),
    /// Nothing usable was found; a GPTK-backed runner must be assembled first.
    AssembleRunner,
}

/// Engine reported back to callers once a runner is chosen. `Auto` always resolves to a Wine
/// pipeline, even when it borrows the wine binary from a GPTK runner.
fn effective_engine(engine: TargetEngine) -> TargetEngine {
    match engine {
        TargetEngine::Auto => TargetEngine::WineStaging,
        other => other,
    }
}

/// Selects the runner source for `engine` given an already-discovered [`RunnerHostState`].
///
/// This is the single place that encodes which runtime may serve which engine, and it never
/// substitutes a Wine-only runtime for a GPTK request: unsatisfiable requests return
/// [`RunnerPlan::AssembleRunner`] so the caller surfaces the configuration error instead of
/// silently launching the wrong engine.
fn plan_runner(state: &RunnerHostState, engine: TargetEngine) -> RunnerPlan {
    match engine {
        // A GPTK request must never be served by a Wine-only runtime.
        TargetEngine::Gptk => match state.gptk_runner.clone() {
            Some(gptk) => RunnerPlan::Existing(gptk),
            None => RunnerPlan::AssembleRunner,
        },
        // Auto and explicit Wine-Staging both want the Wine pipeline.
        TargetEngine::Auto | TargetEngine::WineStaging => {
            if let Some(staging) = state.wine_runtime.clone() {
                return RunnerPlan::Existing(staging);
            }
            if let Some(gptk) = state.gptk_runner.clone() {
                if engine == TargetEngine::Auto {
                    log::warn!("Wine runtime not found; utilizing available GPTK wine binary");
                } else {
                    log::info!(
                        "Wine runtime not found (install via 'brew install --cask wine-staging'); utilizing GPTK runtime for legacy/DX9 pipeline"
                    );
                }
                return RunnerPlan::Existing(gptk);
            }
            RunnerPlan::AssembleRunner
        }
        // KosmicKrisp, DXMT, DXVK, and VKD3D run under Wine.
        TargetEngine::KosmicKrisp
        | TargetEngine::Dxmt
        | TargetEngine::Dxvk
        | TargetEngine::Vkd3d => {
            if let Some(staging) = state.wine_runtime.clone() {
                return RunnerPlan::Existing(staging);
            }
            if let Some(gptk) = state.gptk_runner.clone() {
                return RunnerPlan::Existing(gptk);
            }
            RunnerPlan::AssembleRunner
        }
    }
}

/// Remediation guidance attached to a runner-assembly failure, or `None` when the underlying
/// error is already actionable on its own.
fn assembly_guidance(engine: TargetEngine) -> Option<String> {
    match engine {
        TargetEngine::Gptk => Some(
            "Failed to resolve Apple GPTK runner. Please ensure Apple GPTK 4 components are configured via 'nucleon gptk set-path <DIR>', --gptk-path, or NUCLEON_GPTK_PATH."
                .to_string(),
        ),
        _ => None,
    }
}

/// Attaches engine-specific remediation guidance to a runner-assembly failure.
fn annotate_assembly_failure<T>(result: Result<T>, engine: TargetEngine) -> Result<T> {
    match (result, assembly_guidance(engine)) {
        (Ok(value), _) => Ok(value),
        (Err(err), Some(guidance)) => Err(err.context(guidance)),
        (Err(err), None) => Err(err),
    }
}

/// Resolves the optimal runner for the requested TargetEngine.
pub fn resolve_runner_for_engine(engine: TargetEngine) -> Result<(PathBuf, TargetEngine)> {
    let resolved_engine = effective_engine(engine);
    let plan = plan_runner(&RunnerHostState::discover(), engine);

    let (runner, engine) = match plan {
        RunnerPlan::Existing(runner) => {
            let runner = if resolved_engine == TargetEngine::Gptk
                && runner.starts_with(canonical(&paths::runners_dir()))
                && !cached_runner_is_current(&runner, current_source_stamp(None, None).as_ref())
            {
                assemble_runner(false, None, None)?
            } else {
                runner
            };
            (runner, resolved_engine)
        }
        RunnerPlan::AssembleRunner => annotate_assembly_failure(
            assemble_runner(false, None, None).map(|runners| (runners, resolved_engine)),
            engine,
        )?,
    };

    // Refuse to run a D3DMetal title on a runtime that cannot support it. Wine resolves an absent
    // import by substituting a stub address and only logs a warning, so the game dies seconds later
    // with no indication that the chosen Wine build is the problem.
    require_engine_capabilities(&runner, engine)?;
    Ok((runner, engine))
}

/// Capabilities Apple's GPTK-4 D3DMetal binaries need from Wine, each with every name a Wine build
/// may use to provide it.
///
/// The import is spelled `gdi32.dll!D3DKMTEnumAdapters2`, but modern Wine implements it as
/// `win32u!NtGdiDdDDIEnumAdapters2` and ships no separate gdi32 image at all, so satisfying any alias
/// counts. Matching only the import's own name reports every recent Wine build as incapable.
pub const D3DMETAL_REQUIRED_EXPORTS: [(&str, &[&str]); 1] = [(
    "gdi32!D3DKMTEnumAdapters2",
    &["D3DKMTEnumAdapters2", "NtGdiDdDDIEnumAdapters2"],
)];

fn runtime_unix_lib_dirs(runner_dir: &Path) -> Vec<PathBuf> {
    [
        "lib/wine/x86_64-unix",
        "lib64/wine/x86_64-unix",
        "lib/wine/aarch64-unix",
    ]
    .into_iter()
    .map(|sub| runner_dir.join(sub))
    .filter(|dir| dir.is_dir())
    .collect()
}

/// Reports which D3DMetal-required exports `runner_dir`'s Wine cannot satisfy.
///
/// Export names live verbatim in a Mach-O symbol table, so scanning the runtime's own library images
/// answers the question without spawning anything. A missing image is treated as satisfied: only a
/// name that is absent from every present candidate counts as unsupported.
pub fn missing_d3dmetal_exports(runner_dir: &Path) -> Vec<&'static str> {
    let candidates = ["gdi32.so", "win32u.so"];
    let mut images: Vec<Vec<u8>> = Vec::new();
    for dir in runtime_unix_lib_dirs(runner_dir) {
        for name in candidates {
            let path = dir.join(name);
            if let Ok(bytes) = fs::read(&path) {
                images.push(bytes);
            }
        }
    }
    if images.is_empty() {
        return Vec::new();
    }

    D3DMETAL_REQUIRED_EXPORTS
        .into_iter()
        .filter(|(_, aliases)| {
            !aliases.iter().any(|alias| {
                let needle = alias.as_bytes();
                images
                    .iter()
                    .any(|bytes| bytes.windows(needle.len()).any(|w| w == needle))
            })
        })
        .map(|(name, _)| name)
        .collect()
}

/// Returns an actionable error when `runner_dir` cannot host `engine`'s graphics stack.
pub fn require_engine_capabilities(runner_dir: &Path, engine: TargetEngine) -> Result<()> {
    if engine != TargetEngine::Gptk {
        return Ok(());
    }
    let missing = missing_d3dmetal_exports(runner_dir);
    if missing.is_empty() {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "{} cannot run Apple GPTK 4 (D3DMetal): this Wine does not implement {}. \
         Choose a D3DMetal-capable Wine build (`nucleon wine set <name>`) or map the game to a \
         non-D3DMetal backend such as DXMT or KosmicKrisp.",
        runner_dir.display(),
        missing.join(", ")
    ))
}

/// Human-readable capability summary for `nucleon status` and setup checks.
pub fn describe_d3dmetal_support(runner_dir: &Path) -> Result<(bool, String)> {
    let missing = missing_d3dmetal_exports(runner_dir);
    Ok(if missing.is_empty() {
        (true, "D3DMetal-capable Wine".to_string())
    } else {
        (false, format!("Wine missing {}", missing.join(", ")))
    })
}

/// Inspects a directory or bundle to locate Apple GPTK components (`D3DMetal.framework` and `libd3dshared.dylib`).
pub fn inspect_gptk_dir(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    if !dir.exists() {
        return None;
    }

    if dir.is_dir()
        && dir.file_name().and_then(|n| n.to_str()) == Some("D3DMetal.framework")
        && let Some(parent) = dir.parent()
    {
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
    set_custom_gptk_path_in(&custom_gptk_path_file(), path)
}

/// Validates `path` and records it as the user's GPTK folder in `custom_file`.
fn set_custom_gptk_path_in(custom_file: &Path, path: &Path) -> Result<(PathBuf, PathBuf)> {
    let components = validate_and_resolve_gptk_components(path)?;
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    if let Some(parent) = custom_file.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    fs::write(custom_file, canonical.to_string_lossy().as_bytes())
        .with_context(|| format!("Failed to write {}", custom_file.display()))?;

    log::info!("Registered custom GPTK path at {}", canonical.display());
    Ok(components)
}

/// Clears any configured custom Apple GPTK path.
pub fn clear_custom_gptk_path() -> Result<()> {
    clear_custom_gptk_path_in(&custom_gptk_path_file())
}

/// Removes the persisted GPTK folder recorded in `custom_file`.
fn clear_custom_gptk_path_in(custom_file: &Path) -> Result<()> {
    if custom_file.is_file() {
        let _ = fs::remove_file(custom_file);
    }
    Ok(())
}

/// A configured GPTK location together with how it was configured.
#[derive(Debug, Clone)]
struct GptkCandidate {
    path: PathBuf,
    /// Human-readable origin surfaced by status output (e.g. `from custom path file`).
    source_label: String,
    /// Context attached when the directory is configured but unusable.
    invalid_context: Option<String>,
}

/// Every location GPTK discovery consults, captured as data so resolution is a pure function of
/// its inputs. Tests construct one directly instead of mutating process environment variables or
/// the shared support directory, which keeps them deterministic and parallel-safe.
#[derive(Debug, Clone)]
struct GptkSources {
    /// Environment-configured directories in priority order, with the variable behind each.
    env_paths: Vec<(String, PathBuf)>,
    /// Persisted `nucleon gptk set-path` configuration file.
    custom_path_file: PathBuf,
    /// Assembled runner that may already bundle D3DMetal components.
    assembled_runner: PathBuf,
}

impl GptkSources {
    /// Reads the ambient environment and support directory.
    fn discover() -> Self {
        let env_paths = ["NUCLEON_GPTK_PATH", "GPTK_PATH"]
            .iter()
            .filter_map(|var| {
                let value = env::var(var).ok()?;
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    return None;
                }
                Some(((*var).to_string(), PathBuf::from(trimmed)))
            })
            .collect();

        Self {
            env_paths,
            custom_path_file: custom_gptk_path_file(),
            assembled_runner: paths::current_runner(),
        }
    }
}

/// What to do with a configured-but-invalid GPTK location.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnInvalidPath {
    /// Surface it. Used by launch and validation flows, where silently ignoring a folder the
    /// user explicitly configured would hide the problem.
    Fail,
    /// Keep walking lower-priority sources. Used by read-only status output.
    Ignore,
}

/// GPTK components plus a description of where they came from.
#[derive(Debug, Clone)]
struct DiscoveredGptk {
    framework: PathBuf,
    shared_library: PathBuf,
    source_label: String,
}

/// The single Apple GPTK discovery ladder, in priority order:
/// 1. explicitly provided path (`--gptk-path`)
/// 2. environment-configured paths (`NUCLEON_GPTK_PATH`, then `GPTK_PATH`)
/// 3. persisted user configuration (`custom_gptk_path.txt`)
/// 4. components already staged in the assembled runner
///
/// Automatic filesystem searching is deliberately absent: Nucleon only ever uses a folder the
/// user pointed at. Sources are consulted lazily and validated as they are reached, so an early
/// valid source never reads later configuration.
fn discover_gptk_components(
    sources: &GptkSources,
    custom_path: Option<&Path>,
    on_invalid: OnInvalidPath,
) -> Result<Option<DiscoveredGptk>> {
    let accept =
        |candidate: GptkCandidate| match validate_and_resolve_gptk_components(&candidate.path) {
            Ok((framework, shared_library)) => Ok(Some(DiscoveredGptk {
                framework,
                shared_library,
                source_label: candidate.source_label,
            })),
            Err(err) => match (on_invalid, candidate.invalid_context) {
                (OnInvalidPath::Fail, Some(context)) => Err(err.context(context)),
                (OnInvalidPath::Fail, None) => Err(err),
                (OnInvalidPath::Ignore, _) => Ok(None),
            },
        };

    // 1. Explicitly provided custom path.
    if let Some(p) = custom_path {
        return accept(GptkCandidate {
            path: p.to_path_buf(),
            source_label: "from explicit path argument".to_string(),
            invalid_context: None,
        });
    }

    // 2. Explicit environment variables.
    for (var, path) in &sources.env_paths {
        if let Some(found) = accept(GptkCandidate {
            path: path.clone(),
            source_label: format!("from environment variable {var}"),
            invalid_context: Some(format!(
                "Environment variable {var} points to an invalid GPTK path"
            )),
        })? {
            return Ok(Some(found));
        }
    }

    // 3. Persisted custom path (configured manually by user via `nucleon gptk set-path`).
    if sources.custom_path_file.is_file() {
        match fs::read_to_string(&sources.custom_path_file) {
            Ok(content) => {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    let display_path = sources.custom_path_file.display().to_string();
                    if let Some(found) = accept(GptkCandidate {
                        path: PathBuf::from(trimmed),
                        source_label: "from custom path file".to_string(),
                        invalid_context: Some(format!(
                            "Configured custom GPTK path in {display_path} is invalid"
                        )),
                    })? {
                        return Ok(Some(found));
                    }
                }
            }
            Err(err) => match on_invalid {
                OnInvalidPath::Fail => {
                    return Err(err).with_context(|| {
                        format!(
                            "Failed to read custom GPTK setting from {}",
                            sources.custom_path_file.display()
                        )
                    });
                }
                OnInvalidPath::Ignore => {}
            },
        }
    }

    // 4. Components staged in the assembled runner.
    let fw = sources
        .assembled_runner
        .join("lib/external/D3DMetal.framework");
    let shared = sources
        .assembled_runner
        .join("lib/external/libd3dshared.dylib");
    if fw.is_dir() && shared.is_file() {
        return Ok(Some(DiscoveredGptk {
            framework: fw,
            shared_library: shared,
            source_label: format!(
                "from assembled runner ({})",
                sources.assembled_runner.display()
            ),
        }));
    }

    Ok(None)
}

/// Resolves Apple GPTK components from user configuration.
///
/// A configured path that is invalid returns an error; nothing configured returns `Ok(None)`.
/// See [`discover_gptk_components`] for the priority order.
pub fn find_gptk_components(custom_path: Option<&Path>) -> Result<Option<(PathBuf, PathBuf)>> {
    Ok(
        discover_gptk_components(&GptkSources::discover(), custom_path, OnInvalidPath::Fail)?
            .map(|found| (found.framework, found.shared_library)),
    )
}

/// Resolves GPTK components along with the human-readable resolution source.
///
/// Invalid configured paths are skipped rather than reported, because this feeds status output;
/// use [`find_gptk_components`] when the configuration itself must be validated.
pub fn get_gptk_resolution_info(
    custom_path: Option<&Path>,
) -> Option<((PathBuf, PathBuf), String)> {
    let found =
        discover_gptk_components(&GptkSources::discover(), custom_path, OnInvalidPath::Ignore)
            .ok()??;

    Some(((found.framework, found.shared_library), found.source_label))
}

/// Resolves user-provided GPTK components, failing loudly if not configured or invalid.
pub fn resolve_gptk_components(custom_path: Option<&Path>) -> Result<(PathBuf, PathBuf)> {
    require_configured_gptk(find_gptk_components(custom_path)?)
}

/// Attaches the "not configured" guidance when discovery found nothing to use.
fn require_configured_gptk(components: Option<(PathBuf, PathBuf)>) -> Result<(PathBuf, PathBuf)> {
    components.context(
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
            if c.is_file()
                && let Ok(plist::Value::Dictionary(dict)) = plist::Value::from_file(c)
                && let Some(plist::Value::String(ver)) = dict.get("CFBundleShortVersionString")
            {
                let trimmed = ver.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }

    // 2. Fallback to runner directory name or custom path naming
    let paths_to_check = [custom_path.map(Path::to_path_buf), find_gptk_runner()];

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

/// Records which inputs an assembled runner was built from, so a later launch notices when they moved.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RunnerSourceStamp {
    /// [`gptk_abi::ABI_REVISION`] the staged GPTK binaries were produced with.
    pub abi_revision: u32,
    /// Resolved root of the Wine runtime that was copied in.
    pub wine_root: String,
    /// Identity (size and mtime) of that runtime's `wine` loader, catching in-place upgrades.
    pub wine_loader: String,
    /// Metadata fingerprint of the Wine tree copied by rsync, excluding `lib/external`.
    pub wine_runtime: String,
    /// Identities of the exact runtime images used to determine the Unix-call ABI.
    pub wine_abi_inputs: String,
    /// Directory the D3DMetal components were taken from.
    pub gptk_dir: String,
    /// Identity of the staged `libd3dshared.dylib`'s source file.
    pub gptk_shared: String,
    pub gptk_framework: String,
    pub gptk_windows: String,
    pub gptk_unix: String,
}

impl RunnerSourceStamp {
    fn stamp_file(runner_dir: &Path) -> PathBuf {
        runner_dir.join(".nucleon-source.json")
    }

    fn read(runner_dir: &Path) -> Option<Self> {
        let raw = fs::read_to_string(Self::stamp_file(runner_dir)).ok()?;
        serde_json::from_str(&raw).ok()
    }

    fn write(&self, runner_dir: &Path) -> Result<()> {
        let raw = serde_json::to_string_pretty(self)
            .with_context(|| "Failed to serialise the GPTK runner source record")?;
        atomic_write_file(Self::stamp_file(runner_dir), raw)
            .with_context(|| format!("Failed to write {}", Self::stamp_file(runner_dir).display()))
    }

    /// Builds the record for `wine_root` plus the GPTK components that are about to be staged.
    fn from_sources(wine_root: &Path, framework: &Path, d3d_shared: &Path) -> Result<Self> {
        let loader = ["bin/wine", "bin/wine64"]
            .into_iter()
            .map(|sub| wine_root.join(sub))
            .find(|path| path.is_file())
            .unwrap_or_else(|| wine_root.join("bin/wine"));
        let wine_abi_inputs = gptk_abi::WINE_UNIX_CALL_ABI_CANDIDATES
            .iter()
            .map(|relative| format!("{relative}:{}", file_identity(&wine_root.join(relative))))
            .collect::<Vec<_>>()
            .join("\n");
        let optional_tree = |tree: Option<PathBuf>| -> Result<String> {
            match tree {
                Some(tree) => source_tree_identity(&tree, None),
                None => Ok("absent".to_string()),
            }
        };
        Ok(Self {
            abi_revision: gptk_abi::ABI_REVISION,
            wine_root: canonical(wine_root).display().to_string(),
            wine_loader: file_identity(&loader),
            wine_runtime: source_tree_identity(wine_root, Some(Path::new("lib/external")))?,
            wine_abi_inputs,
            gptk_dir: d3d_shared
                .parent()
                .map(canonical)
                .unwrap_or_default()
                .display()
                .to_string(),
            gptk_shared: file_identity(d3d_shared),
            gptk_framework: source_tree_identity(framework, None)?,
            gptk_windows: optional_tree(gptk_windows_binaries_dir(framework, d3d_shared))?,
            gptk_unix: optional_tree(gptk_unix_binaries_dir(framework, d3d_shared))?,
        })
    }
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// A cheap metadata identity for a file, using the filesystem's full timestamp precision.
fn file_identity(path: &Path) -> String {
    match fs::metadata(path) {
        Ok(meta) => {
            let modified = meta
                .modified()
                .ok()
                .and_then(|when| when.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|age| age.as_nanos().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            format!("{}:{modified}", meta.len())
        }
        Err(_) => "missing".to_string(),
    }
}

/// Deterministic metadata fingerprint of the inputs we copy. Entries are sorted, and symlinks are
/// recorded without following them, so framework `Versions/Current` cannot introduce a loop.
/// This avoids reading hundreds of megabytes on every cache lookup while noticing replaced files,
/// changed symlink targets, and additions/removals. Read errors make the sources unavailable.
fn source_tree_identity(root: &Path, excluded: Option<&Path>) -> Result<String> {
    fn collect(
        root: &Path,
        path: &Path,
        excluded: Option<&Path>,
        rows: &mut Vec<String>,
    ) -> Result<()> {
        let relative = path.strip_prefix(root)?;
        if excluded.is_some_and(|excluded| relative == excluded) {
            return Ok(());
        }
        let metadata = fs::symlink_metadata(path)
            .with_context(|| format!("Failed to inspect source {}", path.display()))?;
        if metadata.is_symlink() {
            rows.push(format!(
                "l:{}:{}",
                relative.display(),
                fs::read_link(path)?.display()
            ));
        } else if metadata.is_dir() {
            rows.push(format!("d:{}", relative.display()));
            for entry in fs::read_dir(path)? {
                collect(root, &entry?.path(), excluded, rows)?;
            }
        } else {
            rows.push(format!("f:{}:{}", relative.display(), file_identity(path)));
        }
        Ok(())
    }
    let root = root.canonicalize()?;
    let mut rows = Vec::new();
    collect(&root, &root, excluded, &mut rows)?;
    rows.sort();
    let mut hash = 0xcbf29ce484222325u64;
    for row in rows {
        for byte in row.bytes().chain(std::iter::once(0)) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    Ok(format!("metadata-v1:{hash:016x}"))
}

/// The stamp the current configuration would produce, or `None` when it cannot be determined.
///
/// Deliberately infallible: a cached runner must stay usable when a source is temporarily unreadable
/// (a GPTK folder on an unmounted volume, for example) rather than fail launches that already work.
fn current_source_stamp(
    custom_wine: Option<&Path>,
    custom_gptk: Option<&Path>,
) -> Option<RunnerSourceStamp> {
    let wine_root = match custom_wine {
        Some(path) => wine::inspect_wine_dir(path, Some("custom"), Some("Custom Wine"))?.root,
        None => wine::get_active_wine_runtime()?.root,
    };
    let found =
        discover_gptk_components(&GptkSources::discover(), custom_gptk, OnInvalidPath::Fail)
            .ok()??;
    // A cached runner is an output, not evidence that its original GPTK source is still available.
    if found.source_label.starts_with("from assembled runner (") {
        return None;
    }
    RunnerSourceStamp::from_sources(&wine_root, &found.framework, &found.shared_library).ok()
}

/// Reports what the assembled runner at `runner_dir` was built from, when that is known.
pub fn runner_source_stamp(runner_dir: &Path) -> Option<RunnerSourceStamp> {
    RunnerSourceStamp::read(runner_dir)
}

/// Decides whether an already-assembled runner may be reused as-is.
///
/// Three outcomes, in order of preference:
///
/// * the recorded inputs match today's configuration → reuse;
/// * they differ (a different Wine is now active, GPTK was updated, or the binaries were staged by
///   an older ABI-bridging rule) → re-assemble;
/// * today's inputs cannot be inspected at all → reuse. Rebuilding blindly would trade a runner that
///   demonstrably works for an error message about a folder that is merely offline.
fn cached_runner_is_current(target: &Path, current: Option<&RunnerSourceStamp>) -> bool {
    let Some(expected) = current else {
        log::debug!(
            "Cannot inspect the current Wine/GPTK sources; keeping the assembled runner at {}",
            target.display()
        );
        return true;
    };
    match RunnerSourceStamp::read(target) {
        Some(recorded) if &recorded == expected => true,
        Some(recorded) => {
            log::info!(
                "Assembled GPTK runner was built from Wine {} with GPTK at {}, but a different \
                 source is now active; re-assembling",
                recorded.wine_root,
                recorded.gptk_dir
            );
            false
        }
        None => {
            log::info!(
                "Assembled GPTK runner at {} has no source record; re-assembling",
                target.display()
            );
            false
        }
    }
}

fn link_current_runner(target: &Path) -> Result<()> {
    link_current_runner_at(target, &paths::current_runner())
}

fn link_current_runner_at(target: &Path, current: &Path) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(current)
        && !metadata.is_symlink()
    {
        bail!(
            "Cannot replace {}: expected a runner symlink. Move this existing file or directory aside before assembling a runner.",
            current.display()
        );
    }
    let parent = current
        .parent()
        .context("Current runner has no parent directory")?;
    let temporary = tempfile::Builder::new()
        .prefix(".runner-link-")
        .tempdir_in(parent)?;
    let link = temporary.path().join("current");
    #[cfg(unix)]
    std::os::unix::fs::symlink(target.canonicalize()?, &link)?;
    fs::rename(&link, current)
        .with_context(|| format!("Failed to publish runner symlink {}", current.display()))?;
    Ok(())
}

/// Publishes a completed immutable generation. Only the symlink is replaced; previous generations
/// remain usable by games already running. Failed staging or publication leaves `current` intact.
fn publish_runner(staging: tempfile::TempDir, current: &Path) -> Result<PathBuf> {
    if !runner_is_assembled(staging.path()) || RunnerSourceStamp::read(staging.path()).is_none() {
        bail!("Refusing to publish an incomplete assembled runner");
    }
    let name = staging
        .path()
        .file_name()
        .context("Staged runner has no name")?
        .to_string_lossy()
        .trim_start_matches('.')
        .to_string();
    let generation = staging.path().with_file_name(name);
    fs::rename(staging.path(), &generation).with_context(|| {
        format!(
            "Failed to publish runner generation {}",
            generation.display()
        )
    })?;
    if let Err(err) = link_current_runner_at(&generation, current) {
        // The generation was never selected, so it can be discarded without touching the old runner.
        if let Err(cleanup) = fs::remove_dir_all(&generation) {
            log::warn!(
                "Unused runner generation remains at {}: {cleanup}",
                generation.display()
            );
        }
        return Err(err);
    }
    Ok(generation)
}

/// Copies a Wine runtime into `dst`, leaving `dst/lib/external` (Nucleon's own staging area) alone.
fn sync_wine_tree(src: &Path, dst: &Path) -> Result<()> {
    fn copy_entry(src: &Path, dst: &Path, relative: &Path) -> Result<()> {
        if relative == Path::new("lib/external") {
            return Ok(());
        }
        let metadata = fs::symlink_metadata(src)?;
        if metadata.is_symlink() {
            if dst.exists() || dst.is_symlink() {
                fs::remove_file(dst)?;
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(fs::read_link(src)?, dst)?;
        } else if metadata.is_dir() {
            fs::create_dir_all(dst)?;
            for entry in fs::read_dir(src)? {
                let entry = entry?;
                copy_entry(
                    &entry.path(),
                    &dst.join(entry.file_name()),
                    &relative.join(entry.file_name()),
                )?;
            }
            fs::set_permissions(dst, metadata.permissions())?;
        } else if metadata.is_file() {
            if dst.is_symlink() {
                fs::remove_file(dst)?;
            }
            fs::copy(src, dst)?;
        } else {
            bail!("Unsupported Wine source entry: {}", src.display());
        }
        Ok(())
    }
    copy_entry(src, dst, Path::new("")).with_context(|| {
        format!(
            "Failed to copy Wine runtime from {} to {}",
            src.display(),
            dst.display()
        )
    })
}

pub fn assemble_runner(
    force: bool,
    custom_wine: Option<&Path>,
    custom_gptk: Option<&Path>,
) -> Result<PathBuf> {
    paths::ensure_dirs()?;
    let target = paths::current_runner()
        .canonicalize()
        .unwrap_or_else(|_| paths::runners_dir().join("gptk-4-beta2"));

    let has_explicit = custom_wine.is_some() || custom_gptk.is_some();
    let assembled = runner_is_assembled(&target);
    if !force
        && !has_explicit
        && assembled
        && cached_runner_is_current(
            &target,
            current_source_stamp(custom_wine, custom_gptk).as_ref(),
        )
    {
        link_current_runner(&target)?;
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

    let (d3dm_fw, d3d_shared) = resolve_gptk_components(custom_gptk)?;
    let source_stamp = RunnerSourceStamp::from_sources(&wine_src, &d3dm_fw, &d3d_shared)?;
    // Which Unix-call ABI the host implements decides whether GPTK's binaries have to be retargeted,
    // so it is read from the runtime itself, before anything of it is staged.
    let host_abi = gptk_abi::detect_wine_unix_call_abi(&wine_src);

    let staging = tempfile::Builder::new()
        .prefix(".gptk-4-beta2-")
        .tempdir_in(paths::runners_dir())?;
    assemble_runner_tree(staging.path(), &wine_src, &d3dm_fw, &d3d_shared, host_abi)?;
    // Reject a source updated while rsync or GPTK staging was in progress.
    if RunnerSourceStamp::from_sources(&wine_src, &d3dm_fw, &d3d_shared)? != source_stamp {
        bail!("Wine or GPTK sources changed during assembly; retry after the update finishes");
    }
    source_stamp.write(staging.path())?;
    publish_runner(staging, &paths::current_runner())
}

fn assemble_runner_tree(
    target: &Path,
    wine_src: &Path,
    d3dm_fw: &Path,
    d3d_shared: &Path,
    host_abi: Option<gptk_abi::WineUnixCallAbi>,
) -> Result<()> {
    // Starting with an empty tree prevents orphaned DLLs from a previous Wine build surviving.
    sync_wine_tree(wine_src, target)?;

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
        copy_dir_all(d3dm_fw, &target_fw)?;
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
        fs::copy(d3d_shared, &target_shared)?;
    }

    // Stage Apple GPTK's D3DMetal binaries from the source redist tree. Whether the Windows images
    // need retargeting depends on which Unix-call ABI this runner's Wine implements, so that is what
    // decides it — not a fixed assumption about the GPTK build or the Wine build.
    match gptk_windows_binaries_dir(d3dm_fw, d3d_shared) {
        Some(redist_win) => {
            let target_win = target.join("lib/wine/x86_64-windows");
            let staged = gptk_abi::stage_windows_tree(&redist_win, &target_win, host_abi)
                .with_context(|| {
                    format!(
                        "Failed to stage Apple GPTK binaries from {} onto the assembled runner",
                        redist_win.display()
                    )
                })?;
            match host_abi {
                Some(abi) if staged.bridged_count() > 0 => {
                    log::info!(
                        "Retargeted {} GPTK Windows binary(ies) onto this Wine's Unix-call ABI ({abi})",
                        staged.bridged_count()
                    );
                }
                Some(abi) => {
                    log::info!(
                        "Staged Apple GPTK binaries unmodified: they already bind to this Wine's \
                         Unix-call ABI ({abi})"
                    );
                }
                None => {
                    log::warn!(
                        "Could not determine the Unix-call ABI implemented by {}; staged Apple GPTK \
                         binaries unmodified. If D3DMetal fails with STATUS_DLL_INIT_FAILED, this \
                         Wine and this GPTK build need an explicit pairing.",
                        wine_src.display()
                    );
                }
            }
        }
        None => {
            log::warn!(
                "Apple GPTK components at {} contain no x86_64-windows binaries, so D3DMetal will \
                 rely on the Wine build's own Direct3D DLLs",
                d3dm_fw.display()
            );
        }
    }

    if let Some(redist_unix) = gptk_unix_binaries_dir(d3dm_fw, d3d_shared) {
        let target_unix = target.join("lib/wine/x86_64-unix");
        copy_dir_all(&redist_unix, &target_unix)
            .with_context(|| format!("Failed to stage {}", redist_unix.display()))?;
    }

    Ok(())
}

/// Where a GPTK distribution keeps its Windows binaries, for either layout of `libd3dshared.dylib`.
///
/// Apple has shipped the components as `redist/lib/external/libd3dshared.dylib` and directly in
/// `redist/lib/`, so the binary tree is found by walking up from wherever the components landed rather
/// than by assuming a fixed depth: an assumption that fails silently, staging a runner whose D3DMetal
/// DLLs never arrive and which then renders through Wine's own Direct3D without anyone noticing.
pub fn gptk_windows_binaries_dir(framework: &Path, shared: &Path) -> Option<PathBuf> {
    gptk_binaries_dir(framework, shared, "x86_64-windows")
}

fn gptk_unix_binaries_dir(framework: &Path, shared: &Path) -> Option<PathBuf> {
    gptk_binaries_dir(framework, shared, "x86_64-unix")
}

fn gptk_binaries_dir(framework: &Path, shared: &Path, arch: &str) -> Option<PathBuf> {
    for anchor in [shared.parent(), framework.parent()].into_iter().flatten() {
        // The `wine` tree sits a few levels above the components, at whichever of those levels it is.
        for ancestor in anchor.ancestors().take(4) {
            let candidate = ancestor.join("wine").join(arch);
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }
    None
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
    let lib_fw_res = runner_dir.join("lib/external/D3DMetal.framework/Versions/Current/Resources");
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

/// Composes a Wine bootstrap environment without game overlay injection or filesystem writes.
pub fn build_execution_env(
    runner_dir: &Path,
    prefix_dir: &Path,
    enable_hud: bool,
) -> HashMap<String, String> {
    build_execution_env_for_engine(
        runner_dir,
        prefix_dir,
        TargetEngine::Gptk,
        enable_hud,
        false,
    )
}

/// Environment variables whose contents point at one specific Wine/GPTK runtime and must therefore
/// never carry entries from another one.
pub const RUNTIME_SEARCH_PATH_VARS: [&str; 3] =
    ["PATH", "DYLD_FALLBACK_LIBRARY_PATH", "WINEDLLPATH"];

fn prepend_path(prefix: &str, value: &str) -> String {
    if value.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}:{value}")
    }
}

/// True when `path` belongs to a Wine (or GPTK-assembled) runtime rather than being generic.
fn looks_runtime_owned(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower.contains("/lib/wine/")
        || lower.ends_with("/lib/wine")
        || lower.contains("d3dmetal.framework")
        || lower.contains("/lib/external")
    {
        return true;
    }
    // A bare `<root>/lib` or `<root>/bin` belongs to a runtime only when that root really is one,
    // which is decided by the presence of its `lib/wine` tree. Keeps `/usr/local/lib`, Homebrew and
    // the Steam client directories untouched.
    if lower.ends_with("/lib") || lower.ends_with("/lib64") || lower.ends_with("/bin") {
        return Path::new(path).parent().is_some_and(|root| {
            root.join("lib/wine").is_dir() || root.join("lib64/wine").is_dir()
        });
    }
    false
}

/// Drops search-path entries owned by a runtime other than `runner_dir`.
///
/// Generic entries (the prefix's Steam directory, the Steam client itself, user directories such as
/// `/usr/local/lib`, Homebrew paths) are preserved exactly as-is; only entries that live inside a
/// *different* Wine runtime's library tree are removed.
pub fn keep_paths_for_runtime(value: &str, runner_dir: &Path) -> String {
    let runner = runner_dir.to_string_lossy().to_lowercase();
    value
        .split(':')
        .filter(|entry| !entry.is_empty())
        .filter(|entry| {
            if !looks_runtime_owned(entry) {
                return true;
            }
            let entry_lower = entry.to_ascii_lowercase();
            // Keep the chosen runtime's own trees, including a resolved-symlink vs raw-path mix.
            entry_lower.starts_with(&runner)
                || runner.starts_with(trim_trailing_slash(&entry_lower))
        })
        .collect::<Vec<_>>()
        .join(":")
}

fn trim_trailing_slash(value: &str) -> &str {
    value.strip_suffix('/').unwrap_or(value)
}

pub fn build_execution_env_for_engine(
    runner_dir: &Path,
    prefix_dir: &Path,
    engine: TargetEngine,
    enable_hud: bool,
    enable_overlay: bool,
) -> HashMap<String, String> {
    let mut env = HashMap::new();
    // One WoW64 prefix and loader handles both PE architectures. Ignore inherited win32 mode.
    env.insert("WINEARCH".to_string(), "win64".to_string());
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
        // `PATH` is intentionally not composed here: the choke point at the end of this function
        // prepends exactly this `bin` directory after removing foreign runtime entries.
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

    // Explicit opt-in uses only the installed client's native renderer.
    // Inherited lists may contain a retired shim or Steam's incompatible loader.
    let renderer = steam_client_dir.join("gameoverlayrenderer.dylib");
    if enable_overlay && renderer.is_file() {
        env.insert(
            "DYLD_INSERT_LIBRARIES".to_string(),
            renderer.display().to_string(),
        );
    }

    let chosen_backend: Box<dyn crate::backend::Backend> = match engine {
        TargetEngine::Auto => {
            let configured = crate::backend::get_active_backend();
            if configured != crate::backend::GraphicsBackend::Auto {
                crate::backend::create_backend(configured)
            } else {
                crate::backend::create_backend(crate::backend::GraphicsBackend::Auto)
            }
        }
        TargetEngine::Gptk => {
            crate::backend::create_backend(crate::backend::GraphicsBackend::D3DMetal)
        }
        TargetEngine::Dxmt => crate::backend::create_backend(crate::backend::GraphicsBackend::Dxmt),
        TargetEngine::Dxvk => crate::backend::create_backend(crate::backend::GraphicsBackend::Dxvk),
        TargetEngine::KosmicKrisp | TargetEngine::Vkd3d => {
            crate::backend::create_backend(crate::backend::GraphicsBackend::KosmicKrisp)
        }
        TargetEngine::WineStaging => {
            crate::backend::create_backend(crate::backend::GraphicsBackend::Auto)
        }
    };

    chosen_backend.apply_env(
        &mut env,
        runner_dir,
        prefix_dir,
        &steam_dir,
        &client_path_str,
        enable_hud,
    );

    if engine == TargetEngine::WineStaging || engine == TargetEngine::Auto {
        env.entry("WINEDLLOVERRIDES".to_string())
            .or_insert_with(|| {
                "steamclient=n,b;steamclient64=n,b;lsteamclient=b;d3d9,d3d10,d3d10_1,d3d10core=b,n"
                    .to_string()
            });
        let lib_dir = runner_dir.join("lib");
        let lib_unix = runner_dir.join("lib/wine/x86_64-unix");
        env.entry("DYLD_FALLBACK_LIBRARY_PATH".to_string())
            .or_insert_with(|| {
                format!(
                    "{}:{}:{}:{}",
                    steam_dir.display(),
                    client_path_str,
                    lib_unix.display(),
                    lib_dir.display()
                )
            });
    }

    // Single choke point for runtime-owned search paths. Every value below is derived from the
    // resolved `runner_dir` and stripped of entries belonging to a *different* Wine build, and each
    // key is forced to exist so nothing inherited from Steam (or from another compatibility tool)
    // can survive into the child. Without this, dyld resolves `ntdll.so`, `winemac_drv_1.so` and
    // friends from whichever runtime happened to be listed first, silently producing a driver that
    // loads and is immediately unloaded.
    for key in RUNTIME_SEARCH_PATH_VARS.into_iter() {
        let composed = env
            .get(key)
            .cloned()
            .or_else(|| env::var(key).ok())
            .unwrap_or_default();
        let derived = keep_paths_for_runtime(&composed, runner_dir);
        let value = if key == "PATH" {
            let bin = runner_dir.join("bin");
            if bin.is_dir() {
                prepend_path(&bin.to_string_lossy(), &derived)
            } else {
                derived
            }
        } else {
            derived
        };
        env.insert(key.to_string(), value);
    }

    // Diagnostics escape hatch: replaces the Wine log channels so a launch can be traced without
    // forking the runner. Applied last, so it wins over each engine's default channel set.
    if let Ok(debug) = env::var("NUCLEON_WINEDEBUG") {
        let debug = debug.trim();
        if !debug.is_empty() {
            env.insert("WINEDEBUG".to_string(), debug.to_string());
        }
    }

    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_runtime_search_paths_drop_foreign_wine_trees() {
        let chosen = "/Users/me/Library/Application Support/heroic/tools/wine/Wine-11.18/Contents/Resources/wine";
        let foreign = "/Users/me/Library/Application Support/heroic/tools/wine/Wine-Crossover-latest/Contents/Resources/wine";
        let value = format!(
            "/prefix/steam:{c_win}:{c_unix}:{foreign}/lib/wine/x86_64-unix:/usr/local/lib",
            c_win = format_args!("{chosen}/lib/wine/x86_64-windows"),
            c_unix = format_args!("{chosen}/lib/wine/x86_64-unix")
        );

        let kept = keep_paths_for_runtime(&value, Path::new(chosen));

        assert!(
            kept.contains("/prefix/steam"),
            "generic entries survive: {kept}"
        );
        assert!(kept.contains("/usr/local/lib"), "{kept}");
        assert!(kept.contains(chosen), "{kept}");
        assert!(
            !kept.contains(foreign),
            "a foreign runtime's ntdll/winemac trees must never reach the child: {kept}"
        );
    }

    #[test]
    fn test_runtime_search_paths_treat_bare_lib_of_a_real_runtime_as_owned() {
        let dir = tempdir().unwrap();
        let chosen = dir.path().join("chosen-wine");
        let foreign = dir.path().join("foreign-wine");
        let generic = dir.path().join("brew");
        for root in [&chosen, &foreign] {
            fs::create_dir_all(root.join("lib/wine/x86_64-unix")).unwrap();
            fs::create_dir_all(root.join("bin")).unwrap();
        }
        fs::create_dir_all(generic.join("lib")).unwrap();

        let value = format!(
            "{}:{}:{}:/usr/bin",
            chosen.join("lib").display(),
            foreign.join("lib").display(),
            generic.join("lib").display()
        );

        let kept = keep_paths_for_runtime(&value, &chosen);

        assert!(kept.contains(&chosen.join("lib").to_string_lossy().to_string()));
        assert!(
            !kept.contains(&foreign.join("lib").to_string_lossy().to_string()),
            "a runtime's bare lib holds libwine.dylib and must not be inherited: {kept}"
        );
        assert!(
            kept.contains(&generic.join("lib").to_string_lossy().to_string()),
            "non-runtime directories are untouched: {kept}"
        );
    }

    fn runtime_with_exports(root: &Path, exports: &[&str]) -> PathBuf {
        let runner = root.join("wine-runtime");
        let unix = runner.join("lib/wine/x86_64-unix");
        fs::create_dir_all(&unix).unwrap();
        // A Mach-O export trie stores names verbatim, so a byte scan sees them; emulate that here.
        let body: String = exports
            .iter()
            .map(|e| format!("__symbol_{e}_\0"))
            .collect::<Vec<_>>()
            .concat();
        fs::write(unix.join("gdi32.so"), body).unwrap();
        fs::write(unix.join("win32u.so"), "unrelated-symbols").unwrap();
        runner
    }

    #[test]
    fn test_d3dmetal_preflight_refuses_a_wine_without_d3dkmt_support() {
        let dir = tempdir().unwrap();
        let runner = runtime_with_exports(dir.path(), &["SomeOtherExport"]);

        assert_eq!(
            missing_d3dmetal_exports(&runner),
            vec!["gdi32!D3DKMTEnumAdapters2"],
            "a Wine build lacking the D3DMetal entry point must be reported"
        );

        let err = require_engine_capabilities(&runner, TargetEngine::Gptk)
            .expect_err("GPTK on an incapable runtime must fail loudly, not stub silently");
        let msg = err.to_string();
        assert!(msg.contains("D3DKMTEnumAdapters2"), "{msg}");
        assert!(msg.contains("DXMT") || msg.contains("KosmicKrisp"), "{msg}");

        for engine in [
            TargetEngine::Dxmt,
            TargetEngine::KosmicKrisp,
            TargetEngine::Vkd3d,
            TargetEngine::WineStaging,
        ] {
            require_engine_capabilities(&runner, engine)
                .expect("only the D3DMetal path needs this entry point");
        }
    }

    #[test]
    fn test_d3dmetal_preflight_recognises_modern_wines_forwarded_name() {
        let dir = tempdir().unwrap();
        // Shape measured on Wine 11.x: no gdi32 image exists and the entry point is exported by
        // win32u under its NtGdi name, forwarding what gdi32.dll imports.
        let runner = dir.path().join("wine-runtime");
        let unix = runner.join("lib/wine/x86_64-unix");
        fs::create_dir_all(&unix).unwrap();
        fs::write(unix.join("win32u.so"), "__NtGdiDdDDIEnumAdapters2\0other").unwrap();

        assert!(
            missing_d3dmetal_exports(&runner).is_empty(),
            "a Wine that forwards the entry point must not be reported as incapable"
        );
        require_engine_capabilities(&runner, TargetEngine::Gptk).unwrap();
    }

    #[test]
    fn test_d3dmetal_preflight_accepts_a_capable_wine() {
        let dir = tempdir().unwrap();
        let runner = runtime_with_exports(dir.path(), &["D3DKMTEnumAdapters2"]);

        assert!(missing_d3dmetal_exports(&runner).is_empty());
        require_engine_capabilities(&runner, TargetEngine::Gptk).unwrap();
        let (ok, detail) = describe_d3dmetal_support(&runner).unwrap();
        assert!(ok, "{detail}");
    }

    #[test]
    fn test_d3dmetal_preflight_is_silent_about_unknown_runtimes() {
        let dir = tempdir().unwrap();
        // No candidate images at all: nothing can be proven missing, so never block a launch.
        require_engine_capabilities(dir.path(), TargetEngine::Gptk)
            .expect("an uninspectable runtime must not be treated as incapable");
    }

    #[test]
    fn test_build_execution_env_kosmickrisp() {
        let runner_dir = PathBuf::from("/tmp/test_runner");
        let prefix_dir = PathBuf::from("/tmp/test_prefix");

        let env = build_execution_env_for_engine(
            &runner_dir,
            &prefix_dir,
            TargetEngine::KosmicKrisp,
            true,
            false,
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

        // Environment candidates are passed in rather than exported, so this cannot interfere with
        // any other test that resolves Vulkan drivers.
        let found = find_kosmickrisp_icd_with(std::slice::from_ref(&icd_file));
        assert_eq!(found, Some(icd_file));
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

        apply_gptk_execution_env(&mut env, &runner_dir, &steam_dir, client_path_str, true);

        assert_eq!(env.get("D3DM_MTL4").map(|s| s.as_str()), Some("1"));
        assert_eq!(
            env.get("D3DM_ENABLE_METALFX").map(|s| s.as_str()),
            Some("1")
        );
        assert_eq!(env.get("D3DM_SUPPORT_DXR").map(|s| s.as_str()), Some("1"));
        assert_eq!(env.get("MTL_HUD_ENABLED").map(|s| s.as_str()), Some("1"));
        assert_eq!(
            env.get("WINEDEBUG").map(|s| s.as_str()),
            Some("warn+all,err+all,+seh")
        );
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
        assert!(
            err1.to_string()
                .contains("Specified GPTK path does not exist")
        );

        // 2. File instead of directory
        let file_path = dir.path().join("file.txt");
        fs::write(&file_path, "not a dir").unwrap();
        let err2 = validate_and_resolve_gptk_components(&file_path).unwrap_err();
        assert!(
            err2.to_string()
                .contains("Specified GPTK path is not a directory")
        );

        // 3. Directory with missing components
        let empty_dir = dir.path().join("empty");
        fs::create_dir_all(&empty_dir).unwrap();
        let err3 = validate_and_resolve_gptk_components(&empty_dir).unwrap_err();
        assert!(
            err3.to_string()
                .contains("Directory does not contain valid Apple GPTK components")
        );

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

    /// GPTK sources rooted in `root`, with nothing configured at any priority level.
    fn gptk_sources_rooted_in(root: &Path) -> GptkSources {
        GptkSources {
            env_paths: Vec::new(),
            custom_path_file: root.join("custom_gptk_path.txt"),
            assembled_runner: root.join("runners/current"),
        }
    }

    #[test]
    fn test_find_and_resolve_gptk_components_strict_validation() {
        let dir = tempdir().unwrap();
        let valid_gptk = dir.path().join("valid_gptk");
        fs::create_dir_all(valid_gptk.join("D3DMetal.framework")).unwrap();
        fs::write(valid_gptk.join("libd3dshared.dylib"), "dylib").unwrap();

        let invalid_gptk = dir.path().join("invalid_gptk");
        fs::create_dir_all(&invalid_gptk).unwrap();

        // 1. Explicit path (valid and invalid) through the public entry point.
        assert!(find_gptk_components(Some(&valid_gptk)).unwrap().is_some());
        assert!(find_gptk_components(Some(&invalid_gptk)).is_err());

        // The remaining priorities are exercised with injected sources: no process environment
        // variables and no shared support directory, so the outcome never depends on thread order,
        // what is installed on the host, or on a previously configured GPTK folder.
        let mut sources = gptk_sources_rooted_in(dir.path());

        // 2. An environment-configured path resolves when valid and fails loudly when not.
        sources.env_paths = vec![("NUCLEON_GPTK_PATH".to_string(), valid_gptk.clone())];
        assert!(
            discover_gptk_components(&sources, None, OnInvalidPath::Fail)
                .unwrap()
                .is_some()
        );

        sources.env_paths = vec![("NUCLEON_GPTK_PATH".to_string(), invalid_gptk.clone())];
        let env_err = discover_gptk_components(&sources, None, OnInvalidPath::Fail).unwrap_err();
        assert!(
            env_err
                .to_string()
                .contains("Environment variable NUCLEON_GPTK_PATH points to an invalid GPTK path"),
            "got: {env_err}"
        );

        // 3. Unconfigured discovery yields nothing, and resolution explains how to configure it.
        sources.env_paths.clear();
        assert!(
            discover_gptk_components(&sources, None, OnInvalidPath::Fail)
                .unwrap()
                .is_none()
        );
        let resolve_err = require_configured_gptk(None).unwrap_err();
        assert!(
            resolve_err
                .to_string()
                .contains("Apple Game Porting Toolkit 4.0 path is not configured"),
            "got: {resolve_err}"
        );

        // 4. Persisted configuration written and removed by the set-path/clear-path commands.
        set_custom_gptk_path_in(&sources.custom_path_file, &valid_gptk).unwrap();
        let persisted = discover_gptk_components(&sources, None, OnInvalidPath::Fail)
            .unwrap()
            .expect("persisted custom GPTK path must be discovered");
        assert_eq!(persisted.source_label, "from custom path file");

        clear_custom_gptk_path_in(&sources.custom_path_file).unwrap();
        assert!(
            discover_gptk_components(&sources, None, OnInvalidPath::Fail)
                .unwrap()
                .is_none()
        );

        // 5. Status output skips the invalid environment path and keeps walking down the ladder.
        set_custom_gptk_path_in(&sources.custom_path_file, &valid_gptk).unwrap();
        sources.env_paths = vec![("NUCLEON_GPTK_PATH".to_string(), invalid_gptk)];
        let status = discover_gptk_components(&sources, None, OnInvalidPath::Ignore)
            .unwrap()
            .expect("status output must fall through to the persisted path");
        assert_eq!(status.source_label, "from custom path file");

        // 6. Components staged in the assembled runner are used when nothing else is configured.
        clear_custom_gptk_path_in(&sources.custom_path_file).unwrap();
        sources.env_paths.clear();
        let staged = sources.assembled_runner.clone();
        fs::create_dir_all(staged.join("lib/external/D3DMetal.framework")).unwrap();
        fs::write(staged.join("lib/external/libd3dshared.dylib"), "dylib").unwrap();
        let from_runner = discover_gptk_components(&sources, None, OnInvalidPath::Fail)
            .unwrap()
            .expect("components staged in the assembled runner must be discovered");
        assert!(
            from_runner
                .source_label
                .starts_with("from assembled runner ("),
            "got: {}",
            from_runner.source_label
        );
    }

    #[test]
    fn test_plan_runner_never_substitutes_wrong_engine() {
        let empty = RunnerHostState::default();
        let wine_only = RunnerHostState {
            wine_runtime: Some(PathBuf::from("/wine")),
            gptk_runner: None,
        };
        let gptk_only = RunnerHostState {
            wine_runtime: None,
            gptk_runner: Some(PathBuf::from("/gptk-runner")),
        };

        // A GPTK request is only ever served by a GPTK-capable runner, never by plain Wine.
        assert_eq!(
            plan_runner(&wine_only, TargetEngine::Gptk),
            RunnerPlan::AssembleRunner
        );
        assert_eq!(
            plan_runner(&gptk_only, TargetEngine::Gptk),
            RunnerPlan::Existing(PathBuf::from("/gptk-runner"))
        );

        // Wine-side engines prefer a real Wine runtime and only borrow the GPTK binary after that.
        for engine in [
            TargetEngine::Auto,
            TargetEngine::WineStaging,
            TargetEngine::KosmicKrisp,
            TargetEngine::Dxmt,
            TargetEngine::Dxvk,
            TargetEngine::Vkd3d,
        ] {
            assert_eq!(
                plan_runner(&wine_only, engine),
                RunnerPlan::Existing(PathBuf::from("/wine")),
                "{engine:?} must use the Wine runtime"
            );
            assert_eq!(
                plan_runner(&gptk_only, engine),
                RunnerPlan::Existing(PathBuf::from("/gptk-runner")),
                "{engine:?} may fall back to the GPTK wine binary"
            );
            assert_eq!(
                plan_runner(&empty, engine),
                RunnerPlan::AssembleRunner,
                "{engine:?} must assemble a runner when nothing is installed"
            );
        }
    }

    #[test]
    fn test_resolve_runner_for_engine_gptk_fails_loudly_when_not_configured() {
        // The host state (a fresh install: no Wine runtime, no GPTK-capable runner) is passed in
        // explicitly instead of being conjured from environment variables and the support
        // directory, so this test neither reads nor writes process-global state and can run
        // concurrently with every other test.
        let host = RunnerHostState::default();

        // 1. A GPTK request must never be quietly served by a Wine-only runtime; it has to go
        //    through assembly, which is where the "not configured" error surfaces.
        assert_eq!(
            plan_runner(&host, TargetEngine::Gptk),
            RunnerPlan::AssembleRunner,
            "TargetEngine::Gptk must not fall back to a Wine runtime when GPTK is missing"
        );

        // 2. Assembly failures for GPTK are reported loudly with actionable guidance pointing at
        //    the user-configured GPTK folder.
        let assembly_failed =
            || Err::<PathBuf, _>(anyhow::anyhow!("No compatible Wine runtime found."));

        let err_msg = annotate_assembly_failure(assembly_failed(), TargetEngine::Gptk)
            .unwrap_err()
            .to_string();
        assert!(
            err_msg.contains("GPTK") || err_msg.contains("Apple Game Porting Toolkit"),
            "expected actionable GPTK guidance, got: {err_msg}"
        );

        // 3. Wine-side engines keep their underlying error untouched (no misleading GPTK advice).
        assert_eq!(
            annotate_assembly_failure(assembly_failed(), TargetEngine::WineStaging)
                .unwrap_err()
                .to_string(),
            "No compatible Wine runtime found."
        );
    }

    #[test]
    fn test_golden_path_launch_flows_gptk4_vs_wine() {
        let temp = tempdir().unwrap();
        let runner_dir = temp.path().join("runner");
        let prefix_dir = temp.path().join("pfx");
        fs::create_dir_all(runner_dir.join("bin")).unwrap();
        fs::create_dir_all(runner_dir.join("lib/wine/x86_64-unix")).unwrap();
        fs::create_dir_all(
            runner_dir.join("lib/external/D3DMetal.framework/Versions/Current/Resources"),
        )
        .unwrap();
        fs::write(runner_dir.join("bin/wine"), b"").unwrap();
        fs::write(runner_dir.join("bin/wineserver"), b"").unwrap();
        fs::write(runner_dir.join("lib/external/libd3dshared.dylib"), b"").unwrap();
        fs::create_dir_all(prefix_dir.join("drive_c/windows/system32")).unwrap();

        // 1. Golden Path: GPTK4 Launch Flow
        let gptk_env = build_execution_env_for_engine(
            &runner_dir,
            &prefix_dir,
            TargetEngine::Gptk,
            true,
            false,
        );
        assert_eq!(gptk_env.get("D3DM_MTL4").map(|s| s.as_str()), Some("1"));
        assert_eq!(
            gptk_env.get("D3DM_ENABLE_METALFX").map(|s| s.as_str()),
            Some("1")
        );
        assert_eq!(
            gptk_env.get("D3DM_SUPPORT_DXR").map(|s| s.as_str()),
            Some("1")
        );
        assert_eq!(
            gptk_env.get("MTL_HUD_ENABLED").map(|s| s.as_str()),
            Some("1")
        );
        assert!(
            !gptk_env.contains_key("WINEMSYNC"),
            "GPTK4 flow must not set WINEMSYNC"
        );
        assert_eq!(gptk_env.get("WINEESYNC").map(|s| s.as_str()), Some("1"));
        let gptk_overrides = gptk_env.get("WINEDLLOVERRIDES").unwrap();
        assert!(gptk_overrides.contains("d3d11,dxgi,d3d12,d3d10core,d3dcompiler_47=n,b"));
        let gptk_dyld = gptk_env.get("DYLD_FALLBACK_LIBRARY_PATH").unwrap();
        assert!(gptk_dyld.contains("D3DMetal.framework"));

        // 2. Golden Path: Wine Flow (DXMT)
        let wine_dxmt_env = build_execution_env_for_engine(
            &runner_dir,
            &prefix_dir,
            TargetEngine::Dxmt,
            false,
            false,
        );
        assert_eq!(
            wine_dxmt_env.get("WINEMSYNC").map(|s| s.as_str()),
            Some("1"),
            "Wine flow must set WINEMSYNC"
        );
        assert!(
            !wine_dxmt_env.contains_key("D3DM_MTL4"),
            "Wine flow must not set D3DM_MTL4"
        );
        let dxmt_overrides = wine_dxmt_env.get("WINEDLLOVERRIDES").unwrap();
        assert!(dxmt_overrides.contains("winemetal=b"));

        // 3. Golden Path: Wine Flow (DXVK)
        let wine_dxvk_env = build_execution_env_for_engine(
            &runner_dir,
            &prefix_dir,
            TargetEngine::Dxvk,
            false,
            false,
        );
        assert_eq!(
            wine_dxvk_env.get("WINEMSYNC").map(|s| s.as_str()),
            Some("1"),
            "Wine flow must set WINEMSYNC"
        );
        assert!(
            !wine_dxvk_env.contains_key("D3DM_MTL4"),
            "Wine flow must not set D3DM_MTL4"
        );
        let dxvk_overrides = wine_dxvk_env.get("WINEDLLOVERRIDES").unwrap();
        assert!(dxvk_overrides.contains("d3d11,dxgi,d3d10core,d3d9=n,b"));
    }

    fn stamp(wine_root: &str) -> RunnerSourceStamp {
        RunnerSourceStamp {
            abi_revision: gptk_abi::ABI_REVISION,
            wine_root: wine_root.to_string(),
            wine_loader: "1000:1700000000000".to_string(),
            wine_runtime: "wine-tree".to_string(),
            wine_abi_inputs: "wine-abi-images".to_string(),
            gptk_dir: "/gptk/redist/lib/external".to_string(),
            gptk_shared: "2000:1700000000000".to_string(),
            gptk_framework: "framework-tree".to_string(),
            gptk_windows: "windows-tree".to_string(),
            gptk_unix: "unix-tree".to_string(),
        }
    }

    fn completed_runner(root: &Path) {
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(root.join("bin/wine"), "known-good Wine").unwrap();
        fs::create_dir_all(root.join("lib/external/D3DMetal.framework")).unwrap();
        stamp("/wine/current").write(root).unwrap();
    }

    #[test]
    fn test_runner_publication_preserves_old_generation_and_rejects_incomplete_work() {
        let dir = tempdir().unwrap();
        let old = dir.path().join("gptk-old");
        completed_runner(&old);
        let current = dir.path().join("current");
        link_current_runner_at(&old, &current).unwrap();
        let old_stamp = fs::read(RunnerSourceStamp::stamp_file(&old)).unwrap();

        // A failure after rsync has copied Wine must still leave the selected generation intact.
        let wine = dir.path().join("wine-source");
        fs::create_dir_all(wine.join("bin")).unwrap();
        fs::write(wine.join("bin/wine"), "replacement Wine").unwrap();
        let gptk = dir.path().join("redist/lib");
        let framework = gptk.join("external/D3DMetal.framework");
        let shared = gptk.join("external/libd3dshared.dylib");
        fs::create_dir_all(&framework).unwrap();
        fs::write(&shared, "shared").unwrap();
        fs::create_dir_all(gptk.join("wine/x86_64-windows")).unwrap();
        fs::write(
            gptk.join("wine/x86_64-windows/dxgi.dll"),
            b"__wine_unix_call_dispatcher\0",
        )
        .unwrap();
        let failed = tempfile::Builder::new()
            .prefix(".gptk-")
            .tempdir_in(dir.path())
            .unwrap();
        assert!(
            assemble_runner_tree(
                failed.path(),
                &wine,
                &framework,
                &shared,
                Some(gptk_abi::WineUnixCallAbi::Plain)
            )
            .is_err()
        );
        assert_eq!(current.canonicalize().unwrap(), old.canonicalize().unwrap());
        assert_eq!(fs::read(old.join("bin/wine")).unwrap(), b"known-good Wine");
        assert_eq!(
            fs::read(RunnerSourceStamp::stamp_file(&old)).unwrap(),
            old_stamp
        );

        let incomplete = tempfile::Builder::new()
            .prefix(".gptk-")
            .tempdir_in(dir.path())
            .unwrap();
        fs::create_dir_all(incomplete.path().join("bin")).unwrap();
        fs::write(incomplete.path().join("bin/wine"), "partial").unwrap();
        assert!(publish_runner(incomplete, &current).is_err());
        assert_eq!(current.canonicalize().unwrap(), old.canonicalize().unwrap());
        assert_eq!(fs::read(old.join("bin/wine")).unwrap(), b"known-good Wine");
        assert_eq!(
            fs::read(RunnerSourceStamp::stamp_file(&old)).unwrap(),
            old_stamp
        );

        let replacement = tempfile::Builder::new()
            .prefix(".gptk-")
            .tempdir_in(dir.path())
            .unwrap();
        completed_runner(replacement.path());
        fs::write(replacement.path().join("bin/wine"), "new Wine").unwrap();
        let published = publish_runner(replacement, &current).unwrap();
        assert_eq!(
            complete_runner_path(&current).unwrap(),
            published.canonicalize().unwrap()
        );
        assert_eq!(fs::read(old.join("bin/wine")).unwrap(), b"known-good Wine");
        assert_eq!(fs::read(current.join("bin/wine")).unwrap(), b"new Wine");
    }

    #[test]
    fn test_hidden_runner_is_never_discovered_and_real_current_directory_is_preserved() {
        let dir = tempdir().unwrap();
        let staging = tempfile::Builder::new()
            .prefix(".gptk-")
            .tempdir_in(dir.path())
            .unwrap();
        completed_runner(staging.path());
        assert!(find_completed_runner_in(dir.path()).is_none());
        let current = dir.path().join("current");
        fs::create_dir(&current).unwrap();
        fs::write(current.join("preserve"), "original").unwrap();
        assert!(publish_runner(staging, &current).is_err());
        assert_eq!(fs::read(current.join("preserve")).unwrap(), b"original");
    }

    #[test]
    fn test_source_stamp_tracks_runtime_framework_and_both_gptk_binary_trees() {
        let dir = tempdir().unwrap();
        let wine = dir.path().join("wine");
        let gptk = dir.path().join("redist/lib");
        let framework = gptk.join("external/D3DMetal.framework");
        let shared = gptk.join("external/libd3dshared.dylib");
        let inputs = [
            wine.join("bin/wine"),
            wine.join(gptk_abi::WINE_UNIX_CALL_ABI_CANDIDATES[0]),
            wine.join("lib/wine/x86_64-unix/winemac.so"),
            framework.join("Versions/A/D3DMetal"),
            gptk.join("wine/x86_64-windows/dxgi.dll"),
            gptk.join("wine/x86_64-unix/dxgi.so"),
            shared.clone(),
        ];
        for input in &inputs {
            fs::create_dir_all(input.parent().unwrap()).unwrap();
            fs::write(input, "original").unwrap();
        }
        let original = RunnerSourceStamp::from_sources(&wine, &framework, &shared).unwrap();
        for input in &inputs {
            fs::write(input, "updated source with different size").unwrap();
            let updated = RunnerSourceStamp::from_sources(&wine, &framework, &shared).unwrap();
            assert_ne!(
                original,
                updated,
                "{} must invalidate the runner",
                input.display()
            );
            fs::write(input, "original").unwrap();
        }
        #[cfg(unix)]
        {
            let link = framework.join("Versions/Current");
            std::os::unix::fs::symlink("A", &link).unwrap();
            let first = RunnerSourceStamp::from_sources(&wine, &framework, &shared).unwrap();
            fs::remove_file(&link).unwrap();
            std::os::unix::fs::symlink("B", &link).unwrap();
            let second = RunnerSourceStamp::from_sources(&wine, &framework, &shared).unwrap();
            assert_ne!(first.gptk_framework, second.gptk_framework);
        }
        assert!(
            RunnerSourceStamp::from_sources(&wine, &dir.path().join("offline"), &shared).is_err()
        );
        let mut legacy = serde_json::to_value(&original).unwrap();
        legacy.as_object_mut().unwrap().remove("gptk_framework");
        fs::write(
            RunnerSourceStamp::stamp_file(dir.path()),
            legacy.to_string(),
        )
        .unwrap();
        assert!(RunnerSourceStamp::read(dir.path()).is_none());
    }

    #[test]
    fn test_assembled_runner_is_reused_only_for_the_sources_it_was_built_from() {
        let dir = tempdir().unwrap();
        let runner = dir.path();
        let current = stamp("/wine/current");

        // A runner assembled before source tracking existed cannot be trusted to be current.
        assert!(!cached_runner_is_current(runner, Some(&current)));

        current.write(runner).unwrap();
        assert!(cached_runner_is_current(runner, Some(&current)));

        // Switching the active Wine must invalidate the assembly instead of silently launching the
        // previous runtime, which is how one engine's fix used to break the other.
        assert!(!cached_runner_is_current(
            runner,
            Some(&stamp("/wine/someone-else"))
        ));

        // A rebuilt GPTK download changes only the component identity.
        let mut updated_gptk = current.clone();
        updated_gptk.gptk_shared = "2004:1700000099000".to_string();
        assert!(!cached_runner_is_current(runner, Some(&updated_gptk)));

        // An older ABI-bridging rule must not be considered current either.
        let mut older_abi = current.clone();
        older_abi.abi_revision = current.abi_revision.saturating_sub(1);
        assert!(!cached_runner_is_current(runner, Some(&older_abi)));

        // Unresolvable sources (unmounted volume, cleared configuration) keep a working runner.
        assert!(cached_runner_is_current(runner, None));
    }

    #[test]
    fn test_source_stamp_round_trips_and_rejects_garbage() {
        let dir = tempdir().unwrap();
        let stamp = stamp("/wine/current");
        stamp.write(dir.path()).unwrap();
        assert_eq!(RunnerSourceStamp::read(dir.path()), Some(stamp.clone()));

        fs::write(RunnerSourceStamp::stamp_file(dir.path()), "not json").unwrap();
        assert_eq!(RunnerSourceStamp::read(dir.path()), None);
    }

    #[test]
    fn test_gptk_binaries_dir_is_found_for_both_component_layouts() {
        let dir = tempdir().unwrap();
        let redist = dir.path().join("redist");

        // Layout 1: components under `redist/lib/external`, binaries under `redist/lib/wine`.
        let external = redist.join("lib/external");
        fs::create_dir_all(&external).unwrap();
        let framework = external.join("D3DMetal.framework");
        fs::create_dir_all(&framework).unwrap();
        let shared = external.join("libd3dshared.dylib");
        fs::write(&shared, b"dylib").unwrap();
        let windows = redist.join("lib/wine/x86_64-windows");
        fs::create_dir_all(&windows).unwrap();
        assert_eq!(
            gptk_windows_binaries_dir(&framework, &shared),
            Some(windows)
        );

        // Layout 2: components directly in `redist/lib`, binaries beside them.
        let dir2 = tempdir().unwrap();
        let redist2 = dir2.path().join("redist");
        let lib = redist2.join("lib");
        fs::create_dir_all(lib.join("external/D3DMetal.framework")).unwrap();
        fs::create_dir_all(lib.join("wine/x86_64-windows")).unwrap();
        let shared2 = lib.join("libd3dshared.dylib");
        fs::write(&shared2, b"dylib").unwrap();
        assert_eq!(
            gptk_windows_binaries_dir(&lib.join("external/D3DMetal.framework"), &shared2),
            Some(lib.join("wine/x86_64-windows"))
        );
    }
}
