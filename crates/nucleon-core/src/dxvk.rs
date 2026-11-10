#![forbid(unsafe_code)]

use crate::paths;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Default release version from the embedded download catalog.
pub fn default_version() -> Result<&'static str> {
    crate::downloads::default_version("dxvk")
}

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

    pub fn has_d3d10(&self) -> bool {
        self.x64_d3d10core.is_some() || self.x86_d3d10core.is_some()
    }

    pub fn has_d3d9(&self) -> bool {
        self.x64_d3d9.is_some() || self.x86_d3d9.is_some()
    }

    /// Returns the supported DirectX version range formatted for UI labels (e.g. "DX10-DX11" or "DX9-DX11").
    pub fn supported_dx_range(&self) -> &'static str {
        let has_9 = self.has_d3d9();
        let has_10 = self.has_d3d10();
        let has_11 = self.has_d3d11();

        match (has_9, has_10, has_11) {
            (true, true | false, true) => "DX9-DX11",
            (true, true, false) => "DX9-DX10",
            (true, false, false) => "DX9",
            (false, true, true) => "DX10-DX11",
            (false, false, true) => "DX11",
            (false, true, false) => "DX10",
            (false, false, false) => "DX10-DX11",
        }
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

/// Inspects a directory to check if it contains a valid DXVK installation.
/// Accepts either the standard release hierarchy (x64/d3d11.dll, x32/d3d11.dll or x86/d3d11.dll),
/// flat directory (d3d11.dll), or nested release directories.
pub fn inspect_candidate_dir(dir: &Path) -> Option<DxvkBundle> {
    if !dir.is_dir() {
        return None;
    }
    crate::fs_util::directories(dir, usize::MAX)
        .find_map(|candidate| inspect_bundle_layout(&candidate))
}

fn inspect_bundle_layout(dir: &Path) -> Option<DxvkBundle> {
    // 1. Check standard release layout: <dir>/x64 and <dir>/x32 (or x86)
    let x64_dir = crate::graphics_bundle::directory(dir, &["x86_64-windows", "x64"]);
    let x86_dir = crate::graphics_bundle::directory(dir, &["i386-windows", "x32", "x86"]);

    let has_x64 = x64_dir.is_dir();
    let has_x86 = x86_dir.is_dir();

    if has_x64 || has_x86 {
        let x64_d3d11 = crate::graphics_bundle::file(&x64_dir, "d3d11.dll");
        let x64_d3d10core = crate::graphics_bundle::file(&x64_dir, "d3d10core.dll");
        let x64_d3d9 = crate::graphics_bundle::file(&x64_dir, "d3d9.dll");
        let x64_dxgi = crate::graphics_bundle::file(&x64_dir, "dxgi.dll");

        let x86_d3d11 = crate::graphics_bundle::file(&x86_dir, "d3d11.dll");
        let x86_d3d10core = crate::graphics_bundle::file(&x86_dir, "d3d10core.dll");
        let x86_d3d9 = crate::graphics_bundle::file(&x86_dir, "d3d9.dll");
        let x86_dxgi = crate::graphics_bundle::file(&x86_dir, "dxgi.dll");

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
                version: crate::graphics_bundle::version(dir),
            });
        }
    }

    // 2. Check flat directory: <dir>/d3d11.dll or <dir>/d3d9.dll
    let flat_d3d11 = crate::graphics_bundle::file(dir, "d3d11.dll");
    let flat_d3d10core = crate::graphics_bundle::file(dir, "d3d10core.dll");
    let flat_d3d9 = crate::graphics_bundle::file(dir, "d3d9.dll");
    let flat_dxgi = crate::graphics_bundle::file(dir, "dxgi.dll");

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
            version: crate::graphics_bundle::version(dir),
        });
    }

    None
}

pub fn custom_dxvk_path_file() -> PathBuf {
    paths::support_dir().join("dxvk_path.txt")
}

