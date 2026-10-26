//! Explicit experimental graphics pairing; never relax ordinary guest PE checks.
use crate::{
    launch::{GraphicsComponents, VulkanDriver},
    runtime::{ExecutionMode, FrontendAbi, GuestAbi, ResolvedRuntime},
};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Deserialize, PartialEq, Eq)]
struct Source {
    url: String,
    revision: String,
}
static_toml::static_toml! {
    #[allow(dead_code)]
    static PINS = include_toml!("../../assets/wine/recipes/arm64ec-graphics.toml");
}
#[derive(Deserialize, PartialEq)]
struct Build {
    architecture: FrontendAbi,
    status: String,
    source: Source,
    mingw_source: Source,
    toolchain: serde_json::Value,
    crt_sha256: String,
    emutls_sha256: String,
    teb_header_sha256: String,
    options: Vec<String>,
    files: BTreeMap<String, String>,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Contract {
    schema: u32,
    frontend_abi: FrontendAbi,
    files: BTreeMap<PathBuf, String>,
    builds: BTreeMap<String, Build>,
}

pub(crate) fn validate_declared(runtime: &ResolvedRuntime) -> Result<()> {
    if runtime.graphics.frontend_abi == FrontendAbi::X64 {
        ensure!(
            runtime.graphics.frontend_manifest.is_none(),
            "frontend_manifest requires the experimental arm64ec graphics ABI"
        );
        return Ok(());
    }
    let graphics = &runtime.graphics;
    ensure!(
        runtime
            .environment
            .get("FEX_TSOENABLED")
            .is_some_and(|value| value == "1")
            && runtime
                .environment
                .get("NUCLEON_EMULATE_KUSD")
                .is_some_and(|value| value == "1"),
        "Experimental ARM64EC graphics requires FEX TSO and shared-data emulation enabled"
    );
    ensure!(
        runtime.mode == ExecutionMode::Arm64Fex
            && runtime.guests == [GuestAbi::X64]
            && runtime.windows_dirs.len() == 1,
        "Experimental ARM64EC graphics requires ARM64/FEX with only x64 guests and one Windows library directory"
    );
    ensure!(
        graphics.dxmt.is_none()
            && runtime.backends.iter().all(|b| matches!(
                b,
                crate::backend::GraphicsBackend::Auto
                    | crate::backend::GraphicsBackend::Dxvk
                    | crate::backend::GraphicsBackend::KosmicKrisp
            )),
        "Experimental ARM64EC graphics supports only the Vulkan DXVK/VKD3D backends"
    );
    let dxvk = graphics
        .dxvk
        .as_ref()
        .context("ARM64EC graphics requires a bundled DXVK directory")?;
    let vkd3d = graphics
        .vkd3d
        .as_ref()
        .context("ARM64EC graphics requires a bundled VKD3D directory")?;
    let manifest = graphics
        .vulkan_icd
        .as_ref()
        .context("ARM64EC graphics requires a bundled Vulkan ICD")?;
    let selected = GraphicsComponents {
        dxvk: Some(
            crate::dxvk::inspect_candidate_dir(dxvk).context("Invalid ARM64EC DXVK layout")?,
        ),
        vkd3d: Some(
            crate::vkd3d::inspect_candidate_dir(vkd3d).context("Invalid ARM64EC VKD3D layout")?,
        ),
        vulkan: vec![VulkanDriver {
            manifest: manifest.clone(),
            library: crate::configuration::resolve_vulkan_icd(manifest)?.library,
        }],
        ..Default::default()
    };
    validate_selected(runtime, &selected)
}

pub(crate) fn validate_selected(
    runtime: &ResolvedRuntime,
    selected: &GraphicsComponents,
) -> Result<()> {
    if runtime.graphics.frontend_abi == FrontendAbi::X64 {
        return Ok(());
    }
    ensure!(
        runtime.mode == ExecutionMode::Arm64Fex && runtime.guests == [GuestAbi::X64],
        "Experimental ARM64EC graphics requires ARM64/FEX with only x64 guests"
    );
    ensure!(
        selected.dxmt.is_none() && !selected.gptk,
        "ARM64EC graphics does not support DXMT/GPTK selection"
    );
    let path = runtime
        .graphics
        .frontend_manifest
        .as_ref()
        .context("ARM64EC graphics requires frontend_manifest pairing provenance")?;
    let text = fs::read_to_string(path)
        .with_context(|| format!("Read frontend pairing {}", path.display()))?;
    let contract: Contract = crate::configuration::json(&text, path)?;
    ensure!(
        contract.schema == 1 && contract.frontend_abi == FrontendAbi::Arm64Ec,
        "Unsupported ARM64EC frontend pairing contract {}",
        path.display()
    );
    let mut covered = BTreeMap::new();
    for (relative, digest) in &contract.files {
        ensure!(
            !relative.as_os_str().is_empty()
                && relative
                    .components()
                    .all(|c| matches!(c, Component::Normal(_))),
            "Invalid frontend pairing path {}",
            relative.display()
        );
        let file = runtime
            .root
            .join(relative)
            .canonicalize()
            .with_context(|| format!("Missing frontend pairing file {}", relative.display()))?;
        ensure!(
            file.starts_with(&runtime.root),
            "Frontend pairing file escapes runtime: {}",
            relative.display()
        );
        ensure!(
            crate::staging::digest(&file)? == *digest,
            "Frontend pairing digest mismatch: {}",
            file.display()
        );
        covered.insert(file, digest);
    }
    let require = |file: &Path| -> Result<&String> {
        covered
            .get(
                &file
                    .canonicalize()
                    .with_context(|| format!("Missing paired component {}", file.display()))?,
            )
            .copied()
            .with_context(|| {
                format!(
                    "Component is not covered by ARM64EC pairing: {}",
                    file.display()
                )
            })
    };
    let windows = runtime
        .windows_dirs
        .first()
        .context("Missing ARM64EC Windows library directory")?;
    for file in [
        runtime.loader.clone(),
        runtime.server.clone(),
        runtime.unix_dir.join("ntdll.so"),
        runtime.unix_dir.join("winevulkan.so"),
        runtime.unix_dir.join("libarm64ecfex.so"),
        windows.join("ntdll.dll"),
        windows.join("winevulkan.dll"),
        windows.join("libarm64ecfex.dll"),
    ] {
        require(&file)?;
    }
    let icd = runtime
        .graphics
        .vulkan_icd
        .as_ref()
        .context("Missing paired Vulkan ICD")?;
    let driver = crate::configuration::resolve_vulkan_icd(icd)?;
    require(icd)?;
    require(&driver.library)?;
    require(
        runtime
            .graphics
            .vulkan_loader
            .as_deref()
            .context("Missing paired Vulkan loader")?,
    )?;
    ensure!(
        selected.vulkan.len() == 1
            && selected.vulkan[0].manifest.canonicalize()? == *icd
            && selected.vulkan[0].library.canonicalize()? == driver.library,
        "Vulkan override does not match the ARM64EC frontend pairing"
    );
    let dxvk = selected
        .dxvk
        .as_ref()
        .context("Missing paired ARM64EC DXVK")?;
    let vkd3d = selected
        .vkd3d
        .as_ref()
        .context("Missing paired ARM64EC VKD3D")?;
    ensure!(
        Some(dxvk.root.canonicalize()?) == runtime.graphics.dxvk
            && Some(vkd3d.root.canonicalize()?) == runtime.graphics.vkd3d,
        "Graphics override does not match the ARM64EC frontend pairing"
    );
    ensure!(
        contract.builds.len() == 2,
        "ARM64EC pairing requires exactly DXVK and VKD3D build receipts"
    );
    let dxvk_build = contract
        .builds
        .get("dxvk")
        .context("Missing DXVK build receipt")?;
    let vkd3d_build = contract
        .builds
        .get("vkd3d")
        .context("Missing VKD3D build receipt")?;
    ensure!(
        dxvk_build.toolchain.is_object() && dxvk_build.toolchain == vkd3d_build.toolchain,
        "ARM64EC frontends require the same compiler toolchain provenance"
    );
    let teb_tsd_offset: &str = PINS.darwin.teb_tsd_offset;
    let thread_pointer = format!("-DNUCLEON_TEB_TSD_OFFSET={teb_tsd_offset}");
    for (component, root, files, expected_source) in [
        (
            "dxvk",
            &dxvk.root,
            [
                ("d3d11.dll", dxvk.x64_d3d11.as_deref()),
                ("dxgi.dll", dxvk.x64_dxgi.as_deref()),
            ],
            (PINS.sources.dxvk.url, PINS.sources.dxvk.revision),
        ),
        (
            "vkd3d",
            &vkd3d.root,
            [
                ("d3d12.dll", Some(vkd3d.x64_d3d12.as_path())),
                ("d3d12core.dll", vkd3d.x64_d3d12core.as_deref()),
            ],
            (PINS.sources.vkd_3_d.url, PINS.sources.vkd_3_d.revision),
        ),
    ] {
        let build = contract
            .builds
            .get(component)
            .with_context(|| format!("Missing {component} build receipt"))?;
        ensure!(
            build.architecture == FrontendAbi::Arm64Ec
                && build.status == "BUILT_NOT_EXECUTION_VALIDATED"
                && (build.source.url.as_str(), build.source.revision.as_str()) == expected_source
                && (
                    build.mingw_source.url.as_str(),
                    build.mingw_source.revision.as_str()
                ) == (PINS.sources.mingw.url, PINS.sources.mingw.revision)
                && ["-Dc_args=", "-Dcpp_args="]
                    .iter()
                    .all(|prefix| build.options.iter().any(|option| {
                        option.strip_prefix(prefix).is_some_and(|args| {
                            args.split_whitespace().any(|arg| arg == "-femulated-tls")
                                && args.split_whitespace().any(|arg| arg == thread_pointer)
                        })
                    })),
            "Unpaired ARM64EC {component} build provenance"
        );
        for (name, expected) in [
            ("dllcrt2.o", &build.crt_sha256),
            ("emutls.o", &build.emutls_sha256),
            ("darwin-teb.h", &build.teb_header_sha256),
        ] {
            ensure!(
                require(&runtime.root.join("share/nucleon/arm64ec/crt").join(name))? == expected,
                "ARM64EC {component} CRT/TLS provenance mismatch: {name}"
            );
        }
        ensure!(
            build.files.len() == 2,
            "Unexpected files in ARM64EC {component} build receipt"
        );
        require(&root.join("build.json"))?;
        // The original complete build receipt must agree with the embedded provenance.
        let original: Build = crate::configuration::json(
            &fs::read_to_string(root.join("build.json"))?,
            &root.join("build.json"),
        )?;
        ensure!(
            &original == build,
            "ARM64EC {component} build receipt differs from pairing provenance"
        );
        for (name, file) in files {
            let file = file.with_context(|| format!("Missing ARM64EC {component} {name}"))?;
            ensure!(
                crate::runtime_checks::graphics_pe(Some(file), true, FrontendAbi::Arm64Ec),
                "Expected ARM64EC graphics ABI for {}",
                file.display()
            );
            ensure!(
                build.files.get(name) == Some(require(file)?),
                "Unpaired ARM64EC graphics component {}",
                file.display()
            );
        }
    }
    // Availability checks only cover the chosen API; reject any extra DLLs staging might copy.
    ensure!(
        dxvk.x64_d3d9.is_none()
            && dxvk.x64_d3d10core.is_none()
            && dxvk.x86_d3d9.is_none()
            && dxvk.x86_d3d10core.is_none()
            && dxvk.x86_d3d11.is_none()
            && dxvk.x86_dxgi.is_none()
            && vkd3d.x86_d3d12.is_none()
            && vkd3d.x86_d3d12core.is_none(),
        "Experimental ARM64EC pairing supports only the pinned x64-guest DX11/DX12 component set"
    );
    Ok(())
}

#[cfg(test)]
pub(crate) fn fixture() -> (tempfile::TempDir, ResolvedRuntime) {
    let (temp, runtime) = crate::runtime::fixture(ExecutionMode::Arm64Fex);
    let root = &runtime.root;
    let native = fs::read(runtime.unix_dir.join("ntdll.so")).unwrap();
    let windows = &runtime.windows_dirs[0];
    fs::write(
        windows.join("winevulkan.dll"),
        crate::runtime_checks::test_pe_image(0xa641),
    )
    .unwrap();
    fs::write(runtime.unix_dir.join("winevulkan.so"), &native).unwrap();
    fs::create_dir_all(root.join("lib/vulkan")).unwrap();
    fs::write(root.join("lib/vulkan/driver.dylib"), &native).unwrap();
    let mut loader = crate::runtime_checks::test_image("vkGetInstanceProcAddr", 1);
    loader[4..8].copy_from_slice(&0x0100000cu32.to_le_bytes());
    fs::write(root.join("lib/vulkan/loader.dylib"), loader).unwrap();
    fs::write(
        root.join("lib/vulkan/icd.json"),
        r#"{"ICD":{"library_path":"driver.dylib","api_version":"1.4.0"}}"#,
    )
    .unwrap();
    let shared = root.join("share/nucleon/arm64ec");
    fs::create_dir_all(shared.join("crt")).unwrap();
    for name in ["dllcrt2.o", "emutls.o", "darwin-teb.h"] {
        fs::write(shared.join("crt").join(name), name).unwrap();
    }
    let pin: toml::Value = toml::from_str(
        crate::repo_assets::ASSETS
            .get_file("wine/recipes/arm64ec-graphics.toml")
            .unwrap()
            .contents_utf8()
            .unwrap(),
    )
    .unwrap();
    let mut builds = serde_json::Map::new();
    for (component, names) in [
        ("dxvk", ["d3d11.dll", "dxgi.dll"]),
        ("vkd3d", ["d3d12.dll", "d3d12core.dll"]),
    ] {
        let bundle = shared.join(component);
        fs::create_dir_all(&bundle).unwrap();
        let mut files = BTreeMap::new();
        for name in names {
            fs::write(
                bundle.join(name),
                crate::runtime_checks::test_pe_image(0xa641),
            )
            .unwrap();
            files.insert(name, crate::staging::digest(&bundle.join(name)).unwrap());
        }
        let build = serde_json::json!({"architecture":"arm64ec", "status":"BUILT_NOT_EXECUTION_VALIDATED",
            "source": pin["sources"][component], "mingw_source":pin["sources"]["mingw"], "toolchain":{"compiler":"fixture"},
            "crt_sha256":crate::staging::digest(&shared.join("crt/dllcrt2.o")).unwrap(),
            "emutls_sha256":crate::staging::digest(&shared.join("crt/emutls.o")).unwrap(),
            "teb_header_sha256":crate::staging::digest(&shared.join("crt/darwin-teb.h")).unwrap(),
            "options":["-Dc_args=-femulated-tls -DNUCLEON_TEB_TSD_OFFSET=0x17f8", "-Dcpp_args=-femulated-tls -DNUCLEON_TEB_TSD_OFFSET=0x17f8"], "files":files});
        fs::write(bundle.join("build.json"), build.to_string()).unwrap();
        builds.insert(component.into(), build);
    }
    let mut files = BTreeMap::new();
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry.unwrap();
        if entry.file_type().is_file() && entry.file_name() != crate::runtime::DESCRIPTOR {
            files.insert(
                entry.path().strip_prefix(root).unwrap().to_path_buf(),
                crate::staging::digest(entry.path()).unwrap(),
            );
        }
    }
    fs::write(
        shared.join("frontend.json"),
        serde_json::json!({"schema":1,"frontend_abi":"arm64ec","files":files,"builds":builds})
            .to_string(),
    )
    .unwrap();
    let descriptor = root.join(crate::runtime::DESCRIPTOR);
    let original = fs::read_to_string(&descriptor).unwrap();
    let original = original + "\n[environment]\nNUCLEON_EMULATE_KUSD='1'\n";
    let text = original.replace(
        "['auto','dxmt','dxvk','kosmickrisp']",
        "['auto','dxvk','kosmickrisp']",
    ) + "\n[graphics]\nfrontend_abi='arm64ec'\nfrontend_manifest='share/nucleon/arm64ec/frontend.json'\ndxvk='share/nucleon/arm64ec/dxvk'\nvkd3d='share/nucleon/arm64ec/vkd3d'\nvulkan_icd='lib/vulkan/icd.json'\nvulkan_loader='lib/vulkan/loader.dylib'\n";
    fs::write(descriptor, text).unwrap();
    let resolved = ResolvedRuntime::inspect(root).unwrap();
    (temp, resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        detector::{GraphicsApi, TargetEngine},
        launch::LaunchPlan,
    };

