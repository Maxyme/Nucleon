//! Shared build/install primitives; policy remains with the caller.
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::{self, BufReader, Seek, Write},
    path::Path,
};

pub fn digest(path: &Path) -> Result<String> {
    crate::staging::digest(path)
}

pub fn download(url: &str, limit: u64) -> Result<fs::File> {
    ensure!(url.starts_with("https://"), "Downloads require HTTPS");
    crate::downloads::download(url, limit)
}
pub fn unpack(path: &Path, target: &Path, limit: u64) -> Result<()> {
    use crate::downloads::{ArchiveFormat, extract_with_limit};
    fs::create_dir_all(target)?;
    let name = path.to_string_lossy();
    let file = fs::File::open(path)?;
    if name.ends_with(".tar.xz") {
        let mut tar = tempfile::tempfile()?;
        // Bound the decompressed spool, as well as tar's extracted contents.
        struct Bounded<'a> {
            file: &'a mut fs::File,
            left: u64,
        }
        impl Write for Bounded<'_> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if bytes.len() as u64 > self.left {
                    return Err(io::Error::other("XZ archive exceeds size limit"));
                }
                let count = self.file.write(bytes)?;
                self.left -= count as u64;
                Ok(count)
            }
            fn flush(&mut self) -> io::Result<()> {
                self.file.flush()
            }
        }
        lzma_rs::xz_decompress(
            &mut BufReader::new(file),
            &mut Bounded {
                file: &mut tar,
                left: limit,
            },
        )?;
        tar.rewind()?;
        crate::downloads::extract_tar(tar, target, limit)
    } else {
        let format = if name.ends_with(".tar.gz") {
            ArchiveFormat::TarGz
        } else if name.ends_with(".tar.zst") {
            ArchiveFormat::TarZstd
        } else if name.ends_with(".zip") {
            ArchiveFormat::Zip
        } else {
            anyhow::bail!("Unsupported archive: {}", path.display())
        };
        extract_with_limit(file, format, target, limit)
    }
}

pub fn copy_file(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target.parent().context("Copy destination has no parent")?)?;
    let mut file = fs::File::open(source)?;
    let mut stage = tempfile::NamedTempFile::new_in(target.parent().unwrap())?;
    io::copy(&mut file, &mut stage)?;
    stage
        .as_file()
        .set_permissions(file.metadata()?.permissions())?;
    stage.as_file().sync_all()?;
    stage.persist(target).map_err(|e| e.error)?;
    Ok(())
}

/// Copy a fresh complete tree without following links or allowing links outside it.
pub fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    let source = source.canonicalize()?;
    fs::create_dir(target)?;
    for entry in walkdir::WalkDir::new(&source)
        .follow_links(false)
        .min_depth(1)
        .sort_by_file_name()
    {
        let entry = entry?;
        let output = target.join(entry.path().strip_prefix(&source)?);
        if entry.file_type().is_dir() {
            fs::create_dir(&output)?;
        } else if entry.file_type().is_symlink() {
            let link = fs::read_link(entry.path())?;
            ensure!(
                !link.is_absolute() && entry.path().canonicalize()?.starts_with(&source),
                "Tree symlink escapes source: {}",
                entry.path().display()
            );
            std::os::unix::fs::symlink(link, output)?;
        } else {
            ensure!(entry.file_type().is_file(), "Unsupported tree entry");
            copy_file(entry.path(), &output)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires an explicitly supplied local XZ archive"]
    fn local_xz_archive_extracts() {
        let archive = std::env::var_os("NUCLEON_TEST_XZ_ARCHIVE")
            .expect("set NUCLEON_TEST_XZ_ARCHIVE to a local tar.xz archive");
        let target = tempfile::tempdir().unwrap();
        unpack(Path::new(&archive), target.path(), 4 * 1024 * 1024 * 1024).unwrap();
        assert!(fs::read_dir(target.path()).unwrap().next().is_some());
    }

    #[test]
    fn xz_extraction_is_bounded_and_copying_preserves_links_and_modes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("file"), b"payload").unwrap();
        fs::set_permissions(source.join("file"), fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("file", source.join("link")).unwrap();
        let target = dir.path().join("copy");
        copy_tree(&source, &target).unwrap();
        assert_eq!(
            fs::read_link(target.join("link")).unwrap(),
            Path::new("file")
        );
        assert_eq!(
            target.join("file").metadata().unwrap().permissions().mode() & 0o777,
            0o755
        );
        let mut tar = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(7);
        header.set_mode(0o644);
        tar.append_data(&mut header, "file", &b"payload"[..])
            .unwrap();
        let bytes = tar.into_inner().unwrap();
        let archive = dir.path().join("input.tar.xz");
        let mut output = fs::File::create(&archive).unwrap();
        lzma_rs::xz_compress(&mut io::Cursor::new(&bytes), &mut output).unwrap();
        unpack(&archive, &dir.path().join("unpacked"), bytes.len() as u64).unwrap();
        assert_eq!(
            fs::read(dir.path().join("unpacked/file")).unwrap(),
            b"payload"
        );
        assert!(unpack(&archive, &dir.path().join("too-small"), 100).is_err());
        std::os::unix::fs::symlink("../input.tar.xz", source.join("escape")).unwrap();
        assert!(copy_tree(&source, &dir.path().join("rejected")).is_err());
    }
}
