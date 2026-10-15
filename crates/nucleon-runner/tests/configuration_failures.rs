use std::{fs, process::Command};

#[test]
fn invalid_environment_overrides_fail_before_prefix_setup_and_are_logged() {
    for (name, value, correction) in [
        ("NUCLEON_ENGINE", "dxmtt", "Choose auto"),
        ("NUCLEON_BACKEND", "typo", "Valid options"),
        ("NUCLEON_STEAM_SHIM", "yes", "Use 1/0"),
        (
            "DXVK_PATH",
            "/does-not-exist/dxvk",
            "remove it from Steam Launch Options",
        ),
        (
            "VK_DRIVER_FILES",
            "/does-not-exist/icd.json",
            "Select a readable ICD",
        ),
        ("NUCLEON_WINE", "", "nucleon wine list"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_nucleon-runner"))
            .env_clear()
            .env("NUCLEON_SUPPORT_DIR", dir.path())
            .env(name, value)
            .args(["run", "/does-not-exist/game.exe"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{name}");
        let log = fs::read_to_string(dir.path().join("nucleon-runner.log")).unwrap();
        assert!(log.contains(name) && log.contains(correction), "{log}");
        assert!(!dir.path().join("default_prefix/pfx").exists());
    }
}

#[test]
fn corrupt_override_and_backend_files_report_the_path_without_deleting_them() {
    for name in ["launch_override_123.json", "backend.toml"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        fs::write(&path, "not valid config").unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_nucleon-runner"))
            .env_clear()
            .env("NUCLEON_SUPPORT_DIR", dir.path())
            .env("STEAM_COMPAT_APP_ID", "123")
            .args(["run", "/does-not-exist/game.exe"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        let log = fs::read_to_string(dir.path().join("nucleon-runner.log")).unwrap();
        assert!(log.contains(&path.display().to_string()), "{log}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "not valid config");
    }
}

#[test]
fn valid_per_game_paths_override_stale_global_paths() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = dir.path().join("dxvk");
    fs::create_dir(&bundle).unwrap();
    fs::write(bundle.join("d3d11.dll"), "fixture").unwrap();
    let manifest = dir.path().join("icd.json");
    fs::write(dir.path().join("driver.dylib"), "fixture").unwrap();
    fs::write(&manifest, r#"{"ICD":{"library_path":"driver.dylib"}}"#).unwrap();
    fs::create_dir(dir.path().join("game_profiles")).unwrap();
    fs::write(
        dir.path().join("game_profiles/123.json"),
        serde_json::json!({"engine":"wine", "dxvk_path":bundle, "vulkan_icd":manifest}).to_string(),
    )
    .unwrap();
    fs::write(dir.path().join("dxvk_path.txt"), "/missing/saved-dxvk").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_nucleon-runner"))
        .env_clear()
        .env("NUCLEON_SUPPORT_DIR", dir.path())
        .env("STEAM_COMPAT_APP_ID", "123")
        .env("KOSMICKRISP_ICD_PATH", "/missing/shadowed-driver.json")
        .env("NUCLEON_STEAM_SHIM", "typo")
        .args(["run", "/does-not-exist/game.exe"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let log = fs::read_to_string(dir.path().join("nucleon-runner.log")).unwrap();
    assert!(log.contains("NUCLEON_STEAM_SHIM"), "{log}");
    assert!(
        !log.contains("shadowed-driver") && !log.contains("saved-dxvk"),
        "{log}"
    );
}

#[test]
fn explicit_diagnostic_profile_is_loaded_and_does_not_consume_global_override() {
    let dir = tempfile::tempdir().unwrap();
    let profile = dir.path().join("diagnostic.json");
    let global = dir.path().join("launch_override_123.json");
    fs::write(&global, "malformed and must remain untouched").unwrap();
    for contents in [None, Some(r#"{"engine":"dxmtt"}"#)] {
        if let Some(text) = contents {
            fs::write(&profile, text).unwrap();
        }
        let output = Command::new(env!("CARGO_BIN_EXE_nucleon-runner"))
            .env_clear()
            .env("NUCLEON_SUPPORT_DIR", dir.path())
            .env("STEAM_COMPAT_APP_ID", "123")
            .env("NUCLEON_GAME_PROFILE", &profile)
            .args(["run", "/does-not-exist/game.exe"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        let log = fs::read_to_string(dir.path().join("nucleon-runner.log")).unwrap();
        assert!(log.contains(&profile.display().to_string()), "{log}");
        assert!(!log.contains("malformed and must"), "{log}");
        assert_eq!(
            fs::read_to_string(&global).unwrap(),
            "malformed and must remain untouched"
        );
        assert!(!dir.path().join("default_prefix/pfx").exists());
    }
}
