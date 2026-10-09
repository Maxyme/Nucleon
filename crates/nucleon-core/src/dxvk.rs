#![forbid(unsafe_code)]

use crate::paths;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_DXVK_VERSION: &str = "v2.4.1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DxvkBundle {
    pub root: PathBuf,
    pub x64_d3d11: Option<PathBuf>,
    pub x64_d3d10core: Option<PathBuf>,
    pub x64_d3d9: Option<PathBuf>,
    pub x64_dxgi: Option<PathBuf>,
    pub x86_d3d11: Option<PathBuf>,
    pub x86_d3d10core: Option<PathBuf>,
    pub x86_d3d9: Option<PathBuf>,
    pub x86_dxgi: Option<PathBuf>,
    pub version: Option<String>,
}

impl DxvkBundle {
    pub fn has_d3d11(&self) -> bool {
        self.x64_d3d11.is_some() || self.x86_d3d11.is_some()
    }

    pub fn has_d3d9(&self) -> bool {
        self.x64_d3d9.is_some() || self.x86_d3d9.is_some()
    }

    pub fn dll_count(&self) -> usize {
        [
            &self.x64_d3d11,
            &self.x64_d3d10core,
            &self.x64_d3d9,
            &self.x64_dxgi,
            &self.x86_d3d11,
            &self.x86_d3d10core,
            &self.x86_d3d9,
            &self.x86_dxgi,
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

fn check_dll(dir: &Path, name: &str) -> Option<PathBuf> {
    let p = dir.join(name);
    if p.is_file() {
        Some(p)
    } else {
        None
    }
}

/// Inspects a directory to check if it contains a valid DXVK installation.
/// Accepts either the standard release hierarchy (x64/d3d11.dll, x32/d3d11.dll or x86/d3d11.dll),
/// flat directory (d3d11.dll), or an immediate subdirectory.
pub fn inspect_candidate_dir(dir: &Path) -> Option<DxvkBundle> {
    if !dir.is_dir() {
        return None;
    }

    // 1. Check standard release layout: <dir>/x64 and <dir>/x32 (or x86)
    let x64_dir = dir.join("x64");
    let x86_dir = if dir.join("x32").is_dir() {
        dir.join("x32")
    } else {
        dir.join("x86")
    };

    let has_x64 = x64_dir.is_dir();
    let has_x86 = x86_dir.is_dir();

    if has_x64 || has_x86 {
        let x64_d3d11 = check_dll(&x64_dir, "d3d11.dll");
        let x64_d3d10core = check_dll(&x64_dir, "d3d10core.dll");
        let x64_d3d9 = check_dll(&x64_dir, "d3d9.dll");
        let x64_dxgi = check_dll(&x64_dir, "dxgi.dll");

        let x86_d3d11 = check_dll(&x86_dir, "d3d11.dll");
        let x86_d3d10core = check_dll(&x86_dir, "d3d10core.dll");
        let x86_d3d9 = check_dll(&x86_dir, "d3d9.dll");
        let x86_dxgi = check_dll(&x86_dir, "dxgi.dll");

        if x64_d3d11.is_some()
            || x64_d3d9.is_some()
            || x86_d3d11.is_some()
            || x86_d3d9.is_some()
            || x64_dxgi.is_some()
        {
            return Some(DxvkBundle {
                root: dir.to_path_buf(),
                x64_d3d11,
                x64_d3d10core,
                x64_d3d9,
                x64_dxgi,
                x86_d3d11,
                x86_d3d10core,
                x86_d3d9,
                x86_dxgi,
                version: read_version_file(dir),
            });
        }
    }

    // 2. Check flat directory: <dir>/d3d11.dll or <dir>/d3d9.dll
    let flat_d3d11 = check_dll(dir, "d3d11.dll");
    let flat_d3d10core = check_dll(dir, "d3d10core.dll");
    let flat_d3d9 = check_dll(dir, "d3d9.dll");
    let flat_dxgi = check_dll(dir, "dxgi.dll");

    if flat_d3d11.is_some() || flat_d3d9.is_some() || flat_dxgi.is_some() {
        return Some(DxvkBundle {
            root: dir.to_path_buf(),
            x64_d3d11: flat_d3d11,
            x64_d3d10core: flat_d3d10core,
            x64_d3d9: flat_d3d9,
            x64_dxgi: flat_dxgi,
            x86_d3d11: None,
            x86_d3d10core: None,
            x86_d3d9: None,
            x86_dxgi: None,
            version: read_version_file(dir),
        });
    }

    // 3. Check immediate subdirectories (e.g. heroic/tools/dxvk-macOS/<version>/)
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

pub fn custom_dxvk_path_file() -> PathBuf {
    paths::support_dir().join("dxvk_path.txt")
}

/// Locates a DXVK bundle from environment, configured custom path,
/// Nucleon support directory, Heroic tools, or well-known system/Proton paths.
pub fn find_dxvk() -> Option<DxvkBundle> {
    // 1. Explicit environment variable: DXVK_PATH
    if let Ok(p) = std::env::var("DXVK_PATH") {
        let path = PathBuf::from(p);
        if let Some(bundle) = inspect_candidate_dir(&path) {
            return Some(bundle);
        }
    }

    // 2. Custom path stored via `nucleon dxvk set-path <path>`
    let custom_file = custom_dxvk_path_file();
    if custom_file.is_file() {
        if let Ok(content) = fs::read_to_string(&custom_file) {
            let p = PathBuf::from(content.trim());
            if let Some(bundle) = inspect_candidate_dir(&p) {
                return Some(bundle);
            }
        }
    }

    // 3. Nucleon managed directory: ~/Library/Application Support/nucleon/dxvk
    let nucleon_dir = paths::dxvk_dir();
    if let Some(bundle) = inspect_candidate_dir(&nucleon_dir) {
        return Some(bundle);
    }

    // 4. Heroic tools directory: ~/Library/Application Support/heroic/tools/dxvk-macOS
    let heroic_dxvk_dir =
        paths::home_dir().join("Library/Application Support/heroic/tools/dxvk-macOS");
    if let Some(bundle) = inspect_candidate_dir(&heroic_dxvk_dir) {
        return Some(bundle);
    }

    // 5. Well-known system directories
    let candidates = [
        PathBuf::from("/opt/homebrew/share/dxvk"),
        PathBuf::from("/usr/local/share/dxvk"),
        paths::home_dir().join(".local/share/dxvk"),
    ];
    for cand in &candidates {
        if let Some(bundle) = inspect_candidate_dir(cand) {
            return Some(bundle);
        }
    }

    // 6. Scan Steam Proton installs for dxvk if present
    let steam_roots = [
        paths::home_dir().join("Library/Application Support/Steam/steamapps/common"),
        paths::home_dir().join(".local/share/Steam/steamapps/common"),
    ];
    for root in &steam_roots {
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten() {
                let p = entry.path();
                let name = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if name.contains("proton") || name.contains("dxvk") {
                    if let Some(bundle) = inspect_candidate_dir(&p) {
                        return Some(bundle);
                    }
                }
            }
        }
    }

    None
}

pub fn is_dxvk_installed() -> bool {
    find_dxvk().is_some()
}

pub fn set_custom_dxvk_path(path: &Path) -> Result<DxvkBundle> {
    let resolved = if path.is_relative() {
        std::env::current_dir()?.join(path)
    } else {
        path.to_path_buf()
    };

    let bundle = inspect_candidate_dir(&resolved).ok_or_else(|| {
        anyhow::anyhow!(
            "Invalid DXVK installation at {}. Expected x64/d3d11.dll, x32/d3d11.dll, or d3d11.dll",
            resolved.display()
        )
    })?;

    fs::write(
        custom_dxvk_path_file(),
        bundle.root.to_string_lossy().as_bytes(),
    )
    .with_context(|| format!("Failed to write {}", custom_dxvk_path_file().display()))?;

    Ok(bundle)
}

pub fn clear_custom_dxvk_path() -> Result<()> {
    let custom_file = custom_dxvk_path_file();
    if custom_file.exists() {
        fs::remove_file(&custom_file)?;
    }
    Ok(())
}

/// Stages DXVK DLLs into a Wine prefix's system32 and syswow64 directories.
pub fn stage_dxvk_into_prefix(bundle: &DxvkBundle, prefix_dir: &Path) -> Result<usize> {
    let sys32 = prefix_dir.join("drive_c/windows/system32");
    let syswow64 = prefix_dir.join("drive_c/windows/syswow64");
    let is_wow64 = syswow64.is_dir();

    let mut staged_count = 0;

    let target_64 = &sys32;
    let target_32 = if is_wow64 { &syswow64 } else { &sys32 };

    let files_64 = [
        ("d3d11.dll", &bundle.x64_d3d11),
        ("d3d10core.dll", &bundle.x64_d3d10core),
        ("d3d9.dll", &bundle.x64_d3d9),
        ("dxgi.dll", &bundle.x64_dxgi),
    ];

    for (name, opt_path) in files_64 {
        if let Some(src) = opt_path {
            if target_64.is_dir() {
                let dst = target_64.join(name);
                if dst.exists() || dst.is_symlink() {
                    let _ = fs::remove_file(&dst);
                }
                fs::copy(src, &dst).with_context(|| {
                    format!("Failed to stage 64-bit DXVK {name} to {}", dst.display())
                })?;
                staged_count += 1;
            }
        }
    }

    let files_32 = [
        ("d3d11.dll", &bundle.x86_d3d11),
        ("d3d10core.dll", &bundle.x86_d3d10core),
        ("d3d9.dll", &bundle.x86_d3d9),
        ("dxgi.dll", &bundle.x86_dxgi),
    ];

    for (name, opt_path) in files_32 {
        if let Some(src) = opt_path {
            if target_32.is_dir() {
                let dst = target_32.join(name);
                if dst.exists() || dst.is_symlink() {
                    let _ = fs::remove_file(&dst);
                }
                fs::copy(src, &dst).with_context(|| {
                    format!("Failed to stage 32-bit DXVK {name} to {}", dst.display())
                })?;
                staged_count += 1;
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

/// Unstages DXVK DLLs from a Wine prefix's syswow64 and system32 directories.
///
/// If a `runner_dir` is provided and contains Wine's builtin DLLs,
/// it restores the builtin DLLs into the prefix.
pub fn unstage_dxvk_from_prefix(prefix_dir: &Path, runner_dir: Option<&Path>) -> Result<usize> {
    let pfx = prefix_dir.join("drive_c/windows");
    let mut count = 0;
    let dll_names = ["d3d11.dll", "d3d10core.dll", "d3d9.dll", "dxgi.dll"];

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
            "Unstaged/restored {count} DXVK DLL(s) in Wine prefix at {}",
            prefix_dir.display()
        );
    }

    Ok(count)
}

/// Fetches official DXVK release archive (.tar.gz) from GitHub and unpacks it.
pub fn fetch_dxvk(version: Option<&str>, dest_dir: Option<&Path>) -> Result<DxvkBundle> {
    let raw = version.unwrap_or(DEFAULT_DXVK_VERSION);
    let tag = if raw.starts_with('v') {
        raw.to_string()
    } else {
        format!("v{raw}")
    };
    let ver_num = tag.trim_start_matches('v');

    let archive_name = format!("dxvk-{ver_num}.tar.gz");
    let url = format!("https://github.com/doitsujin/dxvk/releases/download/{tag}/{archive_name}");

    let target_dir = match dest_dir {
        Some(p) => p.to_path_buf(),
        None => paths::dxvk_dir(),
    };
    fs::create_dir_all(&target_dir)?;

    let temp_dir = tempfile::tempdir()?;
    let extract_dir = temp_dir.path().join("extracted");
    fs::create_dir_all(&extract_dir)?;

    log::info!("Fetching DXVK {tag} from {url}...");

    let resp = ureq::get(&url)
        .set("User-Agent", "nucleon-fetcher")
        .call()
        .with_context(|| format!("Failed to download DXVK from {url}"))?;

    let reader = resp.into_reader();
    let gz = flate2::read::GzDecoder::new(reader);
    let mut tar_archive = tar::Archive::new(gz);
    tar_archive
        .unpack(&extract_dir)
        .with_context(|| "Failed to unpack DXVK tar.gz archive")?;

    let mut source_bundle_dir = extract_dir.clone();
    if let Ok(entries) = fs::read_dir(&extract_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() && (p.join("x64").is_dir() || p.join("d3d11.dll").is_file()) {
                source_bundle_dir = p;
                break;
            }
        }
    }

    // Copy extracted hierarchy into target_dir
    for sub in &["x64", "x32"] {
        let src_sub = source_bundle_dir.join(sub);
        if src_sub.is_dir() {
            let dst_sub = target_dir.join(sub);
            fs::create_dir_all(&dst_sub)?;
            for entry in fs::read_dir(src_sub)?.flatten() {
                let p = entry.path();
                if p.is_file() {
                    if let Some(name) = p.file_name() {
                        fs::copy(&p, dst_sub.join(name))?;
                    }
                }
            }
        }
    }

    let _ = fs::write(target_dir.join("version.txt"), format!("{tag}\n"));

    let bundle = inspect_candidate_dir(&target_dir)
        .context("Extracted archive did not contain valid DXVK DLLs")?;

    log::info!(
        "DXVK {} successfully staged at {}",
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
        let x64 = dir.path().join("x64");
        let x32 = dir.path().join("x32");
        fs::create_dir_all(&x64).unwrap();
        fs::create_dir_all(&x32).unwrap();

        fs::write(x64.join("d3d11.dll"), b"fake dxvk d3d11 64").unwrap();
        fs::write(x64.join("dxgi.dll"), b"fake dxvk dxgi 64").unwrap();
        fs::write(x32.join("d3d11.dll"), b"fake dxvk d3d11 32").unwrap();
        fs::write(dir.path().join("version.txt"), "v2.4.1\n").unwrap();

        let bundle = inspect_candidate_dir(dir.path()).expect("Should detect DXVK bundle");
        assert_eq!(bundle.version.as_deref(), Some("v2.4.1"));
        assert!(bundle.has_d3d11());
        assert!(!bundle.has_d3d9());
        assert_eq!(bundle.dll_count(), 3);
    }

    #[test]
    fn test_inspect_candidate_dir_flat_layout() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("d3d11.dll"), b"flat d3d11").unwrap();

        let bundle = inspect_candidate_dir(dir.path()).expect("Should detect flat DXVK");
        assert!(bundle.has_d3d11());
        assert_eq!(bundle.dll_count(), 1);
    }

    #[test]
    fn test_stage_and_unstage_dxvk_prefix() {
        let bundle_dir = tempdir().unwrap();
        let x64 = bundle_dir.path().join("x64");
        let x32 = bundle_dir.path().join("x32");
        fs::create_dir_all(&x64).unwrap();
        fs::create_dir_all(&x32).unwrap();
        fs::write(x64.join("d3d11.dll"), b"dxvk d3d11 64").unwrap();
        fs::write(x32.join("d3d11.dll"), b"dxvk d3d11 32").unwrap();

        let bundle = inspect_candidate_dir(bundle_dir.path()).unwrap();

        let pfx_dir = tempdir().unwrap();
        let sys32 = pfx_dir.path().join("drive_c/windows/system32");
        let syswow64 = pfx_dir.path().join("drive_c/windows/syswow64");
        fs::create_dir_all(&sys32).unwrap();
        fs::create_dir_all(&syswow64).unwrap();

        let staged = stage_dxvk_into_prefix(&bundle, pfx_dir.path()).unwrap();
        assert_eq!(staged, 2);
        assert_eq!(fs::read(sys32.join("d3d11.dll")).unwrap(), b"dxvk d3d11 64");
        assert_eq!(
            fs::read(syswow64.join("d3d11.dll")).unwrap(),
            b"dxvk d3d11 32"
        );

        // Test unstage without runner
        let unstaged = unstage_dxvk_from_prefix(pfx_dir.path(), None).unwrap();
        assert_eq!(unstaged, 2);
        assert!(!sys32.join("d3d11.dll").exists());
        assert!(!syswow64.join("d3d11.dll").exists());
    }
}
