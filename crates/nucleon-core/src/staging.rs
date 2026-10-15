//! Content-aware preparation receipts. Metadata is a fast path, never a readiness marker alone.
use crate::fs_util::atomic_write_file;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Identity {
    device: u64,
    inode: u64,
    size: u64,
    mode: u32,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl From<&fs::Metadata> for Identity {
    fn from(m: &fs::Metadata) -> Self {
        Self {
            device: m.dev(),
            inode: m.ino(),
            size: m.len(),
            mode: m.mode(),
            modified: (m.mtime(), m.mtime_nsec()),
            changed: (m.ctime(), m.ctime_nsec()),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum State {
    Missing,
    Link { target: PathBuf },
    File { identity: Identity, sha256: String },
}
impl State {
    fn content_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::File { sha256: a, .. }, Self::File { sha256: b, .. }) => a == b,
            _ => self == other,
        }
    }
}
pub fn digest(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).with_context(|| format!("Read {}", path.display()))?;
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 65536];
    loop {
        let n = file.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
pub(crate) fn key(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
fn state(path: &Path, previous: Option<&State>) -> Result<State> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(State::Missing),
        Err(e) => return Err(e).with_context(|| format!("Inspect {}", path.display())),
    };
    if meta.is_symlink() {
        return Ok(State::Link {
            target: fs::read_link(path)?,
        });
    }
    ensure!(
        meta.is_file(),
        "Prepared component is not a regular file: {}",
        path.display()
    );
    let identity = Identity::from(&meta);
    if let Some(State::File {
        identity: old,
        sha256,
    }) = previous
        && old == &identity
    {
        return Ok(State::File {
            identity,
            sha256: sha256.clone(),
        });
    }
    let sha256 =
        crate::managed_runtime::known_digest(path, &identity).unwrap_or_else(|| digest(path))?;
    ensure!(
        Identity::from(&fs::symlink_metadata(path)?) == identity,
        "Component changed while hashing: {}",
        path.display()
    );
    Ok(State::File { identity, sha256 })
}

#[derive(Default, PartialEq, Eq, Serialize, Deserialize)]
struct Receipt {
    schema: u32,
    key: String,
    inputs: BTreeMap<PathBuf, State>,
    outputs: BTreeMap<PathBuf, State>,
}
fn read(path: &Path) -> Result<Receipt> {
    match fs::read(path) {
        Ok(data) => Ok(serde_json::from_slice::<Receipt>(&data)
            .ok()
            .filter(|r| r.schema == 1)
            .unwrap_or_default()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Receipt::default()),
        Err(e) => Err(e).with_context(|| format!("Read preparation receipt {}", path.display())),
    }
}
fn save(path: &Path, previous: &Receipt, receipt: &Receipt) -> Result<()> {
    if previous != receipt {
        fs::create_dir_all(path.parent().context("Receipt has no parent")?)?;
        atomic_write_file(path, serde_json::to_vec(receipt)?)?;
    }
    Ok(())
}
fn invalidate(path: &Path) -> Result<()> {
    crate::backend_files::remove(path)?;
    Ok(())
}

/// Caches commands such as binding/signing or registry setup. Outputs are checked
/// individually, including absent files and symlink targets. Caller holds its owner lock.
pub(crate) fn component(
    path: &Path,
    key: String,
    inputs: &[PathBuf],
    outputs: &[PathBuf],
    operation: impl FnOnce() -> Result<()>,
) -> Result<bool> {
    let previous = read(path)?;
    let mut next = Receipt {
        schema: 1,
        key,
        ..Default::default()
    };
    for input in inputs {
        // Input symlink targets can change; always fingerprint the selected file.
        let input = input
            .canonicalize()
            .with_context(|| format!("Resolve input {}", input.display()))?;
        next.inputs
            .insert(input.clone(), state(&input, previous.inputs.get(&input))?);
    }
    for output in outputs {
        next.outputs
            .insert(output.clone(), state(output, previous.outputs.get(output))?);
    }
    let same_inputs = next.inputs.len() == previous.inputs.len()
        && next
            .inputs
            .iter()
            .all(|(p, s)| previous.inputs.get(p).is_some_and(|old| s.content_eq(old)));
    let same_outputs = next.outputs.len() == previous.outputs.len()
        && next
            .outputs
            .iter()
            .all(|(p, s)| previous.outputs.get(p).is_some_and(|old| s.content_eq(old)));
    if previous.key == next.key && same_inputs && same_outputs {
        save(path, &previous, &next)?;
        return Ok(false);
    }
    invalidate(path)?;
    operation()?;
    // Never bless an input that changed during preparation.
    for (input, old) in &next.inputs {
        ensure!(
            state(input, Some(old))?.content_eq(old),
            "Preparation input changed: {}",
            input.display()
        );
    }
    for output in outputs {
        next.outputs.insert(output.clone(), state(output, None)?);
    }
    save(path, &Receipt::default(), &next)?;
    Ok(true)
}

