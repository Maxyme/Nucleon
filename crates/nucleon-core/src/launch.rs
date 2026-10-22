//! Resolve components once; preflight, staging and execution share the same selection.
use crate::{
    backend::GraphicsBackend,
    detector::{AutoBackends, GraphicsApi, TargetEngine},
    paths,
    runtime::{ExecutionMode, ResolvedRuntime, SteamProtocol},
    runtime_checks,
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone)]
pub struct VulkanDriver {
    pub manifest: PathBuf,
    pub library: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct GraphicsComponents {
    pub dxmt: Option<crate::dxmt::DxmtBundle>,
    pub dxvk: Option<crate::dxvk::DxvkBundle>,
    pub vkd3d: Option<crate::vkd3d::Vkd3dProtonBundle>,
    pub d7vk: Option<crate::d7vk::D7vkBundle>,
    pub vulkan: Vec<VulkanDriver>,
    pub gptk: bool,
}

impl GraphicsComponents {
    /// Standalone GPTK may use only the selected runtime's own matched components.
    pub fn bundled_gptk(runtime: &ResolvedRuntime) -> bool {
        runtime.mode == ExecutionMode::Intel
            && crate::runner::missing_d3dmetal_exports(&runtime.root).is_empty()
            && crate::runner::inspect_gptk_dir(&runtime.root).is_some_and(|(framework, shared)| {
                runtime_checks::native_arch(&framework.join("D3DMetal"), runtime.arch())
                    && runtime_checks::native_arch(&shared, runtime.arch())
                    && crate::runner::gptk_windows_binaries_dir(&framework, &shared).is_some_and(
                        |dir| {
                            ["d3d11.dll", "dxgi.dll", "d3d12.dll"]
                                .iter()
                                .all(|name| runtime_checks::pe(Some(&dir.join(name)), true))
                        },
                    )
            })
    }

    pub fn discover(
        runtime: &ResolvedRuntime,
        icd: Option<&Path>,
        dxvk: Option<&Path>,
    ) -> Result<Self> {
        fn bundle<T>(
            path: Option<&Path>,
            inspect: impl Fn(&Path) -> Option<T>,
            find: impl FnOnce() -> Option<T>,
        ) -> Result<Option<T>> {
            if let Some(path) = path {
                Ok(Some(inspect(path).with_context(|| {
                    format!("Invalid graphics bundle {}", path.display())
                })?))
            } else {
                Ok(find())
            }
        }
        let dxvk_override = std::env::var_os("DXVK_PATH").map(PathBuf::from);
        let dxvk_path = dxvk_override
            .as_deref()
            .or(dxvk)
            .or(runtime.graphics.dxvk.as_deref());
        let manifests = std::env::var_os("VK_DRIVER_FILES")
            .or_else(|| std::env::var_os("VK_ICD_FILENAMES"))
            .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
            .unwrap_or_else(|| {
                icd.or(runtime.graphics.vulkan_icd.as_deref())
                    .map(Path::to_path_buf)
                    .or_else(crate::runner::find_kosmickrisp_icd)
                    .into_iter()
                    .collect()
            });
        let vulkan = manifests
            .into_iter()
            .map(|path| -> Result<_> {
                let manifest = path
                    .canonicalize()
                    .with_context(|| format!("Resolve Vulkan ICD {}", path.display()))?;
                let library = crate::configuration::resolve_vulkan_icd(&manifest)?.library;
                Ok(VulkanDriver { manifest, library })
            })
            .collect::<Result<Vec<_>>>()?;
        let dxmt_override = std::env::var_os("DXMT_PATH").map(PathBuf::from);
        let vkd3d_override = std::env::var_os("VKD3D_PROTON_PATH")
            .or_else(|| std::env::var_os("VKD3D_PATH"))
            .map(PathBuf::from);
        let mut selected = Self {
            dxmt: if runtime.graphics.frontend_abi == crate::runtime::FrontendAbi::Arm64Ec {
                None
            } else {
                bundle(
                    dxmt_override
                        .as_deref()
                        .or(runtime.graphics.dxmt.as_deref()),
                    crate::dxmt::inspect_candidate_dir,
                    crate::dxmt::find_dxmt,
                )?
            },
            dxvk: bundle(
                dxvk_path,
                crate::dxvk::inspect_candidate_dir,
                crate::dxvk::find_dxvk,
            )?,
            vkd3d: bundle(
                vkd3d_override
                    .as_deref()
                    .or(runtime.graphics.vkd3d.as_deref()),
                crate::vkd3d::inspect_candidate_dir,
                crate::vkd3d::find_vkd3d_proton,
            )?,
            d7vk: if runtime.graphics.frontend_abi == crate::runtime::FrontendAbi::X64 {
                crate::d7vk::find_d7vk()
            } else {
                None
            },
            vulkan,
            gptk: runtime.mode == ExecutionMode::Intel
                && crate::runner::find_gptk_components(None)
                    .ok()
                    .flatten()
                    .is_some_and(|(framework, shared)| {
                        runtime_checks::native_arch(&framework.join("D3DMetal"), runtime.arch())
                            && runtime_checks::native_arch(&shared, runtime.arch())
                            && crate::runner::gptk_windows_binaries_dir(&framework, &shared)
                                .is_some_and(|dir| {
                                    ["d3d11.dll", "dxgi.dll", "d3d12.dll"]
                                        .iter()
                                        .all(|n| runtime_checks::pe(Some(&dir.join(n)), true))
                                })
                    })
                && crate::runner::missing_d3dmetal_exports(&runtime.root).is_empty(),
        };
        // Pin source symlinks before preflight so a later "current" switch cannot
        // select another package during staging or environment construction.
        let pin = |p: &mut PathBuf| -> Result<()> {
            *p = p
                .canonicalize()
                .with_context(|| format!("Resolve graphics component {}", p.display()))?;
            Ok(())
        };
        if let Some(b) = &mut selected.dxmt {
            let native = b.root.join(runtime.unix_name()).join("winemetal.so");
            if native.is_file() {
                b.x64_winemetal_so = Some(native);
            }
            pin(&mut b.root)?;
            for path in [
                &mut b.x64_d3d11,
                &mut b.x64_d3d10core,
                &mut b.x64_dxgi,
                &mut b.x64_winemetal,
                &mut b.x64_winemetal_so,
                &mut b.x86_d3d11,
                &mut b.x86_d3d10core,
                &mut b.x86_dxgi,
                &mut b.x86_winemetal,
            ]
            .into_iter()
            .flatten()
            {
                pin(path)?;
            }
        }
        if let Some(b) = &mut selected.dxvk {
            pin(&mut b.root)?;
            for path in [
                &mut b.x64_d3d11,
                &mut b.x64_d3d10core,
                &mut b.x64_d3d9,
                &mut b.x64_dxgi,
                &mut b.x86_d3d11,
                &mut b.x86_d3d10core,
                &mut b.x86_d3d9,
                &mut b.x86_dxgi,
            ]
            .into_iter()
            .flatten()
            {
                pin(path)?;
            }
        }
        if let Some(b) = &mut selected.vkd3d {
            pin(&mut b.root)?;
            pin(&mut b.x64_d3d12)?;
            for path in [&mut b.x64_d3d12core, &mut b.x86_d3d12, &mut b.x86_d3d12core]
                .into_iter()
                .flatten()
            {
                pin(path)?;
            }
        }
        if let Some(b) = &mut selected.d7vk {
            pin(&mut b.root)?;
            pin(&mut b.x86_ddraw)?;
        }
        crate::frontend::validate_selected(runtime, &selected)?;
        Ok(selected)
    }

