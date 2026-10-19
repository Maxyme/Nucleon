use anyhow::{Context, Result, ensure};
use nucleon_core::{
    artifacts,
    fs_util::atomic_write_file,
    process::{OwnedChild, TERMINATION_GRACE},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub static CANCELLED: AtomicBool = AtomicBool::new(false);
pub type Environment = BTreeMap<OsString, OsString>;
pub use nucleon_core::artifacts::{copy_file as copy, copy_tree, digest as sha};
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .into()
}
pub fn json(path: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(
        &fs::read(path).with_context(|| format!("Read {}", path.display()))?,
    )?)
}
pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    fs::create_dir_all(path.parent().context("JSON destination has no parent")?)?;
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    atomic_write_file(path, bytes)
}
/// Rust orchestration identities intentionally use schema 2 and canonical compact JSON.
pub fn key(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
pub fn p(path: impl AsRef<Path>) -> String {
    path.as_ref().to_string_lossy().into_owned()
}
pub fn clean_environment() -> Environment {
    clean_environment_from(std::env::vars_os())
}
fn clean_environment_from(vars: impl Iterator<Item = (OsString, OsString)>) -> Environment {
    vars.filter(|(key, _)| {
        let key = key.to_string_lossy();
        ![
            "WINE",
            "DYLD_",
            "FEX_",
            "MACRUNNER_",
            "NUCLEON_",
            "VKD3D_",
            "DXVK_",
            "DXMT_",
        ]
        .iter()
        .any(|prefix| key.starts_with(prefix))
            && ![
                "VK_DRIVER_FILES",
                "VK_ICD_FILENAMES",
                "MESA_VK_VERSION_OVERRIDE",
                "MVK_CONFIG_ADVERTISE_API_VERSION",
                "VK_LOADER_DEBUG",
            ]
            .contains(&key.as_ref())
    })
    .collect()
}
pub fn build_environment(compiler: &Path) -> Environment {
    let mut env = clean_environment();
    for key in [
        "CC",
        "CXX",
        "CFLAGS",
        "CXXFLAGS",
        "CPPFLAGS",
        "LDFLAGS",
        "CPATH",
        "LIBRARY_PATH",
        "SDKROOT",
        "PKG_CONFIG_PATH",
    ] {
        env.remove(OsStr::new(key));
    }
    set(
        &mut env,
        "PATH",
        format!(
            "{}/bin:/opt/homebrew/opt/bison/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            compiler.display()
        ),
    );
    for (key, value) in [
        ("CC", "/usr/bin/clang"),
        ("CXX", "/usr/bin/clang++"),
        ("LC_ALL", "C"),
        ("TZ", "UTC"),
    ] {
        set(&mut env, key, value);
    }
    env
}
pub fn set(env: &mut Environment, key: &str, value: impl AsRef<OsStr>) {
    env.insert(key.into(), value.as_ref().into());
}
pub fn command(
    program: impl AsRef<OsStr>,
    args: &[String],
    env: Option<&Environment>,
    cwd: Option<&Path>,
) -> Command {
    let mut cmd = Command::new(program);
    cmd.args(args);
    if let Some(env) = env {
        cmd.env_clear().envs(env);
    }
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    cmd
}
pub fn execute(
    cmd: &mut Command,
    log: Option<&Path>,
    timeout: Duration,
    expected: i32,
    grace: Duration,
) -> Result<()> {
    execute_inner(cmd, log, timeout, expected, grace, true)
}
fn execute_inner(
    cmd: &mut Command,
    log: Option<&Path>,
    timeout: Duration,
    expected: i32,
    grace: Duration,
    observe_cancellation: bool,
) -> Result<()> {
    if let Some(path) = log {
        fs::create_dir_all(path.parent().context("Log has no parent")?)?;
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        cmd.stdout(file.try_clone()?).stderr(file);
    }
    let description = format!("{:?}", cmd.get_program());
    let mut child = OwnedChild::spawn(cmd).with_context(|| format!("Start {description}"))?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            child.terminate(Duration::ZERO)?;
            ensure!(
                status.code() == Some(expected),
                "{description} exited {status}; expected {expected}; log: {}",
                log.map(p).unwrap_or_default()
            );
            return Ok(());
        }
        if (observe_cancellation && CANCELLED.load(Ordering::SeqCst)) || start.elapsed() >= timeout
        {
            child.terminate(grace)?;
            anyhow::bail!(
                "{description} interrupted or timed out; log: {}",
                log.map(p).unwrap_or_default()
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
/// Cleanup must still run after the interruption flag has been set.
pub fn cleanup_server(server: &Path, env: &Environment, log: &Path) -> Result<()> {
    let stop = execute_inner(
        &mut command(server, &["-k".into()], Some(env), None),
        Some(log),
        Duration::from_secs(10),
        0,
        TERMINATION_GRACE,
        false,
    );
    if let Err(error) = stop {
        log::debug!("Server stop: {error:#}");
    }
    execute_inner(
        &mut command(server, &["-w".into()], Some(env), None),
        Some(log),
        Duration::from_secs(10),
        0,
        TERMINATION_GRACE,
        false,
    )
}
pub fn run(
    program: impl AsRef<OsStr>,
    args: &[String],
    env: Option<&Environment>,
    cwd: Option<&Path>,
    log: Option<&Path>,
    seconds: u64,
    expected: i32,
) -> Result<()> {
    execute(
        &mut command(program, args, env, cwd),
        log,
        Duration::from_secs(seconds),
        expected,
        TERMINATION_GRACE,
    )
}
pub fn output(
    program: impl AsRef<OsStr>,
    args: &[String],
    env: Option<&Environment>,
    cwd: Option<&Path>,
) -> Result<String> {
    let file = tempfile::NamedTempFile::new()?;
    let mut cmd = command(program, args, env, cwd);
    cmd.stdout(file.reopen()?).stderr(Stdio::null());
    execute(
        &mut cmd,
        None,
        Duration::from_secs(60),
        0,
        TERMINATION_GRACE,
    )?;
    let mut out = String::new();
    fs::File::open(file.path())?
        .take(16 * 1024 * 1024)
        .read_to_string(&mut out)?;
    Ok(out.trim().to_string())
}
pub fn git(root: &Path, args: &[&str]) -> Result<String> {
    output(
        "git",
        &args.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
        None,
        Some(root),
    )
}
pub fn hash_files(root: &Path, names: &[String]) -> Result<BTreeMap<String, String>> {
    names
        .iter()
        .map(|name| Ok((name.clone(), sha(&root.join(name))?)))
        .collect()
}
pub fn require_native() -> Result<()> {
    ensure!(
        cfg!(all(target_os = "macos", target_arch = "aarch64")),
        "Requires native Apple Silicon macOS"
    );
    Ok(())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Pin {
    pub url: String,
    pub revision: String,
    #[serde(default)]
    pub unix_patches: Vec<String>,
}
pub fn source_diff(path: &Path) -> Result<String> {
    let file = tempfile::NamedTempFile::new()?;
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(path)
        .args(["diff", "--binary", "HEAD"])
        .stdout(file.reopen()?);
    execute(
        &mut cmd,
        None,
        Duration::from_secs(60),
        0,
        TERMINATION_GRACE,
    )?;
    sha(file.path())
}
pub fn checkout(path: &Path, pin: &Pin) -> Result<()> {
    ensure!(
        pin.revision.len() == 40 && pin.revision.bytes().all(|b| b.is_ascii_hexdigit()),
        "Source must pin a full Git revision"
    );
    if !path.join(".git").exists() {
        fs::create_dir_all(path)?;
        git(path, &["init", "-q"])?;
        git(path, &["remote", "add", "origin", &pin.url])?;
    }
    ensure!(
        git(path, &["remote", "get-url", "origin"])? == pin.url,
        "Unexpected Git remote"
    );
    if !git(path, &["status", "--porcelain"])?.is_empty() {
        let marker = json(&path.join(".nucleon-overlay.json"))?;
        let untracked = git(path, &["ls-files", "--others", "--exclude-standard"])?;
        ensure!(
            untracked.lines().all(|s| s == ".nucleon-overlay.json")
                && git(path, &["rev-parse", "HEAD"])? == pin.revision
                && marker["diff_sha256"] == source_diff(path)?,
            "Preserve modified source checkout {}",
            path.display()
        );
        return Ok(());
    }
    if git(path, &["cat-file", "-e", &pin.revision]).is_err() {
        git(path, &["fetch", "--depth", "1", "origin", &pin.revision])?;
    }
    git(path, &["checkout", "--detach", &pin.revision])?;
    Ok(())
}
pub fn patches(source: &Path, names: &[String]) -> Result<()> {
    for name in names {
        let args = vec![
            "--batch".into(),
            "-p1".into(),
            "--input".into(),
            p(repo().join(name)),
        ];
        let mut check = args.clone();
        check.extend(["--dry-run".into(), "--forward".into()]);
        let applicable = run("patch", &check, None, Some(source), None, 60, 0).is_ok();
        let mut apply = args;
        apply.extend(if applicable {
            vec!["--forward".into()]
        } else {
            vec!["--dry-run".into(), "--reverse".into()]
        });
        run("patch", &apply, None, Some(source), None, 60, 0)?;
    }
    if !names.is_empty() {
        write_json(
            &source.join(".nucleon-overlay.json"),
            &json!({"diff_sha256":source_diff(source)?,"patches":hash_files(&repo(),names)?}),
        )?;
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ArchivePin {
    pub url: String,
    pub archive: String,
    pub sha256: String,
    pub directory: Option<String>,
}
pub fn fetch_archive(root: &Path, pin: &ArchivePin) -> Result<PathBuf> {
    fs::create_dir_all(root)?;
    let path = root.join(&pin.archive);
    if !path.exists() {
        let mut input = artifacts::download(&pin.url, 2 * 1024 * 1024 * 1024)?;
        let mut stage = tempfile::NamedTempFile::new_in(root)?;
        std::io::copy(&mut input, &mut stage)?;
        ensure!(
            sha(stage.path())? == pin.sha256,
            "Archive checksum mismatch: {}",
            pin.archive
        );
        stage.persist_noclobber(&path).map_err(|e| e.error)?;
    }
    ensure!(
        sha(&path)? == pin.sha256,
        "Archive checksum mismatch: {}",
        pin.archive
    );
    Ok(path)
}

pub struct FileLock(fs::File);
impl FileLock {
    pub fn configuration(path: &Path) -> Result<Self> {
        let absolute = std::path::absolute(path)?;
        let canonical = if absolute.exists() {
            absolute.canonicalize()?
        } else {
            absolute
                .parent()
                .context("Configuration has no parent")?
                .canonicalize()?
                .join(absolute.file_name().context("Configuration has no name")?)
        };
        let hash = format!("{:x}", Sha256::digest(p(canonical).as_bytes()));
        Self::acquire(&std::env::temp_dir().join(format!(
            "nucleon-config-{}-{hash}.lock",
            rustix::process::getuid().as_raw()
        )))
    }
    pub fn acquire(path: &Path) -> Result<Self> {
        fs::create_dir_all(path.parent().context("Lock has no parent")?)?;
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)
            .with_context(|| format!("Another operation is using {}", path.display()))?;
        Ok(Self(file))
    }
}
impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = rustix::fs::flock(&self.0, rustix::fs::FlockOperation::Unlock);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runtime_and_graphics_overrides_do_not_leak_into_developer_children() {
        let env = clean_environment_from(
            [
                ("PATH", "/bin"),
                ("WINEPREFIX", "unrelated"),
                ("NUCLEON_ENGINE", "gptk"),
                ("VK_DRIVER_FILES", "stale"),
                ("DYLD_LIBRARY_PATH", "other"),
                ("FEX_TSOENABLED", "0"),
            ]
            .into_iter()
            .map(|(k, v)| (k.into(), v.into())),
        );
        assert_eq!(env.len(), 1);
        assert_eq!(env[OsStr::new("PATH")], "/bin");
    }
    #[test]
    fn command_status_timeout_and_raw_capture_are_preserved() {
        assert_eq!(
            output(
                "sh",
                &["-c".into(), "printf 'value with spaces'".into()],
                None,
                None
            )
            .unwrap(),
            "value with spaces"
        );
        run(
            "sh",
            &["-c".into(), "exit 7".into()],
            None,
            None,
            None,
            2,
            7,
        )
        .unwrap();
        assert!(
            run(
                "sh",
                &["-c".into(), "exit 7".into()],
                None,
                None,
                None,
                2,
                0
            )
            .is_err()
        );
        let start = Instant::now();
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 30 & wait"]);
        assert!(execute(&mut cmd, None, Duration::from_millis(50), 0, Duration::ZERO).is_err());
        assert!(start.elapsed() < Duration::from_secs(5));
    }
    #[test]
    fn modified_checkouts_are_preserved_and_recorded_overlays_are_reusable() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]).unwrap();
        git(
            root,
            &[
                "remote",
                "add",
                "origin",
                "https://example.invalid/source.git",
            ],
        )
        .unwrap();
        fs::write(root.join("file.c"), "original").unwrap();
        git(root, &["add", "file.c"]).unwrap();
        git(
            root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-qm",
                "fixture",
            ],
        )
        .unwrap();
        let pin = Pin {
            url: "https://example.invalid/source.git".into(),
            revision: git(root, &["rev-parse", "HEAD"]).unwrap(),
            unix_patches: vec![],
        };
        fs::write(root.join("file.c"), "patched").unwrap();
        assert!(checkout(root, &pin).is_err());
        write_json(
            &root.join(".nucleon-overlay.json"),
            &json!({"diff_sha256":source_diff(root).unwrap()}),
        )
        .unwrap();
        checkout(root, &pin).unwrap();
        fs::write(root.join("private"), "untracked").unwrap();
        assert!(checkout(root, &pin).is_err());
        assert_eq!(fs::read_to_string(root.join("file.c")).unwrap(), "patched");
    }
}
