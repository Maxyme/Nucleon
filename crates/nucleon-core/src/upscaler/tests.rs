use super::*;
use crate::repo_assets::FIXTURES;
use crate::{
    runtime::{self, ExecutionMode},
    runtime_checks::test_pe_image,
};

const TEMPLATE: &str = "[Upscalers]\nDx11Upscaler=auto\nDx12Upscaler=auto\n[FrameGen]\nEnabled=auto\nFGInput=auto\nFGOutput=auto\n[DLSS]\nEnabled=auto\nPassThrough=auto\n[Inputs]\nEnableDlssInputs=auto\nEnableXeSSInputs=auto\nEnableFsr2Inputs=auto\nEnableFsr3Inputs=auto\nEnableFfxInputs=auto\n[Libraries]\nOptiDllPath=auto\nNvngxPath=auto\n[Spoofing]\nDxgi=auto\nVulkan=auto\nStreamlineSpoofing=auto\nRegistry=auto\n[NvApi]\nOverrideNvapiDll=auto\n[Menu]\nOverlayMenu=auto\n[Plugins]\nLoadAsiPlugins=auto\nPath=auto\n[Log]\nLogToFile=auto\nLogToConsole=auto\nLogLevel=auto\nLogFileName=auto\n";

struct Fixture {
    _temp: tempfile::TempDir,
    _runtime: tempfile::TempDir,
    root: PathBuf,
    exe: PathBuf,
    plan: LaunchPlan,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let root = base.join("support");
        let game = base.join("Game with spaces");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&game).unwrap();
        let exe = game.join("Game.exe");
        let mut pe = test_pe_image(0x8664);
        pe[150..152].copy_from_slice(&2u16.to_le_bytes());
        fs::write(&exe, pe).unwrap();
        let (runtime_dir, runtime) = runtime::fixture(ExecutionMode::Arm64Fex);
        let mut plan = LaunchPlan::new(
            runtime,
            base.join("pfx"),
            TargetEngine::WineStaging,
            Default::default(),
            true,
            Some(false),
            &GraphicsApi::DirectX12,
        )
        .unwrap()
        .without_steam();
        plan.engine = TargetEngine::Vkd3d;
        let f = Self {
            _temp: temp,
            _runtime: runtime_dir,
            root,
            exe,
            plan,
        };
        f.provider("stock");
        f
    }
    fn provider(&self, name: &str) {
        let manifest = assets::provider_path(&self.root, name).unwrap();
        let dir = manifest.parent().unwrap();
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("OptiScaler.dll"), test_pe_image(0x8664)).unwrap();
        fs::write(dir.join("OptiScaler.ini"), TEMPLATE).unwrap();
        fs::write(dir.join("libxess.dll"), test_pe_image(0x8664)).unwrap();
        let files = ["OptiScaler.dll", "OptiScaler.ini", "libxess.dll"]
            .into_iter()
            .map(|n| {
                (
                    PathBuf::from(n),
                    crate::staging::digest(&dir.join(n)).unwrap(),
                )
            })
            .collect();
        save(
            &manifest,
            &Provider {
                schema: 1,
                provider: name.into(),
                version: "fixture".into(),
                files,
                framework_sha256: None,
            },
        )
        .unwrap();
    }
    fn resolve(&self, backend: Backend) -> Result<Resolved> {
        resolve(
            &self.root,
            &self.plan,
            &self.exe,
            &Settings {
                backend,
                input: Input::Dlss,
                renderer: None,
            },
        )
    }
    fn prepare(&self, backend: Backend) -> Result<Binding> {
        let r = self.resolve(backend)?;
        let i = Installation::open(&self.root, &r)?;
        prepare(&self.root, &i, &r)
    }
}

