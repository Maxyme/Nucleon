//! Exclusive ownership of a game's prefix until its processes have been cleaned up.
use crate::{
    fs_util::atomic_write_file,
    runtime::{ExecutionMode, PrefixCompatibility, ResolvedRuntime},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

pub struct PrefixSession {
    _lock: fs::File,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    schema: u32,
    mode: ExecutionMode,
    prefix: PrefixCompatibility,
}

impl PrefixSession {
    /// Reuse a caller-selected Intel prefix without moving it into managed storage.
    pub fn acquire_existing(
        prefix: &Path,
        runtime: &ResolvedRuntime,
        cancelled: impl Fn() -> bool,
    ) -> Result<Self> {
        ensure!(
            runtime.mode == ExecutionMode::Intel,
            "Existing external prefixes require Intel/Rosetta Wine; use a separate managed prefix for FEX"
        );
        ensure!(!cancelled(), "Prefix launch cancelled");
        ensure!(
            prefix.join("drive_c").is_dir()
                && prefix.join("system.reg").is_file()
                && prefix.join("user.reg").is_file(),
            "Not an initialized Wine prefix: {}",
            prefix.display()
        );
        let marker = prefix.join(".nucleon-runtime-abi");
        if let Some(value) = crate::configuration::optional_file(&marker)? {
            let mode: ExecutionMode =
                serde_json::from_str(&value).context("Invalid existing prefix ABI marker")?;
            ensure!(
                mode == ExecutionMode::Intel,
                "Existing prefix belongs to FEX; select its original runtime or a separate prefix"
            );
        }
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(prefix.join(".nucleon-launch.lock"))?;
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)
            .with_context(|| {
                format!(
                    "Another Nucleon launch is using prefix {}",
                    prefix.display()
                )
            })?;
        Ok(Self { _lock: lock })
    }

    pub fn acquire(
        compat_data: &Path,
        runtime: &ResolvedRuntime,
        timeout: Duration,
        cancelled: impl Fn() -> bool,
    ) -> Result<Self> {
        runtime.prefix.validate()?;
        let prefix = crate::paths::game_prefix(compat_data, runtime);
        if runtime.mode == ExecutionMode::Arm64Fex {
            // A user library root can itself be linked. Nested namespace links must
            // not redirect FEX into a pre-existing Intel prefix.
            let mut path = compat_data.to_path_buf();
            for part in prefix.strip_prefix(compat_data)?.components() {
                path.push(part);
                match fs::symlink_metadata(&path) {
                    Ok(meta) => ensure!(
                        !meta.is_symlink(),
                        "FEX prefix namespace contains a symlink: {}",
                        path.display()
                    ),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        let parent = prefix.parent().context("Prefix has no parent")?;
        fs::create_dir_all(parent)?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(parent.join(".nucleon-launch.lock"))?;
        let start = Instant::now();
        loop {
            ensure!(!cancelled(), "Prefix launch cancelled");
            match rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
                Ok(()) => break,
                Err(rustix::io::Errno::WOULDBLOCK) if start.elapsed() < timeout => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                Err(rustix::io::Errno::WOULDBLOCK) => anyhow::bail!(
                    "Another Nucleon launch is using prefix {}. Close that game and try again",
                    prefix.display()
                ),
                Err(error) => return Err(error.into()),
            }
        }
        if runtime.mode == ExecutionMode::Arm64Fex {
            let identity = Identity {
                schema: 1,
                mode: runtime.mode,
                prefix: runtime.prefix.clone(),
            };
            let receipt = parent.join(".nucleon-prefix.json");
            match fs::read(&receipt) {
                Ok(bytes) => {
                    let previous: Identity = serde_json::from_slice(&bytes).with_context(|| {
                        format!("Invalid prefix identity {}", receipt.display())
                    })?;
                    ensure!(
                        previous == identity,
                        "Prefix {} has incompatible runtime family/ABI. Keep the existing prefix and choose a new prefix family or a compatible runtime",
                        prefix.display()
                    );
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    ensure!(
                        !prefix.exists(),
                        "Unidentified existing FEX prefix {}; use a fresh family namespace",
                        prefix.display()
                    );
                    atomic_write_file(&receipt, serde_json::to_vec_pretty(&identity)?)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(Self { _lock: lock })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime;

    #[test]
    fn existing_intel_prefixes_are_locked_independently_and_reject_fex() {
        let (temp, intel) = runtime::fixture(ExecutionMode::Intel);
        let (_fex_temp, fex) = runtime::fixture(ExecutionMode::Arm64Fex);
        let prefix = temp.path().join("heroic/game");
        let other = temp.path().join("heroic/other-game");
        for path in [&prefix, &other] {
            fs::create_dir_all(path.join("drive_c")).unwrap();
            fs::write(path.join("system.reg"), b"registry").unwrap();
            fs::write(path.join("user.reg"), b"registry").unwrap();
        }
        let lease = PrefixSession::acquire_existing(&prefix, &intel, || false).unwrap();
        assert!(PrefixSession::acquire_existing(&prefix, &intel, || false).is_err());
        drop(PrefixSession::acquire_existing(&other, &intel, || false).unwrap());
        assert!(PrefixSession::acquire_existing(&prefix, &fex, || false).is_err());
        assert_eq!(fs::read(prefix.join("system.reg")).unwrap(), b"registry");
        drop(lease);
        fs::write(prefix.join(".nucleon-runtime-abi"), b"\"arm64-fex\"").unwrap();
        assert!(PrefixSession::acquire_existing(&prefix, &intel, || false).is_err());
    }

    #[test]
    fn compatible_updates_reuse_native_prefix_while_abi_changes_are_rejected() {
        let (temp, mut runtime) = runtime::fixture(ExecutionMode::Arm64Fex);
        let compat = temp.path().join("compatdata/123");
        fs::create_dir_all(compat.join("pfx")).unwrap();
        fs::write(compat.join("pfx/keep"), b"Intel data").unwrap();
        let p = crate::paths::game_prefix(&compat, &runtime);
        {
            let _session =
                PrefixSession::acquire(&compat, &runtime, Duration::ZERO, || false).unwrap();
            fs::create_dir_all(&p).unwrap();
            fs::write(p.join("save"), b"native data").unwrap();
            assert!(PrefixSession::acquire(&compat, &runtime, Duration::ZERO, || false).is_err());
        }
        runtime.root = temp.path().join("next-compatible-runtime");
        drop(PrefixSession::acquire(&compat, &runtime, Duration::ZERO, || false).unwrap());
        runtime.prefix.abi += 1;
        assert!(PrefixSession::acquire(&compat, &runtime, Duration::ZERO, || false).is_err());
        assert_eq!(fs::read(p.join("save")).unwrap(), b"native data");
        assert_eq!(fs::read(compat.join("pfx/keep")).unwrap(), b"Intel data");
    }

    #[test]
    fn linked_native_prefixes_and_cancelled_waits_do_not_acquire_ownership() {
        let (temp, runtime) = runtime::fixture(ExecutionMode::Arm64Fex);
        let compat = temp.path().join("compat");
        let path = crate::paths::game_prefix(&compat, &runtime);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::create_dir_all(compat.join("pfx")).unwrap();
        std::os::unix::fs::symlink(compat.join("pfx"), &path).unwrap();
        assert!(PrefixSession::acquire(&compat, &runtime, Duration::ZERO, || false).is_err());
        fs::remove_file(&path).unwrap();
        assert!(
            PrefixSession::acquire(&compat, &runtime, Duration::from_secs(1), || true).is_err()
        );
        assert!(!path.parent().unwrap().join(".nucleon-prefix.json").exists());
    }
}
