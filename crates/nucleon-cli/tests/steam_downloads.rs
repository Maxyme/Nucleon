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
fn custom_location_previews_are_absolute_read_only_and_mutually_exclusive() {
    for (flag, relative) in [
        ("--library", "game library/steam/123"),
        ("--install-dir", "game library"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let support = temp.path().join("support");
        let output = command(&support)
            .current_dir(temp.path())
            .args([
                "steam",
                "install",
                "123",
                "--username",
                "fixture",
                flag,
                "game library",
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
        assert_eq!(
            value["directory"],
            temp.path()
                .canonicalize()
                .unwrap()
                .join(relative)
                .to_string_lossy()
                .as_ref()
        );
        assert!(!support.exists());
        assert!(!temp.path().join("game library").exists());
    }
    for flag in ["--library", "--install-dir"] {
        let temp = tempfile::tempdir().unwrap();
        let output = command(temp.path())
            .args([
                "steam",
                "install",
                "123",
                "--username",
                "fixture",
                flag,
                "",
                "--dry-run",
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(temp.path().read_dir().unwrap().next().is_none());
    }
    let temp = tempfile::tempdir().unwrap();
    let output = command(temp.path())
        .args([
            "steam",
            "install",
            "123",
            "--username",
            "fixture",
            "--library",
            "/library",
            "--install-dir",
            "/game",
            "--dry-run",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn custom_locations_keep_state_in_support_and_updates_reuse_game_directory() {
    for (flag, relative) in [
        ("--library", "library/steam/123"),
        ("--install-dir", "library"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let support = temp.path().join("support");
        let destination = temp.path().canonicalize().unwrap().join(relative);
        let program = provider(
            temp.path(),
            r#"
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-dir" ]; then shift; game_dir=$1; fi
  shift
done
mkdir -p "$game_dir"
printf MZ > "$game_dir/Game.exe"
printf call >> "$game_dir/calls"
"#,
        );
        let output = command(&support)
            .current_dir(temp.path())
            .args([
                "steam",
                "install",
                "123",
                "--username",
                "fixture",
                flag,
                "library",
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
        let receipt: serde_json::Value =
            serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
        assert_eq!(receipt["directory"], destination.to_string_lossy().as_ref());
        assert_eq!(
            receipt["data_path"],
            support
                .join("games/steam/123/data")
                .to_string_lossy()
                .as_ref()
        );
        assert!(!support.join("games/steam/123/files").exists());
        let output = command(&support)
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
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(destination.join("calls")).unwrap(), b"callcall");
        let previous = fs::read(&record).unwrap();
        for action in ["install", "update"] {
            let other = temp.path().join("other game");
            let output = command(&support)
                .args([
                    "steam",
                    action,
                    "123",
                    "--username",
                    "fixture",
                    "--install-dir",
                ])
                .arg(&other)
                .arg("--downloader")
                .arg(&program)
                .output()
                .unwrap();
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("cannot relocate"));
            assert!(!other.exists());
            assert_eq!(fs::read(&record).unwrap(), previous);
            assert_eq!(fs::read(destination.join("calls")).unwrap(), b"callcall");
        }
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&destination, &alias).unwrap();
        let output = command(&support)
            .args([
                "steam",
                "update",
                "123",
                "--username",
                "fixture",
                "--install-dir",
            ])
            .arg(alias)
            .arg("--dry-run")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