#[test]
fn configuration_is_strict_and_explicit_game_overrides_inheritance() {
    let old: crate::profile::GameProfile = serde_json::from_str("{}").unwrap();
    assert!(old.upscaler.is_none());
    let fsr412: crate::profile::GameProfile =
        serde_json::from_str(r#"{"upscaler":{"backend":"fsr4.1.2","input":"dlss"}}"#).unwrap();
    assert_eq!(fsr412.upscaler.unwrap().backend, Backend::Fsr412);
    for json in [
        r#"{"upscaler":{"backend":"fsr4.1.3"}}"#,
        r#"{"upscaler":{"backed":"fsr2"}}"#,
        r#"{"upscaler":{"backend":"fsr2","input":"wrong"}}"#,
    ] {
        assert!(serde_json::from_str::<crate::profile::GameProfile>(json).is_err());
    }
    let explicit = crate::profile::GameProfile {
        upscaler: Some(Settings::selected(Backend::Game)),
        ..Default::default()
    };
    let fallback = crate::profile::GameProfile {
        upscaler: Some(Settings::selected(Backend::Fsr2)),
        ..Default::default()
    };
    assert_eq!(
        explicit.with_defaults(fallback).upscaler.unwrap().backend,
        Backend::Game
    );
    let app = crate::application::Settings {
        upscaler: Some(Settings::selected(Backend::Fsr31)),
        ..Default::default()
    };
    assert_eq!(app.profile().upscaler.unwrap().backend, Backend::Fsr31);
}

#[test]
fn native_models_resolve_stage_and_switch_with_their_own_version_and_profile() {
    let mut f = Fixture::new();
    let (_runtime_dir, runtime) = runtime::fixture(ExecutionMode::Intel);
    let external = runtime.root.join("lib/external");
    fs::create_dir_all(external.join("D3DMetal.framework")).unwrap();
    let framework = external.join("D3DMetal.framework/D3DMetal");
    fs::write(
        &framework,
        crate::runtime_checks::test_image("framework", 1),
    )
    .unwrap();
    fs::write(
        external.join("libd3dshared.dylib"),
        crate::runtime_checks::test_image("shared", 1),
    )
    .unwrap();
    let windows = runtime.root.join("lib/wine/x86_64-windows");
    fs::create_dir_all(&windows).unwrap();
    for name in ["d3d11.dll", "d3d12.dll", "dxgi.dll", "nvngx-on-metalfx.dll"] {
        fs::write(windows.join(name), test_pe_image(0x8664)).unwrap();
    }
    f.plan.runtime = runtime;
    f.plan.engine = TargetEngine::WineStaging;
    f.provider("metalfx");
    let bridge = windows.join("nvngx-on-metalfx.dll");
    let bridge_dir = f
        .root
        .join("providers/bridge")
        .join(assets::bridge_key(&crate::staging::digest(&bridge).unwrap()).unwrap());
    fs::create_dir_all(&bridge_dir).unwrap();
    fs::write(
        bridge_dir.join("nvngx.dll"),
        FIXTURES
            .get_file("upscaler/ngx-forward.bin")
            .unwrap()
            .contents(),
    )
    .unwrap();
    save(
        &bridge_dir.join("provider.json"),
        &Provider {
            schema: 1,
            provider: "bridge".into(),
            version: "fixture".into(),
            files: [(
                PathBuf::from("nvngx.dll"),
                crate::staging::digest(&bridge_dir.join("nvngx.dll")).unwrap(),
            )]
            .into(),
            framework_sha256: None,
        },
    )
    .unwrap();
    let digest = crate::staging::digest(&framework).unwrap();
    for model in [native::Model::Fsr411, native::Model::Fsr412] {
        let dir = f
            .root
            .join("providers")
            .join(model.provider())
            .join(assets::native_model_key(&digest, model).unwrap());
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("model.bin"), model.version()).unwrap();
        save(
            &dir.join("provider.json"),
            &Provider {
                schema: 1,
                provider: model.provider().into(),
                version: model.version().into(),
                files: [(
                    PathBuf::from("model.bin"),
                    crate::staging::digest(&dir.join("model.bin")).unwrap(),
                )]
                .into(),
                framework_sha256: Some(digest.clone()),
            },
        )
        .unwrap();
    }
    for (backend, version, profile) in [
        (Backend::Fsr4NativeLite, "4.1.1", "lossy"),
        (Backend::Fsr412, "4.1.2", "int8-lite"),
        (Backend::Fsr4Native, "4.1.1", "exact-fast"),
    ] {
        let binding = f.prepare(backend).unwrap();
        assert_eq!(binding.environment["FSR4_VERSION"], version);
        assert_eq!(binding.environment["FSR4_PROFILE"], profile);
        assert!(
            binding.environment["FSR4_ROOT"].contains(backend.native_model().unwrap().provider())
        );
        let installation = Installation::existing(&f.root, &f.exe).unwrap().unwrap();
        assert!(
            fs::read_to_string(installation.state.join("panel.conf"))
                .unwrap()
                .contains(&format!("profile={profile}\n"))
        );
    }
    let manifest = f.resolve(Backend::Fsr412).unwrap().native_provider.unwrap();
    let mut provider: Provider = read(&manifest).unwrap();
    provider.version = "4.1.1".into();
    save(&manifest, &provider).unwrap();
    assert!(
        f.resolve(Backend::Fsr412)
            .unwrap_err()
            .to_string()
            .contains("version")
    );
    assert!(f.resolve(Backend::Fsr4Native).is_ok());
    f.prepare(Backend::Game).unwrap();
    let installation = Installation::existing(&f.root, &f.exe).unwrap().unwrap();
    assert!(!installation.directory.join("nvngx.dll").exists());
}

