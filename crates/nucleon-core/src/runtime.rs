//! Validated execution capabilities, independent of Wine's marketing version.
use crate::{backend::GraphicsBackend, runtime_checks};
use anyhow::{Context, Result, bail, ensure};
use object::Architecture;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

pub const DESCRIPTOR: &str = "nucleon-runtime.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionMode {
    Intel,
    Arm64Fex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GuestAbi {
    X86,
    X64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SteamProtocol {
    #[default]
    Bridge,
    ValveRedirect,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphicsPaths {
    #[serde(default)]
    pub frontend_abi: FrontendAbi,
    pub frontend_manifest: Option<PathBuf>,
    pub dxmt: Option<PathBuf>,
    pub dxvk: Option<PathBuf>,
    pub vkd3d: Option<PathBuf>,
    pub vulkan_icd: Option<PathBuf>,
    pub vulkan_loader: Option<PathBuf>,
}

/// Graphics implementation ABI, independent of the game's guest ABI.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrontendAbi {
    #[default]
    X64,
    Arm64Ec,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Emulator {
    guest: GuestAbi,
    path: PathBuf,
    #[serde(default)]
    unix_library: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Descriptor {
    schema: u32,
    mode: ExecutionMode,
    #[serde(default)]
    prefix: Option<PrefixCompatibility>,
    guests: Vec<GuestAbi>,
    loader: PathBuf,
    server: PathBuf,
    unix_dir: PathBuf,
    windows_dirs: Vec<PathBuf>,
    backends: Vec<GraphicsBackend>,
    #[serde(default)]
    emulators: Vec<Emulator>,
    #[serde(default)]
    environment: BTreeMap<String, String>,
    #[serde(default)]
    steam_protocol: Option<SteamProtocol>,
    #[serde(default)]
    steam_helper: Option<bool>,
    #[serde(default)]
    steam_protocols: Vec<SteamProtocol>,
    #[serde(default)]
    graphics: GraphicsPaths,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedRuntime {
    pub root: PathBuf,
    pub mode: ExecutionMode,
    pub prefix: PrefixCompatibility,
    pub guests: Vec<GuestAbi>,
    pub loader: PathBuf,
    pub server: PathBuf,
    pub unix_dir: PathBuf,
    pub windows_dirs: Vec<PathBuf>,
    pub backends: Vec<GraphicsBackend>,
    pub environment: BTreeMap<String, String>,
    pub steam_protocol: SteamProtocol,
    pub steam_protocols: Vec<SteamProtocol>,
    pub graphics: GraphicsPaths,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrefixCompatibility {
    pub family: String,
    pub abi: u32,
}

impl PrefixCompatibility {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.abi > 0
                && !self.family.is_empty()
                && self.family.len() <= 64
                && self.family.bytes().all(|b| b.is_ascii_lowercase()
                    || b.is_ascii_digit()
                    || matches!(b, b'-' | b'_')),
            "Prefix family must contain 1–64 lowercase letters/digits, '-' or '_', and ABI must be positive"
        );
        Ok(())
    }
}

impl ResolvedRuntime {
    /// Universal binaries must execute the selected slice, independent of the parent ABI.
    pub fn command(&self, program: &Path) -> std::process::Command {
        if runtime_checks::native_arch(program, Architecture::X86_64)
            && runtime_checks::native_arch(program, Architecture::Aarch64)
        {
            let mut command = std::process::Command::new("/usr/bin/arch");
            command
                .arg(match self.mode {
                    ExecutionMode::Intel => "-x86_64",
                    ExecutionMode::Arm64Fex => "-arm64",
                })
                .arg(program);
            command
        } else {
            std::process::Command::new(program)
        }
    }

    pub fn inspect(root: &Path) -> Result<Self> {
        let root = root
            .canonicalize()
            .with_context(|| format!("Resolve Wine {}", root.display()))?;
        let path = root.join(DESCRIPTOR);
        match fs::read_to_string(&path) {
            Ok(text) => Self::from_descriptor(&root, &text)
                .with_context(|| format!("Invalid runtime descriptor {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::infer(&root),
            Err(e) => Err(e).with_context(|| format!("Read {}", path.display())),
        }
    }

    fn from_descriptor(root: &Path, text: &str) -> Result<Self> {
        let d: Descriptor = crate::configuration::toml(text, &root.join(DESCRIPTOR))?;
        ensure!(
            matches!(d.schema, 1 | 2),
            "Unsupported runtime descriptor schema {}",
            d.schema
        );
        let (steam_protocols, steam_protocol) = if d.schema == 1 {
            // Read published schema-1 runtimes without rewriting immutable generations.
            let selected = d.steam_protocol.unwrap_or_default();
            let mut protocols = vec![SteamProtocol::Bridge];
            if selected != SteamProtocol::Bridge {
                protocols.push(selected);
            }
            (protocols, selected)
        } else {
            ensure!(
                d.steam_protocol.is_none() && d.steam_helper.is_none(),
                "Schema 2 declares Steam capabilities; game launch preferences belong in profiles"
            );
            ensure!(
                d.steam_protocols.contains(&SteamProtocol::Bridge)
                    && d.steam_protocols
                        .iter()
                        .enumerate()
                        .all(|(i, p)| !d.steam_protocols[..i].contains(p)),
                "Steam capabilities must include bridge and contain no duplicate protocols"
            );
            (d.steam_protocols, SteamProtocol::Bridge)
        };
        let prefix = match d.prefix {
            Some(prefix) => prefix,
            None if d.mode == ExecutionMode::Intel => PrefixCompatibility {
                family: "intel".into(),
                abi: 1,
            },
            None => anyhow::bail!(
                "Native runtime requires [prefix] family and abi compatibility declarations"
            ),
        };
        prefix.validate()?;
        ensure!(
            !d.guests.is_empty() && !d.windows_dirs.is_empty(),
            "Runtime guest ABIs and Windows directories are required"
        );
        let resolve = |p: &Path| -> Result<PathBuf> {
            ensure!(
                !p.as_os_str().is_empty()
                    && p.components().all(|c| matches!(c, Component::Normal(_))),
                "Runtime paths must be relative without traversal: {}",
                p.display()
            );
            let full = root
                .join(p)
                .canonicalize()
                .with_context(|| format!("Missing runtime component {}", p.display()))?;
            ensure!(
                full.starts_with(root),
                "Runtime component escapes its root: {}",
                p.display()
            );
            Ok(full)
        };
        let loader = resolve(&d.loader)?;
        let server = resolve(&d.server)?;
        let unix_dir = resolve(&d.unix_dir)?;
        let arch = match d.mode {
            ExecutionMode::Intel => Architecture::X86_64,
            ExecutionMode::Arm64Fex => Architecture::Aarch64,
        };
        ensure!(
            runtime_checks::native_arch(&loader, arch)
                && runtime_checks::native_arch(&server, arch),
            "Runtime loader and server must contain the selected {arch:?} Mach-O architecture"
        );
        ensure!(
            runtime_checks::native_arch(&unix_dir.join("ntdll.so"), arch),
            "Runtime ntdll does not match selected {arch:?} ABI"
        );
        let windows_dirs = d
            .windows_dirs
            .iter()
            .map(|p| resolve(p))
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            windows_dirs.iter().all(|p| p.is_dir()),
            "Windows library paths must be directories"
        );
        let core_abis = windows_dirs
            .iter()
            .filter_map(|p| fs::read(p.join("ntdll.dll")).ok())
            .map(|data| runtime_checks::pe_abi(&data))
            .collect::<Result<Vec<_>>>()?;
        for guest in &d.guests {
            let compatible = match (d.mode, guest) {
                (_, GuestAbi::X86) => core_abis.contains(&runtime_checks::PeAbi::X86),
                (ExecutionMode::Intel, GuestAbi::X64) => {
                    core_abis.contains(&runtime_checks::PeAbi::X64)
                }
                (ExecutionMode::Arm64Fex, GuestAbi::X64) => core_abis.iter().any(|abi| {
                    matches!(
                        abi,
                        runtime_checks::PeAbi::Arm64Ec | runtime_checks::PeAbi::Arm64X
                    )
                }),
            };
            ensure!(
                compatible,
                "Missing compatible Windows ntdll for {guest:?} in declared library directories"
            );
        }
        if d.mode == ExecutionMode::Arm64Fex {
            if d.graphics.frontend_abi == FrontendAbi::Arm64Ec {
                ensure!(
                    d.emulators.len() == 1,
                    "Experimental ARM64EC graphics requires exactly one x64 emulator pair"
                );
            }
            for guest in &d.guests {
                let module = d
                    .emulators
                    .iter()
                    .find(|e| e.guest == *guest)
                    .context("FEX runtime requires an emulator for every declared guest ABI")?;
                let companion = module.unix_library.as_ref().context(
                    "FEX emulator requires unix_library declaring its matching native companion; rebuild and reinstall a complete runtime"
                )?;
                let companion = resolve(companion).context(
                    "Missing FEX Unix companion; rebuild and reinstall a complete runtime",
                )?;
                ensure!(
                    companion.parent() == Some(unix_dir.as_path())
                        && runtime_checks::native_arch(&companion, arch)
                        && runtime_checks::has_export(
                            &fs::read(&companion)?,
                            &["wine_unix_call_funcs"]
                        ),
                    "Invalid FEX Unix companion {}: expected a native library in unix_dir exporting __wine_unix_call_funcs",
                    companion.display()
                );
                let module = resolve(&module.path)?;
                if d.graphics.frontend_abi == FrontendAbi::Arm64Ec {
                    ensure!(
                        module
                            .file_name()
                            .is_some_and(|name| name == "libarm64ecfex.dll")
                            && companion
                                .file_name()
                                .is_some_and(|name| name == "libarm64ecfex.so"),
                        "Experimental ARM64EC graphics requires the paired libarm64ecfex.dll/so boundary"
                    );
                }
                let abi = runtime_checks::pe_abi(&fs::read(&module)?)?;
                let expected = match guest {
                    GuestAbi::X64 => runtime_checks::PeAbi::Arm64Ec,
                    GuestAbi::X86 => runtime_checks::PeAbi::Arm64,
                };
                ensure!(
                    abi == expected,
                    "Wrong FEX emulator ABI: {} ({abi:?}, expected {expected:?})",
                    module.display()
                );
                ensure!(
                    windows_dirs.iter().any(|p| module.parent() == Some(p)),
                    "FEX emulator must be in a declared Windows directory"
                );
            }
        } else {
            ensure!(
                d.emulators.is_empty(),
                "Intel execution cannot declare FEX emulators"
            );
        }
        for (key, value) in &d.environment {
            ensure!(
                (matches!(key.as_str(), "FEX_TSOENABLED" | "NUCLEON_EMULATE_KUSD") && value == "1")
                    || (key == "MESA_KK_EXPERIMENTAL"
                        && value == "image_view_min_lod"
                        && d.backends.contains(&GraphicsBackend::KosmicKrisp)
                        && d.graphics.vulkan_icd.is_some()),
                "Unsupported runtime environment setting {key}; descriptor environment only enables validated runtime options"
            );
        }
        let mut environment = d.environment;
        if d.mode == ExecutionMode::Arm64Fex {
            environment.insert("FEX_TSOENABLED".into(), "1".into());
        }
        let optional = |p: Option<PathBuf>, directory: bool| -> Result<Option<PathBuf>> {
            p.map(|p| {
                let p = resolve(&p)?;
                ensure!(
                    if directory { p.is_dir() } else { p.is_file() },
                    "Invalid graphics component {}",
                    p.display()
                );
                Ok(p)
            })
            .transpose()
        };
        let graphics = GraphicsPaths {
            frontend_abi: d.graphics.frontend_abi,
            frontend_manifest: optional(d.graphics.frontend_manifest, false)?,
            dxmt: optional(d.graphics.dxmt, true)?,
            dxvk: optional(d.graphics.dxvk, true)?,
            vkd3d: optional(d.graphics.vkd3d, true)?,
            vulkan_icd: optional(d.graphics.vulkan_icd, false)?,
            vulkan_loader: optional(d.graphics.vulkan_loader, false)?,
        };
        if let Some(loader) = &graphics.vulkan_loader {
            ensure!(
                runtime_checks::native_arch(loader, arch),
                "Vulkan loader architecture does not match Wine"
            );
            ensure!(
                runtime_checks::has_export(&fs::read(loader)?, &["vkGetInstanceProcAddr"]),
                "Vulkan loader does not export vkGetInstanceProcAddr"
            );
        }
        if let Some(icd) = &graphics.vulkan_icd {
            let manifest = crate::configuration::resolve_vulkan_icd(icd)?;
            ensure!(
                !manifest.declared_library.is_absolute(),
                "Bundled Vulkan ICD must use a relative library path"
            );
            let library = manifest.library;
            ensure!(
                library.starts_with(root) && runtime_checks::native_arch(&library, arch),
                "Bundled Vulkan ICD library must stay inside the runtime and match its native ABI"
            );
        }
        let runtime = Self {
            root: root.to_owned(),
            mode: d.mode,
            prefix,
            guests: d.guests,
            loader,
            server,
            unix_dir,
            windows_dirs,
            backends: d.backends,
            environment,
            steam_protocol,
            steam_protocols,
            graphics,
        };
        crate::frontend::validate_declared(&runtime)?;
        Ok(runtime)
    }

    fn infer(root: &Path) -> Result<Self> {
        // Native Wine requires an explicit emulator/ABI contract; never guess FEX from a version.
        let unix_dir = ["lib/wine/x86_64-unix", "lib64/wine/x86_64-unix"]
            .iter()
            .map(|p| root.join(p))
            .find(|p| runtime_checks::native_arch(&p.join("ntdll.so"), Architecture::X86_64));
        let loader = ["bin/wine", "bin/wine64"]
            .iter()
            .map(|p| root.join(p))
            .find(|p| p.is_file())
            .context("Wine loader is missing")?;
        if unix_dir.is_none() && !runtime_checks::native_arch(&loader, Architecture::X86_64) {
            bail!(
                "Cannot infer Intel Wine capabilities at {}; native ARM64/FEX runtimes require {DESCRIPTOR}",
                root.display()
            );
        }
        let server = ["bin/wineserver", "bin/wineserver64"]
            .iter()
            .map(|p| root.join(p))
            .find(|p| p.is_file())
            .context("Wine server is missing")?;
        let unix_dir = unix_dir.unwrap_or_else(|| root.join("lib/wine/x86_64-unix"));
        let windows_dirs = [
            "lib/wine/x86_64-windows",
            "lib64/wine/x86_64-windows",
            "lib/wine/i386-windows",
            "lib64/wine/i386-windows",
            "lib/wine/x86-windows",
        ]
        .iter()
        .map(|p| root.join(p))
        .filter(|p| p.is_dir())
        .collect();
        let mut guests = vec![GuestAbi::X64];
        if [
            "lib/wine/i386-windows/ntdll.dll",
            "lib64/wine/i386-windows/ntdll.dll",
        ]
        .iter()
        .any(|p| runtime_checks::pe(Some(&root.join(p)), false))
        {
            guests.push(GuestAbi::X86);
        }
        Ok(Self {
            root: root.to_owned(),
            mode: ExecutionMode::Intel,
            prefix: PrefixCompatibility {
                family: "intel".into(),
                abi: 1,
            },
            guests,
            loader,
            server,
            unix_dir,
            windows_dirs,
            backends: GraphicsBackend::all().to_vec(),
            environment: BTreeMap::new(),
            steam_protocol: SteamProtocol::Bridge,
            steam_protocols: vec![SteamProtocol::Bridge],
            graphics: GraphicsPaths::default(),
        })
    }

    pub fn arch(&self) -> Architecture {
        match self.mode {
            ExecutionMode::Intel => Architecture::X86_64,
            ExecutionMode::Arm64Fex => Architecture::Aarch64,
        }
    }
    pub fn unix_name(&self) -> &'static str {
        match self.mode {
            ExecutionMode::Intel => "x86_64-unix",
            ExecutionMode::Arm64Fex => "aarch64-unix",
        }
    }
    pub fn require_guest(&self, is64: bool) -> Result<()> {
        let guest = if is64 { GuestAbi::X64 } else { GuestAbi::X86 };
        ensure!(
            self.guests.contains(&guest),
            "Selected runtime does not support {guest:?} guests"
        );
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn fixture(mode: ExecutionMode) -> (tempfile::TempDir, ResolvedRuntime) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let unix = if mode == ExecutionMode::Intel {
        "x86_64-unix"
    } else {
        "aarch64-unix"
    };
    let mut native = runtime_checks::test_image("wine", 1);
    if mode == ExecutionMode::Arm64Fex {
        native[4..8].copy_from_slice(&0x0100000cu32.to_le_bytes());
    }
    for path in [
        "bin/wine".to_string(),
        "bin/wineserver".into(),
        format!("lib/wine/{unix}/ntdll.so"),
        format!("lib/wine/{unix}/libarm64ecfex.so"),
    ] {
        let is_companion = path.ends_with("libarm64ecfex.so");
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut image = if is_companion {
            runtime_checks::test_image("__wine_unix_call_funcs", 1)
        } else {
            native.clone()
        };
        if mode == ExecutionMode::Arm64Fex {
            image[4..8].copy_from_slice(&0x0100000cu32.to_le_bytes());
        }
        fs::write(path, &image).unwrap();
    }
    fs::create_dir_all(root.join("lib/wine/aarch64-windows")).unwrap();
    fs::write(
        root.join("lib/wine/aarch64-windows/ntdll.dll"),
        runtime_checks::test_pe_image(if mode == ExecutionMode::Intel {
            0x8664
        } else {
            0xa641
        }),
    )
    .unwrap();
    fs::write(
        root.join("lib/wine/aarch64-windows/libarm64ecfex.dll"),
        runtime_checks::test_pe_image(0xa641),
    )
    .unwrap();
    let emulators = if mode == ExecutionMode::Arm64Fex {
        "[[emulators]]\nguest='x64'\npath='lib/wine/aarch64-windows/libarm64ecfex.dll'\nunix_library='lib/wine/aarch64-unix/libarm64ecfex.so'"
    } else {
        ""
    };
    let descriptor = format!(
        "schema=1\nmode='{}'\nguests=['x64']\nloader='bin/wine'\nserver='bin/wineserver'\nunix_dir='lib/wine/{unix}'\nwindows_dirs=['lib/wine/aarch64-windows']\nbackends=['auto','dxmt','dxvk','kosmickrisp']\n{emulators}",
        if mode == ExecutionMode::Intel {
            "intel"
        } else {
            "arm64-fex"
        }
    );
    let descriptor = descriptor + "\n[prefix]\nfamily='test-fex'\nabi=1\n";
    fs::write(root.join(DESCRIPTOR), descriptor).unwrap();
    let runtime = ResolvedRuntime::inspect(root).unwrap();
    (temp, runtime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_capabilities_do_not_select_game_launch_policy() {
        let (_temp, original) = fixture(ExecutionMode::Arm64Fex);
        let old = fs::read_to_string(original.root.join(DESCRIPTOR)).unwrap();
        let descriptor = old.replace(
            "schema=1",
            "schema=2\nsteam_protocols=['bridge','valve-redirect']",
        );
        let mut runtime = ResolvedRuntime::from_descriptor(&original.root, &descriptor).unwrap();
        assert_eq!(runtime.prefix, original.prefix);
        assert_eq!(runtime.steam_protocol, SteamProtocol::Bridge);
        let profile = crate::profile::GameProfile {
            steam_protocol: Some(SteamProtocol::ValveRedirect),
            ..Default::default()
        };
        profile.apply_protocol(&mut runtime).unwrap();
        assert_eq!(runtime.steam_protocol, SteamProtocol::ValveRedirect);
        let plan = crate::launch::LaunchPlan::new(
            runtime,
            original.root.join("prefix"),
            crate::detector::TargetEngine::WineStaging,
            Default::default(),
            true,
            None,
            &crate::detector::GraphicsApi::Unknown,
        )
        .unwrap();
        assert!(!plan.use_steam_helper);
        let mut unsupported = ResolvedRuntime::from_descriptor(
            &original.root,
            &descriptor.replace("['bridge','valve-redirect']", "['bridge']"),
        )
        .unwrap();
        assert!(profile.apply_protocol(&mut unsupported).is_err());
        assert!(
            ResolvedRuntime::from_descriptor(
                &original.root,
                &descriptor.replace("['bridge','valve-redirect']", "['bridge','bridge']")
            )
            .is_err()
        );
        assert!(
            ResolvedRuntime::from_descriptor(
                &original.root,
                &descriptor.replace("schema=2", "schema=2\nsteam_helper=true")
            )
            .is_err()
        );
    }

    #[test]
    fn bundled_vulkan_resolution_preserves_relative_path_and_containment_policy() {
        let (temp, runtime) = fixture(ExecutionMode::Arm64Fex);
        let descriptor = temp.path().join(DESCRIPTOR);
        let original = fs::read_to_string(&descriptor).unwrap();
        fs::write(
            &descriptor,
            format!("{original}\n[graphics]\nvulkan_icd='icd.json'\n"),
        )
        .unwrap();
        let native = fs::read(runtime.unix_dir.join("ntdll.so")).unwrap();
        let driver = temp.path().join("driver.dylib");
        fs::write(&driver, &native).unwrap();
        let outside = tempfile::NamedTempFile::new_in(temp.path().parent().unwrap()).unwrap();
        fs::write(outside.path(), &native).unwrap();
        std::os::unix::fs::symlink(outside.path(), temp.path().join("escape.dylib")).unwrap();
        let manifest = temp.path().join("icd.json");
        let write_library = |library: &Path| {
            fs::write(
                &manifest,
                serde_json::json!({"ICD":{"library_path":library}}).to_string(),
            )
            .unwrap();
        };
        write_library(Path::new("driver.dylib"));
        ResolvedRuntime::inspect(temp.path()).unwrap();
        write_library(&driver);
        let error = format!("{:#}", ResolvedRuntime::inspect(temp.path()).unwrap_err());
        assert!(
            error.contains("must use a relative library path"),
            "{error}"
        );
        let traversal = Path::new("..").join(outside.path().file_name().unwrap());
        for library in [Path::new("escape.dylib"), traversal.as_path()] {
            write_library(library);
            let error = format!("{:#}", ResolvedRuntime::inspect(temp.path()).unwrap_err());
            assert!(error.contains("must stay inside the runtime"), "{error}");
        }
        fs::write(&driver, runtime_checks::test_image("driver", 1)).unwrap();
        write_library(Path::new("driver.dylib"));
        let error = format!("{:#}", ResolvedRuntime::inspect(temp.path()).unwrap_err());
        assert!(error.contains("match its native ABI"), "{error}");
    }

    #[test]
    fn native_emulators_require_declared_native_unix_companions_and_exports() {
        let (temp, runtime) = fixture(ExecutionMode::Arm64Fex);
        let descriptor = temp.path().join(DESCRIPTOR);
        let text = fs::read_to_string(&descriptor).unwrap();
        fs::write(
            &descriptor,
            text.replace("unix_library='lib/wine/aarch64-unix/libarm64ecfex.so'", ""),
        )
        .unwrap();
        assert!(
            format!("{:#}", ResolvedRuntime::inspect(temp.path()).unwrap_err())
                .contains("unix_library")
        );
        fs::write(&descriptor, text).unwrap();
        let companion = runtime.unix_dir.join("libarm64ecfex.so");
        fs::remove_file(&companion).unwrap();
        assert!(
            format!("{:#}", ResolvedRuntime::inspect(temp.path()).unwrap_err())
                .contains("Missing FEX Unix companion")
        );
        fs::write(
            &companion,
            runtime_checks::test_image("__wine_unix_call_funcs", 1),
        )
        .unwrap();
        assert!(ResolvedRuntime::inspect(temp.path()).is_err()); // Intel companion
        let mut image = runtime_checks::test_image("wrong_export", 1);
        image[4..8].copy_from_slice(&0x0100000cu32.to_le_bytes());
        fs::write(&companion, &image).unwrap();
        assert!(ResolvedRuntime::inspect(temp.path()).is_err());
    }

    #[test]
    fn explicit_mode_wins_in_a_dual_architecture_tree() {
        let (temp, runtime) = fixture(ExecutionMode::Arm64Fex);
        let intel = temp.path().join("lib/wine/x86_64-unix/ntdll.so");
        fs::create_dir_all(intel.parent().unwrap()).unwrap();
        fs::write(intel, runtime_checks::test_image("wine", 1)).unwrap();
        let selected = ResolvedRuntime::inspect(temp.path()).unwrap();
        assert_eq!(selected.arch(), Architecture::Aarch64);
        assert_eq!(selected.unix_dir, runtime.unix_dir);
        assert_eq!(selected.environment["FEX_TSOENABLED"], "1");
        assert!(selected.require_guest(false).is_err());
        fs::remove_file(temp.path().join(DESCRIPTOR)).unwrap();
        assert_eq!(
            ResolvedRuntime::inspect(temp.path()).unwrap().mode,
            ExecutionMode::Intel
        );
    }

    #[test]
    fn native_wine_requires_descriptor_and_matching_emulator() {
        let (temp, _) = fixture(ExecutionMode::Arm64Fex);
        let path = temp.path().join(DESCRIPTOR);
        let descriptor = fs::read_to_string(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(
            ResolvedRuntime::inspect(temp.path())
                .unwrap_err()
                .to_string()
                .contains(DESCRIPTOR)
        );
        fs::write(&path, descriptor).unwrap();
        fs::write(
            temp.path()
                .join("lib/wine/aarch64-windows/libarm64ecfex.dll"),
            runtime_checks::test_pe_image(0x8664),
        )
        .unwrap();
        assert!(
            format!("{:#}", ResolvedRuntime::inspect(temp.path()).unwrap_err())
                .contains("Wrong FEX emulator ABI")
        );
    }

    #[test]
    fn minimum_lod_requires_exact_setting_and_bundled_kosmickrisp() {
        let (temp, runtime) = fixture(ExecutionMode::Arm64Fex);
        let root = temp.path();
        let path = root.join(DESCRIPTOR);
        let original = fs::read_to_string(&path).unwrap();
        fs::create_dir_all(root.join("lib/vulkan")).unwrap();
        fs::copy(
            runtime.unix_dir.join("ntdll.so"),
            root.join("lib/vulkan/driver.dylib"),
        )
        .unwrap();
        fs::write(root.join("lib/vulkan/icd.json"),
            r#"{"file_format_version":"1.0.1","ICD":{"library_path":"driver.dylib","api_version":"1.4.0"}}"#).unwrap();
        let descriptor = format!(
            "{original}\n[environment]\nMESA_KK_EXPERIMENTAL='image_view_min_lod'\n[graphics]\nvulkan_icd='lib/vulkan/icd.json'\n"
        );
        fs::write(&path, &descriptor).unwrap();
        assert_eq!(
            ResolvedRuntime::inspect(root).unwrap().environment["MESA_KK_EXPERIMENTAL"],
            "image_view_min_lod"
        );
        for invalid in [
            descriptor.replace("image_view_min_lod", "all"),
            descriptor.replace("image_view_min_lod", "image_view_min_lod,custom_border"),
            descriptor.replace(",'kosmickrisp'", ""),
            descriptor.replace("vulkan_icd='lib/vulkan/icd.json'", ""),
        ] {
            assert_ne!(invalid, descriptor);
            fs::write(&path, invalid).unwrap();
            assert!(ResolvedRuntime::inspect(root).is_err());
        }
    }

    #[test]
    fn descriptors_reject_traversal_symlink_escape_wrong_host_and_environment() {
        let (temp, _) = fixture(ExecutionMode::Arm64Fex);
        let path = temp.path().join(DESCRIPTOR);
        let original = fs::read_to_string(&path).unwrap();
        let external = tempfile::tempdir().unwrap();
        fs::write(
            external.path().join("wine"),
            runtime_checks::test_image("wine", 1),
        )
        .unwrap();
        std::os::unix::fs::symlink(external.path().join("wine"), temp.path().join("outside"))
            .unwrap();
        for changed in [
            original.replace("loader='bin/wine'", "loader='../wine'"),
            original.replace("loader='bin/wine'", "loader='outside'"),
            original.replace("mode='arm64-fex'", "mode='intel'"),
            format!("{original}\n[environment]\nWINEPREFIX='/tmp/other'"),
            original.replace("schema=1", "schema=99"),
        ] {
            fs::write(&path, changed).unwrap();
            assert!(ResolvedRuntime::inspect(temp.path()).is_err());
        }
    }
}
