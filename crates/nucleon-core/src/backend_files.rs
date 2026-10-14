//! File operations and rollback for graphics backend changes in a Wine prefix.
#![forbid(unsafe_code)]
use crate::fs_util::atomic_write_file;
use anyhow::{Context, Result, bail};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn copy(source: &Path, target: &Path) -> Result<()> {
    let bytes =
        fs::read(source).with_context(|| format!("Read backend file {}", source.display()))?;
    atomic_write_file(target, bytes)
}

pub fn remove(target: &Path) -> Result<bool> {
    match fs::remove_file(target) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => {
            Err(error).with_context(|| format!("Remove backend file {}", target.display()))
        }
    }
}

pub fn restore(dir: &Path, name: &str, runner: Option<&Path>, subdirs: &[&str]) -> Result<usize> {
    match fs::metadata(dir) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => bail!("Backend directory is not a directory: {}", dir.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Inspect backend directory {}", dir.display()));
        }
    }
    let destination = dir.join(name);
    let mut source = None;
    if let Some(root) = runner {
        for sub in subdirs {
            let candidate = root.join(sub).join(name);
            match fs::metadata(&candidate) {
                Ok(meta) if meta.is_file() => {
                    source = Some(candidate);
                    break;
                }
                Ok(_) => bail!("Wine builtin is not a file: {}", candidate.display()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("Inspect Wine builtin {}", candidate.display()));
                }
            }
        }
    }
    if let Some(source) = source {
        let bytes =
            fs::read(&source).with_context(|| format!("Read builtin {}", source.display()))?;
        if !destination.is_symlink() && fs::read(&destination).ok().as_ref() == Some(&bytes) {
            return Ok(0);
        }
        atomic_write_file(&destination, bytes)?;
        Ok(1)
    } else {
        Ok(usize::from(remove(&destination)?))
    }
}

const DLLS: &[&str] = &[
    "d3d9.dll",
    "d3d10core.dll",
    "d3d11.dll",
    "dxgi.dll",
    "d3d12.dll",
    "d3d12core.dll",
    "ddraw.dll",
    "winemetal.dll",
];
enum Saved {
    Absent,
    Link(PathBuf),
    File(PathBuf, fs::Permissions),
}

/// Serializes Nucleon's switches and restores all managed DLLs when a switch fails.
/// A failed rollback retains its backup and reports its location for recovery.
pub fn transaction<T>(prefix: &Path, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    fs::create_dir_all(prefix)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(prefix.join(".nucleon-backend.lock"))?;
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive)?;
    let backup = tempfile::Builder::new()
        .prefix("nucleon-backend-backup-")
        .tempdir()?;
    let mut saved = Vec::new();
    for subdir in ["system32", "syswow64"] {
        for name in DLLS {
            let path = prefix.join("drive_c/windows").join(subdir).join(name);
            let state = match fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_symlink() => Saved::Link(fs::read_link(&path)?),
                Ok(meta) if meta.is_file() => {
                    let copy = backup.path().join(format!("{subdir}-{name}"));
                    fs::copy(&path, &copy)
                        .with_context(|| format!("Back up {}", path.display()))?;
                    Saved::File(copy, meta.permissions())
                }
                Ok(_) => bail!("Backend DLL is not a file or symlink: {}", path.display()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Saved::Absent,
                Err(error) => {
                    return Err(error).with_context(|| format!("Inspect {}", path.display()));
                }
            };
            saved.push((path, state));
        }
    }
    let recovery = saved
        .iter()
        .map(|(path, state)| {
            let original = match state {
                Saved::Absent => serde_json::json!({"kind": "absent"}),
                Saved::Link(target) => serde_json::json!({"kind": "symlink", "target": target}),
                Saved::File(backup, _) => {
                    serde_json::json!({"kind": "file", "backup": backup.file_name()})
                }
            };
            serde_json::json!({"destination": path, "original": original})
        })
        .collect::<Vec<_>>();
    fs::write(
        backup.path().join("recovery.json"),
        serde_json::to_vec_pretty(&recovery)?,
    )?;
    match operation() {
        Ok(result) => Ok(result),
        Err(error) => {
            let mut failures = Vec::new();
            for (path, state) in saved {
                let restored = (|| -> Result<()> {
                    match state {
                        Saved::Absent => {
                            remove(&path)?;
                        }
                        Saved::Link(target) => {
                            if path.is_symlink() && fs::read_link(&path)? == target {
                                return Ok(());
                            }
                            remove(&path)?;
                            #[cfg(unix)]
                            std::os::unix::fs::symlink(target, &path)?;
                        }
                        Saved::File(source, permissions) => {
                            if path.is_symlink() || fs::read(&path).ok() != fs::read(&source).ok() {
                                copy(&source, &path)?;
                            }
                            fs::set_permissions(&path, permissions)?;
                        }
                    }
                    Ok(())
                })();
                if let Err(failure) = restored {
                    failures.push(format!("{}: {failure:#}", path.display()));
                }
            }
            if failures.is_empty() {
                return Err(error.context("Backend switch failed; previous DLLs restored"));
            }
            let retained = backup.keep();
            Err(error.context(format!(
                "Backend rollback incomplete: {}. Backup retained at {}",
                failures.join("; "),
                retained.display()
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_switch_restores_content_and_symlinks_and_removes_new_files() {
        let temp = tempfile::tempdir().unwrap();
        let system32 = temp.path().join("drive_c/windows/system32");
        fs::create_dir_all(&system32).unwrap();
        let old = system32.join("d3d11.dll");
        fs::write(&old, b"working").unwrap();
        let link = system32.join("dxgi.dll");
        std::os::unix::fs::symlink("original-dxgi", &link).unwrap();
        let result: Result<()> = transaction(temp.path(), || {
            copy(&old, &system32.join("winemetal.dll"))?;
            atomic_write_file(&old, b"replacement")?;
            atomic_write_file(&link, b"replacement")?;
            bail!("later staging failed")
        });
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("previous DLLs restored")
        );
        assert_eq!(fs::read(old).unwrap(), b"working");
        assert_eq!(fs::read_link(link).unwrap(), PathBuf::from("original-dxgi"));
        assert!(!system32.join("winemetal.dll").exists());
    }
    #[test]
    fn missing_source_preserves_existing_dll_and_restoration_errors_are_reported() {
        let temp = tempfile::tempdir().unwrap();
        let dll = temp.path().join("d3d11.dll");
        fs::write(&dll, b"working").unwrap();
        assert!(copy(&temp.path().join("missing"), &dll).is_err());
        assert_eq!(fs::read(&dll).unwrap(), b"working");
        fs::create_dir(temp.path().join("dxgi.dll")).unwrap();
        assert!(restore(temp.path(), "dxgi.dll", None, &[]).is_err());
    }
}