    #[test]
    fn descriptors_preserve_legacy_default_and_reject_incomplete_experimental_contracts() {
        let (_temp, legacy) = crate::runtime::fixture(ExecutionMode::Intel);
        assert_eq!(legacy.graphics.frontend_abi, FrontendAbi::X64);
        let (_temp, runtime) = fixture();
        let path = runtime.root.join(crate::runtime::DESCRIPTOR);
        let original = fs::read_to_string(&path).unwrap();
        for text in [
            original.replace("frontend_abi='arm64ec'", "frontend_abi='unknown'"),
            original.replace(
                "frontend_manifest='share/nucleon/arm64ec/frontend.json'",
                "",
            ),
            original.replace(
                "['auto','dxvk','kosmickrisp']",
                "['auto','dxmt','dxvk','kosmickrisp']",
            ),
            original.replace("frontend_abi='arm64ec'", "frontend_abi='x64'"),
        ] {
            fs::write(&path, text).unwrap();
            assert!(ResolvedRuntime::inspect(&runtime.root).is_err());
        }
        fs::write(&path, original).unwrap();
        let path = runtime.graphics.frontend_manifest.as_ref().unwrap();
        let mut contract: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        contract["files"]["../escape"] = "untrusted".into();
        fs::write(path, contract.to_string()).unwrap();
        let error = format!("{:#}", ResolvedRuntime::inspect(&runtime.root).unwrap_err());
        assert!(error.contains("Invalid frontend pairing path"), "{error}");
    }

