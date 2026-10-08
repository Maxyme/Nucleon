use crate::paths;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const DEFAULT_D7VK_VERSION: &str = "v2.3";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct D7vkBundle {
    pub root: PathBuf,
    pub x86_ddraw: PathBuf,
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

/// Inspects a directory to check if it contains a valid D7VK installation.
/// Accepts either the standard release hierarchy (x32/ddraw.dll), an x86 subfolder, or a flat directory (ddraw.dll).
pub fn inspect_candidate_dir(dir: &Path) -> Option<D7vkBundle> {
    if !dir.is_dir() {
        return None;
    }

    // 1. Check standard release layout: <dir>/x32/ddraw.dll
    let x32_ddraw = dir.join("x32/ddraw.dll");
    if x32_ddraw.is_file() {
        return Some(D7vkBundle {
            root: dir.to_path_buf(),
            x86_ddraw: x32_ddraw,
            version: read_version_file(dir),
        });
    }

    // 2. Check x86 layout: <dir>/x86/ddraw.dll
    let x86_ddraw = dir.join("x86/ddraw.dll");
    if x86_ddraw.is_file() {
        return Some(D7vkBundle {
            root: dir.to_path_buf(),
            x86_ddraw,
            version: read_version_file(dir),
        });
    }

    // 3. Check flat directory: <dir>/ddraw.dll
    let flat_ddraw = dir.join("ddraw.dll");
    if flat_ddraw.is_file() {
        return Some(D7vkBundle {
            root: dir.to_path_buf(),
            x86_ddraw: flat_ddraw,
            version: read_version_file(dir),
        });
    }

    // 4. Check immediate subdirectories (e.g. if extracted archive created <dir>/d7vk-v2.3/)
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let sub_x32 = p.join("x32/ddraw.dll");
                if sub_x32.is_file() {
                    return Some(D7vkBundle {
                        root: p.clone(),
                        x86_ddraw: sub_x32,
                        version: read_version_file(&p).or_else(|| read_version_file(dir)),
                    });
                }
                let sub_flat = p.join("ddraw.dll");
                if sub_flat.is_file() {
                    return Some(D7vkBundle {
                        root: p.clone(),
                        x86_ddraw: sub_flat,
                        version: read_version_file(&p).or_else(|| read_version_file(dir)),
                    });
                }
            }
        }
    }

    None
}

pub fn custom_d7vk_path_file() -> PathBuf {
    paths::support_dir().join("d7vk_path.txt")
}

/// Locates a D7VK bundle from environment, configured custom path,
/// Nucleon support directory, or well-known system/Proton paths.
pub fn find_d7vk() -> Option<D7vkBundle> {
    // 1. Explicit environment variable: D7VK_PATH
    if let Ok(env_path) = std::env::var("D7VK_PATH") {
        let p = PathBuf::from(env_path);
        if let Some(bundle) = inspect_candidate_dir(&p) {
            return Some(bundle);
        }
    }

    // 2. Custom path stored via `nucleon d7vk set-path <path>`
    let custom_file = custom_d7vk_path_file();
    if custom_file.is_file() {
        if let Ok(c) = fs::read_to_string(&custom_file) {
            let p = PathBuf::from(c.trim());
            if let Some(bundle) = inspect_candidate_dir(&p) {
                return Some(bundle);
            }
        }
    }

    // 3. Nucleon managed directory: ~/Library/Application Support/nucleon/d7vk
    let nucleon_dir = paths::d7vk_dir();
    if let Some(bundle) = inspect_candidate_dir(&nucleon_dir) {
        return Some(bundle);
    }

    // 4. Well-known system directories
    let candidates = [
        PathBuf::from("/opt/homebrew/share/d7vk"),
        PathBuf::from("/usr/local/share/d7vk"),
        paths::home_dir().join(".local/share/d7vk"),
    ];
    for cand in &candidates {
        if let Some(bundle) = inspect_candidate_dir(cand) {
            return Some(bundle);
        }
    }

    // 5. Scan Steam Proton installs for d7vk if present
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
                if name.contains("proton") || name.contains("d7vk") {
                    if let Some(bundle) = inspect_candidate_dir(&p) {
                        return Some(bundle);
                    }
                }
            }
        }
    }

    None
}

pub fn is_d7vk_installed() -> bool {
    find_d7vk().is_some()
}

pub fn set_custom_d7vk_path(path: &Path) -> Result<D7vkBundle> {
    let resolved = if path.is_relative() {
        std::env::current_dir()?.join(path)
    } else {
        path.to_path_buf()
    };

    let bundle = inspect_candidate_dir(&resolved).ok_or_else(|| {
        anyhow::anyhow!(
            "Invalid D7VK installation at {}. Expected x32/ddraw.dll, x86/ddraw.dll, or ddraw.dll",
            resolved.display()
        )
    })?;

    fs::write(
        custom_d7vk_path_file(),
        bundle.root.to_string_lossy().as_bytes(),
    )
    .with_context(|| format!("Failed to write {}", custom_d7vk_path_file().display()))?;

    Ok(bundle)
}

