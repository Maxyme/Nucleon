//! Opt-in native execution, using an owned console EXE and private support/prefix data.
#![cfg(target_os = "macos")]
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

fn cli(support: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nucleon"));
    cmd.env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("NUCLEON_SUPPORT_DIR", support)
        .env("SteamAppId", "999")
        .env("NUCLEON_STEAMCLIENT_REDIRECT", "1")
        .env("NUCLEON_WINE", "/missing/foreign-wine")
        .env("NUCLEON_ENGINE", "foreign-engine");
    cmd
}
fn success(cmd: &mut Command) -> Output {
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}
fn receipt(support: &Path) -> serde_json::Value {
    let latest: serde_json::Value =
        serde_json::from_slice(&fs::read(support.join("logs/latest-run.json")).unwrap()).unwrap();
    serde_json::from_slice(
        &fs::read(
            support
                .join("logs")
                .join(latest["directory"].as_str().unwrap())
                .join("receipt.json"),
        )
        .unwrap(),
    )
    .unwrap()
}
fn wine_log(report: &serde_json::Value) -> String {
    fs::read_to_string(report["wine_log"].as_str().unwrap()).unwrap()
}

struct Process(std::process::Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct PrefixCleanup {
    runtime: PathBuf,
    prefix: PathBuf,
}

#[test]
#[ignore = "requires a runtime supporting x86 and i686-w64-mingw32-gcc; executes an owned console EXE in a private prefix"]
fn native_x86_unicode_paths_and_arguments() {
    let runtime = PathBuf::from(std::env::var_os("NUCLEON_TEST_STANDALONE_RUNTIME").unwrap())
        .canonicalize()
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("Grüße 中文.exe");
    success(
        Command::new("i686-w64-mingw32-gcc")
            .args(["-municode", "-O2"])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/standalone.c"))
            .arg("-o")
            .arg(&executable),
    );
    success(
        cli(temp.path())
            .arg("run")
            .arg(&executable)
            .arg("--wine")
            .arg(&runtime)
            .args(["--engine", "wine", "--", "--write", "café 中文", ""]),
    );
    let report = receipt(temp.path());
    let prefix = PathBuf::from(report["prefix"].as_str().unwrap());
    assert_eq!(
        fs::read(prefix.join("drive_c/nucleon-shared.txt")).unwrap(),
        b"installed"
    );
    let bytes = fs::read(prefix.join("drive_c/nucleon-arguments.bin")).unwrap();
    let units: Vec<_> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    assert_eq!(
        String::from_utf16(&units).unwrap(),
        "--write\0café 中文\0\0"
    );
    assert_eq!(
        report["executable"].as_str().unwrap(),
        executable.canonicalize().unwrap().to_str().unwrap()
    );
}
impl Drop for PrefixCleanup {
    fn drop(&mut self) {
        let _ = Command::new(self.runtime.join("bin/wineserver"))
            .env("WINEPREFIX", &self.prefix)
            .arg("-k")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[test]
#[ignore = "requires NUCLEON_TEST_STANDALONE_RUNTIME and x86_64-w64-mingw32-gcc; executes owned EXEs in private prefixes"]
fn native_exe_arguments_bottles_handoff_lock_and_cancellation() {
    let runtime = PathBuf::from(std::env::var_os("NUCLEON_TEST_STANDALONE_RUNTIME").unwrap())
        .canonicalize()
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let support = temp.path().join("support");
    let programs = temp.path().join("programs with spaces");
    fs::create_dir(&programs).unwrap();
    fs::create_dir(&support).unwrap();
    let executable = programs.join("probe.exe");
    success(
        Command::new("x86_64-w64-mingw32-gcc")
            .args(["-municode", "-O2"])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/standalone.c"))
            .arg("-o")
            .arg(&executable),
    );
    let output = cli(&support)
        .current_dir(temp.path())
        .args(["run", "programs with spaces/probe.exe", "--wine"])
        .arg(&runtime)
        .args([
            "--engine",
            "wine",
            "--",
            "--exit17",
            "argument with spaces",
            "--literal",
            "",
        ])
        .output()
        .unwrap();
    let first = receipt(&support);
    let setup = fs::read_to_string(
        Path::new(first["runner_log"].as_str().unwrap()).with_file_name("setup.log"),
    )
    .unwrap_or_default();
    assert_eq!(
        output.status.code(),
        Some(17),
        "{}\nSetup: {setup}",
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(first["origin"], "standalone");
    assert!(first["appid"].is_null());
    let log = wine_log(&first);
    for text in [
        "STEAM_ID=unset",
        "STEAM_DATA=unset",
        "STEAM_REDIRECT=unset",
        "ARG[2]=<argument with spaces>",
        "ARG[3]=<--literal>",
        "ARG[4]=<>",
        "programs with spaces",
    ] {
        assert!(log.contains(text), "{log}");
    }
    let other = programs.join("other.exe");
    fs::copy(&executable, &other).unwrap();
    success(
        cli(&support)
            .arg("run")
            .arg(&other)
            .arg("--wine")
            .arg(&runtime)
            .args(["--engine", "wine"]),
    );
    assert_ne!(first["prefix"], receipt(&support)["prefix"]);
    success(
        cli(&support)
            .args(["bottle", "create", "sample", "--wine"])
            .arg(&runtime)
            .args(["--engine", "wine"]),
    );
    success(
        cli(&support)
            .args(["bottle", "add", "sample", "installer"])
            .arg(&executable)
            .args(["--", "--write"]),
    );
    success(
        cli(&support)
            .args(["bottle", "add", "sample", "game"])
            .arg(&other)
            .args(["--", "--read"]),
    );
    success(cli(&support).args(["bottle", "run", "sample", "installer"]));
    let installed = receipt(&support);
    success(cli(&support).args(["bottle", "run", "sample", "game"]));
    let game = receipt(&support);
    assert_eq!(installed["prefix"], game["prefix"]);
    assert!(wine_log(&game).contains("SHARED=installed"));
    success(
        cli(&support)
            .arg("run")
            .arg(&executable)
            .args(["--bottle", "sample", "--", "--spawn"]),
    );
    assert!(wine_log(&receipt(&support)).contains("CHILD_COMPLETED"));
    success(cli(&support).args([
        "bottle",
        "configure",
        "sample",
        "--hud=false",
        "--clear",
        "graphics-api",
    ]));
    let before = fs::read(support.join("bottles/sample/bottle.json")).unwrap();
    success(
        cli(&support)
            .arg("run")
            .arg(&executable)
            .args(["--bottle", "sample", "--engine", "wine"]),
    );
    assert_eq!(
        fs::read(support.join("bottles/sample/bottle.json")).unwrap(),
        before
    );

    let _cleanup = PrefixCleanup {
        runtime: runtime.clone(),
        prefix: PathBuf::from(installed["prefix"].as_str().unwrap()),
    };
    let mut unrelated = Process(Command::new("/bin/sleep").arg("30").spawn().unwrap());
    let mut running = Process(
        cli(&support)
            .arg("run")
            .arg(&executable)
            .args(["--bottle", "sample", "--", "--hold"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        assert!(
            Instant::now() < deadline,
            "console probe did not become ready"
        );
        assert!(
            running.0.try_wait().unwrap().is_none(),
            "console probe exited early"
        );
        let report = receipt(&support);
        if report["status"] == "running" && wine_log(&report).contains("HOLD_READY") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let blocked = cli(&support)
        .arg("run")
        .arg(&executable)
        .args(["--bottle", "sample"])
        .output()
        .unwrap();
    assert!(!blocked.status.success());
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("Another Nucleon launch"));
    assert!(running.0.try_wait().unwrap().is_none());
    success(Command::new("/bin/kill").args(["-TERM", &running.0.id().to_string()]));
    let deadline = Instant::now() + Duration::from_secs(10);
    while running.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(unrelated.0.try_wait().unwrap().is_none());
    unrelated.0.kill().unwrap();
    unrelated.0.wait().unwrap();
    assert!(
        !Path::new(installed["prefix"].as_str().unwrap())
            .join("drive_c/Program Files (x86)/Steam")
            .exists()
    );
}
