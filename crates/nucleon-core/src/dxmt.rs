#![forbid(unsafe_code)]

use crate::paths;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_DXMT_VERSION: &str = "v0.80";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DxmtBundle {
    pub root: PathBuf,
    pub x64_d3d11: Option<PathBuf>,
    pub x64_d3d10core: Option<PathBuf>,
    pub x64_dxgi: Option<PathBuf>,
    pub x64_winemetal: Option<PathBuf>,
    pub x64_winemetal_so: Option<PathBuf>,
    pub x86_d3d11: Option<PathBuf>,
    pub x86_d3d10core: Option<PathBuf>,
    pub x86_dxgi: Option<PathBuf>,
    pub x86_winemetal: Option<PathBuf>,
    pub version: Option<String>,
}

impl DxmtBundle {
    pub fn has_d3d11(&self) -> bool {
        self.x64_d3d11.is_some() || self.x86_d3d11.is_some()
    }

    pub fn has_d3d10(&self) -> bool {
        self.x64_d3d10core.is_some() || self.x86_d3d10core.is_some()
    }

    /// Returns the DirectX version range supported by DXMT (translating D3D11/D3D10 directly to Metal).
    pub fn supported_dx_range(&self) -> &'static str {
        if self.has_d3d10() && self.has_d3d11() {
            "DX10-DX11"
        } else if self.has_d3d11() {
            "DX11"
        } else if self.has_d3d10() {
            "DX10"
        } else {
            "DX11"
        }
    }

    pub fn dll_count(&self) -> usize {
        [
            &self.x64_d3d11,
            &self.x64_d3d10core,
            &self.x64_dxgi,
            &self.x64_winemetal,
            &self.x86_d3d11,
            &self.x86_d3d10core,
            &self.x86_dxgi,
            &self.x86_winemetal,
        ]
        .iter()
        .filter(|opt| opt.is_some())
        .count()
    }
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

fn check_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let p = dir.join(name);
    if p.is_file() {
        Some(p)
    } else {
        None
    }
}

/// Inspects a directory to check if it contains a valid DXMT installation.
/// Supports standard DXMT release hierarchy (x86_64-windows / i386-windows / x86_64-unix),
/// x64 / x32 layouts, flat directories, or immediate subdirectories.
pub fn inspect_candidate_dir(dir: &Path) -> Option<DxmtBundle> {
    if !dir.is_dir() {
        return None;
    }

    // 1. Check official DXMT layout: x86_64-windows, i386-windows, and x86_64-unix
    let x64_win = if dir.join("x86_64-windows").is_dir() {
        dir.join("x86_64-windows")
    } else {
        dir.join("x64")
    };

    let x86_win = if dir.join("i386-windows").is_dir() {
        dir.join("i386-windows")
    } else if dir.join("x86-windows").is_dir() {
        dir.join("x86-windows")
    } else if dir.join("x32").is_dir() {
        dir.join("x32")
    } else {
        dir.join("x86")
    };

    let x64_unix = if dir.join("x86_64-unix").is_dir() {
        dir.join("x86_64-unix")
    } else {
        dir.to_path_buf()
    };

    if x64_win.is_dir() || x86_win.is_dir() {
        let x64_d3d11 = check_file(&x64_win, "d3d11.dll");
        let x64_d3d10core = check_file(&x64_win, "d3d10core.dll");
        let x64_dxgi = check_file(&x64_win, "dxgi.dll");
        let x64_winemetal = check_file(&x64_win, "winemetal.dll");
        let x64_winemetal_so = check_file(&x64_unix, "winemetal.so");

        let x86_d3d11 = check_file(&x86_win, "d3d11.dll");
        let x86_d3d10core = check_file(&x86_win, "d3d10core.dll");
        let x86_dxgi = check_file(&x86_win, "dxgi.dll");
        let x86_winemetal = check_file(&x86_win, "winemetal.dll");

        if x64_d3d11.is_some() || x86_d3d11.is_some() || x64_winemetal_so.is_some() {
            return Some(DxmtBundle {
                root: dir.to_path_buf(),
                x64_d3d11,
                x64_d3d10core,
                x64_dxgi,
                x64_winemetal,
                x64_winemetal_so,
                x86_d3d11,
                x86_d3d10core,
                x86_dxgi,
                x86_winemetal,
                version: read_version_file(dir),
            });
        }
    }

    // 2. Check flat directory: <dir>/d3d11.dll or <dir>/winemetal.so
    let flat_d3d11 = check_file(dir, "d3d11.dll");
    let flat_d3d10core = check_file(dir, "d3d10core.dll");
    let flat_dxgi = check_file(dir, "dxgi.dll");
    let flat_winemetal = check_file(dir, "winemetal.dll");
    let flat_winemetal_so = check_file(dir, "winemetal.so");

    if flat_d3d11.is_some() || flat_winemetal_so.is_some() {
        return Some(DxmtBundle {
            root: dir.to_path_buf(),
            x64_d3d11: flat_d3d11,
            x64_d3d10core: flat_d3d10core,
            x64_dxgi: flat_dxgi,
            x64_winemetal: flat_winemetal,
            x64_winemetal_so: flat_winemetal_so,
            x86_d3d11: None,
            x86_d3d10core: None,
            x86_dxgi: None,
            x86_winemetal: None,
            version: read_version_file(dir),
        });
    }

    // 3. Check immediate subdirectories (e.g. heroic/tools/dxmt/dxmt-v0.80-builtin)
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if let Some(bundle) = inspect_candidate_dir(&p) {
                    return Some(bundle);
                }
            }
        }
    }

    None
}

