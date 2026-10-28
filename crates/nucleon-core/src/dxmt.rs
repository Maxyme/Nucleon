#![forbid(unsafe_code)]

use crate::paths;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::{fs, io::Read};

/// Default release version from the embedded download catalog.
pub fn default_version() -> Result<&'static str> {
    crate::downloads::default_version("dxmt")
}

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

fn is_builtin_dll(path: &Path) -> bool {
    let mut header = [0; 80];
    fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_ok()
        && &header[64..80] == b"Wine builtin DLL"
}

impl DxmtBundle {
    /// Wine-built DLLs must be resolved from WINEDLLPATH as builtins, not as native PE files.
    pub fn uses_builtin_dlls(&self) -> bool {
        self.x64_d3d11
            .as_deref()
            .or(self.x86_d3d11.as_deref())
            .is_some_and(is_builtin_dll)
    }

    /// Prepend lookup may replace staging only when every supplied PE is a builtin.
    pub(crate) fn all_builtin_dlls(&self) -> bool {
        self.has_d3d11()
            && [
                &self.x64_d3d11,
                &self.x64_d3d10core,
                &self.x64_dxgi,
                &self.x64_winemetal,
                &self.x86_d3d11,
                &self.x86_d3d10core,
                &self.x86_dxgi,
                &self.x86_winemetal,
            ]
            .into_iter()
            .flatten()
            .all(|path| is_builtin_dll(path))
    }

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

/// Inspects a directory to check if it contains a valid DXMT installation.
/// Supports standard DXMT release hierarchy (x86_64-windows / i386-windows / x86_64-unix),
/// x64 / x32 layouts, flat directories, or nested release directories.
pub fn inspect_candidate_dir(dir: &Path) -> Option<DxmtBundle> {
    if !dir.is_dir() {
        return None;
    }
    crate::fs_util::directories(dir, usize::MAX)
        .find_map(|candidate| inspect_bundle_layout(&candidate))
}

fn inspect_bundle_layout(dir: &Path) -> Option<DxmtBundle> {
    // 1. Check official DXMT layout: x86_64-windows, i386-windows, and x86_64-unix
    let x64_win = crate::graphics_bundle::directory(dir, &["x86_64-windows", "x64"]);
    let x86_win =
        crate::graphics_bundle::directory(dir, &["i386-windows", "x86-windows", "x32", "x86"]);

    let x64_unix = crate::graphics_bundle::directory(dir, &["x86_64-unix", "aarch64-unix", ""]);

    if x64_win.is_dir() || x86_win.is_dir() {
        let x64_d3d11 = crate::graphics_bundle::file(&x64_win, "d3d11.dll");
        let x64_d3d10core = crate::graphics_bundle::file(&x64_win, "d3d10core.dll");
        let x64_dxgi = crate::graphics_bundle::file(&x64_win, "dxgi.dll");
        let x64_winemetal = crate::graphics_bundle::file(&x64_win, "winemetal.dll");
        let x64_winemetal_so = crate::graphics_bundle::file(&x64_unix, "winemetal.so");

        let x86_d3d11 = crate::graphics_bundle::file(&x86_win, "d3d11.dll");
        let x86_d3d10core = crate::graphics_bundle::file(&x86_win, "d3d10core.dll");
        let x86_dxgi = crate::graphics_bundle::file(&x86_win, "dxgi.dll");
        let x86_winemetal = crate::graphics_bundle::file(&x86_win, "winemetal.dll");

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
                version: crate::graphics_bundle::version(dir),
            });
        }
    }

    // 2. Check flat directory: <dir>/d3d11.dll or <dir>/winemetal.so
    let flat_d3d11 = crate::graphics_bundle::file(dir, "d3d11.dll");
    let flat_d3d10core = crate::graphics_bundle::file(dir, "d3d10core.dll");
    let flat_dxgi = crate::graphics_bundle::file(dir, "dxgi.dll");
    let flat_winemetal = crate::graphics_bundle::file(dir, "winemetal.dll");
    let flat_winemetal_so = crate::graphics_bundle::file(dir, "winemetal.so");

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
            version: crate::graphics_bundle::version(dir),
        });
    }

    None
}