    pub fn available(
        &self,
        runtime: &ResolvedRuntime,
        api: &GraphicsApi,
        is64: bool,
    ) -> AutoBackends {
        let pe =
            |p: Option<&Path>| runtime_checks::graphics_pe(p, is64, runtime.graphics.frontend_abi);
        let dxvk = self.dxvk.as_ref().is_some_and(|b| {
            let (d9, d10, d11, dxgi) = if is64 {
                (&b.x64_d3d9, &b.x64_d3d10core, &b.x64_d3d11, &b.x64_dxgi)
            } else {
                (&b.x86_d3d9, &b.x86_d3d10core, &b.x86_d3d11, &b.x86_dxgi)
            };
            match api {
                GraphicsApi::DirectX9OrOlder => pe(d9.as_deref()),
                GraphicsApi::DirectX10 => {
                    pe(d10.as_deref()) && pe(d11.as_deref()) && pe(dxgi.as_deref())
                }
                _ => pe(d11.as_deref()) && pe(dxgi.as_deref()),
            }
        });
        let allowed = |b| runtime.backends.contains(&b);
        AutoBackends {
            gptk: is64 && self.gptk && allowed(GraphicsBackend::D3DMetal),
            dxmt: allowed(GraphicsBackend::Dxmt)
                && self.dxmt.as_ref().is_some_and(|b| {
                    let (d3d11, dxgi, metal) = if is64 {
                        (&b.x64_d3d11, &b.x64_dxgi, &b.x64_winemetal)
                    } else {
                        (&b.x86_d3d11, &b.x86_dxgi, &b.x86_winemetal)
                    };
                    pe(d3d11.as_deref())
                        && pe(dxgi.as_deref())
                        && pe(metal.as_deref())
                        && crate::dxmt::validate_runtime(b, Some(&runtime.root)).is_ok()
                        && b.x64_winemetal_so
                            .as_deref()
                            .is_some_and(|p| runtime_checks::native_arch(p, runtime.arch()))
                }),
            dxvk: dxvk && allowed(GraphicsBackend::Dxvk),
            vkd3d: allowed(GraphicsBackend::KosmicKrisp)
                && self.vkd3d.as_ref().is_some_and(|b| {
                    if is64 {
                        pe(Some(&b.x64_d3d12)) && pe(b.x64_d3d12core.as_deref())
                    } else {
                        pe(b.x86_d3d12.as_deref()) && pe(b.x86_d3d12core.as_deref())
                    }
                })
                && self.dxvk.as_ref().is_some_and(|b| {
                    pe(if is64 {
                        b.x64_dxgi.as_deref()
                    } else {
                        b.x86_dxgi.as_deref()
                    })
                }),
            vulkan: !self.vulkan.is_empty()
                && (runtime.mode == ExecutionMode::Intel
                    || runtime.graphics.vulkan_loader.is_some())
                && runtime_checks::native_arch(
                    &runtime.unix_dir.join("winevulkan.so"),
                    runtime.arch(),
                )
                && self
                    .vulkan
                    .iter()
                    .all(|v| runtime_checks::vulkan_arch(&v.manifest, runtime.arch())),
            d7vk: !is64 && self.d7vk.as_ref().is_some_and(|b| pe(Some(&b.x86_ddraw))),
        }
    }
}

/// Optional producer extensions, detected from the native ntdll image once per plan.
#[derive(Debug, Default)]
pub struct RuntimeFeatures {
    pub dll_search_prepend: bool,
    pub guest_supervisor: bool,
}
impl RuntimeFeatures {
    fn inspect(runtime: &ResolvedRuntime) -> Self {
        let Ok(image) = fs::read(runtime.unix_dir.join("ntdll.so")) else {
            return Self::default();
        };
        Self {
            dll_search_prepend: runtime_checks::has_export(
                &image,
                &["isotope_dll_search_prepend_v1"],
            ),
            guest_supervisor: runtime_checks::has_export(&image, &["isotope_guest_supervisor_v1"]),
        }
    }
}

#[derive(Debug)]
pub struct LaunchPlan {
    pub runtime: ResolvedRuntime,
    pub prefix: PathBuf,
    pub engine: TargetEngine,
    pub graphics: GraphicsComponents,
    pub bridge: PathBuf,
    pub is64: bool,
    pub use_steam_helper: bool,
    /// Runtime Steam capabilities are used only for Steam-origin launches.
    pub steam_integration: bool,
    pub log_directory: Option<PathBuf>,
    pub diagnostics: bool,
    pub runtime_features: RuntimeFeatures,
}

impl LaunchPlan {
    pub fn new(
        runtime: ResolvedRuntime,
        prefix: PathBuf,
        engine: TargetEngine,
        graphics: GraphicsComponents,
        is64: bool,
        helper: Option<bool>,
        api: &GraphicsApi,
    ) -> Result<Self> {
        runtime.require_guest(is64)?;
        crate::frontend::validate_selected(&runtime, &graphics)?;
        let available = graphics.available(&runtime, api, is64);
        let usable = match engine {
            TargetEngine::Dxmt => available.dxmt,
            TargetEngine::Dxvk => available.dxvk && available.vulkan,
            TargetEngine::Vkd3d => available.vkd3d && available.vulkan,
            TargetEngine::KosmicKrisp => {
                available.vulkan
                    && match api {
                        GraphicsApi::DirectX12 => available.vkd3d,
                        GraphicsApi::DirectX11
                        | GraphicsApi::DirectX10
                        | GraphicsApi::DirectX9OrOlder => available.dxvk,
                        GraphicsApi::DirectX7OrOlder => available.d7vk,
                        _ => true,
                    }
            }
            TargetEngine::Gptk => {
                runtime.mode == ExecutionMode::Intel
                    && runtime.backends.contains(&GraphicsBackend::D3DMetal)
            }
            _ => runtime.backends.contains(&GraphicsBackend::Auto),
        };
        ensure!(
            usable,
            "Selected {:?} graphics components are missing or incompatible with {:?} Wine/guest ABI",
            engine,
            runtime.mode
        );
        let use_steam_helper = helper.unwrap_or(runtime.steam_helper || !is64);
        let runtime_features = RuntimeFeatures::inspect(&runtime);
        Ok(Self {
            runtime_features,
            runtime,
            prefix: std::path::absolute(prefix)?,
            engine,
            graphics,
            bridge: std::env::var_os("NUCLEON_STEAM_BRIDGE")
                .map(PathBuf::from)
                .unwrap_or_else(paths::bridge_dir),
            is64,
            use_steam_helper,
            steam_integration: true,
            log_directory: None,
            diagnostics: false,
        })
    }

