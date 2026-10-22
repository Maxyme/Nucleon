//! Shared component downloads: validate a fresh tree before publishing it as one generation.
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read, Seek},
    path::Path,
    sync::OnceLock,
    time::Duration,
};

const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;
const MAX_EXTRACTED: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArchiveFormat {
    TarGz,
    TarZstd,
    Zip,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    name: String,
    default_version: String,
    url: String,
}

type Catalog = BTreeMap<String, Source>;

fn parse_catalog(text: &str) -> Result<Catalog> {
    let catalog: Catalog = toml::from_str(text).context("Invalid assets/downloads.toml")?;
    for (id, source) in &catalog {
        ensure!(
            !source.name.trim().is_empty(),
            "Download source {id} has an empty name"
        );
        source
            .resolve(None)
            .with_context(|| format!("Invalid download source {id}"))?;
    }
    Ok(catalog)
}

fn source(id: &str) -> Result<&'static Source> {
    static CATALOG: OnceLock<std::result::Result<Catalog, String>> = OnceLock::new();
    let catalog = CATALOG
        .get_or_init(|| {
            parse_catalog(include_str!("../../../assets/downloads.toml"))
                .map_err(|error| format!("{error:#}"))
        })
        .as_ref()
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    catalog
        .get(id)
        .with_context(|| format!("Download source {id} is missing from assets/downloads.toml"))
}

pub(crate) fn default_version(id: &str) -> Result<&'static str> {
    Ok(&source(id)?.default_version)
}

pub(crate) fn release(id: &str, version: Option<&str>) -> Result<Release> {
    source(id)?.resolve(version)
}

impl Source {
    fn resolve(&self, version: Option<&str>) -> Result<Release> {
        let raw = version.unwrap_or(&self.default_version).trim();
        let number = raw.strip_prefix('v').unwrap_or(raw);
        ensure!(
            !number.is_empty()
                && number.bytes().all(|byte| byte.is_ascii_alphanumeric()
                    || matches!(byte, b'.' | b'_' | b'-' | b'+')),
            "Invalid release version {raw:?}"
        );
        let tag = format!("v{number}");
        let url = self.url.replace("{tag}", &tag).replace("{version}", number);
        ensure!(
            url.starts_with("https://") && !url.contains(['{', '}']),
            "Release URL must use HTTPS and only {{tag}} or {{version}} placeholders"
        );
        let path = url.split(['?', '#']).next().unwrap_or(&url);
        let format = if path.ends_with(".tar.gz") {
            ArchiveFormat::TarGz
        } else if path.ends_with(".tar.zst") {
            ArchiveFormat::TarZstd
        } else if path.ends_with(".zip") {
            ArchiveFormat::Zip
        } else {
            anyhow::bail!(
                "Unsupported archive suffix in release URL {url:?}: use .tar.gz, .tar.zst, or .zip"
            );
        };
        Ok(Release {
            name: self.name.clone(),
            tag,
            url,
            format,
        })
    }
}

pub(crate) struct Release {
    pub name: String,
    pub tag: String,
    pub url: String,
    pub format: ArchiveFormat,
}

pub(crate) fn fetch<T>(
    release: &Release,
    target: &Path,
    inspect: impl Fn(&Path) -> Option<T>,
    stage: impl FnOnce(&T, &Path, &Path) -> Result<()>,
) -> Result<T> {
    log::info!(
        "Fetching {} {} from {}",
        release.name,
        release.tag,
        release.url
    );
    let archive = download(&release.url, MAX_DOWNLOAD)?;
    install_archive(archive, release, target, inspect, stage)
}

pub(crate) fn download(url: &str, limit: u64) -> Result<fs::File> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(60))
        .timeout_write(Duration::from_secs(60))
        .build();
    let response = agent
        .get(url)
        .set("User-Agent", "nucleon-fetcher")
        .call()
        .with_context(|| format!("Failed to download {url}"))?;
    let mut archive = tempfile::tempfile().context("Failed to create download spool")?;
    let size = io::copy(&mut response.into_reader().take(limit + 1), &mut archive)
        .with_context(|| format!("Failed to read download {url}"))?;
    ensure!(size <= limit, "Archive exceeds download size limit");
    archive.rewind()?;
    Ok(archive)
}

