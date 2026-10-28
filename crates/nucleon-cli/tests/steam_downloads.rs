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

#[test]
fn updating_an_adopted_game_never_writes_into_the_original_steam_directory() {
    let temp = tempfile::tempdir().unwrap();
    let support = temp.path().join("support");
    let original = temp.path().join("Steam/steamapps/common/Original Game");
    fs::create_dir_all(&original).unwrap();
    fs::write(original.join("Game.exe"), b"original binary").unwrap();
    fs::write(original.join("original.dat"), b"original game data").unwrap();
    let record = support.join("games/steam/123/installation.json");
    fs::create_dir_all(record.parent().unwrap()).unwrap();
    fs::write(
        &record,
        serde_json::to_vec(&serde_json::json!({
            "schema":1,"appid":123,"directory":original,"original_directory":original,
            "data_path":temp.path().join("existing-prefix"),"branch":"public","language":"english"
        }))
        .unwrap(),
    )
    .unwrap();
    let program = provider(
        temp.path(),
        r#"
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-dir" ]; then shift; game_dir=$1; fi
  shift
done
rm -f "$game_dir"/original.dat
printf MZupdated > "$game_dir/Game.exe"
"#,
    );
    let result = command(&support)
        .args([
            "steam",
            "update",
            "123",
            "--username",
            "fixture",
            "--downloader",
        ])
        .arg(&program)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        fs::read(original.join("Game.exe")).unwrap(),
        b"original binary"
    );
    assert_eq!(
        fs::read(original.join("original.dat")).unwrap(),
        b"original game data"
    );
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(record).unwrap()).unwrap();
    assert_eq!(saved["original_directory"], original.to_str().unwrap());
    assert_eq!(
        saved["directory"],
        support.join("games/steam/123/files").to_str().unwrap()
    );
    assert_eq!(
        saved["data_path"],
        temp.path().join("existing-prefix").to_str().unwrap()
    );
}

#[test]
fn managed_directory_symlink_cannot_redirect_an_update_into_original_files() {
    let temp = tempfile::tempdir().unwrap();
    let support = temp.path().join("support");
    let original = temp.path().join("original");
    fs::create_dir(&original).unwrap();
    fs::write(original.join("Game.exe"), b"keep").unwrap();
    let record = support.join("games/steam/123/installation.json");
    fs::create_dir_all(record.parent().unwrap()).unwrap();
    fs::write(
        &record,
        serde_json::to_vec(&serde_json::json!({
            "schema":1,"appid":123,"directory":original,"original_directory":original,
            "data_path":temp.path().join("data"),"branch":"public","language":"english"
        }))
        .unwrap(),
    )
    .unwrap();
    std::os::unix::fs::symlink(&original, record.parent().unwrap().join("files")).unwrap();
    let program = provider(temp.path(), "exit 99");
    let result = command(&support)
        .args([
            "steam",
            "update",
            "123",
            "--username",
            "fixture",
            "--downloader",
        ])
        .arg(&program)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("overlaps retained Steam game files"));
    assert_eq!(fs::read(original.join("Game.exe")).unwrap(), b"keep");
}