    pub fn prepare_prefix(&self) -> Result<()> {
        crate::prefix::ensure_prefix_for_plan(self)
    }

    pub fn without_steam(mut self) -> Self {
        self.steam_integration = false;
        self.use_steam_helper = false;
        self
    }

    fn search_dxmt_builtins(&self) -> bool {
        self.runtime_features.dll_search_prepend
            && self.engine == TargetEngine::Dxmt
            && self
                .graphics
                .dxmt
                .as_ref()
                .is_some_and(|b| b.all_builtin_dlls())
    }

    pub fn stage_graphics(&self) -> Result<usize> {
        use crate::staging::Desired;
        use std::collections::BTreeMap;
        let windows = self.prefix.join("drive_c/windows");
        let mut desired = BTreeMap::new();
        let only32 = !self.is64 && !windows.join("syswow64").is_dir();
        // Resolve the final layout first. Avoid restoring builtins underneath an
        // unchanged translator, or copying a DLL twice during engine switches.
        for (subdir, x86) in [("system32", false), ("syswow64", true)] {
            let target = windows.join(subdir);
            if !target.is_dir() {
                continue;
            }
            let x86 = x86 || only32;
            for name in crate::backend_files::DLLS {
                let builtin = self
                    .runtime
                    .windows_dirs
                    .iter()
                    .filter(|p| {
                        p.file_name()
                            .is_some_and(|n| n == "i386-windows" || n == "x86-windows")
                            == x86
                    })
                    .map(|p| p.join(name))
                    .find(|p| p.is_file());
                desired.insert(
                    target.join(name),
                    builtin.map(Desired::Copy).unwrap_or(Desired::Absent),
                );
            }
        }
        let mut add = |subdir: &str, files: &[(&str, Option<&Path>)]| {
            if only32 && subdir == "system32" {
                return;
            }
            let target = windows.join(if only32 && subdir == "syswow64" {
                "system32"
            } else {
                subdir
            });
            if target.is_dir() {
                for (name, source) in files {
                    if let Some(source) = source {
                        desired.insert(target.join(name), Desired::Copy(source.to_path_buf()));
                    }
                }
            }
        };
        if self.engine == TargetEngine::Dxmt && !self.search_dxmt_builtins() {
            let b = self
                .graphics
                .dxmt
                .as_ref()
                .context("Missing resolved DXMT")?;
            for (subdir, files) in [
                (
                    "system32",
                    [
                        ("d3d11.dll", b.x64_d3d11.as_deref()),
                        ("d3d10core.dll", b.x64_d3d10core.as_deref()),
                        ("dxgi.dll", b.x64_dxgi.as_deref()),
                        ("winemetal.dll", b.x64_winemetal.as_deref()),
                    ],
                ),
                (
                    "syswow64",
                    [
                        ("d3d11.dll", b.x86_d3d11.as_deref()),
                        ("d3d10core.dll", b.x86_d3d10core.as_deref()),
                        ("dxgi.dll", b.x86_dxgi.as_deref()),
                        ("winemetal.dll", b.x86_winemetal.as_deref()),
                    ],
                ),
            ] {
                add(subdir, &files);
            }
        }
        let vulkan = matches!(
            self.engine,
            TargetEngine::Dxvk | TargetEngine::Vkd3d | TargetEngine::KosmicKrisp
        );
        if vulkan {
            if let Some(b) = &self.graphics.dxvk {
                add(
                    "system32",
                    &[
                        ("d3d11.dll", b.x64_d3d11.as_deref()),
                        ("d3d10core.dll", b.x64_d3d10core.as_deref()),
                        ("d3d9.dll", b.x64_d3d9.as_deref()),
                        ("dxgi.dll", b.x64_dxgi.as_deref()),
                    ],
                );
                add(
                    "syswow64",
                    &[
                        ("d3d11.dll", b.x86_d3d11.as_deref()),
                        ("d3d10core.dll", b.x86_d3d10core.as_deref()),
                        ("d3d9.dll", b.x86_d3d9.as_deref()),
                        ("dxgi.dll", b.x86_dxgi.as_deref()),
                    ],
                );
            }
            if self.engine != TargetEngine::Dxvk {
                if let Some(b) = &self.graphics.vkd3d {
                    add(
                        "system32",
                        &[
                            ("d3d12.dll", Some(&b.x64_d3d12)),
                            ("d3d12core.dll", b.x64_d3d12core.as_deref()),
                        ],
                    );
                    add(
                        "syswow64",
                        &[
                            ("d3d12.dll", b.x86_d3d12.as_deref()),
                            ("d3d12core.dll", b.x86_d3d12core.as_deref()),
                        ],
                    );
                }
                if let Some(b) = &self.graphics.d7vk {
                    add(
                        if windows.join("syswow64").is_dir() || only32 {
                            "syswow64"
                        } else {
                            "system32"
                        },
                        &[("ddraw.dll", Some(&b.x86_ddraw))],
                    );
                }
            }
        }
        let alias = self.prefix.join(".nucleon-vulkan/libMoltenVK.dylib");
        desired.insert(
            alias,
            if vulkan {
                self.graphics
                    .vulkan
                    .first()
                    .map(|d| Desired::Link(d.library.clone()))
                    .unwrap_or(Desired::Absent)
            } else {
                Desired::Absent
            },
        );
        crate::staging::files(
            &self.prefix.join(".nucleon-prepared/graphics.json"),
            &desired,
            |operation| crate::backend_files::transaction(&self.prefix, operation),
        )
    }