pub fn custom_dxmt_path_file() -> PathBuf {
    paths::support_dir().join("dxmt_path.txt")
}

/// Locates a DXMT bundle from environment, configured custom path,
/// Nucleon support directory, Heroic tools, active Wine runtime, or well-known system paths.
pub fn find_dxmt() -> Option<DxmtBundle> {
    // 1. Explicit environment variable: DXMT_PATH
    if let Ok(p) = std::env::var("DXMT_PATH") {
        let path = PathBuf::from(p);
        if let Some(bundle) = inspect_candidate_dir(&path) {
            return Some(bundle);
        }
    }

    // 2. Custom path stored via `nucleon dxmt set-path <path>`
    let custom_file = custom_dxmt_path_file();
    if custom_file.is_file() {
        if let Ok(content) = fs::read_to_string(&custom_file) {
            let p = PathBuf::from(content.trim());
            if let Some(bundle) = inspect_candidate_dir(&p) {
                return Some(bundle);
            }
        }
    }

    // 3. Nucleon managed directory: ~/Library/Application Support/nucleon/dxmt
    let nucleon_dir = paths::dxmt_dir();
    if let Some(bundle) = inspect_candidate_dir(&nucleon_dir) {
        return Some(bundle);
    }

    // 4. Heroic tools directory: ~/Library/Application Support/heroic/tools/dxmt
    let heroic_dxmt_dir = paths::home_dir().join("Library/Application Support/heroic/tools/dxmt");
    if let Some(bundle) = inspect_candidate_dir(&heroic_dxmt_dir) {
        return Some(bundle);
    }

    // 5. Active Wine runtime if it contains DXMT built-in (e.g. Wine-11.18-DXMT)
    if let Some(active_wine) = crate::wine::get_active_wine_runtime() {
        let wine_unix = active_wine.root.join("lib/wine/x86_64-unix/winemetal.so");
        let wine_win = active_wine.root.join("lib/wine/x86_64-windows/d3d11.dll");
        if wine_unix.is_file() || wine_win.is_file() {
            if let Some(bundle) = inspect_candidate_dir(&active_wine.root) {
                return Some(bundle);
            }
        }
    }

    // 6. Well-known system directories
    let candidates = [
        PathBuf::from("/opt/homebrew/share/dxmt"),
        PathBuf::from("/usr/local/share/dxmt"),
        paths::home_dir().join(".local/share/dxmt"),
    ];
    for cand in &candidates {
        if let Some(bundle) = inspect_candidate_dir(cand) {
            return Some(bundle);
        }
    }

    None
}

pub fn is_dxmt_installed() -> bool {
    find_dxmt().is_some()
}

pub fn set_custom_dxmt_path(path: &Path) -> Result<DxmtBundle> {
    let resolved = if path.is_relative() {
        std::env::current_dir()?.join(path)
    } else {
        path.to_path_buf()
    };

    let bundle = inspect_candidate_dir(&resolved).ok_or_else(|| {
        anyhow::anyhow!(
            "Invalid DXMT installation at {}. Expected x86_64-windows/d3d11.dll, winemetal.so, or d3d11.dll",
            resolved.display()
        )
    })?;

    fs::create_dir_all(paths::support_dir())?;
    fs::write(
        custom_dxmt_path_file(),
        resolved.to_string_lossy().as_bytes(),
    )?;
    log::info!("Registered custom DXMT path: {}", resolved.display());
    Ok(bundle)
}

