//! Optional local-artifact validation; does not execute Wine, Steam or a game.
#![cfg(target_os = "macos")]
use nucleon_core::{prefix, runtime::ResolvedRuntime, steam_ntdll};
use std::{fs, path::PathBuf, process::Command};

#[test]
#[ignore = "requires NUCLEON_TEST_ARM64_RUNTIME and NUCLEON_TEST_STEAM_BRIDGE"]
fn actual_fex_images_and_signed_steam_bridge_validate_together() {
    let source = PathBuf::from(std::env::var_os("NUCLEON_TEST_ARM64_RUNTIME").unwrap());
    let bridge = PathBuf::from(std::env::var_os("NUCLEON_TEST_STEAM_BRIDGE").unwrap());
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("runtime");
    // Copy only the binary boundaries inspected by the contract, without loading them.
    for name in [
        "bin/wine",
        "bin/wineserver",
        "lib/wine/aarch64-unix/ntdll.so",
        "lib/wine/aarch64-windows/ntdll.dll",
        "lib/wine/aarch64-windows/libarm64ecfex.dll",
    ] {
        let target = root.join(name);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(source.join(name), target).unwrap();
    }
    fs::write(
        root.join("nucleon-runtime.toml"),
        r#"
schema=1
mode='arm64-fex'
guests=['x64']
loader='bin/wine'
server='bin/wineserver'
unix_dir='lib/wine/aarch64-unix'
windows_dirs=['lib/wine/aarch64-windows']
backends=['auto']
steam_protocol='valve-redirect'
steam_helper=true
[[emulators]]
guest='x64'
path='lib/wine/aarch64-windows/libarm64ecfex.dll'
[prefix]
family='tolkara-fex'
abi=1
"#,
    )
    .unwrap();
    let runtime = ResolvedRuntime::inspect(&root).unwrap();
    let original = fs::read(bridge.join("aarch64-unix/lsteamclient.so")).unwrap();
    let valve = fs::read(bridge.join("steamclient64.dll.valve")).unwrap();
    let prefix = temp.path().join("prefix");
    prefix::stage_bridge_libraries_for_runtime(&prefix, &bridge, &runtime).unwrap();
    steam_ntdll::bind_prefix_for_runtime(&prefix, &runtime).unwrap();
    let steam = prefix.join("drive_c/Program Files (x86)/Steam");
    assert!(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--strict"])
            .arg(steam.join("lsteamclient.so"))
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        fs::read(bridge.join("aarch64-unix/lsteamclient.so")).unwrap(),
        original
    );
    assert_eq!(fs::read(steam.join("steamclient64.dll")).unwrap(), valve);
    assert_eq!(
        fs::read_link(steam.join("ntdll.so")).unwrap(),
        runtime.unix_dir.join("ntdll.so")
    );
}