    #[test]
    fn explicit_arm64ec_pair_supports_vulkan_only_and_stages_without_execution() {
        let (_temp, runtime) = fixture();
        let selected = GraphicsComponents::discover(&runtime, None, None).unwrap();
        assert!(
            !selected
                .available(&runtime, &GraphicsApi::DirectX11, true)
                .dxmt
        );
        assert!(
            !selected
                .available(&runtime, &GraphicsApi::DirectX9OrOlder, true)
                .dxvk
        );
        assert!(
            !selected
                .available(&runtime, &GraphicsApi::DirectX10, true)
                .dxvk
        );
        for (engine, api) in [
            (TargetEngine::Dxvk, GraphicsApi::DirectX11),
            (TargetEngine::Vkd3d, GraphicsApi::DirectX12),
        ] {
            let prefix = tempfile::tempdir().unwrap();
            fs::create_dir_all(prefix.path().join("drive_c/windows/system32")).unwrap();
            let plan = LaunchPlan::new(
                runtime.clone(),
                prefix.path().to_path_buf(),
                engine,
                selected.clone(),
                true,
                Some(false),
                &api,
            )
            .unwrap();
            assert!(plan.stage_graphics().unwrap() > 0);
            assert_eq!(plan.stage_graphics().unwrap(), 0);
            let path = prefix.path().join("drive_c/windows/system32/d3d11.dll");
            assert_eq!(
                fs::read(path).unwrap(),
                crate::runtime_checks::test_pe_image(0xa641)
            );
        }
    }