fn extract(reader: impl Read + Seek, format: ArchiveFormat, directory: &Path) -> Result<()> {
    extract_with_limit(reader, format, directory, MAX_EXTRACTED)
}

pub(crate) fn extract_with_limit(
    reader: impl Read + Seek,
    format: ArchiveFormat,
    directory: &Path,
    limit: u64,
) -> Result<()> {
    match format {
        ArchiveFormat::Zip => {
            let mut archive = zip::ZipArchive::new(reader)?;
            let size = (0..archive.len()).try_fold(0u64, |sum, index| -> Result<u64> {
                sum.checked_add(archive.by_index(index)?.size())
                    .context("Archive size overflow")
            })?;
            ensure!(size <= limit, "Archive exceeds extracted size limit");
            archive.extract(directory)?;
        }
        ArchiveFormat::TarGz => {
            extract_tar(flate2::read::GzDecoder::new(reader), directory, limit)?
        }
        ArchiveFormat::TarZstd => {
            extract_tar(zstd::stream::Decoder::new(reader)?, directory, limit)?
        }
    }
    Ok(())
}

pub(crate) fn extract_tar(reader: impl Read, directory: &Path, limit: u64) -> Result<()> {
    let mut archive = tar::Archive::new(reader);
    let mut size = 0u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        size = size
            .checked_add(entry.size())
            .context("Archive size overflow")?;
        ensure!(size <= limit, "Archive exceeds extracted size limit");
        ensure!(
            entry.unpack_in(directory)?,
            "Archive contains a path outside its extraction directory"
        );
    }
    Ok(())
}

fn install_archive<T>(
    reader: impl Read + Seek,
    release: &Release,
    target: &Path,
    inspect: impl Fn(&Path) -> Option<T>,
    stage: impl FnOnce(&T, &Path, &Path) -> Result<()>,
) -> Result<T> {
    let extracted = tempfile::tempdir()?;
    extract(reader, release.format, extracted.path())
        .with_context(|| format!("Failed to extract {} archive", release.name))?;
    let bundle = crate::fs_util::directories(extracted.path(), 2)
        .find_map(|path| inspect(&path))
        .with_context(|| {
            format!(
                "Extracted archive contains no valid {} bundle",
                release.name
            )
        })?;
    let target = std::path::absolute(target)?;
    if target.is_dir() && fs::read_dir(&target)?.next().transpose()?.is_some() {
        ensure!(
            target.join("version.txt").is_file() && inspect(&target).is_some(),
            "Refusing to replace nonempty destination {}: choose an empty directory or an existing versioned {} installation",
            target.display(),
            release.name
        );
    }
    let parent = target
        .parent()
        .context("Component destination has no parent")?;
    fs::create_dir_all(parent)?;
    let staged = tempfile::Builder::new()
        .prefix(".nucleon-download-")
        .tempdir_in(parent)?;
    stage(&bundle, extracted.path(), staged.path())
        .with_context(|| format!("Failed to stage {} release", release.name))?;
    fs::write(
        staged.path().join("version.txt"),
        format!("{}\n", release.tag),
    )
    .context("Failed to write component version")?;
    ensure!(
        inspect(staged.path()).is_some(),
        "Staged {} bundle failed validation",
        release.name
    );
    publish(staged.path(), &target)?;
    // After an exchange, staged now contains the old generation; TempDir removes it.
    let bundle = inspect(&target).context("Published component could not be inspected")?;
    log::info!(
        "{} {} staged at {}",
        release.name,
        release.tag,
        target.display()
    );
    Ok(bundle)
}