pub fn clear_custom_d7vk_path() -> Result<()> {
    let custom_file = custom_d7vk_path_file();
    if custom_file.exists() {
        fs::remove_file(&custom_file)?;
    }
    Ok(())
}

/// Stages D7VK DLL into a Wine prefix's syswow64 (or system32) directory.
pub fn stage_d7vk_into_prefix(bundle: &D7vkBundle, prefix_dir: &Path) -> Result<usize> {
    let sys32 = prefix_dir.join("drive_c/windows/system32");
    let syswow64 = prefix_dir.join("drive_c/windows/syswow64");

    let mut staged_count = 0;

    // DirectDraw / D3D 1-7 games are 32-bit: stage to syswow64 on 64-bit Wine WoW64 prefixes
    if syswow64.is_dir() {
        let dst_wow64 = syswow64.join("ddraw.dll");
        if dst_wow64.exists() || dst_wow64.is_symlink() {
            let _ = fs::remove_file(&dst_wow64);
        }
        fs::copy(&bundle.x86_ddraw, &dst_wow64).with_context(|| {
            format!("Failed to stage D7VK ddraw.dll to {}", dst_wow64.display())
        })?;
        staged_count += 1;
    } else if sys32.is_dir() {
        // Fallback for pure 32-bit Wine prefixes
        let dst_sys32 = sys32.join("ddraw.dll");
        if dst_sys32.exists() || dst_sys32.is_symlink() {
            let _ = fs::remove_file(&dst_sys32);
        }
        fs::copy(&bundle.x86_ddraw, &dst_sys32).with_context(|| {
            format!("Failed to stage D7VK ddraw.dll to {}", dst_sys32.display())
        })?;
        staged_count += 1;
    }

    log::info!(
        "Staged {} D7VK DLL(s) into Wine prefix at {}",
        staged_count,
        prefix_dir.display()
    );

    Ok(staged_count)
}

fn restore_ddraw_in_dir(dst_dir: &Path, runner_dir: Option<&Path>, subdirs: &[&str]) -> usize {
    let mut count = 0;
    let dst_ddraw = dst_dir.join("ddraw.dll");
    let builtin = runner_dir.and_then(|r| {
        subdirs
            .iter()
            .map(|sub| r.join(sub).join("ddraw.dll"))
            .find(|p| p.is_file())
    });

    if let Some(src) = builtin {
        let need_copy = fs::metadata(&dst_ddraw)
            .and_then(|d| {
                fs::metadata(&src).map(|s| {
                    if d.len() != s.len() {
                        true
                    } else {
                        fs::read(&dst_ddraw).ok() != fs::read(&src).ok()
                    }
                })
            })
            .unwrap_or(true);
        if need_copy {
            let _ = fs::remove_file(&dst_ddraw);
            if fs::copy(&src, &dst_ddraw).is_ok() {
                count += 1;
            }
        }
    } else if (dst_ddraw.exists() || dst_ddraw.is_symlink()) && fs::remove_file(&dst_ddraw).is_ok()
    {
        count += 1;
    }

    count
}

/// Unstages D7VK DLLs from a Wine prefix's syswow64 and system32 directories.
///
/// If a `runner_dir` is provided and contains Wine's builtin `ddraw.dll`,
/// it restores the builtin DLL into the prefix.
pub fn unstage_d7vk_from_prefix(prefix_dir: &Path, runner_dir: Option<&Path>) -> Result<usize> {
    let pfx = prefix_dir.join("drive_c/windows");
    let count = restore_ddraw_in_dir(
        &pfx.join("syswow64"),
        runner_dir,
        &["lib/wine/i386-windows", "lib/wine/x86-windows"],
    ) + restore_ddraw_in_dir(
        &pfx.join("system32"),
        runner_dir,
        &[
            "lib/wine/i386-windows",
            "lib/wine/x86-windows",
            "lib/wine/x86_64-windows",
            "lib64/wine/x86_64-windows",
        ],
    );

    if count > 0 {
        log::info!(
            "Unstaged/restored {count} DirectDraw DLL(s) in Wine prefix at {}",
            prefix_dir.display()
        );
    }

    Ok(count)
}

