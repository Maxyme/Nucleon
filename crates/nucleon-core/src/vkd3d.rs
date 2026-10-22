use crate::paths;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Default release version from the embedded download catalog.
pub fn default_version() -> Result<&'static str> {
    crate::downloads::default_version("vkd3d")
}

#[derive(Debug, Clone)]
pub struct Vkd3dProtonBundle {
    pub root: PathBuf,
    pub x64_d3d12: PathBuf,
    pub x64_d3d12core: Option<PathBuf>,
    pub x86_d3d12: Option<PathBuf>,
    pub x86_d3d12core: Option<PathBuf>,
    pub version: Option<String>,
}

/// Inspects a directory to check if it contains a valid VKD3D-Proton installation.
/// Accepts either the standard release hierarchy (x64/d3d12.dll) or a flat directory (d3d12.dll).
pub fn inspect_candidate_dir(dir: &Path) -> Option<Vkd3dProtonBundle> {
    if !dir.is_dir() {
        return None;
    }

    // Prefer the standard release layout, then the flat layout.
    for x64 in [dir.join("x64"), dir.to_path_buf()] {
        if let Some(x64_d3d12) = crate::graphics_bundle::file(&x64, "d3d12.dll") {
            let x86 = dir.join("x86");
            let standard = x64 != dir;
            return Some(Vkd3dProtonBundle {
                root: dir.to_path_buf(),
                x64_d3d12,
                x64_d3d12core: crate::graphics_bundle::file(&x64, "d3d12core.dll"),
                x86_d3d12: standard
                    .then(|| crate::graphics_bundle::file(&x86, "d3d12.dll"))
                    .flatten(),
                x86_d3d12core: standard
                    .then(|| crate::graphics_bundle::file(&x86, "d3d12core.dll"))
                    .flatten(),
                version: crate::graphics_bundle::version(dir),
            });
        }
    }

    None
}

pub fn custom_vkd3d_path_file() -> PathBuf {
    paths::support_dir().join("vkd3d_proton_path.txt")
}

/// Locates a VKD3D-Proton bundle from environment, configured custom path,
/// Nucleon support directory, or well-known system/Proton paths.
pub fn find_vkd3d_proton() -> Option<Vkd3dProtonBundle> {
    // 1. Explicit environment variables: VKD3D_PROTON_PATH then VKD3D_PATH
    let env_paths = ["VKD3D_PROTON_PATH", "VKD3D_PATH"]
        .iter()
        .filter_map(|var| std::env::var(var).ok().map(PathBuf::from))
        .collect::<Vec<_>>();

    find_vkd3d_proton_with(&env_paths)
}