#[test]
fn resolver_rejects_unproven_routes_without_creating_state() {
    let mut f = Fixture::new();
    assert!(f.resolve(Backend::Fsr31).is_ok());
    assert!(!f.root.join("games").exists());
    f.plan.api = GraphicsApi::DirectX11;
    assert!(f.resolve(Backend::Fsr2).is_ok());
    assert!(
        f.resolve(Backend::Xess)
            .unwrap_err()
            .to_string()
            .contains("DX12")
    );
    f.plan.api = GraphicsApi::Vulkan;
    assert!(f.resolve(Backend::Fsr2).is_err());
    f.plan.api = GraphicsApi::DirectX12;
    assert!(
        f.resolve(Backend::Fsr4NativeLite)
            .unwrap_err()
            .to_string()
            .contains("GPTK")
    );
    f.plan.runtime.graphics.frontend_abi = FrontendAbi::Arm64Ec;
    assert!(
        f.resolve(Backend::Fsr2)
            .unwrap_err()
            .to_string()
            .contains("ARM64EC")
    );
    f.plan.runtime.graphics.frontend_abi = FrontendAbi::X64;
    f.plan.is64 = false;
    assert!(
        f.resolve(Backend::Fsr2)
            .unwrap_err()
            .to_string()
            .contains("x64")
    );
    assert!(!f.root.join("games").exists());
}

#[test]
fn ordinary_launches_do_not_bind_installers_or_guess_missing_inputs() {
    let f = Fixture::new();
    assert!(
        resolve_for_launch(&f.root, &f.plan, &f.exe, &Settings::selected(Backend::Fsr2))
            .unwrap_err()
            .to_string()
            .contains("Installers")
    );
    assert!(
        resolve_for_launch(&f.root, &f.plan, &f.exe, &Settings::selected(Backend::Game)).is_ok()
    );
    assert!(
        resolve(&f.root, &f.plan, &f.exe, &Settings::selected(Backend::Fsr2))
            .unwrap_err()
            .to_string()
            .contains("input")
    );
}

#[test]
fn generated_ini_switches_outputs_and_game_restores_every_owned_file() {
    let f = Fixture::new();
    let first = f.prepare(Backend::Fsr2).unwrap();
    let dir = f.exe.parent().unwrap();
    assert!(
        fs::read_to_string(dir.join("OptiScaler.ini"))
            .unwrap()
            .contains("Dx12Upscaler=fsr22")
    );
    assert!(dir.join("OptiScaler/libxess.dll").exists());
    assert_eq!(first.activation, "not-observed");
    let mut env = HashMap::from([(
        "WINEDLLOVERRIDES".into(),
        "steamclient64=n,b;dxgi=b;d3d11=b".into(),
    )]);
    apply_environment(&first, &mut env).unwrap();
    assert!(env["WINEDLLOVERRIDES"].contains("steamclient64=n,b"));
    assert!(env["WINEDLLOVERRIDES"].contains("d3d11=b"));
    assert!(env["WINEDLLOVERRIDES"].contains("dxgi=n,b"));
    f.prepare(Backend::Fsr31).unwrap();
    assert!(
        fs::read_to_string(dir.join("OptiScaler.ini"))
            .unwrap()
            .contains("Dx12Upscaler=fsr31")
    );
    f.prepare(Backend::Game).unwrap();
    assert!(!dir.join("dxgi.dll").exists());
    assert!(!dir.join("OptiScaler.ini").exists());
    assert!(!dir.join("OptiScaler/libxess.dll").exists());
    let report = status(&f.root, &f.exe).unwrap();
    assert_eq!(report["status"], "restored");
    assert!(report["changed_files"].as_array().unwrap().is_empty());
    f.prepare(Backend::Fsr2).unwrap();
    assert_eq!(status(&f.root, &f.exe).unwrap()["status"], "prepared");
}

