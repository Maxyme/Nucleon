use crate::paths;
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

/// Default release version from the embedded download catalog.
pub fn default_version() -> Result<&'static str> {
    crate::downloads::default_version("d7vk")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct D7vkBundle {
    pub root: PathBuf,
    pub x86_ddraw: PathBuf,
    pub version: Option<String>,
}

/// Inspects a directory to check if it contains a valid D7VK installation.
/// Accepts either the standard release hierarchy (x32/ddraw.dll), an x86 subfolder, or a flat directory (ddraw.dll).
pub fn inspect_candidate_dir(dir: &Path) -> Option<D7vkBundle> {
    if !dir.is_dir() {
        return None;
    }

    for layout in ["x32", "x86", ""] {
        if let Some(x86_ddraw) = crate::graphics_bundle::file(&dir.join(layout), "ddraw.dll") {
            return Some(D7vkBundle {
                root: dir.to_path_buf(),
                x86_ddraw,
                version: crate::graphics_bundle::version(dir),
            });
        }
    }

    // 4. Check immediate release subdirectories, retaining the parent's version fallback.
    crate::fs_util::directories(dir, 1).skip(1).find_map(|p| {
        [p.join("x32/ddraw.dll"), p.join("ddraw.dll")]
            .into_iter()
            .find(|file| file.is_file())
            .map(|x86_ddraw| D7vkBundle {
                version: crate::graphics_bundle::version(&p)
                    .or_else(|| crate::graphics_bundle::version(dir)),
                root: p,
                x86_ddraw,
            })
    })
}

pub fn custom_d7vk_path_file() -> PathBuf {
    paths::support_dir().join("d7vk_path.txt")
}

/// Locates a D7VK bundle from environment, configured custom path,
/// Nucleon support directory, or well-known system/Proton paths.
pub fn find_d7vk() -> Option<D7vkBundle> {
    // 1. Explicit environment variable: D7VK_PATH
    let env_paths = ["D7VK_PATH"]
        .iter()
        .filter_map(|var| std::env::var(var).ok().map(PathBuf::from))
        .collect::<Vec<_>>();

    find_d7vk_with(&env_paths)
}

/// Resolves a D7VK bundle, preferring `env_paths` (directories supplied through the environment,
/// highest priority first). Split out so resolution can be tested without mutating process state.
fn find_d7vk_with(env_paths: &[PathBuf]) -> Option<D7vkBundle> {
    if let Some(bundle) = crate::graphics_bundle::configured(
        env_paths,
        &custom_d7vk_path_file(),
        inspect_candidate_dir,
    ) {
        return Some(bundle);
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
    if let Some(bundle) = crate::graphics_bundle::find(candidates, inspect_candidate_dir) {
        return Some(bundle);
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
                if (name.contains("proton") || name.contains("d7vk"))
                    && let Some(bundle) = inspect_candidate_dir(&p)
                {
                    return Some(bundle);
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
    crate::graphics_bundle::register(
        path,
        &custom_d7vk_path_file(),
        "D7VK (expected x32/ddraw.dll, x86/ddraw.dll, or ddraw.dll)",
        inspect_candidate_dir,
        |bundle| &bundle.root,
    )
}

pub fn clear_custom_d7vk_path() -> Result<()> {
    crate::graphics_bundle::clear(&custom_d7vk_path_file())
}

/// Stages D7VK DLL into a Wine prefix's syswow64 (or system32) directory.
pub fn stage_d7vk_into_prefix(bundle: &D7vkBundle, prefix_dir: &Path) -> Result<usize> {
    let sys32 = prefix_dir.join("drive_c/windows/system32");
    let syswow64 = prefix_dir.join("drive_c/windows/syswow64");
    let target_32 = if syswow64.is_dir() { &syswow64 } else { &sys32 };
    crate::graphics_bundle::stage(
        target_32,
        "32-bit D7VK",
        [("ddraw.dll", Some(bundle.x86_ddraw.as_path()))],
    )
}

fn restore_ddraw_in_dir(
    dst_dir: &Path,
    runner_dir: Option<&Path>,
    subdirs: &[&str],
) -> Result<usize> {
    crate::backend_files::restore(dst_dir, "ddraw.dll", runner_dir, subdirs)
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
    )? + restore_ddraw_in_dir(
        &pfx.join("system32"),
        runner_dir,
        &[
            "lib/wine/i386-windows",
            "lib/wine/x86-windows",
            "lib/wine/x86_64-windows",
            "lib64/wine/x86_64-windows",
        ],
    )?;

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
    use crate::downloads;
    let release = downloads::release("d7vk", version)?;
    let target = dest_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(paths::d7vk_dir);
    downloads::fetch(
        &release,
        &target,
        inspect_candidate_dir,
        |bundle, extracted, staged| {
            downloads::copy_file(&bundle.x86_ddraw, &staged.join("x32/ddraw.dll"), extracted)
        },
    )
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

        // The environment-provided candidate is passed in rather than exported, so this test
        // cannot disturb layer resolution happening in other threads.
        let found = find_d7vk_with(&[dir.path().to_path_buf()]);

        assert!(found.is_some());
        assert_eq!(found.unwrap().root, dir.path());
    }
}