/// Hash once, then revalidate file identities before reusing content-addressed artifacts.
pub(crate) fn source_key(namespace: &str, inputs: &[PathBuf]) -> Result<String> {
    let path = crate::paths::support_dir()
        .join("cache/file-identities")
        .join(format!("{}.json", key(&(namespace, inputs))?));
    let previous = read(&path)?;
    let mut next = Receipt {
        schema: 1,
        key: namespace.into(),
        ..Default::default()
    };
    for input in inputs {
        let input = input.canonicalize()?;
        next.inputs
            .insert(input.clone(), state(&input, previous.inputs.get(&input))?);
    }
    // Paths locate identities, but artifact compatibility depends on ordered
    // contents. Moving an unchanged ntdll into a new generation must not resign.
    let contents = inputs
        .iter()
        .map(|input| {
            let canonical = input.canonicalize()?;
            match &next.inputs[&canonical] {
                State::File { sha256, .. } => Ok(sha256.clone()),
                _ => anyhow::bail!("Missing source file {}", input.display()),
            }
        })
        .collect::<Result<Vec<_>>>()?;
    let result = key(&(namespace, contents))?;
    save(&path, &previous, &next)?;
    Ok(result)
}

#[derive(Debug, Clone, Serialize)]
pub(crate) enum Desired {
    Copy(PathBuf),
    Link(PathBuf),
    Absent,
}

fn within_preparation_root(
    receipt: &Path,
    targets: impl IntoIterator<Item = PathBuf>,
) -> Result<()> {
    let parent = receipt.parent().context("Receipt has no parent")?;
    let root = if parent.file_name().is_some_and(|n| n == ".nucleon-prepared") {
        parent.parent().context("Preparation has no prefix")?
    } else {
        parent
    };
    let root = root.canonicalize()?;
    for target in std::iter::once(receipt.to_path_buf()).chain(targets) {
        let mut ancestor = target
            .parent()
            .context("Preparation target has no parent")?;
        loop {
            match fs::symlink_metadata(ancestor) {
                Ok(_) => break,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    ancestor = ancestor
                        .parent()
                        .context("No existing preparation directory")?
                }
                Err(e) => return Err(e.into()),
            }
        }
        ensure!(
            ancestor.canonicalize()?.starts_with(&root),
            "Preparation directory escapes its owner: {}",
            target.display()
        );
    }
    Ok(())
}