/// Resolves a VKD3D-Proton bundle, preferring `env_paths` (directories supplied through the
/// environment, highest priority first). Split out so resolution can be tested without mutating
/// process state.
fn find_vkd3d_proton_with(env_paths: &[PathBuf]) -> Option<Vkd3dProtonBundle> {
    if let Some(bundle) = crate::graphics_bundle::configured(
        env_paths,
        &custom_vkd3d_path_file(),
        inspect_candidate_dir,
    ) {
        return Some(bundle);
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

    if let Some(bundle) = crate::graphics_bundle::find(system_candidates, inspect_candidate_dir) {
        return Some(bundle);
    }

    // 5. Scan Steam Proton installs for vkd3d-proton if present
    let steam_common = paths::home_dir().join("Library/Application Support/Steam/steamapps/common");
    if steam_common.is_dir()
        && let Ok(entries) = fs::read_dir(&steam_common)
    {
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

    None
}

/// Returns true if VKD3D-Proton is detected on the system.
pub fn is_vkd3d_proton_installed() -> bool {
    find_vkd3d_proton().is_some()
}

/// Sets a custom path to a VKD3D-Proton installation directory.
pub fn set_custom_vkd3d_proton_path(path: &Path) -> Result<Vkd3dProtonBundle> {
    let path = &path
        .canonicalize()
        .with_context(|| format!("Path does not exist: {}", path.display()))?;
    crate::graphics_bundle::register(
        path,
        &custom_vkd3d_path_file(),
        "VKD3D-Proton (d3d12.dll not found)",
        inspect_candidate_dir,
        |bundle| &bundle.root,
    )
}

/// Clears any configured custom VKD3D-Proton path.
pub fn clear_custom_vkd3d_proton_path() -> Result<()> {
    crate::graphics_bundle::clear(&custom_vkd3d_path_file())
}

/// Stages VKD3D-Proton DLLs into a Wine prefix's system32 and syswow64 directories.
pub fn stage_vkd3d_proton_into_prefix(
    bundle: &Vkd3dProtonBundle,
    prefix_dir: &Path,
) -> Result<usize> {
    let sys32 = prefix_dir.join("drive_c/windows/system32");
    let syswow64 = prefix_dir.join("drive_c/windows/syswow64");
    fs::create_dir_all(&sys32)?;
    let target_32 = &syswow64;
    let count = crate::graphics_bundle::stage(
        &sys32,
        "64-bit VKD3D",
        [
            ("d3d12.dll", Some(bundle.x64_d3d12.as_path())),
            ("d3d12core.dll", bundle.x64_d3d12core.as_deref()),
        ],
    )?;
    Ok(count
        + crate::graphics_bundle::stage(
            target_32,
            "32-bit VKD3D",
            [
                ("d3d12.dll", bundle.x86_d3d12.as_deref()),
                ("d3d12core.dll", bundle.x86_d3d12core.as_deref()),
            ],
        )?)
}

fn restore_d3d12_in_dir(
    dst_dir: &Path,
    runner_dir: Option<&Path>,
    subdirs: &[&str],
) -> Result<usize> {
    if !dst_dir.is_dir() {
        return Ok(0);
    }
    let mut count = crate::backend_files::restore(dst_dir, "d3d12.dll", runner_dir, subdirs)?;
    count += crate::backend_files::restore(dst_dir, "d3d12core.dll", runner_dir, subdirs)?;
    Ok(count)
}

/// Unstages VKD3D-Proton DLLs from a Wine prefix's system32 and syswow64 directories.
///
/// If a `runner_dir` is provided and contains Wine's builtin `d3d12.dll` (e.g. from
/// Apple GPTK / D3DMetal), it restores the builtin DLL into the prefix.
pub fn unstage_vkd3d_proton_from_prefix(
    prefix_dir: &Path,
    runner_dir: Option<&Path>,
) -> Result<usize> {
    let pfx = prefix_dir.join("drive_c/windows");
    let count = restore_d3d12_in_dir(
        &pfx.join("system32"),
        runner_dir,
        &["lib/wine/x86_64-windows", "lib64/wine/x86_64-windows"],
    )? + restore_d3d12_in_dir(
        &pfx.join("syswow64"),
        runner_dir,
        &["lib/wine/i386-windows", "lib/wine/x86-windows"],
    )?;

    if count > 0 {
        log::info!(
            "Unstaged/restored {count} DirectX 12 DLL(s) in Wine prefix at {}",
            prefix_dir.display()
        );
    }

    Ok(count)
}

/// Fetches VKD3D-Proton from official GitHub releases into the target directory.
/// Does not commit any files to source code; stores purely in runtime support directory.
pub fn fetch_vkd3d_proton(
    version: Option<&str>,
    dest_dir: Option<&Path>,
) -> Result<Vkd3dProtonBundle> {
    use crate::downloads;
    let release = downloads::release("vkd3d", version)?;
    let target = dest_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(paths::vkd3d_proton_dir);
    downloads::fetch(
        &release,
        &target,
        inspect_candidate_dir,
        |bundle, extracted, staged| {
            downloads::copy_layout(&bundle.root, staged, extracted, &["x64", "x86"])
        },
    )
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

        // The environment-provided candidate is passed in rather than exported, so this test
        // cannot disturb Vulkan/DXVK resolution happening in other threads.
        let found = find_vkd3d_proton_with(&[dir.path().to_path_buf()]);
        assert!(found.is_some());
        assert_eq!(found.unwrap().root, dir.path());
    }

    #[test]
    fn test_unstage_vkd3d_proton_restores_builtin_runner_dll() {
        // Setup VKD3D bundle
        let vkd3d_dir = tempdir().unwrap();
        let x64_dir = vkd3d_dir.path().join("x64");
        fs::create_dir_all(&x64_dir).unwrap();
        fs::write(x64_dir.join("d3d12.dll"), "vkd3d-d3d12").unwrap();
        fs::write(x64_dir.join("d3d12core.dll"), "vkd3d-d3d12core").unwrap();
        let bundle = inspect_candidate_dir(vkd3d_dir.path()).unwrap();

        // Setup prefix and stage VKD3D
        let pfx_dir = tempdir().unwrap();
        stage_vkd3d_proton_into_prefix(&bundle, pfx_dir.path()).unwrap();

        let sys32 = pfx_dir.path().join("drive_c/windows/system32");
        assert!(sys32.join("d3d12.dll").is_file());
        assert!(sys32.join("d3d12core.dll").is_file());

        // Setup mock GPTK runner with builtin D3DMetal d3d12.dll
        let runner_dir = tempdir().unwrap();
        let runner_wine_dlls = runner_dir.path().join("lib/wine/x86_64-windows");
        fs::create_dir_all(&runner_wine_dlls).unwrap();
        fs::write(runner_wine_dlls.join("d3d12.dll"), "apple-gptk-d3dmetal").unwrap();

        // Unstage VKD3D with runner provided
        let cleaned =
            unstage_vkd3d_proton_from_prefix(pfx_dir.path(), Some(runner_dir.path())).unwrap();
        assert!(cleaned > 0);

        // Verify d3d12core.dll was removed and d3d12.dll was restored to GPTK builtin
        assert!(!sys32.join("d3d12core.dll").exists());
        assert_eq!(
            fs::read_to_string(sys32.join("d3d12.dll")).unwrap(),
            "apple-gptk-d3dmetal"
        );
    }
}
