//! Verify and build in a private sibling directory, then atomically publish.
use super::{
    assets::{self, Provider},
    ngx, transaction,
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read, Seek},
    path::{Path, PathBuf},
    process::Command,
};

const DOWNLOAD_LIMIT: u64 = 512 * 1024 * 1024;
const EXTRACT_LIMIT: u64 = 2 * 1024 * 1024 * 1024;

/// Repair incomplete generations only after a replacement has passed all checks.
fn generation(
    target: &Path,
    provider: &str,
    build: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    if assets::read_provider(&target.join("provider.json")).is_ok_and(|p| p.provider == provider) {
        return Ok(());
    }
    let parent = target
        .parent()
        .context("Provider destination has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = tempfile::Builder::new()
        .prefix(".nucleon-provider-")
        .tempdir_in(parent)?;
    let stage = temporary.path().join("generation");
    fs::create_dir(&stage)?;
    build(&stage)?;
    ensure!(
        assets::read_provider(&stage.join("provider.json"))?.provider == provider,
        "Provider builder produced the wrong provider"
    );
    crate::downloads::publish(&stage, target)?;
    Ok(())
}

fn manifest(stage: &Path, provider: &str, version: &str, framework: Option<String>) -> Result<()> {
    let mut files = BTreeMap::new();
    for entry in walkdir::WalkDir::new(stage).follow_links(false) {
        let entry = entry?;
        // Native shader compiler links deliberately pin their resolved contents.
        let relative = entry.path().strip_prefix(stage)?;
        let native_input = [
            "src",
            "hook",
            "third_party",
            "patches",
            "tools",
            "nucleon",
            "dist/crossover",
        ]
        .iter()
        .any(|p| relative.starts_with(Path::new("fsr4").join(p)));
        if entry.path().is_file()
            && (provider != "native"
                || (native_input
                    && !relative
                        .components()
                        .any(|c| c.as_os_str() == "__pycache__")))
        {
            files.insert(
                entry.path().strip_prefix(stage)?.to_path_buf(),
                crate::staging::digest(entry.path())?,
            );
        }
    }
    crate::fs_util::atomic_write_file(
        stage.join("provider.json"),
        serde_json::to_vec_pretty(&Provider {
            schema: 1,
            provider: provider.into(),
            version: version.into(),
            files,
            framework_sha256: framework,
        })?,
    )
}

fn relative(name: &str) -> Result<PathBuf> {
    let name = name.replace('\\', "/");
    ensure!(
        !name.starts_with('/') && !name.contains([':', '\0']),
        "Unsafe archive path"
    );
    let parts = name.trim_end_matches('/').split('/').collect::<Vec<_>>();
    ensure!(
        !parts.is_empty()
            && parts
                .iter()
                .all(|p| !p.is_empty() && *p != "." && *p != ".."),
        "Unsafe archive path"
    );
    Ok(parts.iter().collect())
}

fn wanted(path: &Path) -> bool {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    ["license", "notice"].iter().any(|p| name.starts_with(p))
        || path.extension().is_some_and(|e| {
            ["dll", "ini", "txt", "md"]
                .iter()
                .any(|x| e.eq_ignore_ascii_case(x))
        })
}

fn extract<R: Read + Seek>(input: R, len: u64, stage: &Path, limit: u64) -> Result<()> {
    let mut archive = sevenz_rust::SevenZReader::new(input, len, sevenz_rust::Password::empty())?;
    ensure!(
        archive.archive().files.len() <= 10000,
        "Too many provider archive entries"
    );
    let mut total = 0_u64;
    let mut seen = BTreeSet::new();
    for entry in &archive.archive().files {
        relative(&entry.name)?;
        let mode = entry.windows_attributes >> 16;
        ensure!(
            !entry.is_anti_item
                && entry.windows_attributes & 0x400 == 0
                && (mode & 0o170000 == 0
                    || mode & 0o170000 == 0o100000
                    || mode & 0o170000 == 0o040000),
            "Provider archives cannot contain links or special files"
        );
        total = total
            .checked_add(entry.size)
            .context("Archive size overflow")?;
        ensure!(total <= limit, "Provider archive exceeds extraction limit");
    }
    archive.for_each_entries(|entry, reader| {
        let result = (|| -> Result<()> {
            let path = relative(&entry.name)?;
            if entry.is_directory {
                return Ok(());
            }
            ensure!(
                seen.insert(path.to_string_lossy().to_lowercase()),
                "Duplicate provider archive path"
            );
            let mut bounded = reader.take(entry.size + 1);
            let copied = if wanted(&path) {
                let output = stage.join(path);
                fs::create_dir_all(output.parent().unwrap())?;
                let mut file = fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(output)?;
                let size = io::copy(&mut bounded, &mut file)?;
                file.sync_all()?;
                size
            } else {
                // Solid archives must consume skipped entries too, including CRC checks.
                io::copy(&mut bounded, &mut io::sink())?
            };
            ensure!(copied == entry.size, "Provider archive entry size mismatch");
            Ok(())
        })();
        result
            .map(|()| true)
            .map_err(|e| sevenz_rust::Error::other(e.to_string()))
    })?;
    ensure!(
        stage.join("OptiScaler.ini").is_file(),
        "Release missing OptiScaler.ini"
    );
    Ok(())
}

fn digest_reader(reader: &mut impl Read) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    io::copy(reader, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

fn stock(root: &Path, provider: &str, archive: Option<&Path>) -> Result<()> {
    let catalog = assets::catalog();
    let (url, expected, version) = if provider == "stock" {
        (
            catalog.stock.url,
            catalog.stock.sha_256,
            catalog.stock.version,
        )
    } else {
        (
            catalog.optiscaler.url,
            catalog.optiscaler.sha_256,
            catalog.optiscaler.version,
        )
    };
    let target = assets::provider_path(root, provider)?
        .parent()
        .unwrap()
        .to_path_buf();
    generation(&target, provider, |stage| {
        let mut input = match archive {
            Some(path) => fs::File::open(path)?,
            None => crate::downloads::download(url, DOWNLOAD_LIMIT)?,
        };
        let len = input.metadata()?.len();
        ensure!(
            len <= DOWNLOAD_LIMIT,
            "Provider archive exceeds download limit"
        );
        ensure!(
            digest_reader(&mut input)? == expected,
            "Provider archive checksum mismatch"
        );
        input.rewind()?;
        extract(input, len, stage, EXTRACT_LIMIT)?;
        manifest(stage, provider, version, None)
    })
}

fn bridge(root: &Path, wine: &Path) -> Result<()> {
    let path = wine.join("lib/wine/x86_64-windows/nvngx-on-metalfx.dll");
    let bytes = fs::read(&path).context("Selected Wine has no NGX bridge")?;
    let names = ngx::exports(&bytes)?;
    let digest = digest_reader(&mut bytes.as_slice())?;
    let target = root
        .join("providers/bridge")
        .join(assets::bridge_key(&digest)?);
    generation(&target, "bridge", |stage| {
        let definition = stage.join("nvngx.def");
        let mut text = String::from("LIBRARY nvngx\nEXPORTS\n");
        for name in names.keys() {
            text.push_str(&format!("{name}=nvngx-on-metalfx.{name}\n"));
        }
        fs::write(&definition, text)?;
        let output = stage.join("nvngx.dll");
        let status = Command::new("x86_64-w64-mingw32-gcc")
            .args(["-shared", "-nostdlib"])
            .arg(definition)
            .args(["-Wl,--entry,0", "-Wl,--no-insert-timestamp", "-o"])
            .arg(&output)
            .status()
            .context("NGX forwarding setup requires x86_64-w64-mingw32-gcc")?;
        ensure!(status.success(), "NGX forwarder compilation failed");
        ngx::verify_forwarder(&fs::read(output)?, &names)?;
        manifest(stage, "bridge", "ngx-forward-v1", None)
    })
}

fn native(root: &Path, wine: &Path, source: Option<&Path>) -> Result<()> {
    let wine = wine.canonicalize()?;
    let digest = crate::staging::digest(&wine.join("lib/external/D3DMetal.framework/D3DMetal"))?;
    let target = root
        .join("providers/native")
        .join(assets::native_key(&digest)?);
    generation(&target, "native", |stage| {
        super::native::build(root, &stage.join("fsr4"), &wine, source)?;
        ensure!(
            crate::staging::digest(&wine.join("lib/external/D3DMetal.framework/D3DMetal"))?
                == digest,
            "Original GPTK framework changed during build"
        );
        manifest(stage, "native", "4.1.1", Some(digest))
    })
}

pub fn install(
    root: &Path,
    provider: &str,
    archive: Option<&Path>,
    wine: Option<&Path>,
    source: Option<&Path>,
) -> Result<()> {
    ensure!(
        matches!(provider, "stock" | "metalfx" | "native" | "bridge"),
        "Choose stock, metalfx, native, bridge or patcher"
    );
    fs::create_dir_all(root)?;
    let _lock = transaction::lock(&root.join(".provider.lock"))?;
    match provider {
        "stock" | "metalfx" => stock(root, provider, archive),
        "bridge" => bridge(root, wine.context("Bridge provider requires Wine")?),
        "native" => native(root, wine.context("Native provider requires Wine")?, source),
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo_assets::FIXTURES;
    use proptest::prelude::*;
    use std::io::Cursor;

    fn valid(stage: &Path) -> Result<()> {
        fs::write(
            stage.join("OptiScaler.dll"),
            crate::runtime_checks::test_pe_image(0x8664),
        )?;
        fs::write(stage.join("OptiScaler.ini"), b"fixture")?;
        manifest(stage, "stock", "fixture", None)
    }

    fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = sevenz_rust::SevenZWriter::new(Cursor::new(Vec::new())).unwrap();
        for (name, bytes) in entries {
            let mut entry = sevenz_rust::SevenZArchiveEntry::new();
            entry.name = (*name).into();
            entry.has_stream = true;
            writer
                .push_archive_entry(entry, Some(Cursor::new(*bytes)))
                .unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn failed_initial_build_and_failed_repair_leave_no_published_changes() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("stock");
        let fail = |stage: &Path| -> Result<()> {
            valid(stage)?;
            anyhow::bail!("simulated interrupted build")
        };
        assert!(generation(&target, "stock", fail).is_err());
        assert!(!target.exists());
        fs::create_dir(&target).unwrap();
        fs::write(target.join("OptiScaler.dll"), b"prior interrupted data").unwrap();
        fs::write(target.join("provider.json"), b"{incomplete").unwrap();
        assert!(generation(&target, "stock", fail).is_err());
        assert_eq!(
            fs::read(target.join("OptiScaler.dll")).unwrap(),
            b"prior interrupted data"
        );
        assert_eq!(
            fs::read(target.join("provider.json")).unwrap(),
            b"{incomplete"
        );
        assert_eq!(
            fs::read_dir(temp.path()).unwrap().count(),
            1,
            "staging folders must be cleaned"
        );
        generation(&target, "stock", valid).unwrap();
        assets::read_provider(&target.join("provider.json")).unwrap();
        generation(&target, "stock", |_| {
            panic!("valid generation should be reused")
        })
        .unwrap();
    }

    #[test]
    fn invalid_payload_never_replaces_prior_generation_and_can_be_retried() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("stock");
        generation(&target, "stock", valid).unwrap();
        let prior_manifest = fs::read(target.join("provider.json")).unwrap();
        fs::write(target.join("OptiScaler.ini"), b"damaged").unwrap();
        assert!(
            generation(&target, "stock", |stage| {
                fs::write(stage.join("OptiScaler.dll"), b"invalid PE")?;
                fs::write(stage.join("OptiScaler.ini"), b"replacement")?;
                manifest(stage, "stock", "fixture", None)
            })
            .is_err()
        );
        assert_eq!(
            fs::read(target.join("provider.json")).unwrap(),
            prior_manifest
        );
        assert_eq!(fs::read(target.join("OptiScaler.ini")).unwrap(), b"damaged");
        generation(&target, "stock", valid).unwrap();
        assets::read_provider(&target.join("provider.json")).unwrap();
    }

    #[test]
    fn archive_extracts_only_runtime_files_and_not_install_scripts() {
        let image = crate::runtime_checks::test_pe_image(0x8664);
        let input = archive(&[
            ("setup.bat", b"do not execute"),
            ("OptiScaler.dll", &image),
            ("OptiScaler.ini", b"configuration"),
            ("plugins\\helper.dll", &image),
            ("LICENSE", b"notice"),
        ]);
        let stage = tempfile::tempdir().unwrap();
        extract(
            Cursor::new(&input),
            input.len() as u64,
            stage.path(),
            EXTRACT_LIMIT,
        )
        .unwrap();
        assert!(!stage.path().join("setup.bat").exists());
        assert_eq!(
            fs::read(stage.path().join("plugins/helper.dll")).unwrap(),
            image
        );
        assert!(stage.path().join("LICENSE").is_file());
        manifest(stage.path(), "stock", "fixture", None).unwrap();
        assets::read_provider(&stage.path().join("provider.json")).unwrap();
    }

    #[test]
    fn unsafe_duplicate_oversized_and_truncated_archives_are_rejected() {
        for entries in [
            vec![("../outside.dll", b"x".as_slice())],
            vec![("C:\\outside.dll", b"x".as_slice())],
            vec![("/outside.dll", b"x".as_slice())],
            vec![("one.dll", b"x".as_slice()), ("ONE.dll", b"y".as_slice())],
        ] {
            let input = archive(&entries);
            let stage = tempfile::tempdir().unwrap();
            assert!(
                extract(
                    Cursor::new(&input),
                    input.len() as u64,
                    stage.path(),
                    EXTRACT_LIMIT
                )
                .is_err()
            );
        }
        let input = archive(&[("OptiScaler.ini", b"configuration")]);
        let stage = tempfile::tempdir().unwrap();
        assert!(extract(Cursor::new(&input), input.len() as u64, stage.path(), 1).is_err());
        assert!(fs::read_dir(stage.path()).unwrap().next().is_none());
        let input = &input[..input.len() / 2];
        assert!(
            extract(
                Cursor::new(input),
                input.len() as u64,
                stage.path(),
                EXTRACT_LIMIT
            )
            .is_err()
        );
    }

    #[test]
    fn checksum_failure_does_not_publish_a_provider() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("wrong.7z");
        fs::write(&input, b"untrusted release").unwrap();
        assert!(
            install(temp.path(), "stock", Some(&input), None, None)
                .unwrap_err()
                .to_string()
                .contains("checksum")
        );
        assert!(
            !assets::provider_path(temp.path(), "stock")
                .unwrap()
                .exists()
        );
    }

    #[test]
    fn native_source_failure_preserves_the_prior_generation_and_original_framework() {
        let temp = tempfile::tempdir().unwrap();
        let wine = temp.path().join("wine");
        let framework = wine.join("lib/external/D3DMetal.framework/D3DMetal");
        fs::create_dir_all(framework.parent().unwrap()).unwrap();
        fs::write(&framework, b"original framework").unwrap();
        let source = temp.path().join("unverified-source");
        fs::create_dir(&source).unwrap();
        // A self-declared source receipt is not an authority for executable build inputs.
        fs::write(
            source.join(".nucleon-source.json"),
            br#"{"commit":"329d1d7367b6b948a53486505198a5d254f02809","files":{}}"#,
        )
        .unwrap();
        let target = temp
            .path()
            .join("providers/native")
            .join(assets::native_key(&crate::staging::digest(&framework).unwrap()).unwrap());
        fs::create_dir_all(&target).unwrap();
        fs::write(
            target.join("prior-data"),
            b"preserve failed generation for retry",
        )
        .unwrap();
        for _ in 0..2 {
            assert!(install(temp.path(), "native", None, Some(&wine), Some(&source)).is_err());
            assert_eq!(
                fs::read(target.join("prior-data")).unwrap(),
                b"preserve failed generation for retry"
            );
            assert!(!target.join("provider.json").exists());
            assert_eq!(fs::read(&framework).unwrap(), b"original framework");
            assert_eq!(fs::read_dir(target.parent().unwrap()).unwrap().count(), 1);
        }
    }

    #[test]
    fn publication_failure_preserves_the_destination_and_cleans_staging() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("unexpected-file");
        fs::write(&target, b"keep this file").unwrap();
        assert!(generation(&target, "stock", valid).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"keep this file");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn legacy_bridge_identity_is_discoverable_and_reused_without_rebuilding() {
        let temp = tempfile::tempdir().unwrap();
        let wine = temp.path().join("wine");
        let original = wine.join("lib/wine/x86_64-windows/nvngx-on-metalfx.dll");
        let bytes = FIXTURES
            .get_file("upscaler/ngx-forward.bin")
            .unwrap()
            .contents();
        fs::create_dir_all(original.parent().unwrap()).unwrap();
        fs::write(&original, bytes).unwrap();
        let digest = crate::staging::digest(&original).unwrap();
        let legacy = crate::staging::key(&[
            digest.as_str(),
            "cfe24edc4b0056c1abc42f95b93ff28fd2bf6d13aaf57b0a7caa5b727fbbcc41",
        ])
        .unwrap();
        assert_eq!(assets::bridge_key(&digest).unwrap(), legacy);
        let target = temp.path().join("providers/bridge").join(legacy);
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("nvngx.dll"), bytes).unwrap();
        fs::write(target.join("nvngx.def"), b"legacy definition preserved").unwrap();
        manifest(&target, "bridge", "ngx-forward-v1", None).unwrap();
        install(temp.path(), "bridge", None, Some(&wine), None).unwrap();
        assert_eq!(
            fs::read(target.join("nvngx.def")).unwrap(),
            b"legacy definition preserved"
        );
        assert_eq!(
            assets::bridge_provider(temp.path(), &original).unwrap(),
            target.join("provider.json")
        );
    }

    proptest! {
        #[test]
        fn accepted_archive_paths_are_relative_and_cannot_traverse(name in ".{0,240}") {
            if let Ok(path) = relative(&name) {
                prop_assert!(!path.is_absolute());
                prop_assert!(path.components().all(|c| matches!(c, std::path::Component::Normal(_))));
                prop_assert!(!path.as_os_str().is_empty());
                prop_assert!(!path.to_string_lossy().contains([':', '\0', '\\']));
            }
        }
    }
}