    #[test]
    fn pair_rejects_changed_components_companions_and_uncovered_files() {
        for relative in [
            "share/nucleon/arm64ec/dxvk/dxgi.dll",
            "share/nucleon/arm64ec/vkd3d/d3d12core.dll",
            "lib/wine/aarch64-unix/libarm64ecfex.so",
        ] {
            let (_temp, runtime) = fixture();
            let path = runtime.root.join(relative);
            let mut changed = fs::read(&path).unwrap();
            changed.push(1);
            fs::write(path, changed).unwrap();
            let error = format!("{:#}", ResolvedRuntime::inspect(&runtime.root).unwrap_err());
            assert!(
                error.contains("digest mismatch") && error.contains(relative),
                "{error}"
            );
        }
        let (_temp, runtime) = fixture();
        let path = runtime.graphics.frontend_manifest.as_ref().unwrap();
        let mut contract: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        contract["files"]
            .as_object_mut()
            .unwrap()
            .remove("lib/wine/aarch64-unix/libarm64ecfex.so");
        fs::write(path, contract.to_string()).unwrap();
        let error = format!("{:#}", ResolvedRuntime::inspect(&runtime.root).unwrap_err());
        assert!(
            error.contains("not covered") && error.contains("libarm64ecfex.so"),
            "{error}"
        );
    }

