//! Resolve real CLI requests against inert synthetic artifacts. No Wine or game runs.
#![cfg(target_os = "macos")]
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn cli(support: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nucleon"));
    command
        .env_clear()
        .env("NUCLEON_SUPPORT_DIR", support)
        .env("PATH", "/usr/bin:/bin")
        // A Steam/store parent must not change a saved application preview.
        .env("NUCLEON_ENGINE", "invalid-parent")
        .env("NUCLEON_WINE", "/missing/parent-runtime")
        .env("STEAM_COMPAT_DATA_PATH", "/missing/parent-prefix");
    command
}
fn output(command: &mut Command) -> Value {
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&result.stdout).unwrap()
}
fn pe(machine: u16) -> Vec<u8> {
    let mut bytes = vec![0; 512];
    bytes[..2].copy_from_slice(b"MZ");
    bytes[60..64].copy_from_slice(&128_u32.to_le_bytes());
    bytes[128..132].copy_from_slice(b"PE\0\0");
    bytes[132..134].copy_from_slice(&machine.to_le_bytes());
    bytes[148..150]
        .copy_from_slice(&(if machine == 0x14c { 224_u16 } else { 240_u16 }).to_le_bytes());
    bytes[150..152].copy_from_slice(&2_u16.to_le_bytes());
    bytes[152..154].copy_from_slice(
        &(if machine == 0x14c {
            0x10b_u16
        } else {
            0x20b_u16
        })
        .to_le_bytes(),
    );
    bytes
}
fn runtime(root: &Path) {
    let mut native = vec![0; 32];
    native[0..4].copy_from_slice(&0xfeedfacf_u32.to_le_bytes());
    native[4..8].copy_from_slice(&0x01000007_u32.to_le_bytes());
    native[12..16].copy_from_slice(&6_u32.to_le_bytes());
    for name in [
        "bin/wine",
        "bin/wineserver",
        "lib/wine/x86_64-unix/ntdll.so",
    ] {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, &native).unwrap();
    }
    fs::create_dir_all(root.join("lib/wine/x86_64-windows")).unwrap();
    fs::write(root.join("lib/wine/x86_64-windows/ntdll.dll"), pe(0x8664)).unwrap();
    fs::write(root.join("nucleon-runtime.toml"), "schema=1\nmode='intel'\nguests=['x64']\nloader='bin/wine'\nserver='bin/wineserver'\nunix_dir='lib/wine/x86_64-unix'\nwindows_dirs=['lib/wine/x86_64-windows']\nbackends=['auto']\n[prefix]\nfamily='test'\nabi=1\n").unwrap();
}
struct Fixture {
    _temp: tempfile::TempDir,
    support: PathBuf,
    runtime: PathBuf,
    prefix: PathBuf,
    exe: PathBuf,
    profile: Value,
    auth_marker: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let runtime_path = temp.path().join("runtime with spaces");
        runtime(&runtime_path);
        let exe = temp.path().join("Grüße 中文.exe");
        fs::write(&exe, pe(0x8664)).unwrap();
        let prefix = temp.path().join("external-prefix");
        fs::create_dir(&prefix).unwrap();
        fs::write(prefix.join("save"), b"keep").unwrap();
        let support = temp.path().join("support");
        fs::create_dir_all(support.join("applications")).unwrap();
        let auth_marker = temp.path().join("authenticated");
        let auth = temp.path().join("legendary");
        fs::write(
            &auth,
            format!("#!/bin/sh\ntouch '{}'\nexit 1\n", auth_marker.display()),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&auth, fs::Permissions::from_mode(0o700)).unwrap();
        let profile = json!({"schema":1,"application":{"executable":exe,"arguments":["saved argument", "-AUTH_PASSWORD=secret-value"],"cwd":null},
            "settings":{"wine":runtime_path,"engine":"wine","graphics_api":"dx11","hud":false},"prefix":prefix,
            "environment":{"CUSTOM_GAME_SETTING":"a b"},
            "authentication":{"provider":"legendary","binary":auth,"app_id":"fixture","config":null}});
        fs::write(
            support.join("applications/sample.json"),
            serde_json::to_vec(&profile).unwrap(),
        )
        .unwrap();
        Self {
            _temp: temp,
            support,
            runtime: runtime_path,
            prefix,
            exe,
            profile,
            auth_marker,
        }
    }
    fn unchanged(&self) {
        assert!(
            !self.auth_marker.exists(),
            "resolution must not obtain credentials"
        );
        assert!(!self.support.join("logs").exists());
        assert!(!self.support.join("standalone").exists());
        assert!(!self.prefix.join(".nucleon-data").exists());
        assert_eq!(fs::read(self.prefix.join("save")).unwrap(), b"keep");
    }
}