    /// Bootstrap deliberately excludes game DLL overrides, overlay and client redirection.
    pub fn bootstrap_env(&self) -> HashMap<String, String> {
        let r = &self.runtime;
        let mut env = HashMap::from([
            ("WINEPREFIX".into(), self.prefix.display().to_string()),
            ("WINELOADER".into(), r.loader.display().to_string()),
            ("WINESERVER".into(), r.server.display().to_string()),
            ("WINEDLLPATH".into(), join(&r.windows_dirs)),
            ("DYLD_LIBRARY_PATH".into(), r.unix_dir.display().to_string()),
            (
                "DYLD_FALLBACK_LIBRARY_PATH".into(),
                r.root.join("lib").display().to_string(),
            ),
            (
                "WINEDLLOVERRIDES".into(),
                "winemenubuilder.exe=d;mscoree=d;mshtml=d".into(),
            ),
        ]);
        env.insert(
            "WINEDEBUG".into(),
            if self.diagnostics {
                "-all,warn+all,err+all,+timestamp,+loaddll,+seh"
            } else {
                "-all,warn+all,err+all"
            }
            .into(),
        );
        if let Some(directory) = &self.log_directory {
            env.insert(
                "NUCLEON_SETUP_LOG".into(),
                directory.join("setup.log").display().to_string(),
            );
        }
        env.extend(r.environment.iter().map(|(k, v)| (k.clone(), v.clone())));
        // Wineboot may start the prefix's persistent server. Its synchronization
        // mode must match the subsequent game process, especially older GPTK Wine.
        if r.mode == ExecutionMode::Intel {
            env.insert("ROSETTA_ADVERTISE_AVX".into(), "1".into());
            env.insert("WINEMSYNC".into(), "1".into());
            env.insert("WINEESYNC".into(), "1".into());
        }
        // Set ownership after the descriptor environment: a runtime cannot redirect
        // the watcher to an inherited or unrelated PID.
        if self.runtime_features.guest_supervisor {
            env.insert(
                "ISOTOPE_SUPERVISOR_PID".into(),
                std::process::id().to_string(),
            );
        }
        if self.diagnostics {
            env.insert("ISOTOPE_FAULT_DIAGNOSTICS".into(), "1".into());
        }
        env
    }

    pub fn execution_env(&self, hud: bool, overlay: bool) -> HashMap<String, String> {
        let mut env = self.bootstrap_env();
        let r = &self.runtime;
        let steam = self.prefix.join("drive_c/Program Files (x86)/Steam");
        let client = paths::steam_data_dir().join("Steam.AppBundle/Steam/Contents/MacOS");
        if self.steam_integration {
            env.insert(
                "STEAM_COMPAT_CLIENT_INSTALL_PATH".into(),
                std::env::var("STEAM_COMPAT_CLIENT_INSTALL_PATH")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| client.display().to_string()),
            );
        }
        let mut dlls = r.windows_dirs.clone();
        let mut libs = vec![r.unix_dir.clone(), r.root.join("lib")];
        if self.steam_integration {
            dlls.insert(0, steam.clone());
            libs.insert(0, steam.clone());
            libs.push(client.clone());
        }
        let mut overrides = if !self.steam_integration {
            String::new()
        } else if r.mode == ExecutionMode::Arm64Fex {
            "steamclient=n,b;steamclient64=n,b;lsteamclient=n,b".to_string()
        } else {
            "steamclient=n,b;steamclient64=n,b;lsteamclient=b".to_string()
        };
        match self.engine {
            TargetEngine::Gptk => {
                crate::runner::apply_gptk_execution_env(
                    &mut env,
                    &r.root,
                    &steam,
                    &client.to_string_lossy(),
                    hud,
                );
                if !self.steam_integration {
                    env.insert(
                        "WINEDLLOVERRIDES".into(),
                        "d3d11,dxgi,d3d12,d3d10core,d3dcompiler_47=n,b;nvapi64,nvngx=n,b".into(),
                    );
                    let native = vec![
                        r.root.join("lib/external"),
                        r.root
                            .join("lib/external/D3DMetal.framework/Versions/Current/Resources"),
                        r.unix_dir.clone(),
                        r.root.join("lib"),
                    ];
                    env.insert("DYLD_FALLBACK_LIBRARY_PATH".into(), join(&native));
                    env.insert("DYLD_LIBRARY_PATH".into(), join(&native));
                }
            }
            TargetEngine::Dxmt => {
                let b = self.graphics.dxmt.as_ref().expect("preflight DXMT");
                dlls.insert(0, b.root.clone());
                for path in [b.x64_d3d11.as_deref(), b.x86_d3d11.as_deref()]
                    .into_iter()
                    .flatten()
                {
                    if let Some(parent) = path.parent() {
                        let parent = parent.to_path_buf();
                        if !dlls.contains(&parent) {
                            dlls.insert(0, parent);
                        }
                    }
                }
                if let Some(so) = &b.x64_winemetal_so {
                    libs.insert(0, so.parent().unwrap().to_path_buf());
                }
                if self.search_dxmt_builtins() {
                    let paths: Vec<_> = dlls
                        .iter()
                        .filter(|p| p.starts_with(&b.root))
                        .cloned()
                        .collect();
                    env.insert("WINEDLLPATH_PREPEND".into(), join(&paths));
                }
                overrides.push_str(
                    if b.uses_builtin_dlls()
                        && (r.mode == ExecutionMode::Intel || self.search_dxmt_builtins())
                    {
                        ";d3d11,dxgi,d3d10core,winemetal=b"
                    } else {
                        ";d3d11,dxgi,d3d10core,winemetal=n,b"
                    },
                );
            }
            TargetEngine::Dxvk | TargetEngine::Vkd3d | TargetEngine::KosmicKrisp => {
                let icds = self
                    .graphics
                    .vulkan
                    .iter()
                    .map(|d| d.manifest.clone())
                    .collect::<Vec<_>>();
                env.insert("VK_DRIVER_FILES".into(), join(&icds));
                env.insert("VK_ICD_FILENAMES".into(), join(&icds));
                // Driver directories must precede the SDK loader's directory: both
                // may contain a same-name driver with different patch provenance.
                libs.insert(0, self.prefix.join(".nucleon-vulkan"));
                for driver in &self.graphics.vulkan {
                    libs.insert(0, driver.library.parent().unwrap().to_path_buf());
                }
                if let Some(loader) = &r.graphics.vulkan_loader {
                    libs.push(loader.parent().unwrap().to_path_buf());
                }
                overrides.push_str(";winevulkan,vulkan-1=b");
                if self.engine != TargetEngine::Dxvk && self.graphics.vkd3d.is_some() {
                    overrides.push_str(";d3d12,d3d12core=n,b");
                }
                if self.graphics.dxvk.is_some() {
                    overrides.push_str(";d3d11,dxgi,d3d10core,d3d9=n,b");
                }
                if self.engine != TargetEngine::Dxvk && self.graphics.d7vk.is_some() {
                    overrides.push_str(";ddraw=n,b");
                }
                if let Some(b) = &self.graphics.dxvk {
                    let conf = b.root.join("dxvk.conf");
                    if conf.is_file() && std::env::var_os("DXVK_CONFIG_FILE").is_none() {
                        env.insert("DXVK_CONFIG_FILE".into(), conf.display().to_string());
                    }
                }
            }
            _ => overrides.push_str(";d3d9,d3d10,d3d10_1,d3d10core=b,n"),
        }
        if self.engine != TargetEngine::Gptk {
            env.insert(
                "WINEDLLOVERRIDES".into(),
                overrides.trim_start_matches(';').into(),
            );
        }
        if self.steam_integration && r.steam_protocol == SteamProtocol::ValveRedirect {
            env.insert("NUCLEON_STEAMCLIENT_REDIRECT".into(), "1".into());
            env.entry("WINEDLLOVERRIDES".into())
                .or_default()
                .push_str(";steamclient64=n;lsteamclient=n,b");
        }
        if self.engine != TargetEngine::Gptk {
            env.insert("DYLD_LIBRARY_PATH".into(), join(&libs));
            env.insert("DYLD_FALLBACK_LIBRARY_PATH".into(), join(&libs));
        }
        // Complete toolkit bundles resolve their matched native framework through
        // their own loader paths. Generic Wine paths can break that resolution
        // (notably GPTK 4's dxgi initialization on older bundled Wine).
        if !self.steam_integration
            && self.engine == TargetEngine::WineStaging
            && r.mode == ExecutionMode::Intel
            && r.root.join("lib/external/D3DMetal.framework").is_dir()
        {
            env.remove("DYLD_LIBRARY_PATH");
            env.remove("DYLD_FALLBACK_LIBRARY_PATH");
        }
        env.insert("WINEDLLPATH".into(), join(&dlls));
        env.insert(
            "PATH".into(),
            format!(
                "{}:{}",
                r.root.join("bin").display(),
                crate::runner::keep_paths_for_runtime(
                    &std::env::var("PATH").unwrap_or_default(),
                    &r.root
                )
            ),
        );
        if hud {
            env.insert("MTL_HUD_ENABLED".into(), "1".into());
        }
        if self.steam_integration
            && overlay
            && client.join("gameoverlayrenderer.dylib").is_file()
            && runtime_checks::native_arch(&client.join("gameoverlayrenderer.dylib"), r.arch())
        {
            env.insert(
                "DYLD_INSERT_LIBRARIES".into(),
                client
                    .join("gameoverlayrenderer.dylib")
                    .display()
                    .to_string(),
            );
        }
        // Avoid per-frame/info chatter during ordinary launches, including GPTK.
        env.insert("WINEDEBUG".into(), "-all,warn+all,err+all".into());
        for name in [
            "VKD3D_DEBUG",
            "VKD3D_SHADER_DEBUG",
            "DXVK_LOG_LEVEL",
            "DXMT_LOG_LEVEL",
        ] {
            env.insert(name.into(), "warn".into());
        }
        env.insert("DXVK_LOG_PATH".into(), "none".into());
        env.insert("DXMT_LOG_PATH".into(), "none".into());
        if let Ok(debug) = std::env::var("NUCLEON_WINEDEBUG") {
            env.insert("WINEDEBUG".into(), debug);
        }
        env
    }

