//! Exercise the downloader boundary without credentials, network or a real game.
#![cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{fs, path::Path, process::Command};
fn provider(root: &Path, body: &str) -> std::path::PathBuf {
    let path = root.join("fake downloader");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}
fn command(support: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nucleon"));
    cmd.env("NUCLEON_SUPPORT_DIR", support);
    cmd
}
#[test]
fn preview_never_starts_authentication_or_creates_an_installation() {
    let temp = tempfile::tempdir().unwrap();
    let support = temp.path().join("support");
    let output = command(&support)
        .args([
            "steam",
            "install",
            "123",
            "--username",
            "fixture",
            "--downloader",
            "/missing/downloader",
            "--dry-run",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["steam_client_modified"], false);
    assert!(!support.exists());
}
#[test]
fn success_is_recorded_only_after_real_files_and_failed_updates_preserve_the_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let support = temp.path().join("support with spaces");
    let program = provider(
        temp.path(),
        r#"
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-dir" ]; then shift; game_dir=$1; fi
  shift
done
mkdir -p "$game_dir"
printf MZ > "$game_dir/Game.exe"
"#,
    );
    let output = command(&support)
        .args([
            "steam",
            "install",
            "123",
            "--username",
            "fixture",
            "--downloader",
        ])
        .arg(&program)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let record = support.join("games/steam/123/installation.json");
    let previous = fs::read(&record).unwrap();
    provider(temp.path(), "exit 2");
    let output = command(&support)
        .args([
            "steam",
            "update",
            "123",
            "--username",
            "fixture",
            "--branch",
            "changed",
            "--downloader",
        ])
        .arg(&program)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read(&record).unwrap(), previous);
    assert_eq!(
        fs::read(support.join("games/steam/123/files/Game.exe")).unwrap(),
        b"MZ"
    );
}
#[test]
fn empty_success_does_not_claim_an_installed_game() {
    let temp = tempfile::tempdir().unwrap();
    let support = temp.path().join("support");
    let program = provider(temp.path(), "exit 0");
    let output = command(&support)
        .args([
            "steam",
            "install",
            "123",
            "--username",
            "fixture",
            "--downloader",
        ])
        .arg(&program)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!support.join("games/steam/123/installation.json").exists());
}