pub fn custom_dxmt_path_file() -> PathBuf {
    paths::support_dir().join("dxmt_path.txt")
}

/// Locates a DXMT bundle from environment, configured custom path,
/// Nucleon support directory, Heroic tools, active Wine runtime, or well-known system paths.
pub fn find_dxmt() -> Option<DxmtBundle> {
    if let Some(bundle) = crate::graphics_bundle::configured(
        &std::env::var("DXMT_PATH")
            .ok()
            .map(PathBuf::from)
            .into_iter()
            .collect::<Vec<_>>(),
        &custom_dxmt_path_file(),
        inspect_candidate_dir,
    ) {
        return Some(bundle);
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
        if (wine_unix.is_file() || wine_win.is_file())
            && let Some(bundle) = inspect_candidate_dir(&active_wine.root)
        {
            return Some(bundle);
        }
    }

    // 6. Well-known system directories
    let candidates = [
        PathBuf::from("/opt/homebrew/share/dxmt"),
        PathBuf::from("/usr/local/share/dxmt"),
        paths::home_dir().join(".local/share/dxmt"),
    ];
    if let Some(bundle) = crate::graphics_bundle::find(candidates, inspect_candidate_dir) {
        return Some(bundle);
    }

    None
}

pub fn set_custom_dxmt_path(path: &Path) -> Result<DxmtBundle> {
    crate::graphics_bundle::register(
        path,
        &custom_dxmt_path_file(),
        "DXMT (expected x86_64-windows/d3d11.dll, winemetal.so, or d3d11.dll)",
        inspect_candidate_dir,
        |bundle| &bundle.root,
    )
}

pub fn clear_custom_dxmt_path() -> Result<()> {
    crate::graphics_bundle::clear(&custom_dxmt_path_file())
}

/// DXMT needs either its macdrv function table or the legacy exported entry points.
/// Matching PE/Unix DXMT files alone do not provide this Wine ABI.
pub fn validate_metal_driver(runner: &Path) -> Result<()> {
    let legacy = [
        "get_win_data",
        "release_win_data",
        "macdrv_view_create_metal_view",
        "macdrv_view_get_metal_layer",
        "macdrv_view_release_metal_view",
    ];
    for lib in ["lib/wine", "lib64/wine"] {
        let path = runner.join(lib).join("x86_64-unix/winemac.so");
        if let Ok(bytes) = fs::read(&path)
            && crate::runtime_checks::native_arch(&path, object::Architecture::X86_64)
            && (crate::runtime_checks::has_export(&bytes, &["macdrv_functions"])
                || legacy
                    .iter()
                    .all(|name| crate::runtime_checks::has_export(&bytes, &[*name])))
        {
            return Ok(());
        }
    }
    anyhow::bail!(
        "Wine at {} lacks the exported macdrv Metal window interface required by DXMT. Select a DXMT-compatible Wine build; copying DXMT DLLs alone cannot repair the Wine driver.",
        runner.display()
    )
}