#[test]
fn embedded_fsr3_and_fidelityfx_inputs_do_not_enable_other_hook_families() {
    for input in [Input::Fsr2, Input::Fsr3, Input::Fsr31, Input::Dlss] {
        let f = Fixture::new();
        let mut resolved = f.resolve(Backend::Fsr31).unwrap();
        resolved.input = input;
        resolved.settings.input = input;
        let installation = Installation::open(&f.root, &resolved).unwrap();
        prepare(&f.root, &installation, &resolved).unwrap();
        let text = fs::read_to_string(installation.directory.join("OptiScaler.ini")).unwrap();
        for (name, enabled) in [
            ("EnableDlssInputs", input == Input::Dlss),
            ("EnableFsr2Inputs", input == Input::Fsr2),
            ("EnableFsr3Inputs", input == Input::Fsr3),
            ("EnableFfxInputs", input == Input::Fsr31),
        ] {
            assert!(text.contains(&format!("{name}={enabled}")), "{text}");
        }
        assert!(text.contains("Enabled=false"));
    }
}

#[test]
fn metalfx_bridge_keeps_vendor_name_and_all_owned_dlls_restore_together() {
    let f = Fixture::new();
    let bridge = f.root.join("nvngx-on-metalfx.dll");
    let original = test_pe_image(0x8664);
    fs::write(&bridge, &original).unwrap();
    let key = assets::bridge_key(&crate::staging::digest(&bridge).unwrap()).unwrap();
    let directory = f.root.join("providers/bridge").join(key);
    fs::create_dir_all(&directory).unwrap();
    let forwarding = FIXTURES
        .get_file("upscaler/ngx-forward.bin")
        .unwrap()
        .contents()
        .to_vec();
    fs::write(directory.join("nvngx.dll"), &forwarding).unwrap();
    save(
        &directory.join("provider.json"),
        &Provider {
            schema: 1,
            provider: "bridge".into(),
            version: "fixture".into(),
            files: BTreeMap::from([(
                "nvngx.dll".into(),
                crate::staging::digest(&directory.join("nvngx.dll")).unwrap(),
            )]),
            framework_sha256: None,
        },
    )
    .unwrap();
    let mut resolved = f.resolve(Backend::Fsr31).unwrap();
    resolved.settings.backend = Backend::Metalfx;
    resolved.bridge = Some(bridge.clone());
    let installation = Installation::open(&f.root, &resolved).unwrap();
    prepare(&f.root, &installation, &resolved).unwrap();
    assert_eq!(
        fs::read(installation.directory.join("nvngx-on-metalfx.dll")).unwrap(),
        original
    );
    assert_eq!(
        fs::read(installation.directory.join("nvngx.dll")).unwrap(),
        original
    );
    assert_eq!(
        fs::read(
            installation
                .directory
                .join("OptiScaler/nucleon-metalfx/nvngx.dll")
        )
        .unwrap(),
        forwarding
    );
    restore(&installation).unwrap();
    for name in [
        "nvngx.dll",
        "nvngx-on-metalfx.dll",
        "OptiScaler/nucleon-metalfx/nvngx.dll",
    ] {
        assert!(!installation.directory.join(name).exists());
    }
    assert_eq!(fs::read(bridge).unwrap(), original);
}

