use anyhow::{Context, Result, ensure};
use nucleon_core::{
    fs_util::atomic_write_file,
    process::{OwnedChild, TERMINATION_GRACE},
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub static CANCELLED: AtomicBool = AtomicBool::new(false);
pub type Environment = BTreeMap<OsString, OsString>;
pub use nucleon_core::artifacts::digest as sha;
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
pub fn require_native() -> Result<()> {
    ensure!(
        cfg!(all(target_os = "macos", target_arch = "aarch64")),
        "Requires native Apple Silicon macOS"
    );
    Ok(())
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

#[allow(clippy::too_many_arguments)]
pub fn marker_run(
    program: &Path,
    args: &[String],
    env: &Environment,
    cwd: Option<&Path>,
    log: &Path,
    seconds: u64,
    expected: i32,
    marker: &str,
) -> Result<()> {
    fs::create_dir_all(log.parent().unwrap())?;
    fs::write(log, [])?;
    run(program, args, Some(env), cwd, Some(log), seconds, expected)?;
    ensure!(
        fs::read_to_string(log)?.contains(marker),
        "Missing execution marker {marker}; see {}",
        log.display()
    );
    Ok(())
}
pub fn finish_probe(
    report: &mut Value,
    path: &Path,
    result: Result<()>,
    cleanup: Result<()>,
    success: &str,
) -> Result<()> {
    if let Err(error) = &cleanup {
        report["cleanup_error"] = format!("{error:#}").into();
    }
    let result = result.and(cleanup);
    match &result {
        Ok(()) => report["status"] = success.into(),
        Err(error) => {
            report["status"] = "FAILED".into();
            report["error"] = format!("{error:#}").into();
        }
    }
    result.and(write_json(path, report))
}

#[cfg(test)]
mod probe_tests {
    use super::*;
    #[test]
    fn cleanup_failure_does_not_replace_primary_probe_failure() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("receipt.json");
        let mut report = serde_json::json!({});
        let result = finish_probe(
            &mut report,
            &file,
            Err(anyhow::anyhow!("primary failure")),
            Err(anyhow::anyhow!("cleanup failure")),
            "PASSED",
        );
        assert_eq!(result.unwrap_err().to_string(), "primary failure");
        let report = json(&file).unwrap();
        assert_eq!(report["status"], "FAILED");
        assert_eq!(report["cleanup_error"], "cleanup failure");
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
}