    #[test]
    fn graphics_overrides_and_extra_staged_dlls_fail_before_prefix_mutation() {
        let (_temp, runtime) = fixture();
        let mut selected = GraphicsComponents::discover(&runtime, None, None).unwrap();
        selected.dxvk.as_mut().unwrap().root = runtime.root.join("other-bundle");
        fs::create_dir(&selected.dxvk.as_ref().unwrap().root).unwrap();
        let prefix = runtime.root.join("never-created-prefix");
        let error = LaunchPlan::new(
            runtime.clone(),
            prefix.clone(),
            TargetEngine::Dxvk,
            selected,
            true,
            None,
            &GraphicsApi::DirectX11,
        )
        .unwrap_err();
        assert!(error.to_string().contains("override"));
        assert!(!prefix.exists());
        let mut selected = GraphicsComponents::discover(&runtime, None, None).unwrap();
        selected.dxvk.as_mut().unwrap().x64_d3d9 =
            selected.dxvk.as_ref().unwrap().x64_d3d11.clone();
        assert!(validate_selected(&runtime, &selected).is_err());
        let selected = GraphicsComponents::discover(&runtime, None, None).unwrap();
        assert!(
            LaunchPlan::new(
                runtime,
                prefix.clone(),
                TargetEngine::Dxvk,
                selected,
                false,
                None,
                &GraphicsApi::DirectX11
            )
            .is_err()
        );
        assert!(!prefix.exists());
    }