    pub fn configure_launch_environment(&self, env: &mut HashMap<String, String>) -> Result<()> {
        self.configure_environment(env, crate::configuration::environment)
    }

    fn configure_environment(
        &self,
        env: &mut HashMap<String, String>,
        mut setting: impl FnMut(&str) -> Result<Option<String>>,
    ) -> Result<()> {
        if self.diagnostics {
            env.insert(
                "WINEDEBUG".into(),
                "-all,warn+all,err+all,+timestamp,+loaddll,+seh,+process,+msg".into(),
            );
            env.insert("VK_LOADER_DEBUG".into(), "error,warn,driver".into());
            env.insert("NUCLEON_TRACE_PROCESS_EXIT".into(), "1".into());
            for name in [
                "VKD3D_DEBUG",
                "VKD3D_SHADER_DEBUG",
                "DXVK_LOG_LEVEL",
                "DXMT_LOG_LEVEL",
            ] {
                env.insert(name.into(), "info".into());
            }
            if let Some(directory) = &self.log_directory {
                let windows = format!(
                    "Z:{}",
                    directory
                        .canonicalize()?
                        .display()
                        .to_string()
                        .replace('/', "\\")
                );
                env.insert("DXVK_LOG_PATH".into(), windows.clone());
                env.insert("DXMT_LOG_PATH".into(), windows);
            }
        }
        if let Some(value) = setting("NUCLEON_WINEDEBUG")? {
            anyhow::ensure!(
                !value.trim().is_empty(),
                "NUCLEON_WINEDEBUG must name Wine channels"
            );
            env.insert("WINEDEBUG".into(), value);
        }
        if let Some(value) = setting("NUCLEON_WINEDLLOVERRIDES")? {
            anyhow::ensure!(
                !value.trim().is_empty(),
                "NUCLEON_WINEDLLOVERRIDES must name DLL overrides"
            );
            let overrides = env.entry("WINEDLLOVERRIDES".into()).or_default();
            if !overrides.is_empty() {
                overrides.push(';');
            }
            overrides.push_str(&value);
        }
        if let Some(value) = setting("NUCLEON_TRACE_PROCESS_EXIT")? {
            if crate::configuration::boolean("NUCLEON_TRACE_PROCESS_EXIT", &value)? {
                env.insert("NUCLEON_TRACE_PROCESS_EXIT".into(), "1".into());
            } else {
                env.remove("NUCLEON_TRACE_PROCESS_EXIT");
            }
        }
        for name in FEATURE_OVERRIDES {
            let explicit = setting(&format!("NUCLEON_{name}"))?;
            let value = if explicit.is_some() {
                explicit
            } else if self.runtime.mode == ExecutionMode::Intel {
                setting(name)?
            } else {
                None
            };
            if let Some(value) = value {
                env.insert((*name).into(), value);
            }
        }
        Ok(())
    }

    pub fn wine_args(&self, args: &[String]) -> Result<Vec<String>> {
        if !self.steam_integration || !self.use_steam_helper {
            return Ok(args.to_vec());
        }
        self.validate_steam_helper()?;
        let mut result = vec!["C:\\Program Files (x86)\\Steam\\steam.exe".into()];
        let mut args = args.to_vec();
        if let Some(exe) = args.first_mut()
            && (Path::new(exe).is_absolute() || Path::new(exe).is_file())
        {
            let canonical = Path::new(exe)
                .canonicalize()
                .with_context(|| format!("Cannot resolve game executable path: {exe}"))?;
            *exe = format!("Z:{}", canonical.display().to_string().replace('/', "\\"));
        }
        result.extend(args);
        Ok(result)
    }

    pub(crate) fn validate_steam_helper(&self) -> Result<()> {
        let path = self.bridge.join("steam.exe");
        let bytes = fs::read(&path)
            .with_context(|| format!("Selected Steam protocol requires {}", path.display()))?;
        match runtime_checks::pe_abi(&bytes)? {
            runtime_checks::PeAbi::X64 => self.runtime.require_guest(true),
            runtime_checks::PeAbi::X86 => self.runtime.require_guest(false),
            _ => anyhow::bail!(
                "Steam helper requires a supported guest PE ABI: {}",
                path.display()
            ),
        }
    }
}

fn join(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(":")
}