/// Builtin DXMT DLLs and the Metal window interface must match the selected Wine runtime.
pub fn validate_runtime(bundle: &DxmtBundle, runner_dir: Option<&Path>) -> Result<()> {
    if let Some(runner) = runner_dir
        && runner.join(crate::runtime::DESCRIPTOR).is_file()
    {
        let runtime = crate::runtime::ResolvedRuntime::inspect(runner)?;
        if runtime.mode == crate::runtime::ExecutionMode::Arm64Fex {
            anyhow::ensure!(
                runtime.backends.contains(&GraphicsBackend::Dxmt)
                    && runtime.graphics.dxmt.as_deref().is_some_and(|p| bundle
                        .root
                        .canonicalize()
                        .ok()
                        .as_deref()
                        == Some(p))
                    && bundle
                        .x64_winemetal_so
                        .as_deref()
                        .is_some_and(|p| crate::runtime_checks::native_arch(p, runtime.arch())),
                "DXMT requires the explicitly paired ARM64 runtime bundle and Unix companion"
            );
            return Ok(());
        }
    }
    if let Some(runner) = runner_dir {
        validate_metal_driver(runner)?;
    }
    if bundle.uses_builtin_dlls()
        && let Some(runner) = runner_dir
    {
        for (arch, source) in [
            ("x86_64-windows", bundle.x64_d3d11.as_ref()),
            ("i386-windows", bundle.x86_d3d11.as_ref()),
        ] {
            if let Some(source) = source {
                let builtin = runner.join("lib/wine").join(arch).join("d3d11.dll");
                if fs::read(&builtin)
                    .with_context(|| format!("Missing DXMT builtin {}", builtin.display()))?
                    != fs::read(source)?
                {
                    anyhow::bail!(
                        "This DXMT package contains builtin DLLs, but {} does not contain matching DXMT builtins. Select a compatible DXMT runtime or use macOS DXVK. Nucleon does not replace the source Wine runtime's DLLs.",
                        runner.display()
                    );
                }
            }
        }
    }
    Ok(())
}

