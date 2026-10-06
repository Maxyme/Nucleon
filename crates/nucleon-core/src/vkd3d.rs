use crate::paths;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const DEFAULT_VKD3D_VERSION: &str = "v3.0.1";

#[derive(Debug, Clone)]
pub struct Vkd3dProtonBundle {
    pub root: PathBuf,
    pub x64_d3d12: PathBuf,
    pub x64_d3d12core: Option<PathBuf>,
    pub x86_d3d12: Option<PathBuf>,
    pub x86_d3d12core: Option<PathBuf>,
    pub version: Option<String>,
}

fn read_version_file(dir: &Path) -> Option<String> {
    let ver_file = dir.join("version.txt");
    if ver_file.is_file() {
        if let Ok(c) = fs::read_to_string(&ver_file) {
            let t = c.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

/// Inspects a directory to check if it contains a valid VKD3D-Proton installation.
/// Accepts either the standard release hierarchy (x64/d3d12.dll) or a flat directory (d3d12.dll).
pub fn inspect_candidate_dir(dir: &Path) -> Option<Vkd3dProtonBundle> {
    if !dir.is_dir() {
        return None;
    }

    // 1. Check standard release layout: <dir>/x64/d3d12.dll
    let x64_dir = dir.join("x64");
    if x64_dir.join("d3d12.dll").is_file() {
        let x64_d3d12 = x64_dir.join("d3d12.dll");
        let x64_d3d12core = if x64_dir.join("d3d12core.dll").is_file() {
            Some(x64_dir.join("d3d12core.dll"))
        } else {
            None
        };

        let x86_dir = dir.join("x86");
        let x86_d3d12 = if x86_dir.join("d3d12.dll").is_file() {
            Some(x86_dir.join("d3d12.dll"))
        } else {
            None
        };
        let x86_d3d12core = if x86_dir.join("d3d12core.dll").is_file() {
            Some(x86_dir.join("d3d12core.dll"))
        } else {
            None
        };

        return Some(Vkd3dProtonBundle {
            root: dir.to_path_buf(),
            x64_d3d12,
            x64_d3d12core,
            x86_d3d12,
            x86_d3d12core,
            version: read_version_file(dir),
        });
    }

    // 2. Check flat directory: <dir>/d3d12.dll
    if dir.join("d3d12.dll").is_file() {
        let x64_d3d12 = dir.join("d3d12.dll");
        let x64_d3d12core = if dir.join("d3d12core.dll").is_file() {
            Some(dir.join("d3d12core.dll"))
        } else {
            None
        };

        return Some(Vkd3dProtonBundle {
            root: dir.to_path_buf(),
            x64_d3d12,
            x64_d3d12core,
            x86_d3d12: None,
            x86_d3d12core: None,
            version: read_version_file(dir),
        });
    }

    None
}

pub fn custom_vkd3d_path_file() -> PathBuf {
    paths::support_dir().join("vkd3d_proton_path.txt")
}

/// Locates a VKD3D-Proton bundle from environment, configured custom path,
/// Nucleon support directory, or well-known system/Proton paths.
pub fn find_vkd3d_proton() -> Option<Vkd3dProtonBundle> {
    // 1. Explicit environment variable: VKD3D_PROTON_PATH or VKD3D_PATH
    if let Ok(p) = std::env::var("VKD3D_PROTON_PATH") {
        let path = PathBuf::from(p);
        if let Some(bundle) = inspect_candidate_dir(&path) {
            return Some(bundle);
        }
    }
    if let Ok(p) = std::env::var("VKD3D_PATH") {
        let path = PathBuf::from(p);
        if let Some(bundle) = inspect_candidate_dir(&path) {
            return Some(bundle);
        }
    }

    // 2. Custom path stored via `nucleon vkd3d set-path <path>`
    let custom_file = custom_vkd3d_path_file();
    if custom_file.is_file() {
        if let Ok(content) = fs::read_to_string(&custom_file) {
            let p = PathBuf::from(content.trim());
            if let Some(bundle) = inspect_candidate_dir(&p) {
                return Some(bundle);
            }
        }
    }

    // 3. Nucleon managed directory: ~/Library/Application Support/nucleon/vkd3d-proton
    let nucleon_dir = paths::vkd3d_proton_dir();
    if let Some(bundle) = inspect_candidate_dir(&nucleon_dir) {
        return Some(bundle);
    }

    // 4. Well-known Homebrew and local share directories
    let system_candidates = [
        PathBuf::from("/opt/homebrew/share/vkd3d-proton"),
        PathBuf::from("/usr/local/share/vkd3d-proton"),
        paths::home_dir().join(".local/share/vkd3d-proton"),
    ];

    for candidate in &system_candidates {
        if let Some(bundle) = inspect_candidate_dir(candidate) {
            return Some(bundle);
        }
    }

    // 5. Scan Steam Proton installs for vkd3d-proton if present
    let steam_common = paths::home_dir().join("Library/Application Support/Steam/steamapps/common");
    if steam_common.is_dir() {
        if let Ok(entries) = fs::read_dir(&steam_common) {
            for entry in entries.flatten() {
                let p = entry.path();
                let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if name.starts_with("Proton") {
                    let vkd3d_dirs = [
                        p.join("dist/lib64/wine/vkd3d-proton"),
                        p.join("dist/lib/wine/vkd3d-proton"),
                        p.join("dist/share/default_pfx/drive_c/windows/system32"),
                    ];
                    for vd in &vkd3d_dirs {
                        if let Some(bundle) = inspect_candidate_dir(vd) {
                            return Some(bundle);
                        }
                    }
                }
            }
        }
    }

    None
}

/// Returns true if VKD3D-Proton is detected on the system.
pub fn is_vkd3d_proton_installed() -> bool {
    find_vkd3d_proton().is_some()
}

/// Sets a custom path to a VKD3D-Proton installation directory.
pub fn set_custom_vkd3d_proton_path(path: &Path) -> Result<Vkd3dProtonBundle> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("Path does not exist: {}", path.display()))?;

    let bundle = inspect_candidate_dir(&canonical).context(
        "Specified path does not contain valid VKD3D-Proton binaries (d3d12.dll not found)",
    )?;

    paths::ensure_dirs()?;
    fs::write(
        custom_vkd3d_path_file(),
        canonical.to_string_lossy().as_bytes(),
    )
    .with_context(|| format!("Failed to write {}", custom_vkd3d_path_file().display()))?;

    log::info!(
        "Registered custom VKD3D-Proton path at {}",
        canonical.display()
    );
    Ok(bundle)
}

/// Clears any configured custom VKD3D-Proton path.
pub fn clear_custom_vkd3d_proton_path() -> Result<()> {
    let custom_file = custom_vkd3d_path_file();
    if custom_file.is_file() {
        let _ = fs::remove_file(&custom_file);
    }
    Ok(())
}

/// Stages VKD3D-Proton DLLs into a Wine prefix's system32 and syswow64 directories.
pub fn stage_vkd3d_proton_into_prefix(
    bundle: &Vkd3dProtonBundle,
    prefix_dir: &Path,
) -> Result<usize> {
    let sys32 = prefix_dir.join("drive_c/windows/system32");
    let syswow64 = prefix_dir.join("drive_c/windows/syswow64");

    fs::create_dir_all(&sys32)?;
    let mut staged_count = 0;

    // Stage 64-bit d3d12.dll
    let dst_d3d12 = sys32.join("d3d12.dll");
    if dst_d3d12.exists() || dst_d3d12.is_symlink() {
        let _ = fs::remove_file(&dst_d3d12);
    }
    fs::copy(&bundle.x64_d3d12, &dst_d3d12).with_context(|| {
        format!(
            "Failed to stage 64-bit d3d12.dll to {}",
            dst_d3d12.display()
        )
    })?;
    staged_count += 1;

    // Stage 64-bit d3d12core.dll if present
    if let Some(ref core_src) = bundle.x64_d3d12core {
        let dst_core = sys32.join("d3d12core.dll");
        if dst_core.exists() || dst_core.is_symlink() {
            let _ = fs::remove_file(&dst_core);
        }
        fs::copy(core_src, &dst_core).with_context(|| {
            format!(
                "Failed to stage 64-bit d3d12core.dll to {}",
                dst_core.display()
            )
        })?;
        staged_count += 1;
    }

    // Stage 32-bit d3d12.dll if syswow64 exists and 32-bit binary is present
    if let Some(ref x86_src) = bundle.x86_d3d12 {
        if syswow64.is_dir() {
            let dst_x86 = syswow64.join("d3d12.dll");
            if dst_x86.exists() || dst_x86.is_symlink() {
                let _ = fs::remove_file(&dst_x86);
            }
            if fs::copy(x86_src, &dst_x86).is_ok() {
                staged_count += 1;
            }
        }
    }

    if let Some(ref x86_core) = bundle.x86_d3d12core {
        if syswow64.is_dir() {
            let dst_x86_core = syswow64.join("d3d12core.dll");
            if dst_x86_core.exists() || dst_x86_core.is_symlink() {
                let _ = fs::remove_file(&dst_x86_core);
            }
            if fs::copy(x86_core, &dst_x86_core).is_ok() {
                staged_count += 1;
            }
        }
    }

    log::info!(
        "Staged {} VKD3D-Proton DLL(s) into Wine prefix at {}",
        staged_count,
        prefix_dir.display()
    );

    Ok(staged_count)
}

/// Fetches VKD3D-Proton from official GitHub releases into the target directory.
/// Does not commit any files to source code; stores purely in runtime support directory.
pub fn fetch_vkd3d_proton(
    version: Option<&str>,
    dest_dir: Option<&Path>,
) -> Result<Vkd3dProtonBundle> {
    let tag = version.unwrap_or(DEFAULT_VKD3D_VERSION);
    let raw_ver = tag.trim_start_matches('v');
    let archive_name = format!("vkd3d-proton-{raw_ver}.tar.zst");
    let url = format!(
        "https://github.com/HansKristian-Work/vkd3d-proton/releases/download/{tag}/{archive_name}"
    );

    let target_dir = match dest_dir {
        Some(p) => p.to_path_buf(),
        None => paths::vkd3d_proton_dir(),
    };
    fs::create_dir_all(&target_dir)?;

    let temp_dir = tempfile::tempdir()?;
    let archive_path = temp_dir.path().join(&archive_name);

    log::info!("Fetching VKD3D-Proton {} from {}...", tag, url);

    // Download archive via curl
    let status = Command::new("curl")
        .args([
            "-sSL",
            "--retry",
            "3",
            "-H",
            "User-Agent: nucleon-fetcher",
            "-o",
            archive_path.to_str().unwrap(),
            &url,
        ])
        .status()
        .with_context(|| format!("Failed to execute curl to download {}", url))?;

    if !status.success() || !archive_path.is_file() || fs::metadata(&archive_path)?.len() < 1000 {
        bail!(
            "Failed to download VKD3D-Proton from {}. Check network connection or version tag.",
            url
        );
    }

    // Extract archive
    let extract_dir = temp_dir.path().join("extracted");
    fs::create_dir_all(&extract_dir)?;

    // Try tar with --zstd first, fallback to zstd -d piped into tar
    let tar_status = Command::new("tar")
        .args([
            "--zstd",
            "-xf",
            archive_path.to_str().unwrap(),
            "-C",
            extract_dir.to_str().unwrap(),
        ])
        .status();

    let extracted = if tar_status.map(|s| s.success()).unwrap_or(false) {
        true
    } else {
        // Fallback: use python3 zstandard / subprocess if zstd command isn't directly in tar
        let py_script = format!(
            r#"
import subprocess, sys, shutil

# Try zstd command pipe
try:
    p1 = subprocess.Popen(["zstd", "-d", "-c", "{} "], stdout=subprocess.PIPE)
    p2 = subprocess.Popen(["tar", "-xf", "-", "-C", "{}"], stdin=p1.stdout)
    p1.stdout.close()
    p2.communicate()
    if p2.returncode == 0:
        sys.exit(0)
except Exception:
    pass

sys.exit(1)
"#,
            archive_path.display(),
            extract_dir.display()
        );
        let py_status = Command::new("python3").args(["-c", &py_script]).status();
        py_status.map(|s| s.success()).unwrap_or(false)
    };

    if !extracted {
        bail!(
            "Failed to extract {}. Ensure zstd and tar are installed (e.g. 'brew install zstd').",
            archive_path.display()
        );
    }

    // Locate the extracted release folder (usually vkd3d-proton-<ver>)
    let mut source_bundle_dir = extract_dir.clone();
    if let Ok(entries) = fs::read_dir(&extract_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() && (p.join("x64").is_dir() || p.join("d3d12.dll").is_file()) {
                source_bundle_dir = p;
                break;
            }
        }
    }

    // Copy extracted hierarchy into target_dir
    if source_bundle_dir.join("x64").is_dir() {
        let dst_x64 = target_dir.join("x64");
        fs::create_dir_all(&dst_x64)?;
        for entry in fs::read_dir(source_bundle_dir.join("x64"))?.flatten() {
            let p = entry.path();
            if p.is_file() {
                fs::copy(&p, dst_x64.join(p.file_name().unwrap()))?;
            }
        }
    }
    if source_bundle_dir.join("x86").is_dir() {
        let dst_x86 = target_dir.join("x86");
        fs::create_dir_all(&dst_x86)?;
        for entry in fs::read_dir(source_bundle_dir.join("x86"))?.flatten() {
            let p = entry.path();
            if p.is_file() {
                fs::copy(&p, dst_x86.join(p.file_name().unwrap()))?;
            }
        }
    }

    // Write version file
    let _ = fs::write(target_dir.join("version.txt"), format!("{tag}\n"));

    let bundle = inspect_candidate_dir(&target_dir)
        .context("Extracted archive did not contain valid VKD3D-Proton DLLs")?;

    log::info!(
        "VKD3D-Proton {} successfully staged at {}",
        tag,
        target_dir.display()
    );
    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_inspect_candidate_dir_standard_layout() {
        let dir = tempdir().unwrap();
        let x64_dir = dir.path().join("x64");
        let x86_dir = dir.path().join("x86");
        fs::create_dir_all(&x64_dir).unwrap();
        fs::create_dir_all(&x86_dir).unwrap();

        fs::write(x64_dir.join("d3d12.dll"), "dummy-x64").unwrap();
        fs::write(x64_dir.join("d3d12core.dll"), "dummy-core-x64").unwrap();
        fs::write(x86_dir.join("d3d12.dll"), "dummy-x86").unwrap();
        fs::write(dir.path().join("version.txt"), "v3.0.1\n").unwrap();

        let bundle = inspect_candidate_dir(dir.path()).expect("Should find standard layout");
        assert_eq!(bundle.root, dir.path());
        assert_eq!(bundle.version.as_deref(), Some("v3.0.1"));
        assert!(bundle.x64_d3d12core.is_some());
        assert!(bundle.x86_d3d12.is_some());
        assert!(bundle.x86_d3d12core.is_none());
    }

    #[test]
    fn test_inspect_candidate_dir_flat_layout() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("d3d12.dll"), "dummy-flat").unwrap();

        let bundle = inspect_candidate_dir(dir.path()).expect("Should find flat layout");
        assert_eq!(bundle.root, dir.path());
        assert!(bundle.x64_d3d12.is_file());
        assert!(bundle.x64_d3d12core.is_none());
        assert!(bundle.x86_d3d12.is_none());
    }

    #[test]
    fn test_stage_vkd3d_proton_into_prefix() {
        let src_dir = tempdir().unwrap();
        let x64_dir = src_dir.path().join("x64");
        fs::create_dir_all(&x64_dir).unwrap();
        fs::write(x64_dir.join("d3d12.dll"), "x64-d3d12-content").unwrap();
        fs::write(x64_dir.join("d3d12core.dll"), "x64-d3d12core-content").unwrap();

        let bundle = inspect_candidate_dir(src_dir.path()).unwrap();

        let pfx_dir = tempdir().unwrap();
        let sys32 = pfx_dir.path().join("drive_c/windows/system32");
        fs::create_dir_all(&sys32).unwrap();

        let staged = stage_vkd3d_proton_into_prefix(&bundle, pfx_dir.path()).unwrap();
        assert_eq!(staged, 2);

        assert!(sys32.join("d3d12.dll").is_file());
        assert!(sys32.join("d3d12core.dll").is_file());
        assert_eq!(
            fs::read_to_string(sys32.join("d3d12.dll")).unwrap(),
            "x64-d3d12-content"
        );
    }

    #[test]
    fn test_find_vkd3d_proton_env_override() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("d3d12.dll"), "custom-env-d3d12").unwrap();

        std::env::set_var("VKD3D_PROTON_PATH", dir.path().to_str().unwrap());
        let found = find_vkd3d_proton();
        assert!(found.is_some());
        assert_eq!(found.unwrap().root, dir.path());
        std::env::remove_var("VKD3D_PROTON_PATH");
    }
}