pub const FEATURE_OVERRIDES: &[&str] = &[
    "ROSETTA_ADVERTISE_AVX_COPY",
    "D3DM_MTL4",
    "D3DM_SUPPORT_DXR",
    "VKD3D_CONFIG",
    "VKD3D_FEATURE_LEVEL",
    "VKD3D_SHADER_MODEL",
    "MESA_VK_VERSION_OVERRIDE",
    "MVK_CONFIG_ADVERTISE_API_VERSION",
];
const DIAGNOSTIC_VARIABLES: &[&str] = &[
    "VK_LOADER_DEBUG",
    "VKD3D_DEBUG",
    "VKD3D_SHADER_DEBUG",
    "VKD3D_LOG_FILE",
    "DXVK_LOG_LEVEL",
    "DXVK_LOG_PATH",
    "DXMT_LOG_LEVEL",
    "DXMT_LOG_PATH",
    "NUCLEON_TRACE_PROCESS_EXIT",
    "ISOTOPE_SUPERVISOR_PID",
    "ISOTOPE_FAULT_DIAGNOSTICS",
    "WINEDLLPATH_PREPEND",
];

/// Strip foreign Wine/FEX/dyld state without changing the parent's environment.
pub fn clean_command(command: &mut Command) {
    // Remove explicit command entries too, even if absent from the parent.
    for name in FEATURE_OVERRIDES.iter().chain(DIAGNOSTIC_VARIABLES) {
        command.env_remove(name);
    }
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if [
            "WINE",
            "DYLD_",
            "FEX_",
            "MACRUNNER_",
            "ROSETTA_",
            "ISOTOPE_",
        ]
        .iter()
        .any(|p| name.starts_with(p))
            || name == "STEAM_DYLD_INSERT_LIBRARIES"
            || FEATURE_OVERRIDES.contains(&name.as_ref())
            || DIAGNOSTIC_VARIABLES.contains(&name.as_ref())
        {
            command.env_remove(key);
        }
    }
}

