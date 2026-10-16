//! Read, import and stage a local experimental generation; never execute Wine/Steam.
use nucleon_core::{
    detector::{GraphicsApi, TargetEngine},
    launch::{GraphicsComponents, LaunchPlan},
    managed_runtime,
    runtime::{FrontendAbi, ResolvedRuntime},
};
use std::{fs, path::PathBuf};

#[test]
#[ignore = "requires NUCLEON_TEST_ARM64EC_RUNTIME; no execution or import"]
fn local_arm64ec_generation_resolves_without_execution() {
    let source = PathBuf::from(std::env::var_os("NUCLEON_TEST_ARM64EC_RUNTIME").unwrap());
    let runtime = ResolvedRuntime::inspect(&source).unwrap();
    let selected = GraphicsComponents::discover(&runtime, None, None).unwrap();
    assert_eq!(runtime.graphics.frontend_abi, FrontendAbi::Arm64Ec);
    let temp = tempfile::tempdir().unwrap();
    for (engine, api) in [
        (TargetEngine::Dxvk, GraphicsApi::DirectX11),
        (TargetEngine::Vkd3d, GraphicsApi::DirectX12),
    ] {
        let prefix = temp.path().join(format!("{engine:?}"));
        fs::create_dir_all(prefix.join("drive_c/windows/system32")).unwrap();
        let plan = LaunchPlan::new(
            runtime.clone(),
            prefix,
            engine,
            selected.clone(),
            true,
            Some(false),
            &api,
        )
        .unwrap();
        assert!(plan.stage_graphics().unwrap() > 0);
        assert_eq!(plan.stage_graphics().unwrap(), 0);
    }
    let mut wrong = selected;
    wrong.dxvk.as_mut().unwrap().root = temp.path().to_path_buf();
    let prefix = temp.path().join("must-not-exist");
    assert!(
        LaunchPlan::new(
            runtime,
            prefix.clone(),
            TargetEngine::Dxvk,
            wrong,
            true,
            None,
            &GraphicsApi::DirectX11
        )
        .is_err()
    );
    assert!(!prefix.exists());
}

#[test]
#[ignore = "requires NUCLEON_TEST_ARM64EC_RUNTIME and isolated NUCLEON_SUPPORT_DIR; no Wine/Steam execution"]
fn local_arm64ec_generation_imports_and_stages_without_execution() {
    let source = PathBuf::from(std::env::var_os("NUCLEON_TEST_ARM64EC_RUNTIME").unwrap());
    assert!(
        std::env::var_os("NUCLEON_SUPPORT_DIR").is_some(),
        "Set an isolated support directory"
    );
    let runtime = ResolvedRuntime::inspect(&source).unwrap();
    assert_eq!(runtime.graphics.frontend_abi, FrontendAbi::Arm64Ec);
    let imported = managed_runtime::install(&source).unwrap();
    managed_runtime::verify(&imported).unwrap();
    let runtime = ResolvedRuntime::inspect(&imported).unwrap();
    let selected = GraphicsComponents::discover(&runtime, None, None).unwrap();
    let temp = tempfile::tempdir().unwrap();
    for (engine, api) in [
        (TargetEngine::Dxvk, GraphicsApi::DirectX11),
        (TargetEngine::Vkd3d, GraphicsApi::DirectX12),
    ] {
        let prefix = temp.path().join(format!("{engine:?}"));
        let system = prefix.join("drive_c/windows/system32");
        fs::create_dir_all(&system).unwrap();
        let plan = LaunchPlan::new(
            runtime.clone(),
            prefix,
            engine,
            selected.clone(),
            true,
            Some(false),
            &api,
        )
        .unwrap();
        assert!(plan.stage_graphics().unwrap() > 0);
        assert_eq!(plan.stage_graphics().unwrap(), 0);
        let dxvk = selected.dxvk.as_ref().unwrap();
        assert_eq!(
            fs::read(system.join("dxgi.dll")).unwrap(),
            fs::read(dxvk.x64_dxgi.as_ref().unwrap()).unwrap()
        );
        if engine == TargetEngine::Vkd3d {
            assert_eq!(
                fs::read(system.join("d3d12core.dll")).unwrap(),
                fs::read(
                    selected
                        .vkd3d
                        .as_ref()
                        .unwrap()
                        .x64_d3d12core
                        .as_ref()
                        .unwrap()
                )
                .unwrap()
            );
        }
        let environment = plan.execution_env(false, false);
        assert_eq!(environment["FEX_TSOENABLED"], "1");
        assert_eq!(environment["NUCLEON_EMULATE_KUSD"], "1");
    }
    // Reimport reuses the immutable generation without changing runtime selection.
    assert_eq!(managed_runtime::install(&source).unwrap(), imported);
}