#[test]
#[ignore = "requires NUCLEON_TEST_ARM64_RUNTIME pointing to an assembled DX11/DX12 runtime"]
fn assembled_runtime_resolves_both_graphics_plans_without_executing_wine() {
    use nucleon_core::{
        detector::{GraphicsApi, TargetEngine},
        launch::{GraphicsComponents, LaunchPlan},
        paths,
    };
    let source = PathBuf::from(std::env::var_os("NUCLEON_TEST_ARM64_RUNTIME").unwrap());
    let runtime = ResolvedRuntime::inspect(&source).unwrap();
    let components = GraphicsComponents::discover(&runtime, None, None).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let prefix = paths::game_prefix(temp.path(), &runtime);
    fs::create_dir_all(prefix.join("drive_c/windows/system32")).unwrap();
    fs::create_dir_all(prefix.join("drive_c/windows/syswow64")).unwrap();
    for (engine, api) in [
        (TargetEngine::Dxmt, GraphicsApi::DirectX11),
        (TargetEngine::Vkd3d, GraphicsApi::DirectX12),
    ] {
        let plan = LaunchPlan::new(
            runtime.clone(),
            prefix.clone(),
            engine,
            components.clone(),
            true,
            Some(true),
            &api,
        )
        .unwrap();
        use std::os::unix::fs::MetadataExt;
        assert!(plan.stage_graphics().unwrap() > 0);
        let dxgi = prefix.join("drive_c/windows/system32/dxgi.dll");
        let inode = fs::metadata(&dxgi).unwrap().ino();
        assert_eq!(plan.stage_graphics().unwrap(), 0);
        assert_eq!(fs::metadata(&dxgi).unwrap().ino(), inode);
        fs::remove_file(&dxgi).unwrap();
        assert_eq!(plan.stage_graphics().unwrap(), 1);
        let environment = plan.execution_env(false, false);
        assert_eq!(environment["FEX_TSOENABLED"], "1");
        assert_eq!(environment["NUCLEON_EMULATE_KUSD"], "1");
        assert!(!environment.contains_key("ROSETTA_ADVERTISE_AVX"));
        assert!(environment["WINEDLLPATH"].contains("aarch64-windows"));
    }
    assert!(!prefix.join("user.reg").exists());
}

#[test]
#[ignore = "requires runtime/bridge environment and isolated NUCLEON_SUPPORT_DIR; no Wine execution"]
fn signed_bridge_variants_and_prefix_copies_are_reused_and_repaired() {
    use std::os::unix::fs::MetadataExt;
    assert!(std::env::var_os("NUCLEON_SUPPORT_DIR").is_some());
    let runtime = ResolvedRuntime::inspect(&PathBuf::from(
        std::env::var_os("NUCLEON_TEST_ARM64_RUNTIME").unwrap(),
    ))
    .unwrap();
    let bridge = PathBuf::from(std::env::var_os("NUCLEON_TEST_STEAM_BRIDGE").unwrap());
    let temp = tempfile::tempdir().unwrap();
    let p1 = temp.path().join("prefix1");
    let p2 = temp.path().join("prefix2");
    prefix::prepare_bridge_libraries_for_runtime(&p1, &bridge, &runtime).unwrap();
    let target = p1.join("drive_c/Program Files (x86)/Steam/lsteamclient.so");
    let inode = fs::metadata(&target).unwrap().ino();
    let expected = fs::read(&target).unwrap();
    prefix::prepare_bridge_libraries_for_runtime(&p1, &bridge, &runtime).unwrap();
    assert_eq!(fs::metadata(&target).unwrap().ino(), inode);
    let cache = nucleon_core::paths::support_dir().join("cache/steam-bridge");
    let artifacts = walkdir::WalkDir::new(&cache)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_name() == "lsteamclient.so")
        .map(|e| {
            (
                e.path().to_path_buf(),
                fs::metadata(e.path()).unwrap().ino(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(artifacts.len(), 1);
    let mut compatible = runtime.clone();
    compatible.unix_dir = temp.path().join("compatible-new-generation/aarch64-unix");
    fs::create_dir_all(&compatible.unix_dir).unwrap();
    fs::copy(
        runtime.unix_dir.join("ntdll.so"),
        compatible.unix_dir.join("ntdll.so"),
    )
    .unwrap();
    prefix::prepare_bridge_libraries_for_runtime(&p2, &bridge, &compatible).unwrap();
    assert_eq!(fs::metadata(&artifacts[0].0).unwrap().ino(), artifacts[0].1);
    assert_eq!(
        fs::read(p2.join("drive_c/Program Files (x86)/Steam/lsteamclient.so")).unwrap(),
        expected
    );
    fs::write(&target, b"damaged private copy").unwrap();
    prefix::prepare_bridge_libraries_for_runtime(&p1, &bridge, &runtime).unwrap();
    assert_eq!(fs::read(&target).unwrap(), expected);
    assert!(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--strict"])
            .arg(&target)
            .status()
            .unwrap()
            .success()
    );
}
