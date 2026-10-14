use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Searches directories in a stable, root-first order. Discovery follows directory links as
/// before, but walkdir detects cycles and reports inaccessible entries instead of hiding them.
/// Callers control depth; zero includes only the root, one includes its immediate children.
pub(crate) fn directories(root: &Path, max_depth: usize) -> impl Iterator<Item = PathBuf> {
    walkdir::WalkDir::new(root)
        .follow_links(true)
        .max_depth(max_depth)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| entry.file_type().is_dir())
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry.into_path()),
            Err(error) => {
                log::warn!("Directory discovery skipped an entry: {error}");
                None
            }
        })
}

/// Atomically writes `data` to `target_path`.
///
/// Writes to a temporary file created in the same parent directory as `target_path`
/// (ensuring same-filesystem semantics for atomic rename), and atomically replaces
/// `target_path` using `persist`, after syncing contents and permissions to disk.
/// On Unix the parent directory is then synced to persist the replacement. If that
/// final sync fails, the error states that the new file has already been published.
/// Replaces destination symlinks themselves; their targets are never written to.
pub fn atomic_write_file<P: AsRef<Path>, C: AsRef<[u8]>>(target_path: P, data: C) -> Result<()> {
    let target = target_path.as_ref();
    let parent = target
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    #[cfg(unix)]
    let parent_directory = std::fs::File::open(parent)
        .with_context(|| format!("Failed to open parent directory {}", parent.display()))?;

    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("Failed to create temporary file in {}", parent.display()))?;

    temp.write_all(data.as_ref()).with_context(|| {
        format!(
            "Failed to write data to temporary file for {}",
            target.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Inherit permissions of existing file if present, or set 0o644 default
        let mode = match std::fs::metadata(target) {
            Ok(meta) => meta.permissions().mode(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0o644,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Failed to read existing permissions for {}",
                        target.display()
                    )
                });
            }
        };
        temp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(mode))
            .with_context(|| {
                format!(
                    "Failed to set replacement permissions for {}",
                    target.display()
                )
            })?;
    }

    temp.as_file()
        .sync_all()
        .with_context(|| format!("Failed to sync replacement file for {}", target.display()))?;

    temp.persist(target).map_err(|e| {
        anyhow::anyhow!(
            "Failed to atomically persist to {}: {}",
            target.display(),
            e.error
        )
    })?;

    #[cfg(unix)]
    parent_directory.sync_all().with_context(|| {
        format!(
            "Replacement for {} was published, but syncing parent directory {} failed",
            target.display(),
            parent.display()
        )
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn discovery_preserves_root_priority_and_depth_and_sorts_siblings() {
        let dir = tempdir().unwrap();
        for name in ["z-release", "a-release/nested"] {
            std::fs::create_dir_all(dir.path().join(name)).unwrap();
        }
        let shallow = directories(dir.path(), 1).collect::<Vec<_>>();
        assert_eq!(
            shallow,
            vec![
                dir.path().to_path_buf(),
                dir.path().join("a-release"),
                dir.path().join("z-release")
            ]
        );
        assert_eq!(directories(dir.path(), 0).count(), 1);
        assert_eq!(directories(dir.path(), usize::MAX).count(), 4);

        for root in [
            dir.path().to_path_buf(),
            dir.path().join("a-release/nested"),
        ] {
            std::fs::write(root.join("d3d11.dll"), b"fixture").unwrap();
        }
        assert_eq!(
            crate::dxvk::inspect_candidate_dir(dir.path()).unwrap().root,
            dir.path()
        );
        assert_eq!(
            crate::dxmt::inspect_candidate_dir(dir.path()).unwrap().root,
            dir.path()
        );
        std::fs::remove_file(dir.path().join("d3d11.dll")).unwrap();
        assert_eq!(
            crate::dxvk::inspect_candidate_dir(dir.path()).unwrap().root,
            dir.path().join("a-release/nested")
        );
        assert_eq!(
            crate::dxmt::inspect_candidate_dir(dir.path()).unwrap().root,
            dir.path().join("a-release/nested")
        );
    }

    #[cfg(unix)]
    #[test]
    fn discovery_handles_linked_roots_and_skips_cycles_and_broken_links() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        let root = dir.path().join("root");
        let release = dir.path().join("release");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&release).unwrap();
        std::fs::write(release.join("d3d11.dll"), b"fixture").unwrap();
        symlink(".", root.join("a-cycle")).unwrap();
        symlink("missing", root.join("b-broken")).unwrap();
        symlink("../release", root.join("c-release")).unwrap();
        let alias = dir.path().join("alias");
        symlink("root", &alias).unwrap();
        assert_eq!(
            directories(&alias, usize::MAX).collect::<Vec<_>>(),
            vec![alias.clone(), alias.join("c-release")]
        );
        assert_eq!(
            crate::dxvk::inspect_candidate_dir(&alias).unwrap().root,
            alias.join("c-release")
        );
        assert_eq!(
            crate::dxmt::inspect_candidate_dir(&alias).unwrap().root,
            alias.join("c-release")
        );
    }

    #[test]
    fn test_atomic_write_file_new_and_overwrite() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test_atomic.txt");

        atomic_write_file(&file, b"initial content").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "initial content");

        atomic_write_file(&file, "updated content").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "updated content");
    }

    #[test]
    fn failed_atomic_replacement_preserves_destination_and_cleans_temporary_file() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("existing-directory");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("original"), b"preserve").unwrap();
        let error = atomic_write_file(&target, b"replacement").unwrap_err();
        assert!(error.to_string().contains("Failed to atomically persist"));
        assert_eq!(std::fs::read(target.join("original")).unwrap(), b"preserve");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_replacement_preserves_private_permissions_and_does_not_write_through_links() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempdir().unwrap();
        let private = dir.path().join("private");
        std::fs::write(&private, b"original").unwrap();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o600)).unwrap();
        atomic_write_file(&private, b"updated").unwrap();
        assert_eq!(
            std::fs::metadata(&private).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let link = dir.path().join("link");
        symlink("private", &link).unwrap();
        atomic_write_file(&link, b"separate").unwrap();
        assert!(!std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(
            std::fs::metadata(&link).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::read(&private).unwrap(), b"updated");
        assert_eq!(std::fs::read(&link).unwrap(), b"separate");
        let new_file = dir.path().join("new");
        atomic_write_file(&new_file, b"new").unwrap();
        assert_eq!(
            std::fs::metadata(new_file).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[cfg(unix)]
    #[test]
    fn permission_lookup_failure_preserves_destination_instead_of_using_default_mode() {
        let dir = tempdir().unwrap();
        let link = dir.path().join("cycle");
        std::os::unix::fs::symlink("cycle", &link).unwrap();
        let error = atomic_write_file(&link, b"replacement").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Failed to read existing permissions")
        );
        assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("cycle"));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