#[test]
fn edited_settings_survive_normal_launch_and_are_archived_before_switch_or_restore() {
    let f = Fixture::new();
    f.prepare(Backend::Fsr2).unwrap();
    let path = f.exe.parent().unwrap().join("OptiScaler.ini");
    fs::write(&path, b"user adjustments").unwrap();
    f.prepare(Backend::Fsr2).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"user adjustments");
    assert_eq!(status(&f.root, &f.exe).unwrap()["settings_modified"], true);
    f.prepare(Backend::Fsr31).unwrap();
    let i = Installation::existing(&f.root, &f.exe).unwrap().unwrap();
    let archive = fs::read_dir(i.state.join("user-settings"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(fs::read(archive).unwrap(), b"user adjustments");
}

#[test]
fn existing_mod_and_changed_executable_block_all_game_mutations() {
    let f = Fixture::new();
    let proxy = f.exe.parent().unwrap().join("dxgi.dll");
    fs::write(&proxy, b"ReShade").unwrap();
    assert!(
        f.prepare(Backend::Fsr2)
            .unwrap_err()
            .to_string()
            .contains("unowned")
    );
    assert_eq!(fs::read(&proxy).unwrap(), b"ReShade");
    assert!(!f.exe.parent().unwrap().join("OptiScaler.ini").exists());
    fs::remove_file(&proxy).unwrap();
    f.prepare(Backend::Fsr2).unwrap();
    fs::write(&proxy, b"changed proxy").unwrap();
    let settings = fs::read(f.exe.parent().unwrap().join("OptiScaler.ini")).unwrap();
    assert!(f.prepare(Backend::Game).is_err());
    assert_eq!(
        fs::read(f.exe.parent().unwrap().join("OptiScaler.ini")).unwrap(),
        settings
    );
    assert_eq!(fs::read(&proxy).unwrap(), b"changed proxy");
}

#[test]
fn shared_installation_lock_symlink_escape_and_corrupt_provider_fail_closed() {
    let f = Fixture::new();
    let r = f.resolve(Backend::Fsr2).unwrap();
    let i = Installation::open(&f.root, &r).unwrap();
    assert!(Installation::open(&f.root, &r).is_err());
    drop(i);
    let outside = f._temp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, f.exe.parent().unwrap().join("OptiScaler")).unwrap();
    assert!(f.prepare(Backend::Fsr2).is_err());
    assert!(!outside.join("libxess.dll").exists());
    let manifest = assets::provider_path(&f.root, "stock").unwrap();
    fs::write(
        manifest.parent().unwrap().join("OptiScaler.dll"),
        b"tampered",
    )
    .unwrap();
    assert!(f.resolve(Backend::Fsr2).is_err());
}

#[test]
fn raw_preferences_cannot_override_managed_modules_and_stale_native_env_is_removed() {
    assert!(validate_raw_overrides("steamclient=n,b;dxgi=b", Backend::Fsr2).is_err());
    assert!(validate_raw_overrides("dxgi=n,b;d3dcompiler_47=n,b", Backend::Fsr2).is_ok());
    assert!(compose_overrides("dxgi=b;dxgi=n", &[]).is_err());
    let f = Fixture::new();
    let binding = f.prepare(Backend::Game).unwrap();
    let mut env = HashMap::from([
        ("FSR4_PROFILE".into(), "lossy".into()),
        ("D3DMETAL_FRAMEWORK_PATH".into(), "stale".into()),
    ]);
    apply_environment(&binding, &mut env).unwrap();
    assert!(!env.contains_key("FSR4_PROFILE"));
    assert!(!env.contains_key("D3DMETAL_FRAMEWORK_PATH"));
}