/// Stages DXMT DLLs without changing the source Wine runtime.
pub fn stage_dxmt_into_prefix(
    bundle: &DxmtBundle,
    prefix_dir: &Path,
    runner_dir: Option<&Path>,
) -> Result<usize> {
    validate_runtime(bundle, runner_dir)?;
    let sys32 = prefix_dir.join("drive_c/windows/system32");
    let syswow64 = prefix_dir.join("drive_c/windows/syswow64");
    fs::create_dir_all(&sys32)?;
    fs::create_dir_all(&syswow64)?;
    let target_32 = &syswow64;
    let count = crate::graphics_bundle::stage(
        &sys32,
        "64-bit DXMT",
        [
            ("d3d11.dll", bundle.x64_d3d11.as_deref()),
            ("d3d10core.dll", bundle.x64_d3d10core.as_deref()),
            ("dxgi.dll", bundle.x64_dxgi.as_deref()),
            ("winemetal.dll", bundle.x64_winemetal.as_deref()),
        ],
    )?;
    Ok(count
        + crate::graphics_bundle::stage(
            target_32,
            "32-bit DXMT",
            [
                ("d3d11.dll", bundle.x86_d3d11.as_deref()),
                ("d3d10core.dll", bundle.x86_d3d10core.as_deref()),
                ("dxgi.dll", bundle.x86_dxgi.as_deref()),
                ("winemetal.dll", bundle.x86_winemetal.as_deref()),
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
            "Unstaged/restored {count} DXMT DLL(s) in Wine prefix at {}",
            prefix_dir.display()
        );
    }

    Ok(count)
}

/// Fetches official DXMT release archive (.tar.gz) from GitHub and unpacks it.
pub fn fetch_dxmt(version: Option<&str>, dest_dir: Option<&Path>) -> Result<DxmtBundle> {
    use crate::downloads;
    let release = downloads::release("dxmt", version)?;
    let target = dest_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(paths::dxmt_dir);
    downloads::fetch(
        &release,
        &target,
        inspect_candidate_dir,
        |bundle, extracted, staged| {
            downloads::copy_layout(
                &bundle.root,
                staged,
                extracted,
                &[
                    "x86_64-windows",
                    "i386-windows",
                    "x86-windows",
                    "x86_64-unix",
                    "x64",
                    "x32",
                    "x86",
                ],
            )
        },
    )
}

use crate::backend::{Backend, BackendStatus, GraphicsBackend};

#[derive(Debug, Clone, Copy, Default)]
pub struct DxmtBackend;

impl Backend for DxmtBackend {
    fn backend(&self) -> GraphicsBackend {
        GraphicsBackend::Dxmt
    }

    fn supported_dx_versions(&self) -> &'static str {
        find_dxmt()
            .as_ref()
            .map(|b| b.supported_dx_range())
            .unwrap_or("DX10-DX11")
    }

    fn status(&self) -> BackendStatus {
        let bundle = find_dxmt();
        let installed = bundle.is_some();
        let (location, version) = match bundle {
            Some(ref b) => (Some(b.root.clone()), b.version.clone()),
            None => (None, None),
        };
        BackendStatus {
            backend: GraphicsBackend::Dxmt,
            installed,
            version,
            location,
            supported_dx: self.supported_dx_versions(),
            description: "Direct3D 11 translation to Apple Metal".to_string(),
        }
    }

    fn install(&self, paths: &Path) -> Result<()> {
        set_custom_dxmt_path(paths).map(|_| ())
    }

    fn enable(&self, prefix: &Path, runner_dir: Option<&Path>) -> Result<usize> {
        let bundle = find_dxmt().context("DXMT is not installed. Run 'nucleon backends dxmt fetch' or configure via 'nucleon backends dxmt set-path <DIR>'")?;
        let _ = crate::vkd3d::unstage_vkd3d_proton_from_prefix(prefix, runner_dir);
        let _ = crate::d7vk::unstage_d7vk_from_prefix(prefix, runner_dir);
        let _ = crate::dxvk::unstage_dxvk_from_prefix(prefix, runner_dir);
        stage_dxmt_into_prefix(&bundle, prefix, runner_dir)
    }

    fn disable(&self, prefix: &Path, runner_dir: Option<&Path>) -> Result<usize> {
        unstage_dxmt_from_prefix(prefix, runner_dir)
    }

    fn apply_env(
        &self,
        env: &mut std::collections::HashMap<String, String>,
        runner_dir: &Path,
        _prefix_dir: &Path,
        steam_dir: &Path,
        client_path_str: &str,
        enable_hud: bool,
    ) {
        let bundle = find_dxmt();
        let mode = if bundle.as_ref().is_some_and(DxmtBundle::uses_builtin_dlls) {
            "b"
        } else {
            "n,b"
        };
        env.insert(
            "WINEDLLOVERRIDES".to_string(),
            format!("steamclient=n,b;steamclient64=n,b;lsteamclient=b;d3d11,dxgi,d3d10core={mode};winemetal=b;nvapi64,nvngx=n,b"),
        );

        if let Some(ref dxmt) = bundle {
            let old = env.get("WINEDLLPATH").cloned().unwrap_or_default();
            let mut dll_paths = vec![dxmt.root.to_string_lossy().to_string()];
            for sub in ["x86_64-windows", "i386-windows", "x86_64-unix"] {
                let dir = dxmt.root.join(sub);
                if dir.is_dir() {
                    dll_paths.push(dir.to_string_lossy().to_string());
                }
            }
            if !old.is_empty() {
                dll_paths.push(old);
            }
            env.insert("WINEDLLPATH".to_string(), dll_paths.join(":"));
        }

        if enable_hud {
            env.insert("MTL_HUD_ENABLED".to_string(), "1".to_string());
        }

        let lib_dir = runner_dir.join("lib");
        let lib_unix = runner_dir.join("lib/wine/x86_64-unix");
        let mut dyld_paths = vec![
            steam_dir.to_string_lossy().to_string(),
            client_path_str.to_string(),
            lib_unix.to_string_lossy().to_string(),
            lib_dir.to_string_lossy().to_string(),
        ];
        if let Some(dxmt) = bundle {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn metal_driver_requires_real_exports_and_intel_architecture() {
        let dir = tempdir().unwrap();
        let unix = dir.path().join("lib/wine/x86_64-unix");
        fs::create_dir_all(&unix).unwrap();
        let driver = unix.join("winemac.so");
        fs::write(&driver, b"macdrv_functions").unwrap();
        assert!(validate_metal_driver(dir.path()).is_err());
        let mut image = crate::runtime_checks::test_image("macdrv_functions", 1);
        fs::write(&driver, &image).unwrap();
        assert!(validate_metal_driver(dir.path()).is_ok());
        image[4..8].copy_from_slice(&0x0100000cu32.to_le_bytes());
        fs::write(&driver, image).unwrap();
        assert!(validate_metal_driver(dir.path()).is_err());
    }

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