pub(crate) fn publish(staged: &Path, target: &Path) -> Result<()> {
    let exists = match fs::symlink_metadata(target) {
        Ok(metadata) => {
            ensure!(
                metadata.is_dir() && !metadata.is_symlink(),
                "Component destination must be a directory, not a file or symlink: {}",
                target.display()
            );
            true
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error).context("Could not inspect component destination"),
    };
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let flags = if exists {
            rustix::fs::RenameFlags::EXCHANGE
        } else {
            rustix::fs::RenameFlags::NOREPLACE
        };
        rustix::fs::renameat_with(rustix::fs::CWD, staged, rustix::fs::CWD, target, flags)
            .with_context(|| format!("Failed to publish component at {}", target.display()))?;
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (staged, exists);
        anyhow::bail!("Atomic component publication is unsupported on this platform")
    }
}

/// Publish a retained generation without ever replacing a mapped installation.
pub(crate) fn publish_new(staged: &Path, target: &Path) -> Result<()> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        staged,
        rustix::fs::CWD,
        target,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .with_context(|| format!("Publish runtime without replacing {}", target.display()))?;
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    anyhow::bail!("Atomic runtime publication is unavailable on this platform");
    Ok(())
}

pub(crate) fn copy_file(source: &Path, destination: &Path, extracted: &Path) -> Result<()> {
    ensure!(
        source
            .canonicalize()?
            .starts_with(extracted.canonicalize()?),
        "Component file points outside the extracted archive: {}",
        source.display()
    );
    fs::create_dir_all(destination.parent().context("Staged file has no parent")?)?;
    fs::copy(source, destination).with_context(|| {
        format!(
            "Failed to copy {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

pub(crate) fn copy_layout(
    root: &Path,
    destination: &Path,
    extracted: &Path,
    subdirs: &[&str],
) -> Result<()> {
    for sub in std::iter::once("").chain(subdirs.iter().copied()) {
        let source = root.join(sub);
        if !source.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&source)? {
            let entry = entry?;
            if entry.path().is_file() {
                copy_file(
                    &entry.path(),
                    &destination.join(sub).join(entry.file_name()),
                    extracted,
                )?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    #[test]
    fn catalog_resolves_default_and_explicit_release_urls() {
        let expected = [
            ("dxvk", "v2.4.1", "dxvk-2.4.1.tar.gz", ArchiveFormat::TarGz),
            (
                "dxmt",
                "v0.80",
                "dxmt-v0.80-builtin.tar.gz",
                ArchiveFormat::TarGz,
            ),
            (
                "vkd3d",
                "v3.0.1",
                "vkd3d-proton-3.0.1.tar.zst",
                ArchiveFormat::TarZstd,
            ),
            ("d7vk", "v2.3", "d7vk-v2.3.zip", ArchiveFormat::Zip),
        ];
        for (id, tag, archive, format) in expected {
            let resolved = release(id, None).unwrap();
            assert_eq!(default_version(id).unwrap(), tag);
            assert_eq!(resolved.tag, tag);
            assert_eq!(resolved.format, format);
            assert!(resolved.url.ends_with(&format!("/{tag}/{archive}")));
            let explicit = release(id, Some("9.8-rc1")).unwrap();
            assert_eq!(explicit.tag, "v9.8-rc1");
            assert!(explicit.url.contains("/v9.8-rc1/"));
            assert_eq!(explicit.url, release(id, Some("v9.8-rc1")).unwrap().url);
        }
        assert!(release("missing", None).is_err());
        for version in ["", "v", "../bad", "1?bad", "1/bad"] {
            assert!(release("dxvk", Some(version)).is_err());
        }
    }

    #[test]
    fn catalog_errors_identify_invalid_schema_and_templates() {
        let valid = r#"[test]
name = "Test"
default_version = "v1"
url = "https://example.com/{tag}/test-{version}.zip"
"#;
        assert!(parse_catalog(valid).is_ok());
        let with_query =
            parse_catalog(&valid.replace(".zip\"", ".zip?download=1#asset\"")).unwrap();
        assert_eq!(
            with_query["test"].resolve(None).unwrap().format,
            ArchiveFormat::Zip
        );
        for invalid in [
            valid.replace(".zip", ".rar"),
            valid.replace(".zip", ".rar?filename=.zip"),
            valid.replace("default_version = \"v1\"", "default_version = \"\""),
            valid.replace("https://", "http://"),
            valid.replace("{tag}", "{unknown}"),
            format!("{valid}unknown_field = true\n"),
        ] {
            assert!(parse_catalog(&invalid).is_err());
        }
    }

    fn archive(format: ArchiveFormat, files: &[(&str, &[u8])]) -> Vec<u8> {
        if matches!(format, ArchiveFormat::Zip) {
            let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
            for (name, bytes) in files {
                zip.start_file(
                    *name,
                    zip::write::SimpleFileOptions::default().unix_permissions(0o755),
                )
                .unwrap();
                zip.write_all(bytes).unwrap();
            }
            return zip.finish().unwrap().into_inner();
        }
        let mut tar = tar::Builder::new(Vec::new());
        for (name, bytes) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            // Raw paths let the hostile-path test bypass tar::Builder's own path checks.
            header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
            header.set_cksum();
            tar.append(&header, *bytes).unwrap();
        }
        let tar = tar.into_inner().unwrap();
        match format {
            ArchiveFormat::TarGz => {
                let mut encoder =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(&tar).unwrap();
                encoder.finish().unwrap()
            }
            ArchiveFormat::TarZstd => zstd::stream::encode_all(Cursor::new(tar), 1).unwrap(),
            ArchiveFormat::Zip => unreachable!(),
        }
    }

    fn fixture_release(format: ArchiveFormat) -> Release {
        Release {
            name: "test component".into(),
            tag: "v2".into(),
            url: "unused".into(),
            format,
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn all_components_install_wrapped_archives_and_replace_whole_generations() {
        let cases = [
            (
                "dxvk",
                ArchiveFormat::TarGz,
                "x64/d3d11.dll",
                "x64/d3d11.dll",
            ),
            ("dxvk", ArchiveFormat::TarGz, "d3d11.dll", "d3d11.dll"),
            (
                "dxmt",
                ArchiveFormat::TarGz,
                "x86_64-windows/d3d11.dll",
                "x86_64-windows/d3d11.dll",
            ),
            ("dxmt", ArchiveFormat::TarGz, "d3d11.dll", "d3d11.dll"),
            (
                "vkd3d",
                ArchiveFormat::TarZstd,
                "x64/d3d12.dll",
                "x64/d3d12.dll",
            ),
            ("vkd3d", ArchiveFormat::TarZstd, "d3d12.dll", "d3d12.dll"),
            ("d7vk", ArchiveFormat::Zip, "x32/ddraw.dll", "x32/ddraw.dll"),
            ("d7vk", ArchiveFormat::Zip, "ddraw.dll", "x32/ddraw.dll"),
        ];
        for (name, format, file, installed_file) in cases {
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join(name);
            let inspect = |path: &Path| match name {
                "dxvk" => crate::dxvk::inspect_candidate_dir(path).map(|bundle| bundle.root),
                "dxmt" => crate::dxmt::inspect_candidate_dir(path).map(|bundle| bundle.root),
                "vkd3d" => crate::vkd3d::inspect_candidate_dir(path).map(|bundle| bundle.root),
                _ => crate::d7vk::inspect_candidate_dir(path).map(|bundle| bundle.root),
            };
            for generation in [b"first".as_slice(), b"second".as_slice()] {
                let bytes = archive(format, &[(&format!("release/{file}"), generation)]);
                let result = install_archive(
                    Cursor::new(bytes),
                    &fixture_release(format),
                    &target,
                    inspect,
                    |root, extracted, staged| {
                        if name == "d7vk" {
                            let bundle = crate::d7vk::inspect_candidate_dir(root).unwrap();
                            copy_file(&bundle.x86_ddraw, &staged.join("x32/ddraw.dll"), extracted)
                        } else {
                            copy_layout(root, staged, extracted, &["x64", "x86_64-windows", "x32"])
                        }
                    },
                )
                .unwrap();
                assert_eq!(result, target);
                assert_eq!(fs::read(target.join(installed_file)).unwrap(), generation);
                assert_eq!(
                    fs::read_to_string(target.join("version.txt")).unwrap(),
                    "v2\n"
                );
                assert!(!target.join("obsolete.dll").exists());
                assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
                fs::write(target.join("obsolete.dll"), b"stale").unwrap();
            }
        }
    }

    #[test]
    fn failed_extraction_or_staging_preserves_existing_installation() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("installed");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("original"), b"old").unwrap();
        fs::write(target.join("valid.dll"), b"old DLL").unwrap();
        fs::write(target.join("version.txt"), b"v1\n").unwrap();
        let format = ArchiveFormat::TarGz;
        let inspect = |path: &Path| path.join("valid.dll").is_file().then(|| path.to_path_buf());
        let bytes = archive(format, &[("release/valid.dll", b"new")]);
        assert!(
            install_archive(
                Cursor::new(b"not an archive"),
                &fixture_release(format),
                &target,
                inspect,
                |_, _, _| Ok(())
            )
            .is_err()
        );
        assert!(
            install_archive(
                Cursor::new(bytes.clone()),
                &fixture_release(format),
                &target,
                inspect,
                |root, extracted, staged| {
                    copy_layout(root, staged, extracted, &[])?;
                    anyhow::bail!("Simulated staging failure");
                }
            )
            .is_err()
        );
        // A valid source is still insufficient if the staged tree does not validate.
        assert!(
            install_archive(
                Cursor::new(bytes),
                &fixture_release(format),
                &target,
                inspect,
                |_, _, _| Ok(())
            )
            .is_err()
        );
        assert_eq!(fs::read(target.join("original")).unwrap(), b"old");
        assert_eq!(fs::read(target.join("valid.dll")).unwrap(), b"old DLL");
        assert_eq!(fs::read(target.join("version.txt")).unwrap(), b"v1\n");
        assert_eq!(fs::read_dir(&target).unwrap().count(), 3);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn archives_cannot_write_outside_extraction_directory() {
        for format in [
            ArchiveFormat::TarGz,
            ArchiveFormat::TarZstd,
            ArchiveFormat::Zip,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let extracted = dir.path().join("extracted");
            fs::create_dir(&extracted).unwrap();
            assert!(
                extract(
                    Cursor::new(archive(format, &[("../outside", b"bad")])),
                    format,
                    &extracted
                )
                .is_err()
            );
            assert!(!dir.path().join("outside").exists());
        }
    }

    #[test]
    fn unrelated_nonempty_destinations_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("other-files");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep"), b"unrelated").unwrap();
        let format = ArchiveFormat::TarGz;
        let bytes = archive(format, &[("d3d11.dll", b"new")]);
        let error = install_archive(
            Cursor::new(bytes),
            &fixture_release(format),
            &target,
            crate::dxvk::inspect_candidate_dir,
            |bundle, extracted, staged| copy_layout(&bundle.root, staged, extracted, &[]),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Refusing to replace nonempty destination")
        );
        assert_eq!(fs::read(target.join("keep")).unwrap(), b"unrelated");
        assert_eq!(fs::read_dir(&target).unwrap().count(), 1);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn external_source_links_and_destination_links_are_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let extracted = dir.path().join("extracted");
        let staged = dir.path().join("staged");
        fs::create_dir(&extracted).unwrap();
        fs::create_dir(&staged).unwrap();
        let original = dir.path().join("original");
        fs::write(&original, b"preserve").unwrap();
        let source = extracted.join("linked.dll");
        std::os::unix::fs::symlink(&original, &source).unwrap();
        assert!(copy_file(&source, &staged.join("file"), &extracted).is_err());
        let target = dir.path().join("target");
        std::os::unix::fs::symlink(&extracted, &target).unwrap();
        assert!(publish(&staged, &target).is_err());
        assert!(target.is_symlink());
        assert_eq!(fs::read(original).unwrap(), b"preserve");
    }
}
