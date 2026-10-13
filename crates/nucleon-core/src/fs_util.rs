use anyhow::{Context, Result};
use std::io::Write;
use std::path::Path;

/// Atomically writes `data` to `target_path`.
///
/// Writes to a temporary file created in the same parent directory as `target_path`
/// (ensuring same-filesystem semantics for atomic rename), and atomically replaces
/// `target_path` on success using `persist`.
pub fn atomic_write_file<P: AsRef<Path>, C: AsRef<[u8]>>(target_path: P, data: C) -> Result<()> {
    let target = target_path.as_ref();
    let parent = target.parent().unwrap_or_else(|| Path::new("."));

    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("Failed to create temporary file in {}", parent.display()))?;

    temp.write_all(data.as_ref()).with_context(|| {
        format!(
            "Failed to write data to temporary file for {}",
            target.display()
        )
    })?;
    temp.flush()
        .with_context(|| format!("Failed to flush temporary file for {}", target.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Inherit permissions of existing file if present, or set 0o644 default
        let mode = if let Ok(meta) = std::fs::metadata(target) {
            meta.permissions().mode()
        } else {
            0o644
        };
        let mut perms = temp.as_file().metadata()?.permissions();
        perms.set_mode(mode);
        let _ = temp.as_file().set_permissions(perms);
    }

    temp.persist(target).map_err(|e| {
        anyhow::anyhow!(
            "Failed to atomically persist to {}: {}",
            target.display(),
            e.error
        )
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_atomic_write_file_new_and_overwrite() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test_atomic.txt");

        atomic_write_file(&file, b"initial content").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "initial content");

        atomic_write_file(&file, "updated content").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "updated content");
    }
}