/// Standalone children must not acquire a parent's Steam identity or bridge settings.
pub fn clean_standalone_command(command: &mut Command) {
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("STEAM_") || name.starts_with("Steam") {
            command.env_remove(key);
        }
    }
    for key in [
        "SteamAppId",
        "SteamGameId",
        "NUCLEON_STEAMCLIENT_REDIRECT",
        "NUCLEON_STEAM_SHIM",
        "NUCLEON_FEX_SELECTION",
        "NUCLEON_GAME_PROFILE",
        "NUCLEON_OVERLAY",
        "MTL_HUD_ENABLED",
    ] {
        command.env_remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_gptk_environment_keeps_metal_and_removes_steam() {
        let (_temp, mut runtime) = runtime::fixture(ExecutionMode::Intel);
        assert!(!GraphicsComponents::bundled_gptk(&runtime));
        runtime.backends.push(GraphicsBackend::D3DMetal);
        let plan = LaunchPlan::new(
            runtime,
            PathBuf::from("/tmp/test-prefix"),
            TargetEngine::Gptk,
            GraphicsComponents::default(),
            true,
            Some(false),
            &GraphicsApi::DirectX12,
        )
        .unwrap()
        .without_steam();
        let environment = plan.execution_env(false, false);
        assert!(environment["WINEDLLOVERRIDES"].contains("d3d12"));
        assert!(environment["DYLD_FALLBACK_LIBRARY_PATH"].contains("D3DMetal.framework"));
        for key in [
            "WINEDLLOVERRIDES",
            "DYLD_LIBRARY_PATH",
            "DYLD_FALLBACK_LIBRARY_PATH",
            "WINEDLLPATH",
        ] {
            assert!(!environment[key].to_lowercase().contains("steam"));
        }
        assert!(!environment.contains_key("STEAM_COMPAT_CLIENT_INSTALL_PATH"));
        let bootstrap = plan.bootstrap_env();
        for key in ["WINEESYNC", "WINEMSYNC", "ROSETTA_ADVERTISE_AVX"] {
            assert_eq!(bootstrap[key], "1");
            assert_eq!(bootstrap[key], environment[key]);
        }
        let mut configured = environment.clone();
        plan.configure_environment(&mut configured, |key| {
            Ok(match key {
                "NUCLEON_WINEDLLOVERRIDES" => Some("d3dcompiler_47=n,b".into()),
                "NUCLEON_ROSETTA_ADVERTISE_AVX_COPY" => Some("1".into()),
                _ => None,
            })
        })
        .unwrap();
        assert!(configured["WINEDLLOVERRIDES"].contains("d3d12"));
        assert!(configured["WINEDLLOVERRIDES"].ends_with("d3dcompiler_47=n,b"));
        assert_eq!(configured["ROSETTA_ADVERTISE_AVX_COPY"], "1");
    }

    #[test]
    fn standalone_toolkit_preserves_bundled_native_library_resolution() {
        let (_temp, runtime) = runtime::fixture(ExecutionMode::Intel);
        fs::create_dir_all(runtime.root.join("lib/external/D3DMetal.framework")).unwrap();
        let plan = LaunchPlan::new(
            runtime,
            PathBuf::from("/tmp/standalone-prefix"),
            TargetEngine::WineStaging,
            GraphicsComponents::default(),
            true,
            None,
            &GraphicsApi::Unknown,
        )
        .unwrap()
        .without_steam();
        let env = plan.execution_env(true, false);
        assert!(!env.contains_key("DYLD_LIBRARY_PATH"));
        assert!(!env.contains_key("DYLD_FALLBACK_LIBRARY_PATH"));
        assert!(env.contains_key("WINEDLLPATH"));
        assert_eq!(env["WINEESYNC"], plan.bootstrap_env()["WINEESYNC"]);
        assert_eq!(env["MTL_HUD_ENABLED"], "1");
    }

    #[test]
    fn standalone_ignores_runtime_steam_capabilities_and_needs_no_bridge() {
        let (_temp, mut runtime) = runtime::fixture(ExecutionMode::Arm64Fex);
        runtime.steam_helper = true;
        runtime.steam_protocol = SteamProtocol::ValveRedirect;
        let plan = LaunchPlan::new(
            runtime,
            PathBuf::from("/tmp/standalone-prefix"),
            TargetEngine::WineStaging,
            GraphicsComponents::default(),
            true,
            None,
            &GraphicsApi::Unknown,
        )
        .unwrap()
        .without_steam();
        let env = plan.execution_env(false, true);
        for key in [
            "STEAM_COMPAT_CLIENT_INSTALL_PATH",
            "NUCLEON_STEAMCLIENT_REDIRECT",
            "DYLD_INSERT_LIBRARIES",
        ] {
            assert!(!env.contains_key(key), "{key}");
        }
        for key in ["WINEDLLPATH", "DYLD_LIBRARY_PATH", "WINEDLLOVERRIDES"] {
            assert!(!env[key].contains("Steam"), "{}", env[key]);
        }
        assert!(!env["WINEDLLOVERRIDES"].contains("steamclient"));
        let args = vec!["/tmp/Game.exe".into(), "argument with spaces".into()];
        assert_eq!(plan.wine_args(&args).unwrap(), args);
    }

    #[test]
    fn optional_runtime_extensions_are_detected_from_native_symbols() {
        let (_temp, runtime) = runtime::fixture(ExecutionMode::Arm64Fex);
        let path = runtime.unix_dir.join("ntdll.so");
        fs::write(&path, test_image("isotope_guest_supervisor_v1", 1)).unwrap();
        let features = RuntimeFeatures::inspect(&runtime);
        assert!(features.guest_supervisor);
        assert!(!features.dll_search_prepend);
        fs::write(&path, test_image("isotope_dll_search_prepend_v1", 1)).unwrap();
        let features = RuntimeFeatures::inspect(&runtime);
        assert!(!features.guest_supervisor);
        assert!(features.dll_search_prepend);
        fs::write(&path, b"ISOTOPE_SUPERVISOR_PID=1").unwrap();
        let features = RuntimeFeatures::inspect(&runtime);
        assert!(!features.guest_supervisor);
        assert!(!features.dll_search_prepend);
    }

    #[test]
    fn diagnostics_and_native_capability_forcing_require_explicit_opt_in() {
        let (temp, runtime) = runtime::fixture(ExecutionMode::Arm64Fex);
        let mut plan = LaunchPlan::new(
            runtime,
            temp.path().join("pfx"),
            TargetEngine::WineStaging,
            Default::default(),
            true,
            Some(false),
            &GraphicsApi::Unknown,
        )
        .unwrap();
        let mut normal = plan.execution_env(false, false);
        plan.configure_environment(&mut normal, |key| {
            Ok((key == "VKD3D_CONFIG").then(|| "dxr".into()))
        })
        .unwrap();
        assert_eq!(normal["WINEDEBUG"], "-all,warn+all,err+all");
        assert_eq!(normal["VKD3D_DEBUG"], "warn");
        assert_eq!(normal["DXVK_LOG_PATH"], "none");
        assert!(!normal.contains_key("VK_LOADER_DEBUG"));
        assert!(!normal.contains_key("NUCLEON_TRACE_PROCESS_EXIT"));
        assert!(!normal.contains_key("VKD3D_CONFIG"));

        plan.diagnostics = true;
        plan.log_directory = Some(temp.path().to_owned());
        let mut diagnostic = normal;
        plan.configure_environment(&mut diagnostic, |key| {
            Ok(match key {
                "NUCLEON_TRACE_PROCESS_EXIT" => Some("false".into()),
                "NUCLEON_VKD3D_CONFIG" => Some("dxr".into()),
                _ => None,
            })
        })
        .unwrap();
        assert!(diagnostic["WINEDEBUG"].contains("+loaddll"));
        assert_eq!(diagnostic["VKD3D_DEBUG"], "info");
        assert!(diagnostic["DXVK_LOG_PATH"].starts_with("Z:\\"));
        assert!(diagnostic.contains_key("VK_LOADER_DEBUG"));
        assert!(!diagnostic.contains_key("NUCLEON_TRACE_PROCESS_EXIT"));
        assert_eq!(diagnostic["VKD3D_CONFIG"], "dxr");
    }

    #[test]
    fn foreign_diagnostics_and_feature_overrides_are_scrubbed_before_policy() {
        let mut command = Command::new("/usr/bin/env");
        for name in FEATURE_OVERRIDES.iter().chain(DIAGNOSTIC_VARIABLES) {
            command.env(name, "foreign");
        }
        clean_command(&mut command);
        let output = command.output().unwrap();
        assert!(output.status.success());
        let output = String::from_utf8(output.stdout).unwrap();
        for name in FEATURE_OVERRIDES.iter().chain(DIAGNOSTIC_VARIABLES) {
            assert!(
                !output
                    .lines()
                    .any(|line| line.starts_with(&format!("{name}=")))
            );
        }
    }

    #[test]
    fn native_dxmt_requires_the_paired_bundle_and_uses_native_override() {
        let (temp, _) = runtime::fixture(ExecutionMode::Arm64Fex);
        let bundle = temp.path().join("dxmt");
        for dir in ["x86_64-windows", "aarch64-unix"] {
            fs::create_dir_all(bundle.join(dir)).unwrap();
        }
        let mut pe = test_pe_image(0x8664);
        pe[64..80].copy_from_slice(b"Wine builtin DLL");
        for name in ["d3d11.dll", "dxgi.dll", "winemetal.dll"] {
            fs::write(bundle.join("x86_64-windows").join(name), &pe).unwrap();
        }
        let mut arm = test_image("metal", 1);
        arm[4..8].copy_from_slice(&0x0100000cu32.to_le_bytes());
        fs::write(bundle.join("aarch64-unix/winemetal.so"), arm).unwrap();
        let dxmt = crate::dxmt::inspect_candidate_dir(&bundle).unwrap();
        assert!(crate::dxmt::validate_runtime(&dxmt, Some(temp.path())).is_err());
        let descriptor = temp.path().join(runtime::DESCRIPTOR);
        let mut text = fs::read_to_string(&descriptor).unwrap();
        text.push_str("\n[graphics]\ndxmt='dxmt'\n");
        fs::write(&descriptor, text).unwrap();
        let runtime = ResolvedRuntime::inspect(temp.path()).unwrap();
        assert!(crate::dxmt::validate_runtime(&dxmt, Some(temp.path())).is_ok());
        let mut plan = LaunchPlan::new(
            runtime,
            temp.path().join("prefix"),
            TargetEngine::Dxmt,
            GraphicsComponents {
                dxmt: Some(dxmt),
                ..Default::default()
            },
            true,
            Some(false),
            &GraphicsApi::DirectX11,
        )
        .unwrap();
        let env = plan.execution_env(false, false);
        assert!(env["WINEDLLOVERRIDES"].contains("d3d11,dxgi,d3d10core,winemetal=n,b"));
        assert!(env["WINEDLLPATH"].contains("x86_64-windows"));
        assert!(env["DYLD_LIBRARY_PATH"].contains("aarch64-unix"));
        assert!(!env.contains_key("WINEDLLPATH_PREPEND"));
        assert!(!env.contains_key("ISOTOPE_SUPERVISOR_PID"));

        // Switching a prepared prefix to search-based loading restores its Wine
        // builtins, so a previously staged backend cannot shadow the new one.
        let system32 = plan.prefix.join("drive_c/windows/system32");
        fs::create_dir_all(&system32).unwrap();
        let builtin = plan.runtime.windows_dirs[0].join("d3d11.dll");
        fs::write(&builtin, b"original Wine builtin").unwrap();
        plan.stage_graphics().unwrap();
        assert_eq!(fs::read(system32.join("d3d11.dll")).unwrap(), pe);
        plan.runtime_features.dll_search_prepend = true;
        plan.runtime_features.guest_supervisor = true;
        plan.stage_graphics().unwrap();
        assert_eq!(
            fs::read(system32.join("d3d11.dll")).unwrap(),
            b"original Wine builtin"
        );
        assert!(!system32.join("winemetal.dll").exists());
        assert_eq!(plan.stage_graphics().unwrap(), 0);
        let env = plan.execution_env(false, false);
        assert!(env["WINEDLLPATH_PREPEND"].contains("x86_64-windows"));
        assert!(env["WINEDLLOVERRIDES"].contains("d3d11,dxgi,d3d10core,winemetal=b"));
        assert_eq!(
            env["ISOTOPE_SUPERVISOR_PID"],
            std::process::id().to_string()
        );
        assert!(!env.contains_key("ISOTOPE_FAULT_DIAGNOSTICS"));
        plan.diagnostics = true;
        assert_eq!(plan.bootstrap_env()["ISOTOPE_FAULT_DIAGNOSTICS"], "1");
        let bundle = plan.graphics.dxmt.as_ref().unwrap();
        fs::write(bundle.x64_dxgi.as_ref().unwrap(), test_pe_image(0x8664)).unwrap();
        assert!(bundle.uses_builtin_dlls());
        assert!(!bundle.all_builtin_dlls());
        assert!(
            !plan
                .execution_env(false, false)
                .contains_key("WINEDLLPATH_PREPEND")
        );
    }
    use crate::{
        runtime,
        runtime_checks::{test_image, test_pe_image},
    };

    #[test]
    fn resolved_graphics_drive_both_staging_and_environment() {
        let (temp, mut runtime) = runtime::fixture(ExecutionMode::Arm64Fex);
        let bundle = temp.path().join("selected-dxvk");
        fs::create_dir(&bundle).unwrap();
        let mut pe = test_pe_image(0x8664);
        pe.extend_from_slice(b"selected package");
        for name in ["d3d11.dll", "dxgi.dll"] {
            fs::write(bundle.join(name), &pe).unwrap();
        }
        let driver = temp.path().join("driver.dylib");
        let mut arm = test_image("driver", 1);
        arm[4..8].copy_from_slice(&0x0100000cu32.to_le_bytes());
        fs::write(&driver, &arm).unwrap();
        fs::write(runtime.unix_dir.join("winevulkan.so"), &arm).unwrap();
        let loader = temp.path().join("libvulkan.1.dylib");
        fs::write(&loader, &arm).unwrap();
        runtime.graphics.vulkan_loader = Some(loader);
        let manifest = temp.path().join("driver.json");
        fs::write(
            &manifest,
            r#"{"ICD":{"library_path":"driver.dylib","api_version":"1.4.0"}}"#,
        )
        .unwrap();
        let graphics = GraphicsComponents {
            dxvk: crate::dxvk::inspect_candidate_dir(&bundle),
            vulkan: vec![VulkanDriver {
                manifest,
                library: driver,
            }],
            ..Default::default()
        };
        let prefix = temp.path().join("prefix");
        let plan = LaunchPlan::new(
            runtime,
            prefix.clone(),
            TargetEngine::Dxvk,
            graphics,
            true,
            Some(false),
            &GraphicsApi::DirectX11,
        )
        .unwrap();
        fs::create_dir_all(prefix.join("drive_c/windows/system32")).unwrap();
        fs::create_dir_all(prefix.join("drive_c/windows/syswow64")).unwrap();
        plan.stage_graphics().unwrap();
        assert_eq!(
            fs::read(prefix.join("drive_c/windows/system32/dxgi.dll")).unwrap(),
            pe
        );
        let env = plan.execution_env(false, false);
        assert_eq!(
            env["VK_DRIVER_FILES"],
            plan.graphics.vulkan[0].manifest.display().to_string()
        );
        assert!(env["DYLD_LIBRARY_PATH"].contains("aarch64-unix"));
        assert!(!env["DYLD_LIBRARY_PATH"].contains("x86_64-unix"));
        assert!(!env.contains_key("ROSETTA_ADVERTISE_AVX"));
        assert_eq!(env["FEX_TSOENABLED"], "1");
        assert!(!env.contains_key("VKD3D_CONFIG"));
    }

    #[test]
    fn protocol_uses_windows_executable_path_and_respects_explicit_choice() {
        let (temp, mut runtime) = runtime::fixture(ExecutionMode::Arm64Fex);
        runtime.steam_helper = true;
        let mut plan = LaunchPlan::new(
            runtime.clone(),
            temp.path().join("prefix"),
            TargetEngine::WineStaging,
            Default::default(),
            true,
            None,
            &GraphicsApi::Unknown,
        )
        .unwrap();
        plan.bridge = temp.path().join("bridge");
        fs::create_dir(&plan.bridge).unwrap();
        fs::write(plan.bridge.join("steam.exe"), test_pe_image(0x8664)).unwrap();
        let exe = temp.path().join("game with spaces.exe");
        fs::write(&exe, test_pe_image(0x8664)).unwrap();
        let args = vec![exe.display().to_string(), "argument with spaces".into()];
        let wrapped = plan.wine_args(&args).unwrap();
        assert_eq!(wrapped.len(), 3);
        assert!(wrapped[1].starts_with("Z:\\"));
        assert!(wrapped[1].ends_with("game with spaces.exe"));
        assert_eq!(wrapped[2], args[1]);
        let missing = temp.path().join("missing game.exe");
        let broken = temp.path().join("broken game.exe");
        std::os::unix::fs::symlink(&missing, &broken).unwrap();
        for path in [&missing, &broken] {
            let error = plan.wine_args(&[path.display().to_string()]).unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("Cannot resolve game executable path: {}", path.display())
            );
            assert_eq!(
                error.downcast_ref::<std::io::Error>().unwrap().kind(),
                std::io::ErrorKind::NotFound
            );
        }
        let windows_args = vec!["C:\\Games\\game.exe".into(), "argument with spaces".into()];
        assert_eq!(&plan.wine_args(&windows_args).unwrap()[1..], windows_args);
        let direct = LaunchPlan::new(
            runtime,
            temp.path().join("other"),
            TargetEngine::WineStaging,
            Default::default(),
            true,
            Some(false),
            &GraphicsApi::Unknown,
        )
        .unwrap();
        assert_eq!(direct.wine_args(&args).unwrap(), args);
        let unresolved = vec![missing.display().to_string()];
        assert_eq!(direct.wine_args(&unresolved).unwrap(), unresolved);
    }

    #[test]
    fn explicit_incompatible_graphics_fail_before_prefix_mutation() {
        let (temp, runtime) = runtime::fixture(ExecutionMode::Arm64Fex);
        let prefix = temp.path().join("prefix");
        assert!(
            LaunchPlan::new(
                runtime,
                prefix.clone(),
                TargetEngine::Dxmt,
                Default::default(),
                true,
                None,
                &GraphicsApi::DirectX11
            )
            .is_err()
        );
        assert!(!prefix.exists());
    }
}