#[test]
fn preview_is_read_only_and_interrupted_journal_restores_original_mode_and_symlink() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new();
    let r = f.resolve(Backend::Fsr2).unwrap();
    preview(&f.root, &r).unwrap();
    assert!(!f.root.join("games").exists());
    f.prepare(Backend::Fsr2).unwrap();
    let i = Installation::existing(&f.root, &f.exe).unwrap().unwrap();
    let path = i.state.join("install.json");
    let mut journal: serde_json::Value = read(&path).unwrap();
    let backup = i.state.join("original.dll");
    fs::write(&backup, b"original proxy").unwrap();
    fs::set_permissions(&backup, fs::Permissions::from_mode(0o755)).unwrap();
    let link = i.state.join("original.ini");
    symlink("user.ini", &link).unwrap();
    fs::write(i.directory.join("user.ini"), b"user config").unwrap();
    for entry in journal["files"].as_array_mut().unwrap() {
        if entry["relative"] == "dxgi.dll" {
            entry["original"] =
                serde_json::json!({"sha256":crate::staging::digest(&backup).unwrap(),"mode":0o755});
            entry["backup"] = backup.display().to_string().into();
        } else if entry["relative"] == "OptiScaler.ini" {
            entry["original"] = serde_json::json!({"symlink":"user.ini"});
            entry["backup"] = link.display().to_string().into();
        }
    }
    journal["status"] = "installing".into();
    save(&path, &journal).unwrap();
    assert!(prepare(&f.root, &i, &r).is_err());
    restore(&i).unwrap();
    assert_eq!(
        fs::read(i.directory.join("dxgi.dll")).unwrap(),
        b"original proxy"
    );
    assert_eq!(
        fs::metadata(i.directory.join("dxgi.dll"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(
        fs::read_link(i.directory.join("OptiScaler.ini")).unwrap(),
        Path::new("user.ini")
    );
    assert_eq!(
        fs::read(i.directory.join("user.ini")).unwrap(),
        b"user config"
    );
}

#[test]
fn migration_keeps_legacy_original_backups_and_cannot_undo_the_new_owner() {
    let f = Fixture::new();
    let game = f.exe.parent().unwrap();
    let launcher = game.join("Talos2.exe");
    fs::copy(&f.exe, &launcher).unwrap();
    let renderer = game.join("Talos2/Binaries/Win64/Talos2-Win64-Shipping.exe");
    fs::create_dir_all(renderer.parent().unwrap()).unwrap();
    fs::copy(&f.exe, &renderer).unwrap();
    let proxy = renderer.with_file_name("dxgi.dll");
    fs::write(&proxy, test_pe_image(0x8664)).unwrap();
    let old = f.root.join("talos2-fsr4-lite");
    fs::create_dir_all(&old).unwrap();
    let backup = old.join("original-dxgi.dll");
    fs::write(&backup, b"original modification").unwrap();
    let framework = f
        .plan
        .runtime
        .root
        .join("lib/external/D3DMetal.framework/D3DMetal");
    fs::create_dir_all(framework.parent().unwrap()).unwrap();
    fs::write(&framework, b"original framework").unwrap();
    fs::create_dir_all(&f.plan.prefix).unwrap();
    let mode = 0o644;
    save(&old.join("profile.json"),&serde_json::json!({"game":game,"renderer":renderer,"wine":f.plan.runtime.root,"prefix":f.plan.prefix,
        "framework_original_sha256":crate::staging::digest(&framework).unwrap(),"support_files":{},"environment":{"FSR4_PROFILE":"lossy","NUCLEON_WINEDEBUG":"-all"}})).unwrap();
    save(&old.join("install.json"),&serde_json::json!({"schema":1,"status":"prepared","game":game,"files":[{
        "relative":proxy.strip_prefix(game).unwrap(),"installed":{"sha256":crate::staging::digest(&proxy).unwrap(),"mode":mode},
        "original":{"sha256":crate::staging::digest(&backup).unwrap(),"mode":mode},"backup":backup}]})).unwrap();
    assert_eq!(
        status(&f.root, &launcher).unwrap()["status"],
        "legacy-prepared"
    );
    transaction::migrate_talos_checked(&f.root, |_, _| Ok(())).unwrap();
    let legacy: serde_json::Value = read(&old.join("install.json")).unwrap();
    assert_eq!(legacy["status"], "migrated");
    // A crash after publishing the new journal but before retiring the old one
    // can be resumed without replacing a different owner's preparation.
    let mut pending = legacy.clone();
    pending["status"] = "prepared".into();
    save(&old.join("install.json"), &pending).unwrap();
    transaction::migrate_talos_checked(&f.root, |_, _| Ok(())).unwrap();
    assert_eq!(status(&f.root, &launcher).unwrap()["status"], "prepared");
    let installation = Installation::existing(&f.root, &launcher).unwrap().unwrap();
    restore(&installation).unwrap();
    assert_eq!(fs::read(proxy).unwrap(), b"original modification");
    assert_eq!(fs::read(backup).unwrap(), b"original modification");
}

#[test]
fn bundle_detection_accepts_matched_legacy_wine_libraries_and_process_guard_ignores_cli_arguments()
{
    let f = Fixture::new();
    let (_runtime_dir, runtime) = runtime::fixture(ExecutionMode::Intel);
    let external = runtime.root.join("lib/external");
    fs::create_dir_all(external.join("D3DMetal.framework")).unwrap();
    fs::write(
        external.join("D3DMetal.framework/D3DMetal"),
        crate::runtime_checks::test_image("framework", 1),
    )
    .unwrap();
    fs::write(
        external.join("libd3dshared.dylib"),
        crate::runtime_checks::test_image("shared", 1),
    )
    .unwrap();
    let windows = runtime.root.join("lib/wine/x86_64-windows");
    fs::create_dir_all(&windows).unwrap();
    for name in ["d3d11.dll", "d3d12.dll", "dxgi.dll", "nvngx-on-metalfx.dll"] {
        fs::write(windows.join(name), test_pe_image(0x8664)).unwrap();
    }
    assert!(bundled_metalfx(&runtime));
    let manifest = assets::provider_path(&f.root, "stock").unwrap();
    assert!(manifest.is_file());
    assert!(!transaction::wine_command("/usr/local/bin/nucleon"));
    assert!(!transaction::wine_command("/usr/local/bin/cargo"));
    assert!(transaction::wine_command(
        "/A Wine Bundle/bin/wine64-preloader"
    ));
    assert!(transaction::wine_command("Talos2-Win64-Shipping.exe"));
    fs::remove_file(windows.join("nvngx-on-metalfx.dll")).unwrap();
    assert!(!bundled_metalfx(&runtime));
}

#[test]
fn native_observation_distinguishes_a_loaded_hook_from_active_processing_and_fallback() {
    let f = Fixture::new();
    let mut binding = f.prepare(Backend::Game).unwrap();
    let state = f.root.join("observations");
    fs::create_dir_all(&state).unwrap();
    binding
        .environment
        .insert("FSR4_STATE_DIR".into(), state.display().to_string());
    fs::write(
        state.join("status.conf"),
        "active=1\nreplaced=0\nencodes=0\n",
    )
    .unwrap();
    assert_eq!(observation(&binding).unwrap()["activation"], "not-observed");
    fs::write(
        state.join("status.conf"),
        "active=1\nreplaced=42\nencodes=42\nstubs=0\nfsr_version=4.1.1\nprofile=lossy\n\
         pid=123\nt=1792691892\nmodel=i8mpp\npreset=drs\nvariant=fsr411_2160_lossy\n\
         rw=1707\nrh=960\nuw=2560\nuh=1440\nuwA=2560\nuhA=1440\nframe_ms=19.421\n",
    )
    .unwrap();
    let report = observation(&binding).unwrap();
    assert_eq!(report["activation"], "native-status-active");
    assert_eq!(report["encodes"], 42);
    assert_eq!(report["replaced"], 42);
    assert_eq!(report["stubs"], 0);
    assert_eq!(report["model"], "i8mpp");
    assert_eq!(report["preset"], "drs");
    assert_eq!(report["variant"], "fsr411_2160_lossy");
    assert_eq!(report["render_size"], serde_json::json!([1707, 960]));
    assert_eq!(report["output_size"], serde_json::json!([2560, 1440]));
    assert_eq!(
        report["aligned_output_size"],
        serde_json::json!([2560, 1440])
    );
    assert_eq!(report["reported_at_unix_seconds"], 1792691892_u64);
    assert_eq!(report["submission_interval_ms"], 19.421);
    fs::write(
        state.join("status.conf"),
        "active=0\nreplaced=42\nencodes=43\nreason=depth unavailable\n",
    )
    .unwrap();
    let report = observation(&binding).unwrap();
    assert_eq!(report["activation"], "metalfx-fallback");
    assert_eq!(report["reason"], "depth unavailable");
    assert_eq!(report["encodes"], 43);
    assert_eq!(report["replaced"], 42);
    assert!(report["output_size"].is_null());
    fs::write(
        state.join("status.conf"),
        "active=1\nreplaced=42\nencodes=42\nstubs=-1\nt=invalid\nrw=1707\nrh=0\nuw=2560\nframe_ms=NaN\n",
    )
    .unwrap();
    let report = observation(&binding).unwrap();
    assert!(report["submission_interval_ms"].is_null());
    assert!(report["reported_at_unix_seconds"].is_null());
    assert!(report["render_size"].is_null());
    assert!(report["output_size"].is_null());
    assert!(report["stubs"].is_null());
    begin_observation(&binding).unwrap();
    assert_eq!(observation(&binding).unwrap()["activation"], "not-observed");
}