/// Locates a DXVK bundle from environment, configured custom path,
/// Nucleon support directory, Heroic tools, or well-known system paths.
pub fn find_dxvk() -> Option<DxvkBundle> {
    if let Some(bundle) = crate::graphics_bundle::configured(
        &std::env::var("DXVK_PATH")
            .ok()
            .map(PathBuf::from)
            .into_iter()
            .collect::<Vec<_>>(),
        &custom_dxvk_path_file(),
        inspect_candidate_dir,
    ) {
        return Some(bundle);
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
    if let Some(bundle) = crate::graphics_bundle::find(candidates, inspect_candidate_dir) {
        return Some(bundle);
    }

    None
}

pub fn set_custom_dxvk_path(path: &Path) -> Result<DxvkBundle> {
    crate::graphics_bundle::register(
        path,
        &custom_dxvk_path_file(),
        "DXVK (expected x64/d3d11.dll, x32/d3d11.dll, or d3d11.dll)",
        inspect_candidate_dir,
        |bundle| &bundle.root,
    )
}

pub fn clear_custom_dxvk_path() -> Result<()> {
    crate::graphics_bundle::clear(&custom_dxvk_path_file())
}

/// Stages DXVK DLLs into a Wine prefix's system32 and syswow64 directories.
pub fn stage_dxvk_into_prefix(bundle: &DxvkBundle, prefix_dir: &Path) -> Result<usize> {
    let sys32 = prefix_dir.join("drive_c/windows/system32");
    let syswow64 = prefix_dir.join("drive_c/windows/syswow64");
    let target_32 = if syswow64.is_dir() { &syswow64 } else { &sys32 };
    let count = crate::graphics_bundle::stage(
        &sys32,
        "64-bit DXVK",
        [
            ("d3d11.dll", bundle.x64_d3d11.as_deref()),
            ("d3d10core.dll", bundle.x64_d3d10core.as_deref()),
            ("d3d9.dll", bundle.x64_d3d9.as_deref()),
            ("dxgi.dll", bundle.x64_dxgi.as_deref()),
        ],
    )?;
    Ok(count
        + crate::graphics_bundle::stage(
            target_32,
            "32-bit DXVK",
            [
                ("d3d11.dll", bundle.x86_d3d11.as_deref()),
                ("d3d10core.dll", bundle.x86_d3d10core.as_deref()),
                ("d3d9.dll", bundle.x86_d3d9.as_deref()),
                ("dxgi.dll", bundle.x86_dxgi.as_deref()),
            ],
        )?)
}

fn restore_dll_in_dir(
    dst_dir: &Path,
    dll_name: &str,
    runner_dir: Option<&Path>,
    subdirs: &[&str],
) -> Result<usize> {
    crate::backend_files::restore(dst_dir, dll_name, runner_dir, subdirs)
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
        )?;
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
        )?;
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
    use crate::downloads;
    let release = downloads::release("dxvk", version)?;
    let target = dest_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(paths::dxvk_dir);
    downloads::fetch(
        &release,
        &target,
        inspect_candidate_dir,
        |bundle, extracted, staged| {
            downloads::copy_layout(
                &bundle.root,
                staged,
                extracted,
                &["x64", "x32", "x86", "x86_64-windows", "i386-windows"],
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
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

    #[test]
    fn test_dxvk_supported_dx_range() {
        let mut bundle = DxvkBundle {
            root: PathBuf::from("/tmp/dxvk"),
            x64_d3d11: Some(PathBuf::from("/tmp/dxvk/x64/d3d11.dll")),
            x64_d3d10core: Some(PathBuf::from("/tmp/dxvk/x64/d3d10core.dll")),
            x64_d3d9: None,
            x64_dxgi: Some(PathBuf::from("/tmp/dxvk/x64/dxgi.dll")),
            x86_d3d11: None,
            x86_d3d10core: None,
            x86_d3d9: None,
            x86_dxgi: None,
            version: None,
        };
        // DX10 and DX11 present, no DX9
        assert_eq!(bundle.supported_dx_range(), "DX10-DX11");

        // Add DX9
        bundle.x64_d3d9 = Some(PathBuf::from("/tmp/dxvk/x64/d3d9.dll"));
        assert_eq!(bundle.supported_dx_range(), "DX9-DX11");

        // Only DX9
        bundle.x64_d3d10core = None;
        bundle.x64_d3d11 = None;
        assert_eq!(bundle.supported_dx_range(), "DX9");

        // Only DX11
        bundle.x64_d3d9 = None;
        bundle.x64_d3d11 = Some(PathBuf::from("/tmp/dxvk/x64/d3d11.dll"));
        assert_eq!(bundle.supported_dx_range(), "DX11");
    }
}