pub fn clear_custom_dxmt_path() -> Result<()> {
    let f = custom_dxmt_path_file();
    if f.exists() {
        fs::remove_file(&f)?;
        log::info!("Cleared custom DXMT path");
    }
    Ok(())
}

/// Stages DXMT DLLs into a Wine prefix's system32 and syswow64 directories.
/// If `winemetal.so` is present and `runner_dir` is provided, ensures `winemetal.so`
/// is available in the runner's unix library search path.
pub fn stage_dxmt_into_prefix(
    bundle: &DxmtBundle,
    prefix_dir: &Path,
    runner_dir: Option<&Path>,
) -> Result<usize> {
    let pfx = prefix_dir.join("drive_c/windows");
    let target_64 = pfx.join("system32");
    let target_32 = pfx.join("syswow64");

    fs::create_dir_all(&target_64)?;
    fs::create_dir_all(&target_32)?;

    let mut staged_count = 0;

    let files_64 = [
        ("d3d11.dll", bundle.x64_d3d11.as_deref()),
        ("d3d10core.dll", bundle.x64_d3d10core.as_deref()),
        ("dxgi.dll", bundle.x64_dxgi.as_deref()),
        ("winemetal.dll", bundle.x64_winemetal.as_deref()),
    ];

    for (name, opt_path) in files_64 {
        if let Some(src) = opt_path {
            let dst = target_64.join(name);
            if dst.exists() || dst.is_symlink() {
                let _ = fs::remove_file(&dst);
            }
            fs::copy(src, &dst).with_context(|| {
                format!("Failed to stage 64-bit DXMT {name} to {}", dst.display())
            })?;
            staged_count += 1;
        }
    }

    let files_32 = [
        ("d3d11.dll", bundle.x86_d3d11.as_deref()),
        ("d3d10core.dll", bundle.x86_d3d10core.as_deref()),
        ("dxgi.dll", bundle.x86_dxgi.as_deref()),
        ("winemetal.dll", bundle.x86_winemetal.as_deref()),
    ];

    for (name, opt_path) in files_32 {
        if let Some(src) = opt_path {
            if target_32.is_dir() {
                let dst = target_32.join(name);
                if dst.exists() || dst.is_symlink() {
                    let _ = fs::remove_file(&dst);
                }
                fs::copy(src, &dst).with_context(|| {
                    format!("Failed to stage 32-bit DXMT {name} to {}", dst.display())
                })?;
                staged_count += 1;
            }
        }
    }

    // Ensure winemetal.so is present in runner's lib/wine/x86_64-unix if missing
    if let (Some(so_src), Some(r_dir)) = (bundle.x64_winemetal_so.as_deref(), runner_dir) {
        let unix_lib_dir = r_dir.join("lib/wine/x86_64-unix");
        if unix_lib_dir.is_dir() {
            let unix_dst = unix_lib_dir.join("winemetal.so");
            if !unix_dst.exists() {
                let _ = fs::copy(so_src, &unix_dst);
            }
        }
    }

    Ok(staged_count)
}

fn restore_dll_in_dir(
    dst_dir: &Path,
    dll_name: &str,
    runner_dir: Option<&Path>,
    subdirs: &[&str],
) -> usize {
    let dst_file = dst_dir.join(dll_name);
    if !dst_file.exists() && !dst_file.is_symlink() {
        return 0;
    }

    let builtin = runner_dir.and_then(|r| {
        subdirs
            .iter()
            .map(|sub| r.join(sub).join(dll_name))
            .find(|p| p.is_file())
    });

    if let Some(src) = builtin {
        let need_copy = fs::metadata(&dst_file)
            .and_then(|d| {
                fs::metadata(&src).map(|s| {
                    if d.len() != s.len() {
                        true
                    } else {
                        fs::read(&dst_file).ok() != fs::read(&src).ok()
                    }
                })
            })
            .unwrap_or(true);
        if need_copy {
            let _ = fs::remove_file(&dst_file);
            if fs::copy(&src, &dst_file).is_ok() {
                return 1;
            }
        }
    } else if (dst_file.exists() || dst_file.is_symlink()) && fs::remove_file(&dst_file).is_ok() {
        return 1;
    }

    0
}