#[test]
fn saved_and_direct_previews_share_selected_choices_and_have_no_launch_effects() {
    let f = Fixture::new();
    let saved =
        output(cli(&f.support).args(["app", "run", "sample", "--dry-run", "--", "a b", ""]));
    let direct = output(
        cli(&f.support)
            .arg("run")
            .arg(&f.exe)
            .arg("--wine")
            .arg(&f.runtime)
            .arg("--prefix")
            .arg(&f.prefix)
            .args([
                "--engine",
                "wine",
                "--graphics-api",
                "dx11",
                "--hud=false",
                "--dry-run",
            ]),
    );
    for key in [
        "runtime",
        "engine",
        "graphics_api",
        "prefix",
        "cwd",
        "is_64_bit",
        "hud",
        "upscaler",
        "graphics",
    ] {
        assert_eq!(saved[key], direct[key], "{key}");
    }
    assert_eq!(
        saved["arguments"],
        json!(["saved argument", "-AUTH_PASSWORD=<redacted>", "a b", ""])
    );
    assert!(
        !serde_json::to_string(&saved)
            .unwrap()
            .contains("secret-value")
    );
    assert_eq!(
        saved["runtime"],
        f.runtime.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(saved["hud"], false);
    f.unchanged();
}

fn error(output: Output) -> String {
    assert!(!output.status.success());
    String::from_utf8(output.stderr).unwrap()
}

#[test]
fn preview_and_execution_reject_the_same_incompatible_plan_before_authentication() {
    let f = Fixture::new();
    let preview = error(
        cli(&f.support)
            .args(["app", "run", "sample", "--engine", "gptk", "--dry-run"])
            .output()
            .unwrap(),
    );
    let execution = error(
        cli(&f.support)
            .args(["app", "run", "sample", "--engine", "gptk"])
            .output()
            .unwrap(),
    );
    assert_eq!(preview, execution);
    assert!(preview.contains("GPTK requires"), "{preview}");
    fs::write(&f.exe, pe(0x14c)).unwrap();
    let preview = error(
        cli(&f.support)
            .args(["app", "run", "sample", "--dry-run"])
            .output()
            .unwrap(),
    );
    let execution = error(
        cli(&f.support)
            .args(["app", "run", "sample"])
            .output()
            .unwrap(),
    );
    assert_eq!(preview, execution);
    assert!(preview.contains("X86"), "{preview}");
    f.unchanged();
}

#[test]
fn dispatch_uses_the_frozen_profile_even_if_saved_profile_and_global_default_change() {
    let f = Fixture::new();
    fs::write(
        f.support.join("applications/sample.json"),
        b"corrupt after selection",
    )
    .unwrap();
    fs::write(
        f.support.join("active_wine.txt"),
        "/missing/replacement-default",
    )
    .unwrap();
    let preview = output(
        cli(&f.support)
            .env(
                "NUCLEON_LAUNCH_REQUEST",
                serde_json::to_string(&f.profile).unwrap(),
            )
            .args(["app", "dispatch", "sample", "--dry-run"]),
    );
    assert_eq!(
        preview["runtime"],
        f.runtime.canonicalize().unwrap().to_str().unwrap()
    );
    f.unchanged();
}

#[test]
fn steam_direct_preview_preserves_the_existing_prefix_and_has_no_client_side_effects() {
    let f = Fixture::new();
    let appid = 7654321;
    let directory = f.exe.parent().unwrap();
    let records = f.support.join("games/steam").join(appid.to_string());
    fs::create_dir_all(&records).unwrap();
    fs::write(
        records.join("installation.json"),
        serde_json::to_vec(&json!({
            "schema":1,"appid":appid,"directory":directory,"data_path":f.prefix,
            "branch":"public","language":"english"
        }))
        .unwrap(),
    )
    .unwrap();
    let data = f.support.join("game_profiles");
    fs::create_dir_all(&data).unwrap();
    fs::write(
        data.join(format!("{appid}.json")),
        serde_json::to_vec(&json!({"use_steam_shim":false})).unwrap(),
    )
    .unwrap();
    let result = output(
        cli(&f.support)
            .args(["launch", &appid.to_string(), "--exe"])
            .arg(&f.exe)
            .arg("--wine")
            .arg(&f.runtime)
            .args(["--engine", "wine", "--dry-run", "--", "a b", ""]),
    );
    assert_eq!(result["appid"], appid);
    assert_eq!(result["prefix"], f.prefix.join("pfx").to_str().unwrap());
    assert_eq!(result["arguments"], json!(["a b", ""]));
    assert_eq!(result["steamworks"], true);
    assert_eq!(result["steam_client_modified"], false);
    assert!(!f.support.join("logs").exists());
    assert!(!f.prefix.join("pfx").exists());
    assert_eq!(fs::read(f.prefix.join("save")).unwrap(), b"keep");
    assert!(
        !f.support
            .join(format!("launch_override_{appid}.json"))
            .exists()
    );
}