/// Decide all writes before entering the graphics rollback transaction. Different
/// engines can share identical DLLs without restoring and then recopying them.
pub(crate) fn files(
    receipt_path: &Path,
    desired: &BTreeMap<PathBuf, Desired>,
    transaction: impl FnOnce(&mut dyn FnMut() -> Result<usize>) -> Result<usize>,
) -> Result<usize> {
    within_preparation_root(receipt_path, desired.keys().cloned())?;
    let previous = read(receipt_path)?;
    let mut next = Receipt {
        schema: 1,
        key: key(desired)?,
        ..Default::default()
    };
    let mut changes = Vec::new();
    for (destination, wanted) in desired {
        let current = state(destination, previous.outputs.get(destination))?;
        let matches = match wanted {
            Desired::Copy(source) => {
                let source = source.canonicalize()?;
                let source_state = state(&source, previous.inputs.get(&source))?;
                ensure!(
                    matches!(source_state, State::File { .. }),
                    "Missing staging source {}",
                    source.display()
                );
                let same = current.content_eq(&source_state);
                next.inputs.insert(source, source_state);
                same
            }
            Desired::Link(target) => {
                current
                    == State::Link {
                        target: target.clone(),
                    }
            }
            Desired::Absent => current == State::Missing,
        };
        next.outputs.insert(destination.clone(), current);
        if !matches {
            changes.push((destination, wanted));
        }
    }
    if changes.is_empty() {
        save(receipt_path, &previous, &next)?;
        return Ok(0);
    }
    invalidate(receipt_path)?;
    let mut operation = || -> Result<usize> {
        for (destination, wanted) in &changes {
            fs::create_dir_all(
                destination
                    .parent()
                    .context("Staging target has no parent")?,
            )?;
            match wanted {
                Desired::Copy(source) => {
                    let data = fs::read(source)?;
                    let source = source.canonicalize()?;
                    let State::File { sha256, .. } = &next.inputs[&source] else {
                        unreachable!()
                    };
                    ensure!(
                        &format!("{:x}", Sha256::digest(&data)) == sha256,
                        "Staging source changed: {}",
                        source.display()
                    );
                    atomic_write_file(destination, data)?;
                }
                Desired::Link(target) => {
                    let temp = tempfile::tempdir_in(destination.parent().unwrap())?;
                    let link = temp.path().join("link");
                    std::os::unix::fs::symlink(target, &link)?;
                    fs::rename(link, destination)?;
                }
                Desired::Absent => {
                    crate::backend_files::remove(destination)?;
                }
            }
        }
        Ok(changes.len())
    };
    let count = transaction(&mut operation)?;
    for (path, wanted) in changes {
        let current = if let Desired::Copy(source) = wanted {
            // The exact source bytes were checked before atomic publication;
            // capture their new identity without hashing the same DLL again.
            let source = source.canonicalize()?;
            let State::File { sha256, .. } = &next.inputs[&source] else {
                unreachable!()
            };
            State::File {
                identity: Identity::from(&fs::symlink_metadata(path)?),
                sha256: sha256.clone(),
            }
        } else {
            state(path, None)?
        };
        next.outputs.insert(path.clone(), current);
    }
    save(receipt_path, &Receipt::default(), &next)?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unchanged_files_keep_inodes_and_only_changed_or_missing_files_are_repaired() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        let x = temp.path().join("out/x");
        let y = temp.path().join("out/y");
        fs::write(&a, b"aaa").unwrap();
        fs::write(&b, b"bbb").unwrap();
        let receipt = temp.path().join("receipt");
        let desired = BTreeMap::from([
            (x.clone(), Desired::Copy(a.clone())),
            (y.clone(), Desired::Copy(b.clone())),
        ]);
        let stage = || files(&receipt, &desired, |op| op()).unwrap();
        assert_eq!(stage(), 2);
        let ino = fs::metadata(&x).unwrap().ino();
        let receipt_ino = fs::metadata(&receipt).unwrap().ino();
        assert_eq!(stage(), 0);
        assert_eq!(fs::metadata(&receipt).unwrap().ino(), receipt_ino);
        fs::write(&b, b"ccc").unwrap();
        assert_eq!(stage(), 1);
        assert_eq!(fs::metadata(&x).unwrap().ino(), ino);
        fs::write(&y, b"bad").unwrap();
        assert_eq!(stage(), 1);
        fs::remove_file(&y).unwrap();
        assert_eq!(stage(), 1);
        assert_eq!(fs::read(&y).unwrap(), b"ccc");
    }
    #[test]
    fn redirected_output_directories_do_not_modify_host_files() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::write(&source, b"new").unwrap();
        let original = outside.path().join("dll");
        fs::write(&original, b"keep").unwrap();
        std::os::unix::fs::symlink(outside.path(), temp.path().join("out")).unwrap();
        let desired = BTreeMap::from([(temp.path().join("out/dll"), Desired::Copy(source))]);
        assert!(files(&temp.path().join("receipt"), &desired, |op| op()).is_err());
        assert_eq!(fs::read(original).unwrap(), b"keep");
    }

    #[test]
    fn failed_preparation_does_not_publish_readiness_and_changes_retry() {
        let temp = tempfile::tempdir().unwrap();
        let receipt = temp.path().join("receipt");
        let input = temp.path().join("input");
        let out = temp.path().join("out");
        fs::write(&input, b"source").unwrap();
        let inputs = [input.clone()];
        let outputs = [out.clone()];
        assert!(
            component(&receipt, "key".into(), &inputs, &outputs, || anyhow::bail!(
                "failure"
            ))
            .is_err()
        );
        assert!(!receipt.exists());
        assert!(
            component(&receipt, "key".into(), &inputs, &outputs, || {
                fs::write(&out, b"built")?;
                Ok(())
            })
            .unwrap()
        );
        assert!(
            !component(&receipt, "key".into(), &inputs, &outputs, || panic!(
                "must reuse"
            ))
            .unwrap()
        );
        fs::write(&out, b"broken").unwrap();
        assert!(
            component(&receipt, "key".into(), &inputs, &outputs, || {
                fs::write(&out, b"built")?;
                Ok(())
            })
            .unwrap()
        );
    }
}