    #[test]
    fn pair_rejects_wrong_abi_even_when_the_file_digest_is_updated() {
        for machine in [0x8664, 0xaa64, 0xa64e] {
            let (_temp, runtime) = fixture();
            let relative = "share/nucleon/arm64ec/dxvk/dxgi.dll";
            let file = runtime.root.join(relative);
            fs::write(&file, crate::runtime_checks::test_pe_image(machine)).unwrap();
            let path = runtime.graphics.frontend_manifest.as_ref().unwrap();
            let mut contract: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
            contract["files"][relative] = crate::staging::digest(&file).unwrap().into();
            fs::write(path, contract.to_string()).unwrap();
            let error = format!("{:#}", ResolvedRuntime::inspect(&runtime.root).unwrap_err());
            assert!(
                error.contains("Expected ARM64EC graphics ABI") && error.contains("dxgi.dll"),
                "{error}"
            );
        }
    }

    #[test]
    fn pair_rejects_wrong_sources_or_thread_pointer_options() {
        for source_change in [true, false] {
            let (_temp, runtime) = fixture();
            let path = runtime.graphics.frontend_manifest.as_ref().unwrap();
            let mut contract: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
            if source_change {
                contract["builds"]["dxvk"]["source"]["revision"] = "unknown".into();
            } else {
                contract["builds"]["dxvk"]["options"] = serde_json::json!([
                    "-Dc_args=-femulated-tls -DNUCLEON_TEB_TSD_OFFSET=0x17f800"
                ]);
            }
            fs::write(path, contract.to_string()).unwrap();
            let error = format!("{:#}", ResolvedRuntime::inspect(&runtime.root).unwrap_err());
            assert!(
                error.contains("Unpaired ARM64EC dxvk build provenance"),
                "{error}"
            );
        }
    }

    #[test]
    fn graphics_matcher_does_not_relax_guest_or_other_frontend_abis() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        for machine in [0x8664, 0xaa64, 0xa64e, 0xa641, 0x14c] {
            fs::write(temp.path(), crate::runtime_checks::test_pe_image(machine)).unwrap();
            assert_eq!(
                crate::runtime_checks::graphics_pe(Some(temp.path()), true, FrontendAbi::Arm64Ec),
                machine == 0xa641
            );
            assert_eq!(
                crate::runtime_checks::pe(Some(temp.path()), true),
                machine == 0x8664
            );
            assert!(!crate::runtime_checks::graphics_pe(
                Some(temp.path()),
                false,
                FrontendAbi::Arm64Ec
            ));
        }
    }
}