/// Fetches D7VK from official GitHub releases into the target directory.
pub fn fetch_d7vk(version: Option<&str>, dest_dir: Option<&Path>) -> Result<D7vkBundle> {
    let raw = version.unwrap_or(DEFAULT_D7VK_VERSION);
    let tag = if raw.starts_with('v') {
        raw.to_string()
    } else {
        format!("v{raw}")
    };

    let archive_name = format!("d7vk-{tag}.zip");
    let url =
        format!("https://github.com/WinterSnowfall/d7vk/releases/download/{tag}/{archive_name}");

    let target_dir = match dest_dir {
        Some(p) => p.to_path_buf(),
        None => paths::d7vk_dir(),
    };
    fs::create_dir_all(&target_dir)?;

    let temp_dir = tempfile::tempdir()?;
    let archive_path = temp_dir.path().join(&archive_name);

    log::info!("Fetching D7VK {tag} from {url}...");

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
        .with_context(|| format!("Failed to execute curl to download {url}"))?;

    if !status.success() || !archive_path.is_file() || fs::metadata(&archive_path)?.len() < 1000 {
        bail!("Failed to download D7VK from {url}. Check network connection or version tag.");
    }

    let extract_dir = temp_dir.path().join("extracted");
    fs::create_dir_all(&extract_dir)?;

    let unzip_status = Command::new("unzip")
        .args([
            "-q",
            "-o",
            archive_path.to_str().unwrap(),
            "-d",
            extract_dir.to_str().unwrap(),
        ])
        .status()
        .with_context(|| "Failed to execute unzip")?;

    if !unzip_status.success() {
        bail!("Failed to unzip D7VK archive: {}", archive_path.display());
    }

    let candidate = inspect_candidate_dir(&extract_dir)
        .ok_or_else(|| anyhow::anyhow!("Could not find ddraw.dll in extracted D7VK archive"))?;

    let target_x32 = target_dir.join("x32");
    fs::create_dir_all(&target_x32)?;
    let target_ddraw = target_x32.join("ddraw.dll");
    if target_ddraw.exists() || target_ddraw.is_symlink() {
        let _ = fs::remove_file(&target_ddraw);
    }
    fs::copy(&candidate.x86_ddraw, &target_ddraw)?;
    let _ = fs::write(target_dir.join("version.txt"), &tag);

    let bundle = inspect_candidate_dir(&target_dir).ok_or_else(|| {
        anyhow::anyhow!(
            "Failed to verify staged D7VK bundle in {}",
            target_dir.display()
        )
    })?;

    log::info!(
        "Successfully fetched and staged D7VK {} at {}",
        tag,
        bundle.root.display()
    );

    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_inspect_candidate_dir_x32_layout() {
        let dir = tempdir().unwrap();
        let x32 = dir.path().join("x32");
        fs::create_dir_all(&x32).unwrap();
        fs::write(x32.join("ddraw.dll"), "fake-ddraw").unwrap();
        fs::write(dir.path().join("version.txt"), "v2.3").unwrap();

        let bundle = inspect_candidate_dir(dir.path()).unwrap();
        assert_eq!(bundle.root, dir.path());
        assert_eq!(bundle.x86_ddraw, x32.join("ddraw.dll"));
        assert_eq!(bundle.version.as_deref(), Some("v2.3"));
    }

    #[test]
    fn test_inspect_candidate_dir_flat_layout() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("ddraw.dll"), "fake-flat-ddraw").unwrap();

        let bundle = inspect_candidate_dir(dir.path()).unwrap();
        assert_eq!(bundle.root, dir.path());
        assert_eq!(bundle.x86_ddraw, dir.path().join("ddraw.dll"));
        assert!(bundle.version.is_none());
    }

    #[test]
    fn test_stage_and_unstage_d7vk_prefix() {
        let d7vk_dir = tempdir().unwrap();
        let x32 = d7vk_dir.path().join("x32");
        fs::create_dir_all(&x32).unwrap();
        fs::write(x32.join("ddraw.dll"), "d7vk-ddraw-content").unwrap();
        let bundle = inspect_candidate_dir(d7vk_dir.path()).unwrap();

        let pfx_dir = tempdir().unwrap();
        let wow64 = pfx_dir.path().join("drive_c/windows/syswow64");
        fs::create_dir_all(&wow64).unwrap();

        // Stage
        let staged = stage_d7vk_into_prefix(&bundle, pfx_dir.path()).unwrap();
        assert_eq!(staged, 1);
        assert_eq!(
            fs::read_to_string(wow64.join("ddraw.dll")).unwrap(),
            "d7vk-ddraw-content"
        );

        // Runner with builtin ddraw.dll
        let runner_dir = tempdir().unwrap();
        let runner_i386 = runner_dir.path().join("lib/wine/i386-windows");
        fs::create_dir_all(&runner_i386).unwrap();
        fs::write(runner_i386.join("ddraw.dll"), "wine-builtin-ddraw").unwrap();

        // Unstage restores builtin
        let unstaged = unstage_d7vk_from_prefix(pfx_dir.path(), Some(runner_dir.path())).unwrap();
        assert_eq!(unstaged, 1);
        assert_eq!(
            fs::read_to_string(wow64.join("ddraw.dll")).unwrap(),
            "wine-builtin-ddraw"
        );
    }

    #[test]
    fn test_find_d7vk_env_override() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("ddraw.dll"), "fake").unwrap();

        std::env::set_var("D7VK_PATH", dir.path().to_str().unwrap());
        let found = find_d7vk();
        std::env::remove_var("D7VK_PATH");

        assert!(found.is_some());
        assert_eq!(found.unwrap().root, dir.path());
    }
}