/// Unstages DXMT DLLs from a Wine prefix's syswow64 and system32 directories.
pub fn unstage_dxmt_from_prefix(prefix_dir: &Path, runner_dir: Option<&Path>) -> Result<usize> {
    let pfx = prefix_dir.join("drive_c/windows");
    let mut count = 0;
    let dll_names = ["d3d11.dll", "d3d10core.dll", "dxgi.dll", "winemetal.dll"];

    for name in dll_names {
        count += restore_dll_in_dir(
            &pfx.join("syswow64"),
            name,
            runner_dir,
            &["lib/wine/i386-windows", "lib/wine/x86-windows"],
        );
        count += restore_dll_in_dir(
            &pfx.join("system32"),
            name,
            runner_dir,
            &[
                "lib/wine/x86_64-windows",
                "lib64/wine/x86_64-windows",
                "lib/wine/i386-windows",
                "lib/wine/x86-windows",
            ],
        );
    }

    if count > 0 {
        log::info!(
            "Unstaged/restored {count} DXMT DLL(s) in Wine prefix at {}",
            prefix_dir.display()
        );
    }

    Ok(count)
}

/// Fetches official DXMT release archive (.tar.gz) from GitHub and unpacks it.
pub fn fetch_dxmt(version: Option<&str>, dest_dir: Option<&Path>) -> Result<DxmtBundle> {
    let raw = version.unwrap_or(DEFAULT_DXMT_VERSION);
    let tag = if raw.starts_with('v') {
        raw.to_string()
    } else {
        format!("v{raw}")
    };

    let archive_name = format!("dxmt-{tag}-builtin.tar.gz");
    let url = format!("https://github.com/3Shain/dxmt/releases/download/{tag}/{archive_name}");

    let target_dir = match dest_dir {
        Some(p) => p.to_path_buf(),
        None => paths::dxmt_dir(),
    };
    fs::create_dir_all(&target_dir)?;

    let temp_dir = tempfile::tempdir()?;
    let extract_dir = temp_dir.path().join("extracted");
    fs::create_dir_all(&extract_dir)?;

    log::info!("Fetching DXMT {tag} from {url}...");

    let resp = ureq::get(&url)
        .set("User-Agent", "nucleon-fetcher")
        .call()
        .with_context(|| format!("Failed to download DXMT from {url}"))?;

    let reader = resp.into_reader();
    let gz = flate2::read::GzDecoder::new(reader);
    let mut tar_archive = tar::Archive::new(gz);
    tar_archive
        .unpack(&extract_dir)
        .with_context(|| "Failed to unpack DXMT tar.gz archive")?;

    let mut source_bundle_dir = extract_dir.clone();
    if let Ok(entries) = fs::read_dir(&extract_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir()
                && (p.join("x86_64-windows").is_dir()
                    || p.join("x64").is_dir()
                    || p.join("d3d11.dll").is_file())
            {
                source_bundle_dir = p;
                break;
            }
        }
    }

    // Copy extracted hierarchy into target_dir
    let subdirs = [
        "x86_64-windows",
        "i386-windows",
        "x86_64-unix",
        "x64",
        "x32",
    ];
    for sub in &subdirs {
        let src_sub = source_bundle_dir.join(sub);
        if src_sub.is_dir() {
            let dst_sub = target_dir.join(sub);
            fs::create_dir_all(&dst_sub)?;
            for file_entry in fs::read_dir(&src_sub)?.flatten() {
                let fp = file_entry.path();
                if fp.is_file() {
                    if let Some(name) = fp.file_name() {
                        fs::copy(&fp, dst_sub.join(name))?;
                    }
                }
            }
        }
    }

    // Also copy any flat DLLs or so files if present
    for entry in fs::read_dir(&source_bundle_dir)?.flatten() {
        let fp = entry.path();
        if fp.is_file() {
            if let Some(name) = fp.file_name() {
                let _ = fs::copy(&fp, target_dir.join(name));
            }
        }
    }

    let _ = fs::write(target_dir.join("version.txt"), format!("{tag}\n"));

    inspect_candidate_dir(&target_dir).ok_or_else(|| {
        anyhow::anyhow!(
            "Failed to validate DXMT installation after extracting to {}",
            target_dir.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_inspect_candidate_dir_standard_layout() {
        let dir = tempdir().unwrap();
        let x64 = dir.path().join("x86_64-windows");
        let x32 = dir.path().join("i386-windows");
        let unix = dir.path().join("x86_64-unix");
        fs::create_dir_all(&x64).unwrap();
        fs::create_dir_all(&x32).unwrap();
        fs::create_dir_all(&unix).unwrap();

        fs::write(x64.join("d3d11.dll"), b"fake dxmt d3d11 64").unwrap();
        fs::write(x64.join("winemetal.dll"), b"fake dxmt winemetal 64").unwrap();
        fs::write(x32.join("d3d11.dll"), b"fake dxmt d3d11 32").unwrap();
        fs::write(unix.join("winemetal.so"), b"fake dxmt winemetal unix").unwrap();
        fs::write(dir.path().join("version.txt"), "v0.80\n").unwrap();

        let bundle = inspect_candidate_dir(dir.path()).expect("Should detect DXMT bundle");
        assert_eq!(bundle.version.as_deref(), Some("v0.80"));
        assert!(bundle.has_d3d11());
        assert_eq!(bundle.supported_dx_range(), "DX11");
        assert_eq!(bundle.dll_count(), 3);
    }

    #[test]
    fn test_inspect_candidate_dir_flat_layout() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("d3d11.dll"), b"flat d3d11").unwrap();

        let bundle = inspect_candidate_dir(dir.path()).expect("Should detect flat DXMT");
        assert!(bundle.has_d3d11());
        assert_eq!(bundle.dll_count(), 1);
    }

    #[test]
    fn test_stage_and_unstage_dxmt_prefix() {
        let bundle_dir = tempdir().unwrap();
        let x64 = bundle_dir.path().join("x86_64-windows");
        let x32 = bundle_dir.path().join("i386-windows");
        fs::create_dir_all(&x64).unwrap();
        fs::create_dir_all(&x32).unwrap();
        fs::write(x64.join("d3d11.dll"), b"dxmt d3d11 64").unwrap();
        fs::write(x32.join("d3d11.dll"), b"dxmt d3d11 32").unwrap();

        let bundle = inspect_candidate_dir(bundle_dir.path()).unwrap();

        let pfx_dir = tempdir().unwrap();
        let sys32 = pfx_dir.path().join("drive_c/windows/system32");
        let syswow64 = pfx_dir.path().join("drive_c/windows/syswow64");
        fs::create_dir_all(&sys32).unwrap();
        fs::create_dir_all(&syswow64).unwrap();

        let staged = stage_dxmt_into_prefix(&bundle, pfx_dir.path(), None).unwrap();
        assert_eq!(staged, 2);
        assert_eq!(fs::read(sys32.join("d3d11.dll")).unwrap(), b"dxmt d3d11 64");
        assert_eq!(
            fs::read(syswow64.join("d3d11.dll")).unwrap(),
            b"dxmt d3d11 32"
        );

        // Test unstage without runner
        let unstaged = unstage_dxmt_from_prefix(pfx_dir.path(), None).unwrap();
        assert_eq!(unstaged, 2);
        assert!(!sys32.join("d3d11.dll").exists());
        assert!(!syswow64.join("d3d11.dll").exists());
    }

    #[test]
    fn test_dxmt_supported_dx_range() {
        let mut bundle = DxmtBundle {
            root: PathBuf::from("/tmp/dxmt"),
            x64_d3d11: Some(PathBuf::from("/tmp/dxmt/x86_64-windows/d3d11.dll")),
            x64_d3d10core: Some(PathBuf::from("/tmp/dxmt/x86_64-windows/d3d10core.dll")),
            x64_dxgi: Some(PathBuf::from("/tmp/dxmt/x86_64-windows/dxgi.dll")),
            x64_winemetal: Some(PathBuf::from("/tmp/dxmt/x86_64-windows/winemetal.dll")),
            x64_winemetal_so: Some(PathBuf::from("/tmp/dxmt/x86_64-unix/winemetal.so")),
            x86_d3d11: None,
            x86_d3d10core: None,
            x86_dxgi: None,
            x86_winemetal: None,
            version: None,
        };
        assert_eq!(bundle.supported_dx_range(), "DX10-DX11");

        bundle.x64_d3d10core = None;
        assert_eq!(bundle.supported_dx_range(), "DX11");
    }
}
